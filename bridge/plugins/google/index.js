/**
 * Google Gemini bridge plugin.
 *
 * Routes Gemini API requests through the bridge for centralized
 * logging and credential isolation. Uses the bridge SDK v2 API.
 */

import { logInference } from '../../lib/inference-log.js';

/**
 * Register the Google Gemini provider with the bridge SDK.
 * @param {import('../../lib/plugin-sdk.js').PluginSDK} sdk
 */
export function register(sdk) {
  sdk.registerProvider({
    name: 'google',
    models: [
      'gemini-2.5-pro',
      'gemini-2.5-flash',
      'gemini-2.0-flash',
    ],
    streamEndpoint: '/api/google/messages/stream',
    messageEndpoint: '/api/google/messages',
    streamHandler: handleStream,
    messageHandler: handleMessage,
    modelsHandler: handleModels,
  });
}

async function handleModels(req, res) {
  try {
    const apiKey = req.body?.apiKey || req.headers['x-api-key'] || process.env.GOOGLE_API_KEY;
    if (!apiKey) {
      return res.status(401).json({ error: { message: 'No Google API key provided' } });
    }

    // Return configured models
    res.json({
      models: [
        { id: 'gemini-2.5-pro', name: 'Gemini 2.5 Pro' },
        { id: 'gemini-2.5-flash', name: 'Gemini 2.5 Flash' },
        { id: 'gemini-2.0-flash', name: 'Gemini 2.0 Flash' },
      ],
    });
  } catch (err) {
    res.status(500).json({ error: { message: err.message } });
  }
}

async function handleMessage(req, res) {
  const startTime = Date.now();
  try {
    const { model, messages, apiKey: bodyApiKey, max_tokens, system } = req.body;
    const apiKey = bodyApiKey || req.headers['x-api-key'] || process.env.GOOGLE_API_KEY;

    if (!apiKey) {
      return res.status(401).json({ error: { message: 'No Google API key provided' } });
    }

    console.log('[Bridge:Google] Non-streaming request:', {
      model,
      messageCount: messages?.length || 0,
      hasSystem: !!system,
      timestamp: new Date().toISOString(),
    });

    // Convert messages to Gemini format
    const geminiMessages = convertToGeminiFormat(messages);

    const requestBody = {
      contents: geminiMessages,
      generationConfig: {
        maxOutputTokens: max_tokens || 4096,
      },
    };

    // Add system instruction if provided
    if (system) {
      requestBody.systemInstruction = {
        parts: [{ text: typeof system === 'string' ? system : JSON.stringify(system) }],
      };
    }

    const response = await fetch(
      `https://generativelanguage.googleapis.com/v1beta/models/${model}:generateContent?key=${apiKey}`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(requestBody),
      }
    );

    const data = await response.json();
    const latencyMs = Date.now() - startTime;

    if (!response.ok) {
      const errorMsg = data.error?.message || `Gemini API error ${response.status}`;
      logInference({
        provider: 'google',
        model: model || 'unknown',
        input_tokens: 0,
        output_tokens: 0,
        latency_ms: latencyMs,
        streaming: false,
        error: errorMsg,
      });
      return res.status(response.status).json({
        type: 'error',
        error: { type: 'api_error', message: errorMsg },
      });
    }

    // Log inference
    logInference({
      provider: 'google',
      model: model || 'unknown',
      input_tokens: data.usageMetadata?.promptTokenCount || 0,
      output_tokens: data.usageMetadata?.candidatesTokenCount || 0,
      latency_ms: latencyMs,
      streaming: false,
    });

    // Normalize response to Anthropic format
    const normalized = normalizeGeminiResponse(data, model);

    console.log('[Bridge:Google] Request complete:', {
      model,
      inputTokens: data.usageMetadata?.promptTokenCount || 0,
      outputTokens: data.usageMetadata?.candidatesTokenCount || 0,
      durationMs: latencyMs,
    });

    res.json(normalized);
  } catch (err) {
    const latencyMs = Date.now() - startTime;
    logInference({
      provider: 'google',
      model: req.body?.model || 'unknown',
      input_tokens: 0,
      output_tokens: 0,
      latency_ms: latencyMs,
      streaming: false,
      error: err.message,
    });
    console.error('[Bridge:Google] Request error:', err);
    res.status(500).json({
      type: 'error',
      error: { type: 'api_error', message: err.message },
    });
  }
}

