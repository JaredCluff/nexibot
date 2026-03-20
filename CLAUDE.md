# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

### Desktop App (NexiBot / Tauri)
```bash
cd ui && npm install && cd ..                  # Install UI dependencies (required first time)
cd bridge && npm install && cd ..              # Install bridge core dependencies (required first time)
cd bridge/plugins/anthropic && npm install && cd ../../..  # Install Anthropic plugin deps
cd bridge/plugins/openai && npm install && cd ../../..     # Install OpenAI plugin deps
cargo tauri dev                                # Development mode with hot reload
cargo tauri build                              # Production build (output in src-tauri/target/release/bundle/)
cargo tauri build --target aarch64-apple-darwin  # Build for specific target
cargo test --workspace                         # Run all workspace tests
cargo test --workspace -- --test-threads=1     # Run tests single-threaded (for SQLite tests)
cargo clippy --workspace                       # Lint
cargo fmt --all                                # Format code
```

#### Windows Build
On Windows, native DLLs (ONNX Runtime, sherpa) must be bundled with the installer.
`build.rs` automatically stages DLLs from `target/release/deps/` to `src-tauri/` during compilation.
Use the Windows config overlay to include them in the NSIS/MSI bundle:
```bash
cargo tauri build --config src-tauri/tauri.conf.windows.json
```

#### macOS Architecture-Specific Build
```bash
cargo tauri build --target aarch64-apple-darwin --config src-tauri/tauri.conf.aarch64.json
cargo tauri build --target x86_64-apple-darwin --config src-tauri/tauri.conf.x86_64.json
```

### UI Only
```bash
cd ui && npm run dev            # Run UI dev server standalone
cd ui && npm run build          # Build UI for production
```

### Bridge Service
```bash
cd bridge && npm start             # Start bridge service (port 18790)
cd bridge && npm run dev           # Development mode with auto-restart
BRIDGE_PORT=9000 npm start         # Custom port
./bridge/start-bridge.sh           # Auto-installs all deps and starts
```

## Architecture

This is a Rust workspace with three members:
- **k2k-common** (`k2k-common/`): Shared K2K protocol library
- **nexibot-tauri** (`src-tauri/`): Tauri desktop application (NexiBot) with GUI
- **cli** (`cli/`): CLI commands

The root `Cargo.toml` is workspace-only (no `[package]` section).

### Core Modules (`src-tauri/src/`)
- `config.rs` - YAML configuration with XDG paths, profile composition, env var overrides (`NEXIBOT_*`)
- `k2k_client.rs` - WebSocket client connecting to Agent Hub (K2K protocol)
- `embeddings/` - ONNX Runtime for local embedding generation (all-MiniLM-L6-v2), LRU cache (2048 entries)
- `search/` - File walker, indexer, and search functionality
- `vectordb/` - Embedded LanceDB for semantic search

### LLM Integration (`src-tauri/src/`)
- `claude.rs` - Claude client abstraction for API calls
- `router.rs` - Model request routing to appropriate provider
- `llm_provider.rs` - LLM provider trait definition and capability flags
- `tool_loop.rs` - Tool execution loop (calls tools, returns results to LLM)
- `tool_converter.rs` - Tool definition format conversion between providers
- `token_estimate.rs` - Token counting for context window management

### Providers (`src-tauri/src/providers/`)
- `mod.rs` - ModelRegistry with 4-level fallback chain (agent primary -> agent backup -> global default -> hardcoded fallback)
- `anthropic.rs` - Anthropic Claude client with streaming
- `openai_compat.rs` - OpenAI-compatible API client (works with any OpenAI-protocol service)
- `ollama.rs` - Ollama local model client
- `google.rs` - Google Gemini client
- `auth_profiles.rs` - Auth credential rotation with cooldown-aware fallover
- `conversation.rs` - Message formatting for different providers
- `model_router.rs` - Dynamic model selection based on config and fallback logic
- `system_prompt.rs` - System prompt management per provider

