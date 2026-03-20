/**
 * NexiBot Anthropic Bridge Service
 *
 * This service acts as a bridge between NexiBot (Rust/Tauri) and the official
 * Anthropic TypeScript SDK. It enables OAuth token support by leveraging the
 * SDK's special handling for OAuth authentication.
 *
 * Architecture:
 *   NexiBot (Rust) → HTTP/SSE → Bridge (Node.js/SDK) → Anthropic API
 */

import Anthropic from '@anthropic-ai/sdk';
import OpenAI from 'openai';
import express from 'express';
import cors from 'cors';
import { createHash } from 'node:crypto';

/**
 * Return the first 8 hex chars of SHA-256(key) for log identification
 * without exposing any key material.
 */
function keyFingerprint(key) {
  return createHash('sha256').update(key).digest('hex').substring(0, 8);
}

const app = express();
const PORT = process.env.BRIDGE_PORT || 18790;

// Restrict CORS to localhost origins only — the bridge should never be
// accessible from arbitrary web pages.
app.use(cors({
  origin: [
    'http://127.0.0.1',
    'http://localhost',
    'https://tauri.localhost',
    /^http:\/\/127\.0\.0\.1:\d+$/,
    /^http:\/\/localhost:\d+$/,
  ],
}));
app.use(express.json({ limit: '10mb' }));

/**
 * Check if an API key is an OAuth token
 */
function isOAuthToken(apiKey) {
  return apiKey && apiKey.includes('sk-ant-oat');
}

/**
 * Create Anthropic client with proper OAuth handling
 */
function createClient(apiKey, options = {}) {
  const isOAuth = isOAuthToken(apiKey);

  if (isOAuth) {
    console.log('[Bridge] Creating OAuth client');
    console.log('[Bridge] Token fingerprint (sha256):', keyFingerprint(apiKey));

    // Mimic Claude Code headers exactly (from OpenClaw/pi-ai)
    const defaultHeaders = {
      'accept': 'application/json',
      'anthropic-dangerous-direct-browser-access': 'true',
      'anthropic-beta': 'claude-code-20250219,oauth-2025-04-20,fine-grained-tool-streaming-2025-05-14,interleaved-thinking-2025-05-14',
      'user-agent': 'claude-cli/2.1.2 (external, cli)',
      'x-app': 'cli',
    };

    // CRITICAL: Use authToken parameter for OAuth, NOT apiKey
    return new Anthropic({
      apiKey: null,
      authToken: apiKey,
      defaultHeaders,
      dangerouslyAllowBrowser: true,
      timeout: 600_000, // 10 min — matches OpenClaw's DEFAULT_AGENT_TIMEOUT_SECONDS
    });
  }

  console.log('[Bridge] Creating API key client');
  console.log('[Bridge] API key fingerprint (sha256):', keyFingerprint(apiKey));

  // Regular API key client
  return new Anthropic({
    apiKey: apiKey,
    dangerouslyAllowBrowser: true,
    timeout: 600_000, // 10 min — matches OpenClaw's DEFAULT_AGENT_TIMEOUT_SECONDS
  });
}

/**
 * Build system prompt with Claude Code identity for OAuth tokens
 */
function buildSystemPrompt(isOAuth, userSystemPrompt) {
  if (isOAuth) {
    const ccIdentity = {
      type: 'text',
      text: 'You are Claude Code, Anthropic\'s official CLI for Claude.',
    };

    if (userSystemPrompt) {
      if (typeof userSystemPrompt === 'string') {
        return [
          ccIdentity,
          { type: 'text', text: userSystemPrompt }
        ];
      } else if (Array.isArray(userSystemPrompt)) {
        return [ccIdentity, ...userSystemPrompt];
      }
    }

    return [ccIdentity];
  }

  return userSystemPrompt;
}

/**
 * Convert tool names to Claude Code canonical casing for OAuth
 */
const claudeCodeTools = [
  'Read', 'Write', 'Edit', 'Bash', 'Grep', 'Glob',
  'AskUserQuestion', 'EnterPlanMode', 'ExitPlanMode',
  'KillShell', 'NotebookEdit', 'Skill', 'Task',
  'TaskOutput', 'TodoWrite', 'WebFetch', 'WebSearch',
];

const ccToolLookup = new Map(
  claudeCodeTools.map(t => [t.toLowerCase(), t])
);

function toClaudeCodeName(name) {
  return ccToolLookup.get(name.toLowerCase()) || name;
}

function convertToolsForOAuth(tools, isOAuth) {
  if (!tools || !Array.isArray(tools)) return tools;
  if (!isOAuth) return tools;

  return tools.map(tool => ({
    ...tool,
    name: toClaudeCodeName(tool.name),
  }));
}

