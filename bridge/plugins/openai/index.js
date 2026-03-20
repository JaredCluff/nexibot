/**
 * OpenAI Bridge Plugin
 *
 * Provides OpenAI chat model access with responses normalized to
 * Anthropic message format so the Rust provider code is unchanged.
 */

import OpenAI from 'openai';

/**
 * Create OpenAI client
 */
function createOpenAIClient(apiKey) {
  console.log('[Bridge:OpenAI] Creating OpenAI client');
  return new OpenAI({ apiKey });
}

/**
 * Convert messages from Anthropic format to OpenAI format.
 * Anthropic uses role: "user"/"assistant" with string content.
 * OpenAI uses the same roles but tool results use role: "tool".
 */
function convertMessagesForOpenAI(messages, systemPrompt) {
  const result = [];

  // Add system message
  if (systemPrompt) {
    const systemText = typeof systemPrompt === 'string'
      ? systemPrompt
      : Array.isArray(systemPrompt)
        ? systemPrompt.map(b => b.text || '').join('\n')
        : '';
    if (systemText) {
      result.push({ role: 'system', content: systemText });
    }
  }

  for (const msg of messages) {
    result.push({ role: msg.role, content: msg.content });
  }

  return result;
}

/**
 * Normalize OpenAI response to Anthropic response shape
 */
function normalizeOpenAIResponse(response) {
  const choice = response.choices?.[0];
  if (!choice) {
    return {
      id: response.id || 'unknown',
      type: 'message',
      role: 'assistant',
      content: [],
      model: response.model || 'unknown',
      stop_reason: 'end_turn',
    };
  }

  const content = [];

  // Add text content
  if (choice.message?.content) {
    content.push({
      type: 'text',
      text: choice.message.content,
    });
  }

  // Add tool calls
  if (choice.message?.tool_calls) {
    for (const tc of choice.message.tool_calls) {
      let input = {};
      try {
        input = JSON.parse(tc.function?.arguments || '{}');
      } catch {
        input = {};
      }
      content.push({
        type: 'tool_use',
        id: tc.id,
        name: tc.function?.name || '',
        input,
      });
    }
  }

  // Map stop reason
  let stop_reason = 'end_turn';
  if (choice.finish_reason === 'tool_calls') {
    stop_reason = 'tool_use';
  } else if (choice.finish_reason === 'length') {
    stop_reason = 'max_tokens';
  }

  return {
    id: response.id || 'unknown',
    type: 'message',
    role: 'assistant',
    content,
    model: response.model || 'unknown',
    stop_reason,
  };
}

/**
 * Register OpenAI plugin routes on the Express app.
 */