### Bridge Service (`bridge/`)
Plugin-based Node.js bridge service for provider SDK integration.
- Listens on `http://127.0.0.1:18790`
- Plugin system: discovers and loads plugins from `bridge/plugins/` (built-in) and `BRIDGE_PLUGINS_DIR` (external)
- Core: `server.js` (plugin loader), `lib/normalize.js`, `lib/search.js`, `lib/utils.js`
- Anthropic plugin (`plugins/anthropic/`): OAuth support via `@anthropic-ai/sdk` with `authToken`, Claude Code identity injection, tool name casing
- OpenAI plugin (`plugins/openai/`): OpenAI SDK proxy with response normalization to Anthropic format
- DuckDuckGo search proxy (core, avoids bot detection with browser-like TLS fingerprint)
- Endpoints: `/health`, `/api/search`, `/api/models`, `/api/messages/stream`, `/api/messages`, `/api/openai/models`, `/api/openai/messages/stream`, `/api/openai/messages`

### Session & Memory (`src-tauri/src/`)
- `sessions.rs` - Session manager with inbox messaging, bounded at MAX_INBOX_SIZE=1000, MAX_SESSIONS=100
- `session_overrides.rs` - Per-session model/configuration overrides
- `memory.rs` - Memory manager with MAX_SESSION_MESSAGES=500, MAX_MEMORIES=50,000, MAX_SESSIONS=200
- `memory_store/` - SQLite-backed memory with FTS5 full-text search (O(log n) BM25 ranked search, WAL mode, auto-migration from legacy JSON) and hybrid MMR re-ranking
  - `sqlite_store.rs` - SQLite with FTS5 virtual table, triggers for INSERT/DELETE/UPDATE sync
  - `hybrid_search.rs` - Text (FTS) + vector (cosine) scoring with MMR re-ranking

### Security Modules (`src-tauri/src/security/`)
- `ssrf.rs` - SSRF protection with DNS resolution, IPv4/IPv6 private range detection (fail-closed)
- `external_content.rs` - Boundary markers for untrusted content, prompt injection detection, Unicode homoglyph detection
- `log_redactor.rs` - Secret/token redaction in tracing logs
- `audit.rs` - 17-check security audit system with auto-fix capabilities
- `skill_scanner.rs` - Static analysis of skill code for dangerous patterns
- `dangerous_tools.rs` - Registry of dangerous/gateway-denied/elevated tools
- `exec_approval.rs` - Execution approval modes (Deny/Allowlist/Prompt/Full)
- `tool_policy.rs` - Per-agent tool allow/deny lists with glob patterns
- `rate_limit.rs` - Token bucket rate limiter per key
- `session_encryption.rs` - AES-256-GCM session transcript encryption with Argon2id key derivation
- `credentials.rs` - Secure credential storage via OS keyring
- `workspace.rs` - Workspace confinement
- `path_validation.rs` - Path whitelist enforcement
- `constant_time.rs` - Timing-safe comparisons (subtle crate)
- `env_sanitize.rs` - Environment variable sanitization
- `safe_bins.rs` - Safe binary allowlist

### Defense Pipeline (`src-tauri/src/defense/`)
- `mod.rs` - DefenseConfig, DefensePipeline orchestration with fail-closed behavior (configurable via `fail_open`)
- `deberta.rs` - DeBERTa v3 ONNX-based prompt injection detection (<10ms inference)
- `llama_guard.rs` - Llama Guard 3 content safety classification

### Voice Pipeline (`src-tauri/src/voice/`)
- `mod.rs` - Voice pipeline orchestration: wake word detection, STT, TTS, push-to-talk
- `audio.rs` - Audio capture via cpal
- `preprocessing.rs` - Audio preprocessing (noise gate, normalization)
- `wakeword.rs` - OpenWakeWord ONNX model detection with auto-download
- `vad.rs` - Voice Activity Detection via Silero VAD ONNX
- `stt/mod.rs` - Speech-to-text backend selection
- `stt/cloud.rs` - Cloud STT (API-based fallback)
- `stt/sensevoice.rs` - Local SenseVoice ONNX STT
- `stt/macos_speech.rs` - macOS Speech Framework STT
- `stt/windows_speech.rs` - Windows SAPI STT
- `tts/mod.rs` - Text-to-speech backend selection
- `tts/cloud.rs` - Cloud TTS (ElevenLabs, etc.)
- `tts/piper.rs` - Local Piper ONNX TTS
- `tts/macos_say.rs` - macOS `say` command TTS
- `tts/espeak.rs` - Linux espeak-ng TTS
- `tts/windows_sapi.rs` - Windows SAPI TTS

### Canvas & Visual (`src-tauri/src/canvas/`)
- `mod.rs` - Canvas state management, PanelContentType enum (Markdown, Code, Table, Json, Image, Html)
- `protocol.rs` - Canvas operation protocol definitions
- `renderer.rs` - Tauri event emitter for frontend rendering