/**
 * Normalize message content for the Anthropic API.
 *
 * The Rust client stores assistant content blocks (tool_use) and user content
 * blocks (tool_result) as serialized JSON strings in Message.content.
 * The API expects these as actual JSON arrays, not strings.
 * This function detects stringified arrays and parses them back.
 */
function normalizeMessages(messages) {
  return messages.map(msg => {
    if (typeof msg.content === 'string' && msg.content.trimStart().startsWith('[')) {
      try {
        const parsed = JSON.parse(msg.content);
        if (Array.isArray(parsed) && parsed.length > 0 && typeof parsed[0] === 'object' && parsed[0].type) {
          return { ...msg, content: parsed };
        }
      } catch {
        // Not valid JSON — leave as string
      }
    }
    return msg;
  });
}

/**
 * Validate and repair tool_use/tool_result pairing in normalized messages.
 *
 * The Anthropic API requires that every tool_result block in a user message
 * references a tool_use block in the IMMEDIATELY PRECEDING assistant message.
 * History trimming in the Rust client can occasionally produce mismatched pairs;
 * this function strips orphaned tool_result blocks before the API call so the
 * request is never rejected with "unexpected tool_use_id".
 *
 * Must be called AFTER normalizeMessages so content is already parsed to arrays.
 */
function validateAndRepairMessages(messages) {
  const repaired = [...messages];
  let repairs = 0;

  for (let i = 1; i < repaired.length; i++) {
    const msg = repaired[i];
    if (msg.role !== 'user') continue;
    const content = msg.content;
    if (!Array.isArray(content)) continue;

    const toolResults = content.filter(b => b.type === 'tool_result');
    if (toolResults.length === 0) continue;

    const prev = repaired[i - 1];
    const prevContent = (prev && Array.isArray(prev.content)) ? prev.content : [];
    const validIds = new Set(
      prevContent.filter(b => b.type === 'tool_use' && b.id).map(b => b.id)
    );

    if (prev && prev.role !== 'assistant') {
      // No preceding assistant message at all — every tool_result here is orphaned
      console.warn(`[Bridge] Repair: user[${i}] has tool_results but no preceding assistant — removing all`);
      const cleaned = content.filter(b => b.type !== 'tool_result');
      repairs += toolResults.length;
      if (cleaned.length === 0) {
        repaired.splice(i, 1);
        i--;
      } else {
        repaired[i] = { ...msg, content: cleaned };
      }
      continue;
    }

    const cleanedContent = content.filter(b => {
      if (b.type !== 'tool_result') return true;
      if (validIds.has(b.tool_use_id)) return true;
      console.warn(`[Bridge] Repair: removing orphaned tool_result ${b.tool_use_id} (not in preceding assistant tool_uses)`);
      repairs++;
      return false;
    });

    if (cleanedContent.length !== content.length) {
      if (cleanedContent.length === 0) {
        repaired.splice(i, 1);
        i--;
      } else {
        repaired[i] = { ...msg, content: cleanedContent };
      }
    }
  }

  if (repairs > 0) {
    console.warn(`[Bridge] Repaired ${repairs} orphaned tool_result block(s) before API call`);
  }
  return repaired;
}

/**
 * Health check endpoint
 */
app.get('/health', (req, res) => {
  res.json({
    status: 'healthy',
    service: 'nexibot-anthropic-bridge',
    version: '1.0.0',
    timestamp: new Date().toISOString(),
  });
});

