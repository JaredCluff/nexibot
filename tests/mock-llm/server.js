/**
 * Mock LLM Server — Record/Replay Proxy
 *
 * Modes:
 *   record  — proxy to real API, save request/response pairs to fixtures/
 *   replay  — serve from fixtures only, fail on cache miss
 *   auto    — replay if cached, record if not (default)
 *
 * Supports:
 *   - Anthropic Messages API (/v1/messages, streaming + non-streaming)
 *   - OpenAI Chat Completions API (/v1/chat/completions)
 *   - Health check (/health)
 *
 * Environment:
 *   MOCK_PORT          — listen port (default: 18799)
 *   MOCK_MODE          — record | replay | auto (default: auto)
 *   ANTHROPIC_API_KEY  — required for record mode (Anthropic)
 *   OPENAI_API_KEY     — required for record mode (OpenAI)
 *   FIXTURES_DIR       — path to fixtures directory (default: ./fixtures)
 */

import express from 'express';
import cors from 'cors';
import crypto from 'crypto';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const PORT = parseInt(process.env.MOCK_PORT || '18799', 10);
const MODE = process.env.MOCK_MODE || 'auto';
const FIXTURES_DIR = process.env.FIXTURES_DIR || path.join(__dirname, 'fixtures');
const ANTHROPIC_API_KEY = process.env.ANTHROPIC_API_KEY || '';
const OPENAI_API_KEY = process.env.OPENAI_API_KEY || '';

// Ensure fixtures dir exists
fs.mkdirSync(FIXTURES_DIR, { recursive: true });

const app = express();
app.use(cors());

// Parse JSON body but also keep raw body for hashing
app.use(express.json({ limit: '10mb' }));

// ─── Helpers ────────────────────────────────────────────────────────────────

/**
 * Generate a stable cache key from a request body.
 * We hash the model + messages + tools (ignoring ephemeral fields like stream, max_tokens).
 */
function cacheKey(body, provider) {
  const stable = {
    provider,
    model: body.model || 'unknown',
    messages: body.messages || [],
    tools: body.tools || [],
    system: body.system || '',
  };
  const hash = crypto.createHash('sha256')
    .update(JSON.stringify(stable))
    .digest('hex')
    .substring(0, 16);
  const modelSlug = (body.model || 'unknown').replace(/[^a-zA-Z0-9-]/g, '_');
  return `${provider}_${modelSlug}_${hash}`;
}

function fixturePath(key) {
  return path.join(FIXTURES_DIR, `${key}.json`);
}

function fixtureExists(key) {
  return fs.existsSync(fixturePath(key));
}

function loadFixture(key) {
  return JSON.parse(fs.readFileSync(fixturePath(key), 'utf-8'));
}

function saveFixture(key, data) {
  fs.writeFileSync(fixturePath(key), JSON.stringify(data, null, 2), 'utf-8');
  console.log(`[MOCK] Saved fixture: ${key}`);
}

let stats = { hits: 0, misses: 0, errors: 0, recordings: 0 };

// ─── Anthropic Messages API ─────────────────────────────────────────────────

app.post('/v1/messages', async (req, res) => {
  const body = req.body;
  const isStreaming = body.stream === true;
  const key = cacheKey(body, 'anthropic');

  console.log(`[MOCK] Anthropic request: model=${body.model}, stream=${isStreaming}, key=${key}`);

  // Check cache
  if (fixtureExists(key)) {
    stats.hits++;
    console.log(`[MOCK] Cache HIT: ${key}`);
    const fixture = loadFixture(key);

    if (isStreaming) {
      return sendAnthropicStream(res, fixture);
    }
    return res.status(200).json(fixture.response);
  }

  // Cache miss
  stats.misses++;
  console.log(`[MOCK] Cache MISS: ${key}`);

  if (MODE === 'replay') {
    return res.status(503).json({
      error: { type: 'mock_error', message: `No fixture found for key: ${key}. Run in record mode first.` }
    });
  }

  // Record mode: proxy to real API
  if (!ANTHROPIC_API_KEY) {
    return res.status(500).json({
      error: { type: 'config_error', message: 'ANTHROPIC_API_KEY not set. Cannot record.' }
    });
  }

  try {
    // Always request non-streaming for recording, then convert
    const proxyBody = { ...body, stream: false };
    const upstream = await fetch('https://api.anthropic.com/v1/messages', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'x-api-key': ANTHROPIC_API_KEY,
        'anthropic-version': '2023-06-01',
      },
      body: JSON.stringify(proxyBody),
    });

    const responseBody = await upstream.json();

    if (!upstream.ok) {
      stats.errors++;
      console.error(`[MOCK] Upstream error: ${upstream.status}`, responseBody);
      return res.status(upstream.status).json(responseBody);
    }

    // Save fixture
    const fixture = {
      request: { model: body.model, messages: body.messages, system: body.system, tools: body.tools },
      response: responseBody,
      recorded_at: new Date().toISOString(),
    };
    saveFixture(key, fixture);
    stats.recordings++;

    if (isStreaming) {
      return sendAnthropicStream(res, fixture);
    }
    return res.status(200).json(responseBody);

  } catch (err) {
    stats.errors++;
    console.error(`[MOCK] Proxy error:`, err.message);
    return res.status(502).json({
      error: { type: 'proxy_error', message: err.message }
    });
  }
});

