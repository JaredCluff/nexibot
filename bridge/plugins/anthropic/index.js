/**
 * Anthropic Bridge Plugin
 *
 * Provides Anthropic Claude API access with OAuth token support,
 * Claude Code identity injection, and tool name casing conversion.
 */

import { isOAuthToken, createClient } from './lib/client.js';
import { buildSystemPrompt } from './lib/system-prompt.js';
import { convertToolsForOAuth } from './lib/tool-names.js';

/**
 * Register Anthropic plugin routes on the Express app.
 */
export function register(app, { utils, logger }) {
  const { normalizeMessages, validateAndRepairMessages, keyFingerprint } = utils;

  /**
   * Streaming messages endpoint
   *
   * POST /api/messages/stream
   */
  app.post('/api/messages/stream', async (req, res) => {
    const startTime = Date.now();
    const { apiKey, model, max_tokens, system, messages, tools, temperature, thinking, betas } = req.body;

    if (!apiKey) {
      return res.status(400).json({ error: 'Missing apiKey' });
    }

    if (!model) {
      return res.status(400).json({ error: 'Missing model' });
    }

    if (!messages || !Array.isArray(messages)) {
      return res.status(400).json({ error: 'Missing or invalid messages array' });
    }

    const isOAuth = isOAuthToken(apiKey);
    console.log('[Bridge:Anthropic] Streaming request:', {
      model,
      messageCount: messages.length,
      hasSystem: !!system,
      hasTools: !!tools,
      hasBetas: !!betas,
      isOAuth,
      timestamp: new Date().toISOString(),
    });

    try {
      const client = createClient(apiKey, { keyFingerprint });

      // Normalize messages: convert stringified JSON content to actual arrays,
      // then repair any orphaned tool_result blocks before sending to the API.
      const normalizedMessages = validateAndRepairMessages(normalizeMessages(messages));

      // Build request params
      const params = {
        model,
        max_tokens: max_tokens || 4096,
        messages: normalizedMessages,
      };

      // Add system prompt with Claude Code identity for OAuth
      const systemPrompt = buildSystemPrompt(isOAuth, system);
      if (systemPrompt) {
        params.system = systemPrompt;
      }

      // Convert tools for OAuth
      if (tools) {
        params.tools = convertToolsForOAuth(tools, isOAuth);
      }

      // Add temperature if specified
      if (temperature !== undefined) {
        params.temperature = temperature;
      }

      // Add extended thinking if specified
      if (thinking) {
        params.thinking = thinking;
      }

      // Forward beta feature flags (e.g. computer-use-2025-01-24)
      if (betas && Array.isArray(betas)) {
        params.betas = betas;
      }

      console.log('[Bridge:Anthropic] Request params:', {
        model: params.model,
        max_tokens: params.max_tokens,
        systemType: Array.isArray(params.system) ? 'array' : typeof params.system,
        systemLength: Array.isArray(params.system) ? params.system.length :
                      typeof params.system === 'string' ? params.system.length : 0,
        messageCount: params.messages.length,
        toolCount: params.tools?.length || 0,
        thinking: !!params.thinking,
        betas: params.betas || [],
      });

      // Set headers for SSE
      res.setHeader('Content-Type', 'text/event-stream');
      res.setHeader('Cache-Control', 'no-cache');
      res.setHeader('Connection', 'keep-alive');

      // Create streaming request
      const stream = client.messages.stream(params);

      let eventCount = 0;
      let textChunks = 0;

      // Stream events to client
      for await (const event of stream) {
        eventCount++;

        // Send event as SSE
        res.write(`data: ${JSON.stringify(event)}\n\n`);

        // Track text chunks
        if (event.type === 'content_block_delta' && event.delta?.type === 'text_delta') {
          textChunks++;
        }

        // Log progress every 100 events
        if (eventCount % 100 === 0) {
          console.log(`[Bridge:Anthropic] Streamed ${eventCount} events, ${textChunks} text chunks`);
        }
      }

      const duration = Date.now() - startTime;
      console.log('[Bridge:Anthropic] Streaming complete:', {
        eventCount,
        textChunks,
        durationMs: duration,
      });

      // Send completion marker
      res.write('data: [DONE]\n\n');
      res.end();

    } catch (error) {
      console.error('[Bridge:Anthropic] Streaming error:', error);

      const errorResponse = {
        type: 'error',
        error: {
          type: error.type || 'api_error',
          message: error.message,
        },
      };

      // Try to send error as SSE if headers not sent yet
      if (!res.headersSent) {
        res.setHeader('Content-Type', 'text/event-stream');
        res.write(`data: ${JSON.stringify(errorResponse)}\n\n`);
        res.end();
      } else {
        res.write(`data: ${JSON.stringify(errorResponse)}\n\n`);
        res.end();
      }
    }
  });

  /**
   * Non-streaming messages endpoint
   *
   * POST /api/messages
   */
  app.post('/api/messages', async (req, res) => {
    const startTime = Date.now();
    const { apiKey, model, max_tokens, system, messages, tools, temperature, thinking, betas } = req.body;

    if (!apiKey) {
      return res.status(400).json({ error: 'Missing apiKey' });
    }

    if (!model) {
      return res.status(400).json({ error: 'Missing model' });
    }

    if (!messages || !Array.isArray(messages)) {
      return res.status(400).json({ error: 'Missing or invalid messages array' });
    }

    const isOAuth = isOAuthToken(apiKey);
    console.log('[Bridge:Anthropic] Non-streaming request:', {
      model,
      messageCount: messages.length,
      hasSystem: !!system,
      hasTools: !!tools,
      hasBetas: !!betas,
      isOAuth,
      timestamp: new Date().toISOString(),
    });

    try {
      const client = createClient(apiKey, { keyFingerprint });

      // Normalize messages: convert stringified JSON content to actual arrays,
      // then repair any orphaned tool_result blocks before sending to the API.
      const normalizedMessages = validateAndRepairMessages(normalizeMessages(messages));

      // Build request params
      const params = {
        model,
        max_tokens: max_tokens || 4096,
        messages: normalizedMessages,
      };

      // Add system prompt with Claude Code identity for OAuth
      const systemPrompt = buildSystemPrompt(isOAuth, system);
      if (systemPrompt) {
        params.system = systemPrompt;
      }

      // Convert tools for OAuth
      if (tools) {
        params.tools = convertToolsForOAuth(tools, isOAuth);
      }

      // Add temperature if specified
      if (temperature !== undefined) {
        params.temperature = temperature;
      }

      // Add extended thinking if specified
      if (thinking) {
        params.thinking = thinking;
      }

      // Forward beta feature flags (e.g. computer-use-2025-01-24)
      if (betas && Array.isArray(betas)) {
        params.betas = betas;
      }

      console.log('[Bridge:Anthropic] Request params:', {
        model: params.model,
        max_tokens: params.max_tokens,
        systemType: Array.isArray(params.system) ? 'array' : typeof params.system,
        messageCount: params.messages.length,
        toolCount: params.tools?.length || 0,
        thinking: !!params.thinking,
        betas: params.betas || [],
      });

      // Make non-streaming request
      console.log('[Bridge:Anthropic] Sending non-streaming request to API...');
      const response = await client.messages.create(params);
      console.log('[Bridge:Anthropic] API responded after', Date.now() - startTime, 'ms');

      const duration = Date.now() - startTime;
      console.log('[Bridge:Anthropic] Request complete:', {
        id: response.id,
        model: response.model,
        stopReason: response.stop_reason,
        inputTokens: response.usage.input_tokens,
        outputTokens: response.usage.output_tokens,
        durationMs: duration,
      });

      res.json(response);

    } catch (error) {
      console.error('[Bridge:Anthropic] Request error:', error);

      res.status(error.status || 500).json({
        type: 'error',
        error: {
          type: error.type || 'api_error',
          message: error.message,
        },
      });
    }
  });

  /**
   * List available Anthropic models
   *
   * GET /api/models?apiKey=sk-ant-...
   */
  app.get('/api/models', async (req, res) => {
    const apiKey = req.query.apiKey || req.headers['x-api-key'];

    if (!apiKey) {
      return res.json([]);
    }

    console.log('[Bridge:Anthropic] Listing Anthropic models');

    try {
      const client = createClient(apiKey, { keyFingerprint });
      const response = await client.models.list({ limit: 100 });

      const models = [];
      for await (const model of response) {
        models.push({
          id: model.id,
          display_name: model.display_name || model.id,
          created_at: model.created_at,
        });
      }

      // Sort by display_name
      models.sort((a, b) => a.display_name.localeCompare(b.display_name));

      console.log(`[Bridge:Anthropic] Found ${models.length} Anthropic models`);
      res.json(models);
    } catch (error) {
      console.error('[Bridge:Anthropic] Failed to list Anthropic models:', error.message);
      res.json([]);
    }
  });
}

/**
 * Health check for this plugin.
 */
export function health() {
  return { status: 'ok', provider: 'anthropic' };
}