async function handleStream(req, res) {
  const startTime = Date.now();
  try {
    const { model, messages, apiKey: bodyApiKey, max_tokens, system } = req.body;
    const apiKey = bodyApiKey || req.headers['x-api-key'] || process.env.GOOGLE_API_KEY;

    if (!apiKey) {
      return res.status(401).json({ error: { message: 'No Google API key provided' } });
    }

    console.log('[Bridge:Google] Streaming request:', {
      model,
      messageCount: messages?.length || 0,
      hasSystem: !!system,
      timestamp: new Date().toISOString(),
    });

    const geminiMessages = convertToGeminiFormat(messages);

    const requestBody = {
      contents: geminiMessages,
      generationConfig: {
        maxOutputTokens: max_tokens || 4096,
      },
    };

    if (system) {
      requestBody.systemInstruction = {
        parts: [{ text: typeof system === 'string' ? system : JSON.stringify(system) }],
      };
    }

    const response = await fetch(
      `https://generativelanguage.googleapis.com/v1beta/models/${model}:streamGenerateContent?alt=sse&key=${apiKey}`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(requestBody),
      }
    );

    if (!response.ok) {
      const errorText = await response.text();
      throw new Error(`Gemini API error ${response.status}: ${errorText}`);
    }

    res.setHeader('Content-Type', 'text/event-stream');
    res.setHeader('Cache-Control', 'no-cache');
    res.setHeader('Connection', 'keep-alive');

    // Forward SSE events
    const reader = response.body.getReader();
    const decoder = new TextDecoder();

    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        const chunk = decoder.decode(value, { stream: true });
        res.write(chunk);
      }
    } finally {
      const latencyMs = Date.now() - startTime;
      logInference({
        provider: 'google',
        model: model || 'unknown',
        input_tokens: 0,
        output_tokens: 0,
        latency_ms: latencyMs,
        streaming: true,
      });

      console.log('[Bridge:Google] Streaming complete:', {
        model,
        durationMs: latencyMs,
      });

      res.write('data: [DONE]\n\n');
      res.end();
    }
  } catch (err) {
    const latencyMs = Date.now() - startTime;
    logInference({
      provider: 'google',
      model: req.body?.model || 'unknown',
      input_tokens: 0,
      output_tokens: 0,
      latency_ms: latencyMs,
      streaming: true,
      error: err.message,
    });
    console.error('[Bridge:Google] Streaming error:', err);

    if (!res.headersSent) {
      res.status(500).json({
        type: 'error',
        error: { type: 'api_error', message: err.message },
      });
    } else {
      res.write(`data: ${JSON.stringify({ type: 'error', error: { type: 'api_error', message: err.message } })}\n\n`);
      res.end();
    }
  }
}

function convertToGeminiFormat(messages) {
  if (!messages || !Array.isArray(messages)) return [];

  return messages
    .filter(m => m.role !== 'system') // System messages handled separately
    .map(m => ({
      role: m.role === 'assistant' ? 'model' : 'user',
      parts: [{ text: typeof m.content === 'string' ? m.content : JSON.stringify(m.content) }],
    }));
}

function normalizeGeminiResponse(data, model) {
  const candidate = data.candidates?.[0];
  const text = candidate?.content?.parts?.[0]?.text || '';

  return {
    id: `msg_${Date.now()}`,
    type: 'message',
    role: 'assistant',
    model: model,
    content: [{ type: 'text', text }],
    usage: {
      input_tokens: data.usageMetadata?.promptTokenCount || 0,
      output_tokens: data.usageMetadata?.candidatesTokenCount || 0,
    },
    stop_reason: candidate?.finishReason === 'STOP' ? 'end_turn' : candidate?.finishReason || 'end_turn',
  };
}

/**
 * Health check for this plugin.
 */
export function health() {
  return { status: 'ok', provider: 'google' };
}
