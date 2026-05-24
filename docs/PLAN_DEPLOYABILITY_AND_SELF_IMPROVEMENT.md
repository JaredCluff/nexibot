# Plan: Deployability + Self-Improvement Loop

**Goal:** Make NexiBot deployable on bare-minimum VPS resources with a single command,
competitive with Hermes Agent's $5/month deploy story, and close the episodic
self-improvement gap identified in the benchmarking session.

---

## Background

Hermes Agent (Nous Research, Feb 2026) is the fastest-growing open-source agent
framework (140k+ stars, most-used on OpenRouter). Its two key competitive advantages
over NexiBot are:

1. **Deployability** — `docker run` on a $5/month VPS, zero local toolchain required.
2. **Self-improvement loop** — After each task, Hermes writes a structured episodic
   retrospective (what was tried, what succeeded, what failed). Before future similar
   tasks, it retrieves those records and adjusts strategy. NexiBot has rich retrieval
   memory but no equivalent post-task learning mechanism.

NexiBot already has a solid foundation: `Containerfile`, `compose.yaml`, `NEXIBOT_HEADLESS=1`
server mode, and `ExecutionSummary` emitted at the end of each tool loop. The work
below closes the remaining gaps.

---

## Phase 1 — Container Deploy Polish

**Status:** Foundation complete (`Containerfile`, `compose.yaml` exist). Remaining work:

### 1.1 `.env.example`
Create a root-level `.env.example` with all supported env vars, one-line descriptions,
and safe placeholder values. This is the primary onboarding artifact for a VPS deploy.

```
.env.example
```