/**
 * Convert a non-streaming Anthropic response into SSE stream format.
 */
function sendAnthropicStream(res, fixture) {
  const response = fixture.response;

  res.setHeader('Content-Type', 'text/event-stream');
  res.setHeader('Cache-Control', 'no-cache');
  res.setHeader('Connection', 'keep-alive');

  // message_start
  res.write(`event: message_start\ndata: ${JSON.stringify({
    type: 'message_start',
    message: {
      id: response.id || 'msg_mock_' + Date.now(),
      type: 'message',
      role: 'assistant',
      content: [],
      model: response.model,
      stop_reason: null,
      usage: { input_tokens: response.usage?.input_tokens || 0, output_tokens: 0 },
    }
  })}\n\n`);

  // Send each content block
  for (let i = 0; i < (response.content || []).length; i++) {
    const block = response.content[i];

    // content_block_start
    if (block.type === 'text') {
      res.write(`event: content_block_start\ndata: ${JSON.stringify({
        type: 'content_block_start', index: i,
        content_block: { type: 'text', text: '' }
      })}\n\n`);

      // Stream text in chunks
      const text = block.text || '';
      const chunkSize = 20;
      for (let j = 0; j < text.length; j += chunkSize) {
        const chunk = text.substring(j, j + chunkSize);
        res.write(`event: content_block_delta\ndata: ${JSON.stringify({
          type: 'content_block_delta', index: i,
          delta: { type: 'text_delta', text: chunk }
        })}\n\n`);
      }
    } else if (block.type === 'tool_use') {
      res.write(`event: content_block_start\ndata: ${JSON.stringify({
        type: 'content_block_start', index: i,
        content_block: { type: 'tool_use', id: block.id, name: block.name, input: {} }
      })}\n\n`);

      // Send tool input as JSON delta
      const inputStr = JSON.stringify(block.input || {});
      res.write(`event: content_block_delta\ndata: ${JSON.stringify({
        type: 'content_block_delta', index: i,
        delta: { type: 'input_json_delta', partial_json: inputStr }
      })}\n\n`);
    }

    // content_block_stop
    res.write(`event: content_block_stop\ndata: ${JSON.stringify({
      type: 'content_block_stop', index: i
    })}\n\n`);
  }

  // message_delta + message_stop
  res.write(`event: message_delta\ndata: ${JSON.stringify({
    type: 'message_delta',
    delta: { stop_reason: response.stop_reason || 'end_turn' },
    usage: { output_tokens: response.usage?.output_tokens || 0 }
  })}\n\n`);

  res.write(`event: message_stop\ndata: ${JSON.stringify({ type: 'message_stop' })}\n\n`);
  res.end();
}

// ─── OpenAI Chat Completions API ────────────────────────────────────────────

app.post('/v1/chat/completions', async (req, res) => {
  const body = req.body;
  const isStreaming = body.stream === true;
  const key = cacheKey(body, 'openai');

  console.log(`[MOCK] OpenAI request: model=${body.model}, stream=${isStreaming}, key=${key}`);

  if (fixtureExists(key)) {
    stats.hits++;
    console.log(`[MOCK] Cache HIT: ${key}`);
    const fixture = loadFixture(key);

    if (isStreaming) {
      return sendOpenAIStream(res, fixture);
    }
    return res.status(200).json(fixture.response);
  }

  stats.misses++;
  console.log(`[MOCK] Cache MISS: ${key}`);

  if (MODE === 'replay') {
    return res.status(503).json({
      error: { message: `No fixture found for key: ${key}. Run in record mode first.`, type: 'mock_error' }
    });
  }

  if (!OPENAI_API_KEY) {
    return res.status(500).json({
      error: { message: 'OPENAI_API_KEY not set. Cannot record.', type: 'config_error' }
    });
  }

  try {
    const proxyBody = { ...body, stream: false };
    const upstream = await fetch('https://api.openai.com/v1/chat/completions', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Authorization': `Bearer ${OPENAI_API_KEY}`,
      },
      body: JSON.stringify(proxyBody),
    });

    const responseBody = await upstream.json();

    if (!upstream.ok) {
      stats.errors++;
      return res.status(upstream.status).json(responseBody);
    }

    const fixture = {
      request: { model: body.model, messages: body.messages, tools: body.tools },
      response: responseBody,
      recorded_at: new Date().toISOString(),
    };
    saveFixture(key, fixture);
    stats.recordings++;

    if (isStreaming) {
      return sendOpenAIStream(res, fixture);
    }
    return res.status(200).json(responseBody);

  } catch (err) {
    stats.errors++;
    return res.status(502).json({ error: { message: err.message, type: 'proxy_error' } });
  }
});

