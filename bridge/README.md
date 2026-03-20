# NexiBot Bridge Service

Plugin-based Node.js service that bridges NexiBot (Rust/Tauri) to provider SDKs. Each provider is a plugin that can be independently added or removed.

## Architecture

```
NexiBot (Rust/Tauri)
    |
    | HTTP/SSE (port 18790)
    v
Bridge Service (Node.js + Express)
    |
    +-- plugins/anthropic/  --> Anthropic API (@anthropic-ai/sdk)
    +-- plugins/openai/     --> OpenAI API (openai SDK)
    +-- lib/search.js       --> DuckDuckGo search proxy
```

## Quick Start

```bash
cd bridge
npm install                                    # Core deps (express, cors)
cd plugins/anthropic && npm install && cd ../.. # Anthropic plugin deps
cd plugins/openai && npm install && cd ../..    # OpenAI plugin deps
npm start                                       # Start bridge on port 18790
```

Or use the startup script: `./start-bridge.sh` (installs all deps automatically).

## Plugin System

Plugins are loaded from two locations at startup:

1. **Built-in**: `bridge/plugins/` (shipped with NexiBot)
2. **External**: `BRIDGE_PLUGINS_DIR` env var path (user-installed)

### Plugin Structure

```
my-plugin/
  plugin.json     # Manifest (name, version, bridge_api_version, entry)
  index.js        # Entry: exports register(app, context) and health()
  package.json    # Plugin-specific npm dependencies
```

### plugin.json

```json
{
  "name": "my-plugin",
  "version": "1.0.0",
  "description": "My bridge plugin",
  "bridge_api_version": "1",
  "entry": "index.js"
}
```

### Plugin Entry Module

```js
export function register(app, { utils, logger }) {
  // utils.normalizeMessages()
  // utils.validateAndRepairMessages()
  // utils.keyFingerprint()
  app.post('/api/my-endpoint', async (req, res) => { ... });
}

export function health() {
  return { status: 'ok', provider: 'my-provider' };
}
```

## API Endpoints

### Core (always available)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health check with loaded plugin list |
| POST | `/api/search` | DuckDuckGo search proxy |

### Anthropic Plugin

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/messages/stream` | Streaming Claude messages (SSE) |
| POST | `/api/messages` | Non-streaming Claude messages |
| GET | `/api/models` | List Anthropic models |

### OpenAI Plugin

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/openai/messages/stream` | Streaming OpenAI messages (Anthropic SSE format) |
| POST | `/api/openai/messages` | Non-streaming OpenAI messages (Anthropic format) |
| GET | `/api/openai/models` | List OpenAI chat models |

## External Plugins

To install a plugin from a separate repo:

```bash
# Example: install the Anthropic plugin from a private repo
mkdir -p ~/.config/nexibot/bridge-plugins
cd ~/.config/nexibot/bridge-plugins
git clone <repo-url> nexibot-bridge-anthropic
cd nexibot-bridge-anthropic && npm install

# Tell the bridge where to find external plugins
export BRIDGE_PLUGINS_DIR="$HOME/.config/nexibot/bridge-plugins"
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `BRIDGE_PORT` | Listen port (default: `18790`) |
| `BRIDGE_PLUGINS_DIR` | Path to external plugins directory |

## Security

- Listens on `127.0.0.1` only (not accessible from external networks)
- API keys are never logged (only SHA-256 fingerprints for debugging)
- CORS restricted to localhost origins

## License

Same as NexiBot parent project