### Agent Orchestration (`src-tauri/src/`)
- `agent.rs` - Single agent configuration and capabilities
- `agent_team.rs` - Agent orchestrator with TF-IDF capability matching, category bonuses, and fallback chain
- `orchestration.rs` - Subagent spawning with depth/concurrency controls, tree visualization, and `nexibot_orchestrate` tool definition
- `subagent_executor.rs` - Executes spawned agent tasks through the LLM tool loop with concurrency control and timeout enforcement
- `shared_workspace.rs` - Key-value workspace for inter-agent data sharing within orchestration runs (scoped by orchestration ID, TTL support)
- `circuit_breaker.rs` - Per-agent circuit breaker registry (Closed/Open/HalfOpen states) preventing cascading failures
- `hooks.rs` - Plugin hook system (before/after message, tool call, model override, error)
- `commands/agent_cmds.rs` - Agent CRUD, orchestration Tauri command with dependency DAG execution (parallel rounds, circular dependency detection)

### Channel Integrations (`src-tauri/src/`)
- `channel.rs` - Base channel types and ChannelSource enum
- `telegram.rs` - Telegram Bot API integration (teloxide)
- `whatsapp.rs` - WhatsApp Cloud API integration
- `discord.rs` - Discord bot integration (serenity)
- `slack.rs` - Slack bot integration
- `signal.rs` - Signal CLI REST API integration
- `teams.rs` - Microsoft Teams Bot Framework integration
- `matrix.rs` - Matrix Client-Server API integration
- `email.rs` - IMAP/SMTP email channel with thread tracking
- `webhooks.rs` - Webhook server hosting WhatsApp and Slack routes

### Infrastructure Modules (`src-tauri/src/`)
- `gateway/` - WebSocket gateway for multi-user server mode
  - `mod.rs` - GatewayConfig, AuthMode (Token/Password/Open)
  - `ws_server.rs` - WebSocket server with periodic cleanup (5min interval, 1hr idle timeout)
  - `auth.rs` - Token auth, password auth (Argon2id hashing)
  - `session_mgr.rs` - Per-connection session management
  - `protocol.rs` - Gateway JSON message protocol
  - `metrics.rs` - Connection metrics
  - `admin.rs` - Admin dashboard
- `sandbox/` - Docker container sandbox for command execution
  - `mod.rs` - SandboxConfig with memory/CPU limits, timeout, blocked paths
  - `docker.rs` - Docker container lifecycle
  - `policy.rs` - Security policy enforcement
  - `validate.rs` - Path and mount validation
- `mobile/` - iOS/watchOS companion app hooks
  - `mod.rs` - Mobile integration config
  - `api.rs` - REST API endpoints for mobile clients
  - `push.rs` - Push notification handling (APNs hooks)
- `platform/` - Cross-platform abstractions
  - `mod.rs` - is_macos(), is_windows(), is_linux(), current_platform()
  - `macos_bridge.rs` - macOS Speech framework, native control
  - `windows_bridge.rs` - Windows SAPI, native control
  - `linux_bridge.rs` - Linux D-Bus, X11, Wayland

### Skills & Automation (`src-tauri/src/`)
- `skills.rs` - Skill loader, executor, hot-reload watcher
- `skill_security.rs` - Skill code security scanning
- `clawhub.rs` - ClawHub skill marketplace integration

### Tools (`src-tauri/src/`)
- `computer_use.rs` - Computer Use API (screenshot, mouse, keyboard) with confirmation gates
- `browser.rs` - Browser automation via chromiumoxide (CDP) with domain allowlist
- `guardrails.rs` - Guardrails system with security levels (Standard/Enhanced/Maximum)
- `mcp.rs` - MCP (Model Context Protocol) server integration

### Other Modules (`src-tauri/src/`)
- `bridge.rs` - Bridge service manager for Node.js bridge child process
- `oauth.rs` - OAuth profile management (auth-profiles.json)
- `oauth_flow.rs` - OAuth flow handlers
- `pairing.rs` - DM pairing security for all channels with 12-char codes (60-bit entropy, 15-min expiry)
- `scheduler.rs` - Task scheduler with cron expression support
- `task_manager.rs` - Background task management
- `heartbeat.rs` - Periodic heartbeat for keep-alive
- `soul.rs` - Agent personality/character configuration
- `subscription.rs` - Subscription management
- `user_identity.rs` - User identity manager (multi-user, cross-channel)
- `rate_limiter.rs` - Rate limiting for API calls
- `native_control.rs` - Native OS control integration
- `headless.rs` - Headless (CLI-only) mode support