/**
 * Streaming messages endpoint
 *
 * POST /api/messages/stream
 *
 * Request body:
 * {
 *   "apiKey": "sk-ant-...",
 *   "model": "claude-sonnet-4-5-20250929",
 *   "max_tokens": 4096,
 *   "system": "...",
 *   "messages": [...],
 *   "tools": [...],
 *   "temperature": 0.7,
 *   "betas": ["computer-use-2025-01-24"]
 * }
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
  console.log('[Bridge] Streaming request:', {
    model,
    messageCount: messages.length,
    hasSystem: !!system,
    hasTools: !!tools,
    hasBetas: !!betas,
    isOAuth,
    timestamp: new Date().toISOString(),
  });

  try {
    const client = createClient(apiKey);

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

    console.log('[Bridge] Request params:', {
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
        console.log(`[Bridge] Streamed ${eventCount} events, ${textChunks} text chunks`);
      }
    }

    const duration = Date.now() - startTime;
    console.log('[Bridge] Streaming complete:', {
      eventCount,
      textChunks,
      durationMs: duration,
    });

    // Send completion marker
    res.write('data: [DONE]\n\n');
    res.end();

  } catch (error) {
    console.error('[Bridge] Streaming error:', error);

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
 *
 * Same request body as /api/messages/stream
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
  console.log('[Bridge] Non-streaming request:', {
    model,
    messageCount: messages.length,
    hasSystem: !!system,
    hasTools: !!tools,
    hasBetas: !!betas,
    isOAuth,
    timestamp: new Date().toISOString(),
  });

  try {
    const client = createClient(apiKey);

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

    console.log('[Bridge] Request params:', {
      model: params.model,
      max_tokens: params.max_tokens,
      systemType: Array.isArray(params.system) ? 'array' : typeof params.system,
      messageCount: params.messages.length,
      toolCount: params.tools?.length || 0,
      thinking: !!params.thinking,
      betas: params.betas || [],
    });

    // Make non-streaming request
    console.log('[Bridge] Sending non-streaming request to API...');
    const response = await client.messages.create(params);
    console.log('[Bridge] API responded after', Date.now() - startTime, 'ms');

    const duration = Date.now() - startTime;
    console.log('[Bridge] Request complete:', {
      id: response.id,
      model: response.model,
      stopReason: response.stop_reason,
      inputTokens: response.usage.input_tokens,
      outputTokens: response.usage.output_tokens,
      durationMs: duration,
    });

    res.json(response);

  } catch (error) {
    console.error('[Bridge] Request error:', error);

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
 * Create OpenAI client
 */
function createOpenAIClient(apiKey) {
  console.log('[Bridge] Creating OpenAI client');
  return new OpenAI({ apiKey });
}

/**
 * Convert OpenAI messages from Anthropic format.
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
 * OpenAI non-streaming messages endpoint
 *
 * POST /api/openai/messages
 *
 * Request body matches Anthropic format, response is normalized to Anthropic shape.
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

  console.log('[Bridge] OpenAI non-streaming request:', {
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
    console.log('[Bridge] OpenAI request complete:', {
      id: response.id,
      model: response.model,
      stopReason: normalized.stop_reason,
      inputTokens: response.usage?.prompt_tokens,
      outputTokens: response.usage?.completion_tokens,
      durationMs: duration,
    });

    res.json(normalized);

  } catch (error) {
    console.error('[Bridge] OpenAI request error:', error);
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

  console.log('[Bridge] OpenAI streaming request:', {
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

        for (const tcIndex in currentToolCalls) {
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
    console.log('[Bridge] OpenAI streaming complete:', {
      textChunks,
      toolCalls: Object.keys(currentToolCalls).length,
      durationMs: duration,
    });

    res.write('data: [DONE]\n\n');
    res.end();

  } catch (error) {
    console.error('[Bridge] OpenAI streaming error:', error);

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
 * List available Anthropic models
 *
 * GET /api/models?apiKey=sk-ant-...
 */
app.get('/api/models', async (req, res) => {
  const apiKey = req.query.apiKey || req.headers['x-api-key'];

  if (!apiKey) {
    return res.json([]);
  }

  console.log('[Bridge] Listing Anthropic models');

  try {
    const client = createClient(apiKey);
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

    console.log(`[Bridge] Found ${models.length} Anthropic models`);
    res.json(models);
  } catch (error) {
    console.error('[Bridge] Failed to list Anthropic models:', error.message);
    res.json([]);
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
    console.log('[Bridge] Listing models via ChatGPT backend API (OAuth token)');
    try {
      const response = await fetch('https://chatgpt.com/backend-api/codex/models?client_version=0.99.0', {
        headers: { 'Authorization': `Bearer ${apiKey}` },
      });

      if (!response.ok) {
        const errorText = await response.text();
        console.error('[Bridge] ChatGPT models API failed:', response.status, errorText);
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

      console.log(`[Bridge] Found ${models.length} ChatGPT models`);
      res.json(models);
    } catch (error) {
      console.error('[Bridge] Failed to list ChatGPT models:', error.message);
      res.json([]);
    }
    return;
  }

  console.log('[Bridge] Listing OpenAI models via API key');

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

    console.log(`[Bridge] Found ${models.length} OpenAI chat models`);
    res.json(models);
  } catch (error) {
    console.error('[Bridge] Failed to list OpenAI models:', error.message);
    res.json([]);
  }
});

/**
 * DuckDuckGo search proxy
 *
 * Node.js fetch has a browser-like TLS fingerprint that avoids CAPTCHAs.
 * Rust's reqwest gets blocked by DuckDuckGo's bot detection.
 *
 * POST /api/search
 * { "query": "search terms", "num_results": 10 }
 */