function sendOpenAIStream(res, fixture) {
  const response = fixture.response;
  const choice = response.choices?.[0] || {};
  const message = choice.message || {};

  res.setHeader('Content-Type', 'text/event-stream');
  res.setHeader('Cache-Control', 'no-cache');

  // role chunk
  res.write(`data: ${JSON.stringify({
    id: response.id || 'chatcmpl-mock',
    object: 'chat.completion.chunk',
    model: response.model,
    choices: [{ index: 0, delta: { role: 'assistant', content: '' }, finish_reason: null }]
  })}\n\n`);

  // content chunks
  const text = message.content || '';
  const chunkSize = 20;
  for (let i = 0; i < text.length; i += chunkSize) {
    res.write(`data: ${JSON.stringify({
      id: response.id || 'chatcmpl-mock',
      object: 'chat.completion.chunk',
      model: response.model,
      choices: [{ index: 0, delta: { content: text.substring(i, i + chunkSize) }, finish_reason: null }]
    })}\n\n`);
  }

  // tool_calls if present
  if (message.tool_calls) {
    for (const tc of message.tool_calls) {
      res.write(`data: ${JSON.stringify({
        id: response.id || 'chatcmpl-mock',
        object: 'chat.completion.chunk',
        model: response.model,
        choices: [{ index: 0, delta: { tool_calls: [tc] }, finish_reason: null }]
      })}\n\n`);
    }
  }

  // finish
  res.write(`data: ${JSON.stringify({
    id: response.id || 'chatcmpl-mock',
    object: 'chat.completion.chunk',
    model: response.model,
    choices: [{ index: 0, delta: {}, finish_reason: choice.finish_reason || 'stop' }]
  })}\n\n`);

  res.write('data: [DONE]\n\n');
  res.end();
}

// ─── Utility Endpoints ──────────────────────────────────────────────────────

app.get('/health', (_req, res) => {
  res.json({ status: 'ok', mode: MODE, stats });
});

app.get('/stats', (_req, res) => {
  // List all fixtures
  const fixtures = fs.readdirSync(FIXTURES_DIR)
    .filter(f => f.endsWith('.json'))
    .map(f => {
      const data = JSON.parse(fs.readFileSync(path.join(FIXTURES_DIR, f), 'utf-8'));
      return {
        key: f.replace('.json', ''),
        model: data.request?.model,
        recorded_at: data.recorded_at,
      };
    });
  res.json({ mode: MODE, stats, fixtures });
});

app.delete('/fixtures', (_req, res) => {
  const files = fs.readdirSync(FIXTURES_DIR).filter(f => f.endsWith('.json'));
  for (const f of files) {
    fs.unlinkSync(path.join(FIXTURES_DIR, f));
  }
  res.json({ deleted: files.length });
});

// ─── Models endpoint (for bridge compatibility) ─────────────────────────────

app.get('/v1/models', (_req, res) => {
  res.json({
    data: [
      { id: 'claude-sonnet-4-20250514', object: 'model', owned_by: 'anthropic' },
      { id: 'claude-haiku-4-5-20251001', object: 'model', owned_by: 'anthropic' },
      { id: 'gpt-4o', object: 'model', owned_by: 'openai' },
    ]
  });
});

// ─── Start ──────────────────────────────────────────────────────────────────

app.listen(PORT, () => {
  console.log(`[MOCK LLM] Running on http://127.0.0.1:${PORT}`);
  console.log(`[MOCK LLM] Mode: ${MODE}`);
  console.log(`[MOCK LLM] Fixtures: ${FIXTURES_DIR}`);
  console.log(`[MOCK LLM] Anthropic key: ${ANTHROPIC_API_KEY ? 'configured' : 'NOT SET'}`);
  console.log(`[MOCK LLM] OpenAI key: ${OPENAI_API_KEY ? 'configured' : 'NOT SET'}`);
});