### Tauri Commands (`src-tauri/src/commands/`)
100+ Tauri commands exposed to the React frontend, organized by domain:
- `chat.rs` - send_message, send_message_with_events, compact_conversation
- `session_cmds.rs` - Session lifecycle, model listing from bridge
- `session_mgmt.rs` - Named sessions, inter-agent messaging
- `memory.rs` / `memory_tool.rs` - Memory CRUD and search
- `config_cmds.rs` - Configuration get/update
- `oauth.rs` - OAuth profile management, auth flow
- `subscription.rs` - Subscription management
- `soul.rs` / `soul_tool.rs` - Soul/personality management
- `guardrails_cmds.rs` - Guardrails configuration
- `defense.rs` - Defense pipeline status and tool permissions
- `skills.rs` - Skill CRUD, templates, security analysis
- `voice.rs` - Voice service lifecycle, STT/TTS testing, push-to-talk
- `bridge.rs` - Bridge service health and lifecycle
- `mcp_cmds.rs` - MCP server management
- `computer_use_cmds.rs` - Accessibility permission checks
- `k2k_cmds.rs` - K2K search integration
- `telegram_cmds.rs` / `whatsapp_cmds.rs` / `webhook_cmds.rs` - Channel configs
- `scheduler_cmds.rs` - Scheduled task CRUD
- `agent_cmds.rs` - Agent capabilities, task submission
- `clawhub_cmds.rs` - ClawHub marketplace
- `autonomous_cmds.rs` - Autonomous mode configuration
- `startup_cmds.rs` - Autostart configuration
- `pairing_cmds.rs` - Pairing approval/denial, DM policies
- `task_cmds.rs` - Background task listing
- `updater_cmds.rs` - App updates
- `audit_cmds.rs` / `cli_audit.rs` - Security audit
- Tool commands: `search_tool.rs`, `fetch_tool.rs`, `filesystem_tool.rs`, `execute_tool.rs`, `settings_tool.rs`

### Key Architecture Points
- Agent initiates outbound WebSocket connections only (firewall-friendly)
- Security whitelist is enforced locally and cannot be changed by remote commands
- Uses ONNX Runtime for all ML inference (embeddings, wake word, defense, VAD) - fully offline capable
- Embedding cache (LRU, 2048 entries) eliminates redundant 60ms ONNX inference
- Memory store uses SQLite FTS5 for O(log n) search (replaced O(n*m*k) linear scan)
- Session/inbox sizes are bounded with oldest-first eviction
- Gateway sessions cleaned periodically (5min interval, 1hr idle timeout)
- Wake word models auto-download on first run from OpenWakeWord GitHub releases
- Defense pipeline fails closed when no models are loaded (configurable via `fail_open`)
- SSRF protection fails closed on DNS resolution errors
- Gateway auth uses Argon2id password hashing (PHC format) and constant-time token comparison
- External channel messages are wrapped with content boundary markers before LLM processing
- Skills are scanned for dangerous patterns on load (eval, subprocess, unsafe blocks blocked)
- Guardrails detect heredoc injection, command/process substitution, and backtick execution
- Docker sandbox env vars are sanitized in strict mode before container creation
- Config profile names are validated against path traversal (CWE-22)
- Config composition: base config <- profile overlay <- `NEXIBOT_*` env vars
- Bridge is plugin-based: provider-specific code lives in `bridge/plugins/`, loaded at startup
- Bridge Anthropic plugin uses SDK `authToken` for OAuth tokens, `apiKey` for API keys
- OAuth profiles stored at `~/Library/Application Support/ai.nexibot.desktop/auth-profiles.json`
- Config stored at `~/Library/Application Support/ai.nexibot.desktop/config.yaml`

## Data Paths

| Path (macOS) | Purpose |
|---|---|
| `~/Library/Application Support/ai.nexibot.desktop/config.yaml` | Main configuration |
| `~/Library/Application Support/ai.nexibot.desktop/auth-profiles.json` | OAuth tokens |
| `~/Library/Application Support/ai.nexibot.desktop/models/` | Downloaded ONNX models |
| `~/.config/nexibot/memory/` | Memory store (SQLite DB) |
| `~/.config/nexibot/skills/` | Skill definitions |
| `~/.config/nexibot/soul/` | Agent personality |
| `~/.config/nexibot/identity/` | User identity |
| `~/.config/nexibot/pairing/` | Pairing requests |