export function register(app, { utils, logger }) {
  /**
   * OpenAI non-streaming messages endpoint
   *
   * POST /api/openai/messages
   */
  app.post('/api/openai/messages', async (req, res) => {
    const startTime = Date.now();
    const { apiKey, model, max_tokens, system, messages, tools, temperature } = req.body;

    if (!apiKey) {
      return res.status(400).json({ error: 'Missing apiKey (OpenAI API key)' });
    }

    if (!model) {
      return res.status(400).json({ error: 'Missing model' });
    }

    if (!messages || !Array.isArray(messages)) {
      return res.status(400).json({ error: 'Missing or invalid messages array' });
    }

    console.log('[Bridge:OpenAI] Non-streaming request:', {
      model,
      messageCount: messages.length,
      hasSystem: !!system,
      hasTools: !!tools,
      timestamp: new Date().toISOString(),
    });

    try {
      const client = createOpenAIClient(apiKey);
      const openaiMessages = convertMessagesForOpenAI(messages, system);

      const params = {
        model,
        messages: openaiMessages,
        max_tokens: max_tokens || 4096,
      };

      if (tools && tools.length > 0) {
        params.tools = tools;
      }

      if (temperature !== undefined) {
        params.temperature = temperature;
      }

      const response = await client.chat.completions.create(params);

      const normalized = normalizeOpenAIResponse(response);

      const duration = Date.now() - startTime;
      console.log('[Bridge:OpenAI] Request complete:', {
        id: response.id,
        model: response.model,
        stopReason: normalized.stop_reason,
        inputTokens: response.usage?.prompt_tokens,
        outputTokens: response.usage?.completion_tokens,
        durationMs: duration,
      });

      res.json(normalized);

    } catch (error) {
      console.error('[Bridge:OpenAI] Request error:', error);
      res.status(error.status || 500).json({
        type: 'error',
        error: {
          type: 'api_error',
          message: error.message,
        },
      });
    }
  });

  /**
   * OpenAI streaming messages endpoint
   *
   * POST /api/openai/messages/stream
   *
   * Streams response normalized to Anthropic SSE format.
   */
  app.post('/api/openai/messages/stream', async (req, res) => {
    const startTime = Date.now();
    const { apiKey, model, max_tokens, system, messages, tools, temperature } = req.body;

    if (!apiKey) {
      return res.status(400).json({ error: 'Missing apiKey (OpenAI API key)' });
    }

    if (!model) {
      return res.status(400).json({ error: 'Missing model' });
    }

    if (!messages || !Array.isArray(messages)) {
      return res.status(400).json({ error: 'Missing or invalid messages array' });
    }

    console.log('[Bridge:OpenAI] Streaming request:', {
      model,
      messageCount: messages.length,
      hasSystem: !!system,
      hasTools: !!tools,
      timestamp: new Date().toISOString(),
    });

    try {
      const client = createOpenAIClient(apiKey);
      const openaiMessages = convertMessagesForOpenAI(messages, system);

      const params = {
        model,
        messages: openaiMessages,
        max_tokens: max_tokens || 4096,
        stream: true,
      };

      if (tools && tools.length > 0) {
        params.tools = tools;
      }

      if (temperature !== undefined) {
        params.temperature = temperature;
      }

      res.setHeader('Content-Type', 'text/event-stream');
      res.setHeader('Cache-Control', 'no-cache');
      res.setHeader('Connection', 'keep-alive');

      const stream = await client.chat.completions.create(params);

      let contentBlockIndex = 0;
      let currentToolCalls = {};
      let hasStartedText = false;
      let textChunks = 0;

      for await (const chunk of stream) {
        const delta = chunk.choices?.[0]?.delta;
        const finishReason = chunk.choices?.[0]?.finish_reason;

        if (delta) {
          // Text content
          if (delta.content) {
            if (!hasStartedText) {
              res.write(`data: ${JSON.stringify({
                type: 'content_block_start',
                index: contentBlockIndex,
                content_block: { type: 'text', text: '' },
              })}\n\n`);
              hasStartedText = true;
            }

            res.write(`data: ${JSON.stringify({
              type: 'content_block_delta',
              index: contentBlockIndex,
              delta: { type: 'text_delta', text: delta.content },
            })}\n\n`);
            textChunks++;
          }

          // Tool calls
          if (delta.tool_calls) {
            for (const tc of delta.tool_calls) {
              const tcIndex = tc.index;
              if (tc.id) {
                // New tool call starting
                if (hasStartedText) {
                  res.write(`data: ${JSON.stringify({
                    type: 'content_block_stop',
                    index: contentBlockIndex,
                  })}\n\n`);
                  contentBlockIndex++;
                  hasStartedText = false;
                }

                currentToolCalls[tcIndex] = {
                  id: tc.id,
                  name: tc.function?.name || '',
                  arguments: '',
                };

                res.write(`data: ${JSON.stringify({
                  type: 'content_block_start',
                  index: contentBlockIndex + tcIndex,
                  content_block: {
                    type: 'tool_use',
                    id: tc.id,
                    name: tc.function?.name || '',
                  },
                })}\n\n`);
              }

              if (tc.function?.arguments) {
                if (currentToolCalls[tcIndex]) {
                  currentToolCalls[tcIndex].arguments += tc.function.arguments;
                }
                res.write(`data: ${JSON.stringify({
                  type: 'content_block_delta',
                  index: contentBlockIndex + tcIndex,
                  delta: {
                    type: 'input_json_delta',
                    partial_json: tc.function.arguments,
                  },
                })}\n\n`);
              }
            }
          }
        }

        // Handle finish
        if (finishReason) {
          // Close any open content blocks
          if (hasStartedText) {
            res.write(`data: ${JSON.stringify({
              type: 'content_block_stop',
              index: contentBlockIndex,
            })}\n\n`);
          }

          for (const tcIndex of Object.keys(currentToolCalls)) {
            res.write(`data: ${JSON.stringify({
              type: 'content_block_stop',
              index: contentBlockIndex + parseInt(tcIndex),
            })}\n\n`);
          }

          // Map finish reason
          let stopReason = 'end_turn';
          if (finishReason === 'tool_calls') stopReason = 'tool_use';
          else if (finishReason === 'length') stopReason = 'max_tokens';

          res.write(`data: ${JSON.stringify({
            type: 'message_delta',
            delta: { stop_reason: stopReason },
          })}\n\n`);
        }
      }

      const duration = Date.now() - startTime;
      console.log('[Bridge:OpenAI] Streaming complete:', {
        textChunks,
        toolCalls: Object.keys(currentToolCalls).length,
        durationMs: duration,
      });

      res.write('data: [DONE]\n\n');
      res.end();

    } catch (error) {
      console.error('[Bridge:OpenAI] Streaming error:', error);

      const errorResponse = {
        type: 'error',
        error: {
          type: 'api_error',
          message: error.message,
        },
      };

      if (!res.headersSent) {
        res.setHeader('Content-Type', 'text/event-stream');
      }
      res.write(`data: ${JSON.stringify(errorResponse)}\n\n`);
      res.end();
    }
  });

  /**
   * List available OpenAI models (chat models only)
   *
   * GET /api/openai/models?apiKey=sk-...
   */
  app.get('/api/openai/models', async (req, res) => {
    const apiKey = req.query.apiKey || req.headers['x-api-key'];

    if (!apiKey) {
      return res.json([]);
    }

    // ChatGPT OAuth tokens (JWTs) use a different models endpoint than API keys
    const isChatGPTToken = apiKey.startsWith('eyJ');

    if (isChatGPTToken) {
      console.log('[Bridge:OpenAI] Listing models via ChatGPT backend API (OAuth token)');
      try {
        const response = await fetch('https://chatgpt.com/backend-api/codex/models?client_version=0.99.0', {
          headers: { 'Authorization': `Bearer ${apiKey}` },
        });

        if (!response.ok) {
          const errorText = await response.text();
          console.error('[Bridge:OpenAI] ChatGPT models API failed:', response.status, errorText);
          return res.json([]);
        }

        const data = await response.json();
        const models = (data.models || [])
          .filter(m => m.supported_in_api)
          .map(m => ({
            id: m.slug,
            display_name: m.display_name || m.slug,
            created_at: null,
          }))
          .sort((a, b) => a.id.localeCompare(b.id));

        console.log(`[Bridge:OpenAI] Found ${models.length} ChatGPT models`);
        res.json(models);
      } catch (error) {
        console.error('[Bridge:OpenAI] Failed to list ChatGPT models:', error.message);
        res.json([]);
      }
      return;
    }

    console.log('[Bridge:OpenAI] Listing OpenAI models via API key');

    try {
      const client = createOpenAIClient(apiKey);
      const response = await client.models.list();

      // Filter to chat-capable models only
      const chatPrefixes = ['gpt-', 'o1', 'o3', 'o4', 'chatgpt'];
      const excludePrefixes = ['gpt-3.5-turbo-instruct'];

      const models = response.data
        .filter(m => {
          const id = m.id.toLowerCase();
          const isChat = chatPrefixes.some(p => id.startsWith(p));
          const isExcluded = excludePrefixes.some(p => id.startsWith(p));
          return isChat && !isExcluded;
        })
        .map(m => ({
          id: m.id,
          display_name: m.id,
          created_at: m.created ? new Date(m.created * 1000).toISOString() : null,
        }))
        .sort((a, b) => a.id.localeCompare(b.id));

      console.log(`[Bridge:OpenAI] Found ${models.length} OpenAI chat models`);
      res.json(models);
    } catch (error) {
      console.error('[Bridge:OpenAI] Failed to list OpenAI models:', error.message);
      res.json([]);
    }
  });
}

/**
 * Health check for this plugin.
 */
export function health() {
  return { status: 'ok', provider: 'openai' };
}