Key sections:
- LLM keys (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GOOGLE_API_KEY`, etc.)
- Channel bots (`TELEGRAM_BOT_TOKEN`, `DISCORD_BOT_TOKEN`, `SLACK_BOT_TOKEN`, etc.)
- Voice providers (`DEEPGRAM_API_KEY`, `ELEVENLABS_API_KEY`, `CARTESIA_API_KEY`,
  `OPENAI_API_KEY` for Whisper, `GOOGLE_STT_API_KEY`, `AZURE_SPEECH_KEY`,
  `AZURE_SPEECH_REGION`)
- Runtime tuning (`RUST_LOG`, `BRIDGE_PORT`, `NEXIBOT_HEADLESS`)

### 1.2 Minimal config preset
Add `config/presets/minimal.yaml` — a fully-commented config that works out of the
box with just `ANTHROPIC_API_KEY` + one channel token. All optional features are
present but commented out so users can layer them in.

Structure mirrors the layering philosophy:
```yaml
# Tier 1 — always on
llm: ...
memory: ...

# Tier 2 — add a channel bot
telegram: { enabled: false, token: "${TELEGRAM_BOT_TOKEN}" }
discord:  { enabled: false, token: "${DISCORD_BOT_TOKEN}" }

# Tier 3 — voice
voice:
  enabled: false
  stt: { provider: deepgram }
  tts: { provider: elevenlabs }

# Tier 4 — security
defense: { enabled: false }

# Tier 5 — orchestration
orchestration: { enabled: false }
```

### 1.3 Verify quick-start path end-to-end
Test the minimal install path on a clean environment:
```bash
cp .env.example .env          # fill in ANTHROPIC_API_KEY + TELEGRAM_BOT_TOKEN
docker compose up -d
docker compose logs -f nexibot
```
Confirm: bridge healthy, bot responds, memory persists across restart.

### 1.4 Document in README
Add a "Deploy in 60 seconds" section to README.md showing the three-step VPS install.

---

## Phase 2 — Episodic Self-Improvement Loop

**Status:** Not implemented. Closes the primary functional gap vs Hermes Agent.

### 2.1 New module: `src-tauri/src/episodic_memory.rs`

SQLite-backed episodic store. New table in the existing memory SQLite DB
(`~/.config/nexibot/memory/`).

```sql
CREATE TABLE IF NOT EXISTS episodic_records (
    id          TEXT PRIMARY KEY,
    created_at  INTEGER NOT NULL,
    goal        TEXT NOT NULL,         -- first user message, truncated to 512 chars
    outcome     TEXT NOT NULL,         -- 'success' | 'partial' | 'failure'
    iterations  INTEGER NOT NULL,
    elapsed_ms  INTEGER NOT NULL,
    tools_used  TEXT NOT NULL,         -- JSON array of tool names
    what_worked TEXT NOT NULL,         -- JSON array of observations
    what_failed TEXT NOT NULL,         -- JSON array of observations
    embedding   BLOB                   -- all-MiniLM-L6-v2, 384-dim f32
);
```

Key types:

```rust
pub enum EpisodicOutcome { Success, Partial, Failure }

pub struct EpisodicRecord {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub goal: String,
    pub outcome: EpisodicOutcome,
    pub iterations: usize,
    pub elapsed_ms: u64,
    pub tools_used: Vec<String>,
    pub what_worked: Vec<String>,
    pub what_failed: Vec<String>,
    pub embedding: Option<Vec<f32>>,
}
```

Key methods on `EpisodicStore`:
- `write(record: EpisodicRecord)` — insert/replace
- `search_similar(goal: &str, limit: usize) -> Vec<EpisodicRecord>` — cosine similarity on embedding
- `prune_old(keep: usize)` — LRU eviction at 10k records

### 2.2 Outcome classification

After the tool loop exits, classify the outcome from `ExecutionSummary`:
- `Success` — loop exited cleanly without hitting max_iterations, no error content in final message
- `Partial` — hit max_iterations but produced a substantive response
- `Failure` — error message or empty response

Extract `what_worked` / `what_failed` from the tool call sequence:
- Tool calls that returned non-error results → `what_worked`
- Tool calls that returned errors or were retried → `what_failed`

### 2.3 Integration into `tool_loop.rs`

**Post-task (write retrospective):**

At the point where `ExecutionSummary` is constructed (end of run), add:
```rust
if let Some(episodic) = &self.episodic_store {
    let record = build_episodic_record(&goal, &summary, &tool_results);
    episodic.write(record).await.ok();
}
```

**Pre-task (retrieve and inject):**

Before the first LLM call in the loop, query the episodic store:
```rust
if let Some(episodic) = &self.episodic_store {
    let past = episodic.search_similar(&goal, 3).await.unwrap_or_default();
    if !past.is_empty() {
        // Prepend to system prompt:
        // "Past experience on similar tasks:\n{formatted_retrospectives}"
    }
}
```

Format injected as a compact block (not a full dump) — max 400 tokens to avoid
crowding the context window. Include only records where outcome != Success OR
where a non-obvious tool pattern was used.

### 2.4 Config flag

```yaml
episodic_memory:
  enabled: true
  max_records: 10000
  inject_limit: 3          # max retrospectives injected per task
  inject_threshold: 0.72   # min cosine similarity to inject
```

Enabled by default (low overhead, high value). Can be disabled for low-resource
deployments.

### 2.5 GAPS.md update

Add new row:
```
| 14 | Post-task episodic retrospective / self-improvement loop | Hermes Agent | ✅ Closed | v0.11.0 |
```

---

## Phase 3 — Missing Voice Providers

**Status:** Deepgram ✅, Whisper/OpenAI ✅, ElevenLabs ✅, Cartesia ✅.
Missing for the declared minimal tier: Google Cloud Speech, Azure/MS Speech.

### 3.1 Google Cloud Speech-to-Text — `src-tauri/src/voice/stt/google.rs`

REST API (v1): `POST https://speech.googleapis.com/v1/speech:recognize`

Config:
```yaml
voice:
  stt:
    provider: google
    google_api_key: "${GOOGLE_STT_API_KEY}"
    google_language_code: "en-US"
    google_model: "latest_long"   # or "latest_short", "telephony"
```

### 3.2 Azure Speech Service — `src-tauri/src/voice/stt/azure.rs`

REST batch recognition: `POST https://{region}.stt.speech.microsoft.com/speech/recognition/conversation/cognitiveservices/v1`

Config:
```yaml
voice:
  stt:
    provider: azure
    azure_speech_key: "${AZURE_SPEECH_KEY}"
    azure_speech_region: "eastus"
    azure_speech_language: "en-US"
```

### 3.3 Azure Neural TTS — `src-tauri/src/voice/tts/azure.rs`

REST: `POST https://{region}.tts.speech.microsoft.com/cognitiveservices/v1`
Output: audio/mpeg or audio/wav. SSML input for voice/style control.

Config:
```yaml
voice:
  tts:
    provider: azure
    azure_speech_key: "${AZURE_SPEECH_KEY}"
    azure_speech_region: "eastus"
    azure_voice_name: "en-US-JennyNeural"
```

---

## Delivery Order

| # | Item | Phase | Size |
|---|------|-------|------|
| 1 | `.env.example` | 1 | Small |
| 2 | `config/presets/minimal.yaml` | 1 | Small |
| 3 | README "Deploy in 60 seconds" section | 1 | Small |
| 4 | `episodic_memory.rs` module + SQLite schema | 2 | Medium |
| 5 | Post-task retrospective writer in `tool_loop.rs` | 2 | Medium |
| 6 | Pre-task retrospective retriever + system prompt injection | 2 | Medium |
| 7 | Episodic config flag in `config.rs` | 2 | Small |
| 8 | GAPS.md row 14 | 2 | Trivial |
| 9 | Google Cloud Speech STT | 3 | Medium |
| 10 | Azure Speech STT | 3 | Medium |
| 11 | Azure Neural TTS | 3 | Medium |

---

## Target Version

All three phases target **v0.11.0**.