## Important Constraints

- **MANDATORY CONFIG BACKUP**: Before EVERY build, deploy, or any operation that could affect the running app, you MUST back up the live config file. Run: `cp "$HOME/Library/Application Support/ai.nexibot.desktop/config.yaml" "$HOME/Library/Application Support/ai.nexibot.desktop/config.yaml.pre-build"`. NEVER skip this. Configs contain secrets (bot tokens, API keys) that are irreplaceable if lost. After deploy, verify the config is intact.
- **MANDATORY APP INSTALL PROCEDURE**: When installing a new build to /Applications, you MUST (1) quit the running app first (`osascript -e 'tell application "NexiBot" to quit'`), (2) remove the old bundle (`rm -rf /Applications/NexiBot.app`), (3) copy with `ditto` to preserve macOS metadata (`ditto "src-tauri/target/release/bundle/macos/NexiBot.app" /Applications/NexiBot.app`). NEVER use `cp -r` and NEVER overwrite a running app bundle — macOS code-signature validation will kill the process mid-run with SIGKILL (Code Signature Invalid) when a new code page is demand-paged from the replaced binary.
- **Wake word detection**: Default to OpenWakeWord ONNX models. Local STT-based fallback is allowed as a disabled-by-default option for when ONNX models are unavailable.
- **Commit messages**: Do NOT include AI attribution (no "Generated with Claude Code", no "Co-Authored-By: Claude", no AI-related signatures).
- After making code changes, commit and push when complete.

## Environment Variables

| Variable | Description |
|----------|-------------|
| `K2K_HUB_URL` | WebSocket URL of Agent Hub |
| `K2K_AUTH_TOKEN` | JWT authentication token |
| `K2K_DEVICE_ID` | Override device ID |
| `RUST_LOG` | Rust logging filter (e.g., `nexibot_tauri=debug`) |
| `WAKEWORD_MODELS_DIR` | Override wake word models directory |
| `NEXIBOT_PROFILE` | Config profile name (loads `config.{profile}.yaml` overlay) |
| `NEXIBOT_MODEL` | Override Claude model |
| `NEXIBOT_API_KEY` | Override Claude API key |
| `NEXIBOT_GATEWAY_ENABLED` | Enable WebSocket gateway (`true`/`false`) |
| `NEXIBOT_GATEWAY_PORT` | Gateway listen port |
| `NEXIBOT_DEFENSE_ENABLED` | Enable defense pipeline (`true`/`false`) |
| `NEXIBOT_SANDBOX_ENABLED` | Enable Docker sandbox (`true`/`false`) |
| `ANTHROPIC_BRIDGE_URL` | Override bridge URL (default `http://127.0.0.1:18790`) |
| `BRIDGE_PORT` | Bridge listen port (default `18790`) |

## Key Dependencies

### Rust (Cargo.toml)
- **tauri** 2.10 - Desktop framework with tray-icon, image-png
- **k2k-common** - Shared K2K protocol library (local path)
- **tokio** 1 - Async runtime (full features)
- **reqwest** 0.12 - HTTP client (rustls-tls, streaming, multipart)
- **axum** 0.7 - HTTP server (for OAuth callback, gateway)
- **ort** =2.0.0-rc.10 - ONNX Runtime (pinned; wake word, embeddings, defense, VAD)
- **sherpa-rs** 0.6 - SenseVoice STT + Silero VAD
- **rusqlite** 0.35 - SQLite with FTS5 (bundled)
- **lru** 0.12 - Fixed-capacity LRU cache (embedding cache)
- **teloxide** 0.13 - Telegram bot
- **serenity** 0.12 - Discord bot
- **rmcp** 0.14 - MCP client
- **chromiumoxide** 0.7 - Chrome DevTools Protocol
- **aes-gcm** 0.10 - Session encryption
- **argon2** 0.5 - Password hashing
- **keyring** 3 - OS-native credential storage

### Node.js (bridge/)
- **express** ^4.21.2 - Web server (core bridge/package.json)
- **cors** ^2.8.5 - CORS middleware (core bridge/package.json)
- **@anthropic-ai/sdk** ^0.77.0 - Anthropic TypeScript SDK (plugins/anthropic/package.json)
- **openai** ^4.67.0 - OpenAI SDK (plugins/openai/package.json)
