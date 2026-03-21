/**
 * DeepSeek bridge plugin (OpenAI-compatible API).
 *
 * Routes DeepSeek API requests through the bridge. Uses the
 * OpenAI-compatible endpoint with DeepSeek's base URL.
 */

import { logInference } from '../../lib/inference-log.js';

const DEEPSEEK_BASE_URL = 'https://api.deepseek.com/v1';

/**
 * Register the DeepSeek provider with the bridge SDK.
 * @param {import('../../lib/plugin-sdk.js').PluginSDK} sdk
 */
export function register(sdk) {
  sdk.registerProvider({
    name: 'deepseek',
    models: [
      'deepseek-chat',
      'deepseek-reasoner',
    ],
    streamEndpoint: '/api/deepseek/messages/stream',
    messageEndpoint: '/api/deepseek/messages',
    streamHandler: handleStream,
    messageHandler: handleMessage,
    modelsHandler: handleModels,
  });
}

async function handleModels(req, res) {
  try {
    const apiKey = req.body?.apiKey || req.headers['x-api-key'] || process.env.DEEPSEEK_API_KEY;
    if (!apiKey) {
      return res.status(401).json({ error: { message: 'No DeepSeek API key provided' } });
    }

    const response = await fetch(`${DEEPSEEK_BASE_URL}/models`, {
      headers: { Authorization: `Bearer ${apiKey}` },
    });

    const data = await response.json();
    res.json({
      models: (data.data || []).map(m => ({
        id: m.id,
        name: m.id,
      })),
    });
  } catch (err) {
    console.error('[Bridge:DeepSeek] Failed to list models:', err.message);
    res.status(500).json({ error: { message: err.message } });
  }
}

async function handleMessage(req, res) {
  const startTime = Date.now();
  try {
    const { model, messages, apiKey: bodyApiKey, max_tokens, system } = req.body;
    const apiKey = bodyApiKey || req.headers['x-api-key'] || process.env.DEEPSEEK_API_KEY;

    if (!apiKey) {
      return res.status(401).json({ error: { message: 'No DeepSeek API key provided' } });
    }

    console.log('[Bridge:DeepSeek] Non-streaming request:', {
      model,
      messageCount: messages?.length || 0,
      hasSystem: !!system,
      timestamp: new Date().toISOString(),
    });

    // Build OpenAI-compatible messages with system prompt
    const openaiMessages = [];
    if (system) {
      const systemText = typeof system === 'string'
        ? system
        : Array.isArray(system)
          ? system.map(b => b.text || '').join('\n')
          : '';
      if (systemText) {
        openaiMessages.push({ role: 'system', content: systemText });
      }
    }
    for (const msg of (messages || [])) {
      openaiMessages.push({ role: msg.role, content: msg.content });
    }

    const response = await fetch(`${DEEPSEEK_BASE_URL}/chat/completions`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${apiKey}`,
      },
      body: JSON.stringify({
        model: model || 'deepseek-chat',
        messages: openaiMessages,
        max_tokens: max_tokens || 4096,
        stream: false,
      }),
    });

    const data = await response.json();
    const latencyMs = Date.now() - startTime;

    if (!response.ok) {
      const errorMsg = data.error?.message || `DeepSeek API error ${response.status}`;
      logInference({
        provider: 'deepseek',
        model: model || 'deepseek-chat',
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

    logInference({
      provider: 'deepseek',
      model: model || 'deepseek-chat',
      input_tokens: data.usage?.prompt_tokens || 0,
      output_tokens: data.usage?.completion_tokens || 0,
      latency_ms: latencyMs,
      streaming: false,
    });

    // Normalize to Anthropic format
    const text = data.choices?.[0]?.message?.content || '';

    console.log('[Bridge:DeepSeek] Request complete:', {
      id: data.id,
      model,
      inputTokens: data.usage?.prompt_tokens || 0,
      outputTokens: data.usage?.completion_tokens || 0,
      durationMs: latencyMs,
    });

    res.json({
      id: data.id || `msg_${Date.now()}`,
      type: 'message',
      role: 'assistant',
      model: model,
      content: [{ type: 'text', text }],
      usage: {
        input_tokens: data.usage?.prompt_tokens || 0,
        output_tokens: data.usage?.completion_tokens || 0,
      },
      stop_reason: data.choices?.[0]?.finish_reason === 'stop' ? 'end_turn' : 'end_turn',
    });
  } catch (err) {
    const latencyMs = Date.now() - startTime;
    logInference({
      provider: 'deepseek',
      model: req.body?.model || 'unknown',
      input_tokens: 0,
      output_tokens: 0,
      latency_ms: latencyMs,
      streaming: false,
      error: err.message,
    });
    console.error('[Bridge:DeepSeek] Request error:', err);
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
    const apiKey = bodyApiKey || req.headers['x-api-key'] || process.env.DEEPSEEK_API_KEY;

    if (!apiKey) {
      return res.status(401).json({ error: { message: 'No DeepSeek API key provided' } });
    }

    console.log('[Bridge:DeepSeek] Streaming request:', {
      model,
      messageCount: messages?.length || 0,
      hasSystem: !!system,
      timestamp: new Date().toISOString(),
    });

    // Build OpenAI-compatible messages with system prompt
    const openaiMessages = [];
    if (system) {
      const systemText = typeof system === 'string'
        ? system
        : Array.isArray(system)
          ? system.map(b => b.text || '').join('\n')
          : '';
      if (systemText) {
        openaiMessages.push({ role: 'system', content: systemText });
      }
    }
    for (const msg of (messages || [])) {
      openaiMessages.push({ role: msg.role, content: msg.content });
    }

    const response = await fetch(`${DEEPSEEK_BASE_URL}/chat/completions`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${apiKey}`,
      },
      body: JSON.stringify({
        model: model || 'deepseek-chat',
        messages: openaiMessages,
        max_tokens: max_tokens || 4096,
        stream: true,
      }),
    });

    if (!response.ok) {
      const errorText = await response.text();
      throw new Error(`DeepSeek API error ${response.status}: ${errorText}`);
    }

    res.setHeader('Content-Type', 'text/event-stream');
    res.setHeader('Cache-Control', 'no-cache');
    res.setHeader('Connection', 'keep-alive');

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
        provider: 'deepseek',
        model: model || 'deepseek-chat',
        input_tokens: 0,
        output_tokens: 0,
        latency_ms: latencyMs,
        streaming: true,
      });

      console.log('[Bridge:DeepSeek] Streaming complete:', {
        model,
        durationMs: latencyMs,
      });

      res.end();
    }
  } catch (err) {
    const latencyMs = Date.now() - startTime;
    logInference({
      provider: 'deepseek',
      model: req.body?.model || 'unknown',
      input_tokens: 0,
      output_tokens: 0,
      latency_ms: latencyMs,
      streaming: true,
      error: err.message,
    });
    console.error('[Bridge:DeepSeek] Streaming error:', err);

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

/**
 * Health check for this plugin.
 */
export function health() {
  return { status: 'ok', provider: 'deepseek' };
}
