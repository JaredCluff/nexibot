# Changelog

All notable changes to NexiBot will be documented in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.11.0] - 2026-05-24

### Added

- **Episodic self-improvement loop**: Post-task retrospectives stored in SQLite with vector
  similarity search. Before each task, up to 3 similar past experiences are injected into
  the system prompt, helping the agent learn from prior runs.
- **Deployability polish**: `.env.example` with all supported environment variables,
  `config/presets/minimal.yaml` for single-key VPS deployment, and a "Deploy in 60 seconds"
  section in README.
- **Telegram P2 QoL features** (config fields, wired in v0.11.1):
  - Configurable ack/done reaction emojis + scope (`reaction_scope`: all/direct/group-mentions/off)
  - Error policy + cooldown (`error_policy`: reply/silent, `error_cooldown_ms`)
  - Per-DM thread reply mode (`dm_thread_replies`: off/inbound/always)
  - Polling stall detection (`polling_stall_threshold_ms`)
  - Bot sender filtering (`sender_type_policy`: humans-only/humans-and-allowlisted-bots/open)
  - Custom command menu (`custom_commands` registered with `setMyCommands`)
  - Response prefix + link preview control

### Fixed

- Merged 11 unmerged audit-pass branches (37–45) covering HTTP timeouts, TaskStore bounds,
  UTF-8 truncation safety, circuit breaker caps, Telegram LLM lock eviction, MCP tool naming,
  iframe sandbox, DAG task timeouts, and retry logic fixes.

## [0.8.1] - 2026-03-24

### Added

- **Self-learning skills**: Agent autonomously writes new skills after completing complex tasks
  (score-based heuristic: tool diversity, turn length, recurrence patterns)
- **`/save-as-skill` command**: Explicit user-triggered skill capture from any conversation turn
- **Skill improvement loop**: Skills track usage outcomes; after 5 uses the agent proposes
  an improved version via async LLM sub-call and rewrites the SKILL.md automatically
- **`nexibot_create_skill` / `nexibot_update_skill` as LLM tools**: Agent can explicitly
  create or update skills mid-conversation without a slash command
- **Parallel tool execution**: Independent tool calls within a batch now execute concurrently
  via `tokio::task::JoinSet`, reducing multi-tool turn latency
- **PII redaction**: User messages and tool results are scanned for email addresses, phone
  numbers, SSNs, credit card numbers, and IP addresses before being sent to LLM providers;
  replaced with typed tokens (`[EMAIL]`, `[PHONE]`, etc.)
- **GAPS.md**: Living document tracking competitor feature gaps and their closure status

### Fixed

- `skill_lifecycle` SQLite DB correctly inherits WAL mode and 0600 permissions from memory store conventions

## [0.8.0] - 2026-03-18

Initial open source release.

### Features

- Multi-provider LLM support: Anthropic Claude, OpenAI, Google Gemini, Ollama (local)
- 4-level model fallback chain with cooldown-aware failover
- 8+ messaging channels: Telegram, Discord, Slack, WhatsApp, Signal, Teams, Matrix, Email
- Voice assistant with OpenWakeWord wake word detection (ONNX)
- Local STT via SenseVoice ONNX, macOS Speech Framework, Windows SAPI
- Local TTS via Piper ONNX, macOS say, espeak-ng, Windows SAPI
- Cloud STT/TTS fallback (Deepgram, ElevenLabs)
- Local semantic search: LanceDB vectors + SQLite FTS5 with hybrid MMR re-ranking
- On-device ML: DeBERTa v3 prompt injection detection, Silero VAD, all-MiniLM-L6-v2 embeddings
- 2048-entry LRU embedding cache
- Agent orchestration with TF-IDF capability matching and subagent spawning
- MCP (Model Context Protocol) server integration
- Browser automation via Chrome DevTools Protocol
- Computer Use API (screenshot, mouse, keyboard) with confirmation gates
- K2K federation protocol for knowledge routing (k2k-common crate)
- Skills system with hot-reload, security scanning, and ClawHub marketplace
- Session memory with SQLite FTS5 full-text search
- AES-256-GCM session encryption with Argon2id key derivation
- 17-check security audit system
- SSRF protection with fail-closed DNS resolution
- DM pairing security for messaging channels
- Headless / container mode (Podman)
- Cross-platform: macOS, Windows, Linux
- Anthropic Bridge: Node.js OAuth token support
