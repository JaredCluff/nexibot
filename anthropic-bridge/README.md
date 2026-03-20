# NexiBot Anthropic Bridge Service

This Node.js service acts as a bridge between NexiBot (Rust/Tauri) and the official Anthropic TypeScript SDK, enabling OAuth token support.

## Why This Exists

Anthropic restricts OAuth tokens (from Claude Pro/Max subscriptions) to work only with Claude Code. The official TypeScript SDK has special handling for OAuth tokens that raw HTTP requests lack. This bridge service:

1. Uses the official `@anthropic-ai/sdk` with OAuth support
2. Mimics Claude Code's authentication headers exactly
3. Injects Claude Code identity into system prompts
4. Converts tool names to Claude Code canonical casing
5. Provides a simple HTTP/SSE interface for NexiBot

## Architecture

```
NexiBot (Rust/Tauri)
    │
    │ HTTP/SSE (port 18790)
    ▼
Bridge Service (Node.js)
    │
    │ @anthropic-ai/sdk
    ▼
Anthropic API
```

## Installation

```bash
cd anthropic-bridge
npm install
```

## Usage

### Start the Bridge

```bash
npm start
```

The bridge will listen on `http://127.0.0.1:18790` by default.

### Development Mode (Auto-restart)

```bash
npm run dev
```

### Custom Port

```bash
BRIDGE_PORT=9000 npm start
```

## API Endpoints

### Health Check

```bash
GET /health
```

Response:
```json
{
  "status": "healthy",
  "service": "nexibot-anthropic-bridge",
  "version": "1.0.0",
  "timestamp": "2026-02-07T20:00:00.000Z"
}
```

### Streaming Messages

```bash
POST /api/messages/stream
Content-Type: application/json

{
  "apiKey": "sk-ant-oat01-...",
  "model": "claude-sonnet-4-5-20250929",
  "max_tokens": 4096,
  "system": "You are a helpful assistant.",
  "messages": [
    {
      "role": "user",
      "content": "Hello!"
    }
  ],
  "tools": [...],
  "temperature": 0.7
}
```

Response: Server-Sent Events (SSE) stream

```
data: {"type":"message_start","message":{"id":"msg_01...","type":"message",...}}

data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}

...

data: [DONE]
```

### Non-Streaming Messages

```bash
POST /api/messages
Content-Type: application/json

{
  "apiKey": "sk-ant-oat01-...",
  "model": "claude-sonnet-4-5-20250929",
  "max_tokens": 4096,
  "system": "You are a helpful assistant.",
  "messages": [
    {
      "role": "user",
      "content": "Hello!"
    }
  ]
}
```

Response: Complete message object

```json
{
  "id": "msg_01...",
  "type": "message",
  "role": "assistant",
  "content": [
    {
      "type": "text",
      "text": "Hello! How can I help you today?"
    }
  ],
  "model": "claude-sonnet-4-5-20250929",
  "stop_reason": "end_turn",
  "usage": {
    "input_tokens": 10,
    "output_tokens": 15
  }
}
```

## OAuth Token Detection

The bridge automatically detects OAuth tokens by checking if the API key contains `sk-ant-oat` (OAuth Access Token).

When an OAuth token is detected, the bridge:
1. Uses `authToken` parameter instead of `apiKey`
2. Adds Claude Code impersonation headers
3. Injects "You are Claude Code" system prompt
4. Converts tool names to CC casing (`read` → `Read`, `write` → `Write`, etc.)

## Logging

The bridge logs all requests with details:

```
[Bridge] Streaming request: { model: 'claude-sonnet-4-5-20250929', messageCount: 2, hasSystem: true, hasTools: false, isOAuth: true, ... }
[Bridge] Request params: { model: 'claude-sonnet-4-5-20250929', max_tokens: 4096, systemType: 'array', ... }
[Bridge] Streamed 100 events, 87 text chunks
[Bridge] Streaming complete: { eventCount: 234, textChunks: 201, durationMs: 3542 }
```

## Error Handling

Errors are returned as JSON:

```json
{
  "type": "error",
  "error": {
    "type": "invalid_request_error",
    "message": "This credential is only authorized for use with Claude Code..."
  }
}
```

For streaming requests, errors are sent as SSE events:

```
data: {"type":"error","error":{"type":"api_error","message":"..."}}
```

## Security

- The bridge only listens on `127.0.0.1` (localhost), not accessible from external networks
- API keys are never logged (only prefixes for debugging)
- CORS is enabled for local development only

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health check |
| GET | `/api/models` | List Anthropic models (requires x-api-key header) |
| GET | `/api/openai/models` | List OpenAI chat models (requires x-api-key header) |
| POST | `/api/messages/stream` | Streaming Claude messages (SSE) |
| POST | `/api/messages` | Non-streaming Claude messages |
| POST | `/api/openai/messages/stream` | Streaming OpenAI messages (normalized to Anthropic SSE format) |
| POST | `/api/openai/messages` | Non-streaming OpenAI messages (normalized to Anthropic response shape) |
| POST | `/api/search` | DuckDuckGo search proxy |

## Dependencies

- `@anthropic-ai/sdk` ^0.77.0 - Official Anthropic TypeScript SDK (with models.list API)
- `openai` ^4.67.0 - OpenAI SDK for multi-provider support
- `express` ^4.21.2 - Web framework
- `cors` ^2.8.5 - CORS middleware

## License

Same as NexiBot parent project
