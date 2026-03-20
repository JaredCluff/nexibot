# Changelog

All notable changes to NexiBot will be documented in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

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