app.post('/api/search', async (req, res) => {
  const { query, num_results = 10 } = req.body;

  if (!query) {
    return res.status(400).json({ error: 'Missing query' });
  }

  console.log('[Bridge] DuckDuckGo search:', { query, num_results });

  try {
    const encoded = query.replace(/ /g, '+').replace(/[^\w+.-]/g, c =>
      '%' + c.charCodeAt(0).toString(16).toUpperCase().padStart(2, '0')
    );

    const response = await fetch('https://html.duckduckgo.com/html/', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/x-www-form-urlencoded',
        'Referer': 'https://duckduckgo.com/',
        'User-Agent': 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
      },
      body: `q=${encoded}&b=&kl=&df=`,
    });

    if (!response.ok) {
      return res.status(502).json({ error: `DuckDuckGo returned ${response.status}` });
    }

    const html = await response.text();

    // Check for CAPTCHA
    if (html.includes('anomaly') && html.includes('botnet')) {
      return res.status(503).json({ error: 'DuckDuckGo returned CAPTCHA' });
    }

    // Parse results from HTML
    const results = [];
    const blocks = html.split('class="links_main');

    for (let i = 1; i < blocks.length && results.length < num_results; i++) {
      const block = blocks[i];

      // Extract URL from href before result__a
      const aPos = block.indexOf('class="result__a"');
      if (aPos === -1) continue;

      const before = block.substring(0, aPos);
      const hrefMatch = before.match(/href="([^"]+)"/g);
      if (!hrefMatch) continue;

      const lastHref = hrefMatch[hrefMatch.length - 1];
      let url = lastHref.replace('href="', '').replace('"', '');

      // Unwrap DuckDuckGo redirect
      const uddgIdx = url.indexOf('uddg=');
      if (uddgIdx !== -1) {
        const encoded = url.substring(uddgIdx + 5);
        const ampIdx = encoded.indexOf('&');
        url = decodeURIComponent(ampIdx !== -1 ? encoded.substring(0, ampIdx) : encoded);
      } else if (url.startsWith('//')) {
        url = 'https:' + url;
      }

      // Extract title
      const after = block.substring(aPos);
      const titleMatch = after.match(/class="result__a"[^>]*>([^<]*(?:<[^/][^>]*>[^<]*)*)<\/a>/);
      const title = titleMatch ? titleMatch[1].replace(/<[^>]+>/g, '').trim() : '';

      // Extract snippet
      const snippetMatch = block.match(/class="result__snippet"[^>]*>([^<]*(?:<[^/][^>]*>[^<]*)*)<\/(?:a|span|td)>/);
      const snippet = snippetMatch ? snippetMatch[1].replace(/<[^>]+>/g, '').trim() : '';

      if (title && url && !url.includes('duckduckgo.com')) {
        results.push({ title, url, snippet });
      }
    }

    console.log(`[Bridge] DuckDuckGo returned ${results.length} results`);
    res.json({ results });

  } catch (error) {
    console.error('[Bridge] DuckDuckGo search error:', error.message);
    res.status(500).json({ error: error.message });
  }
});

/**
 * Shutdown handler
 */
process.on('SIGINT', () => {
  console.log('[Bridge] Shutting down...');
  process.exit(0);
});

process.on('SIGTERM', () => {
  console.log('[Bridge] Shutting down...');
  process.exit(0);
});

/**
 * Start server
 */
app.listen(PORT, '127.0.0.1', () => {
  console.log('╔═══════════════════════════════════════════════════════════╗');
  console.log('║         NexiBot Anthropic Bridge Service                 ║');
  console.log('╚═══════════════════════════════════════════════════════════╝');
  console.log('');
  console.log(`✓ Bridge listening on http://127.0.0.1:${PORT}`);
  console.log(`✓ Using official Anthropic TypeScript SDK`);
  console.log(`✓ OAuth token support enabled`);
  console.log('');
  console.log('Endpoints:');
  console.log(`  GET  http://127.0.0.1:${PORT}/health`);
  console.log(`  GET  http://127.0.0.1:${PORT}/api/models`);
  console.log(`  GET  http://127.0.0.1:${PORT}/api/openai/models`);
  console.log(`  POST http://127.0.0.1:${PORT}/api/messages/stream`);
  console.log(`  POST http://127.0.0.1:${PORT}/api/messages`);
  console.log(`  POST http://127.0.0.1:${PORT}/api/openai/messages/stream`);
  console.log(`  POST http://127.0.0.1:${PORT}/api/openai/messages`);
  console.log('');
  console.log('Press Ctrl+C to stop');
  console.log('');
});
