# Plan: Telegram Integration Parity with OpenClaw

**Goal:** Close the feature gap between NexiBot's `telegram.rs` and OpenClaw's Telegram channel
implementation. Prioritized by user impact and implementation complexity.

---

## Baseline Comparison

NexiBot's `src-tauri/src/telegram.rs` (2553 lines) is solid but trails OpenClaw on:
streaming delivery, group/topic granularity, reply threading, webhook mode, message
formatting, reaction handling, and a handful of UX/config quality-of-life features.

---

## Priority 1 — High Impact, Core Gaps

> **Status:** All P1 features implemented in v0.11.0.

### 1.1 Streaming Progress Drafts ✅

**Status:** Implemented in v0.11.0.

**What OpenClaw does:**  
Maintains a single editable "working" draft message while tools run. Shows tool
progress lines (`🛠️ Bash: run tests`). Edits the draft in place to become the final
answer when the turn completes. Falls back to a fresh send for media, approvals,
or oversized content.

**What NexiBot does:**  
Sends a typing indicator, runs the tool loop, sends the final response. No streaming
preview; user sees nothing until the response is complete.

**Implementation:**

New struct in `telegram.rs`:
```rust
struct ProgressDraft {
    chat_id: i64,
    message_id: i32,
    thread_id: Option<i32>,
    lines: Vec<String>,
    created_at: Instant,
}
```

New config fields in `TelegramConfig` (`src-tauri/src/config/channels.rs`):
```yaml
streaming:
  mode: "off"          # off | partial | progress
  max_progress_lines: 4
  show_tool_names: true
```

Flow:
1. Before `router::route_message()`, send a placeholder draft (`"⏳ Working…"`) and
   store its `message_id`.
2. Pass a `progress_callback: Arc<dyn Fn(String) + Send + Sync>` into the tool loop
   observer. Each tool invocation calls this with a one-line status.
3. Debounce edits to ≤1 per second (Telegram rate limit: 1 edit/sec per chat).
4. On completion: edit draft → final response text (if ≤4000 chars and no media).
   Otherwise delete draft and send final as new message.
5. Guard: if draft was deleted externally, skip edit, send fresh.

Files to modify:
- `src-tauri/src/telegram.rs` — `handle_telegram_message()` (lines 752–842),
  new `send_progress_draft()`, `edit_progress_draft()`, `finalize_draft()` helpers
- `src-tauri/src/config/channels.rs` — add `TelegramStreamingConfig`
- `src-tauri/src/tool_loop.rs` — add optional `progress_fn` callback parameter

---

### 1.2 Group-Level Config (`groups.<chatId>`) ✅

**Status:** Implemented in v0.11.0.

**What OpenClaw does:**  
Per-group settings with inheritance:
```yaml
channels.telegram.groups:
  "-1001234567890":
    requireMention: true
    allowFrom: ["123456789"]
    groupPolicy: open
    agentId: "my_agent"
    systemPrompt: "You are a coding assistant."
    skills: ["git", "search"]
    topics:
      "42":
        agentId: "topic_agent"
        requireMention: false
      "*":            # wildcard default for all topics
        requireMention: true
```

**What NexiBot does:**  
Global `allowed_chat_ids` + `admin_chat_ids` only. `thread_routing_enabled` is a
single bool — no per-group or per-topic overrides.

**Implementation:**

New types in `src-tauri/src/config/channels.rs`:
```rust
pub struct TelegramTopicConfig {
    pub agent_id: Option<String>,
    pub require_mention: Option<bool>,
    pub allow_from: Option<Vec<i64>>,
    pub group_policy: Option<GroupPolicy>,
    pub enabled: Option<bool>,
    pub system_prompt: Option<String>,
    pub skills: Option<Vec<String>>,
}

pub struct TelegramGroupConfig {
    pub require_mention: bool,
    pub allow_from: Vec<i64>,
    pub group_policy: GroupPolicy,
    pub agent_id: Option<String>,
    pub system_prompt: Option<String>,
    pub skills: Vec<String>,
    pub enabled: bool,
    pub topics: HashMap<String, TelegramTopicConfig>,  // "threadId" or "*"
}
```

Add to `TelegramConfig`:
```rust
pub groups: HashMap<i64, TelegramGroupConfig>,
```

Changes to `handle_telegram_message()`:
- After extracting `chat_id` + `thread_id`, resolve the effective config:
  1. Look up `groups[chat_id]` → if missing, apply global defaults
  2. If `thread_id` present, look up `topics[thread_id.to_string()]` or `topics["*"]`
  3. Merge: topic overrides group, group overrides global
- Use resolved `require_mention`, `allow_from`, `agent_id`, `system_prompt`

Files to modify:
- `src-tauri/src/config/channels.rs`
- `src-tauri/src/telegram.rs` — authorization block (lines 449–512), session key
  builder (lines 773–827), agent resolution

---

### 1.3 Per-Topic Agent Routing ✅

**Status:** Implemented in v0.11.0 (via `ResolvedConfig.agent_id`).

**What OpenClaw does:**  
Each forum topic can bind to a specific agent. The bot becomes a different persona
per topic — e.g., topic 42 = code review agent, topic 17 = search agent.

**What NexiBot does:**  
`thread_routing_enabled` gives isolated conversation history per topic but all
topics use the same agent.

**Implementation:**  
Dependent on 1.2 (uses `resolved_config.agent_id`).

In `handle_telegram_message()`, after resolving group/topic config, pass
`agent_id_override: resolved_config.agent_id` into the `IncomingMessage` before
calling `router::route_message()`. The router already supports agent overrides
via `TelegramBotState::agent_id_override` — extend this to per-message resolution.

Files to modify:
- `src-tauri/src/telegram.rs` — message routing block (lines 752–842)
- `src-tauri/src/channel.rs` — add `agent_id` field to `IncomingMessage` if not present

---

### 1.4 Reply Threading Modes (`replyToMode`) ✅

**Status:** Implemented in v0.11.0.

**What OpenClaw does:**  
- `off` (default): flat sends
- `first`: bot replies to the message that triggered the turn (native Telegram reply)
- `all`: bot replies to every relevant message

Automatic native quote excerpts when replying (capped at 1024 UTF-16 code units;
falls back to plain reply on rejection).

**What NexiBot does:**  
All responses are flat (no `reply_to_message_id`). No quote support.

**Implementation:**

New config field:
```yaml
channels.telegram.replyToMode: "off"   # off | first | all
```

In `handle_telegram_message()`, capture `trigger_message_id` from the incoming
update. When sending the response, include `reply_to_message_id: trigger_message_id`
if `replyToMode != off`.

Quote support: When `replyToMode == first` and the original message text is
available, include `quote: { text: ..., position: 0 }` in `sendMessage` (Bot API
parameter). Truncate at 1024 UTF-16 code units. Retry as plain reply if Telegram
rejects (HTTP 400 → retry without quote).

Files to modify:
- `src-tauri/src/telegram.rs` — response send block (lines 848–926), new helper
  `send_reply()` that wraps the raw API call with optional quote

---

### 1.5 Webhook Mode ✅

**Status:** Implemented in v0.11.0.

**What OpenClaw does:**  
Optional webhook mode alongside long polling. Useful for VPS deployments where
the bot should be reachable via a public URL. Processes updates asynchronously
through per-chat lanes.

**What NexiBot does:**  
Long-polling only (via teloxide). No webhook support.

**Implementation:**

New config fields:
```yaml
channels.telegram.webhook:
  enabled: false
  url: "https://example.com/telegram-webhook"
  secret: "random_secret_token"
  path: "/telegram-webhook"
  host: "127.0.0.1"
  port: 8443
```

NexiBot already has an HTTP server via axum (`webhooks.rs`). Add a new webhook
route to the existing axum router:
- `POST /telegram-webhook` — validate `X-Telegram-Bot-Api-Secret-Token` header,
  parse `Update` JSON, dispatch to `handle_telegram_message()` via the same
  `TelegramBotState` used by polling.

Startup logic: if `webhook.enabled == true`, register webhook with Telegram
(`setWebhook`) and skip polling loop. If false, `deleteWebhook` and start polling.

Files to modify:
- `src-tauri/src/telegram.rs` — `start_telegram_bot()` (lines 203–308)
- `src-tauri/src/webhooks.rs` — add telegram route
- `src-tauri/src/config/channels.rs` — add `TelegramWebhookConfig`

---

### 1.6 Markdown → Telegram HTML Formatting ✅

**Status:** Implemented in v0.11.0.

**What OpenClaw does:**  
Converts markdown to Telegram-safe HTML before sending. Falls back to plain text
if Telegram rejects the parsed HTML. Parse mode: HTML.

**What NexiBot does:**  
Sends plain text in 4096-character chunks. No markdown rendering, no parse mode.

**Implementation:**

New helper function `markdown_to_telegram_html(text: &str) -> String`:
- Convert `**bold**` → `<b>bold</b>`
- Convert `*italic*` → `<i>italic</i>`
- Convert `` `code` `` → `<code>code</code>`
- Convert ```` ```lang\nblock\n``` ```` → `<pre><code class="language-lang">block</code></pre>`
- Escape `<`, `>`, `&` in non-tag text
- Strip unsupported markdown (tables, images, etc.)

Send with `parse_mode: "HTML"`. On 400 error from Telegram, retry as `parse_mode`
omitted (plain text).

New config field:
```yaml
channels.telegram.formatting:
  parse_mode: "html"       # html | none
  chunk_mode: "newline"    # newline | character
  chunk_limit: 4000
  link_preview: true
```

Files to modify:
- `src-tauri/src/telegram.rs` — response send block (lines 848–926), new
  `format_for_telegram()` and chunked `send_formatted_message()` helpers

---

## Priority 2 — Medium Impact

### 2.1 Reaction Notifications (Inbound) 🚧

**Status:** Not yet implemented.

**What OpenClaw does:**  
Receives and routes user reactions to bot messages back as context. Three modes:
`own` (user reacted to bot's message), `all`, `off`. Reaction level: `ack/minimal/extensive`.

**What NexiBot does:**  
Only sends reactions (👀/✅). Does not receive or process inbound reactions.

**Implementation:**

Add `message_reaction` update handler to the teloxide dispatcher. When a user
reacts to the bot's message, emit a memory event or route to the agent as a
lightweight context update (no tool loop; just append to session context).

New config:
```yaml
channels.telegram.reactions:
  notifications: "off"    # off | own | all
  level: "ack"            # ack | minimal | extensive
```

---

### 2.2 Configurable Ack Reaction + Scope ✅

**Status:** Implemented in v0.11.0.

**What OpenClaw does:**  
Configurable per-account ack emoji and scope:
- `ackReaction`: any single emoji (default: `"👀"`)
- `ackReactionScope`: `all | direct | group-all | group-mentions | off`

**What NexiBot does:**  
Hardcoded `REACTION_PROCESSING = "👀"` and `REACTION_DONE = "✅"` (lines 29–31).
Always sent when `reactions_enabled`.

**Implementation:**

Move constants to config:
```yaml
channels.telegram.reactions:
  ack_emoji: "👀"
  done_emoji: "✅"
  scope: "all"            # all | direct | group-mentions | off
```

Update `TelegramConfig` and replace constant references in
`handle_telegram_message()` (lines 809–846) with config-resolved values.

---

### 2.3 Error Policy + Cooldown ✅

**Status:** Implemented in v0.11.0.

**What OpenClaw does:**  
- `errorPolicy: "reply" | "silent"` — optionally suppress error replies
- `errorCooldownMs: 60000` — don't send a new error message if one was sent
  recently (prevents error spam in busy groups)
- Per-group overrides

**What NexiBot does:**  
Always sends error text. No cooldown. No silent mode.

**Implementation:**

Add to `TelegramConfig`:
```yaml
channels.telegram.error_policy: "reply"   # reply | silent
channels.telegram.error_cooldown_ms: 60000
```

In `TelegramBotState`, add `last_error_at: RwLock<HashMap<i64, Instant>>`.
In the error path of `handle_telegram_message()`, check cooldown before sending.

---

### 2.4 Per-DM Thread Reply Mode ✅

**Status:** Implemented in v0.11.0.

**What OpenClaw does:**  
- `channels.telegram.dm.threadReplies: "inbound" | "always" | "off"`
- `inbound`: reply-thread only when the inbound message is itself a reply
- `always`: always reply in-thread
- `off` (default): flat

Per-DM override: `channels.telegram.direct.<chatId>.threadReplies`

**What NexiBot does:**  
No DM-level threading. All DMs are flat.

**Implementation:**

Add to config:
```yaml
channels.telegram.dm:
  thread_replies: "off"    # off | inbound | always
```

In `handle_telegram_message()`, when `chat_type == Private`, check if the incoming
message has `reply_to_message_id`. If so (and `thread_replies: inbound`), set
`reply_to_message_id` on the outbound response to create a visible thread.

---

### 2.5 Polling Stall Detection ✅

**Status:** Implemented in v0.11.0.

**What OpenClaw does:**  
`pollingStallThresholdMs: 120000` — detects when the polling loop hasn't received
an update for longer than the threshold (network issue, proxy, IPv6 problem) and
logs a warning.

**What NexiBot does:**  
Basic retry in the polling loop but no stall detection or logging.

**Implementation:**

In the polling loop (`start_telegram_bot()`, lines 252–305), wrap the update
receiver with a `tokio::time::timeout`. On timeout:
- Log `WARN: Telegram polling stall detected (>Xms). Checking connectivity...`
- Attempt a `getMe` probe; log result
- Continue polling (don't crash)

New config:
```yaml
channels.telegram.polling_stall_threshold_ms: 120000
```

---

### 2.6 Sender-Type Policy (Bot Filtering) ✅

**Status:** Implemented in v0.11.0.

**What OpenClaw does:**  
`senderTypePolicy: "humans-only" | "humans-and-allowlisted-bots" | "open"`

Default is `humans-only` — bot-to-bot messages are silently ignored unless the
sender bot's ID is in `allowFrom`.

**What NexiBot does:**  
No bot sender filtering. Any sender that passes the allowlist check is processed.

**Implementation:**

Add to `TelegramConfig`:
```yaml
channels.telegram.sender_type_policy: "humans-only"
```

In `handle_telegram_message()` authorization block (lines 449–512), add check:
```rust
if config.sender_type_policy == SenderTypePolicy::HumansOnly && from.is_bot {
    return Ok(()); // silently ignore
}
```

---

### 2.7 Custom Command Menu ✅

**Status:** Implemented in v0.11.0.

**What OpenClaw does:**  
`customCommands` array registers slash commands in the BotFather menu via
`setMyCommands` at startup. Commands are normalized (lowercase, no leading slash).

**What NexiBot does:**  
Built-in commands only. No `setMyCommands` call.

**Implementation:**

Add to `TelegramConfig`:
```yaml
channels.telegram.custom_commands:
  - command: "backup"
    description: "Run git backup"
  - command: "deploy"
    description: "Deploy to production"
```

In `start_telegram_bot()`, after validating the token, call `setMyCommands` with
the merged list of built-in + custom commands.

---

## Priority 3 — Quality of Life

### 3.1 Link Preview Control ✅

**Status:** Implemented in v0.11.0.

**What OpenClaw does:**

New config field:
```yaml
channels.telegram.link_preview: true
```

Add `disable_web_page_preview: !config.link_preview` to all `sendMessage` calls.
Currently NexiBot always sends with previews enabled (Telegram default).

### 3.2 Response Prefix ✅

**Status:** Implemented in v0.11.0.

```yaml
channels.telegram.response_prefix: ""    # e.g. "🤖 "
```

Prepend to all outbound message text before chunking.

### 3.3 Media Group / Album Buffering 🚧

**Status:** Not yet implemented.

**What OpenClaw does:**  
Buffers album updates (`mediaGroupId`) for `mediaGroupFlushMs: 500` ms before
processing as a single multi-photo message.

**What NexiBot does:**  
Each photo in an album is processed independently.

**Implementation:**

In `handle_telegram_message()`, when `update.message.media_group_id` is set,
buffer the update keyed by `media_group_id` in a `DashMap<String, Vec<Update>>`
with a 500ms flush timeout. After flush, merge all photo annotations into one
`IncomingMessage`.

### 3.4 History Limit Config 🚧

**Status:** Not yet implemented.

OpenClaw exposes `historyLimit` (group context window) and `dmHistoryLimit`
per-chat. NexiBot's session history is managed globally in `memory.rs`.

Add to `TelegramConfig`:
```yaml
channels.telegram.history_limit: 50        # messages in group context
channels.telegram.dm_history_limit: 100    # messages in DM context
```

Pass to session/memory manager when creating or resuming a session.

---

## Delivery Order

| # | Feature | Priority | Complexity | Target | Status |
|---|---------|----------|------------|--------|--------|
| 1 | Streaming progress drafts (1.1) | P1 | High | v0.11.0 | ✅ Done |
| 2 | Group-level config + allowFrom/agentId (1.2) | P1 | High | v0.11.0 | ✅ Done |
| 3 | Per-topic agent routing (1.3) | P1 | Low (needs 1.2) | v0.11.0 | ✅ Done |
| 4 | Reply threading + quote excerpts (1.4) | P1 | Medium | v0.11.0 | ✅ Done |
| 5 | Webhook mode (1.5) | P1 | Medium | v0.11.0 | ✅ Done |
| 6 | Markdown → Telegram HTML formatting (1.6) | P1 | Medium | v0.11.0 | ✅ Done |
| 7 | Reaction notifications inbound (2.1) | P2 | Medium | — | 🚧 Pending |
| 8 | Configurable ack reaction + scope (2.2) | P2 | Low | v0.11.0 | ✅ Done |
| 9 | Error policy + cooldown (2.3) | P2 | Low | v0.11.0 | ✅ Done |
| 10 | Per-DM thread reply mode (2.4) | P2 | Low | v0.11.0 | ✅ Done |
| 11 | Polling stall detection (2.5) | P2 | Low | v0.11.0 | ✅ Done |
| 12 | Sender-type policy / bot filtering (2.6) | P2 | Low | v0.11.0 | ✅ Done |
| 13 | Custom command menu (2.7) | P2 | Low | v0.11.0 | ✅ Done |
| 14 | Link preview control (3.1) | P3 | Trivial | v0.11.0 | ✅ Done |
| 15 | Response prefix (3.2) | P3 | Trivial | v0.11.0 | ✅ Done |
| 16 | Album / media group buffering (3.3) | P3 | Medium | — | 🚧 Pending |
| 17 | History limit config (3.4) | P3 | Low | — | 🚧 Pending |

---

## Key Files

| File | Role |
|------|------|
| `src-tauri/src/telegram.rs` | All changes land here (primary) |
| `src-tauri/src/config/channels.rs` | All new config structs |
| `src-tauri/src/webhooks.rs` | Webhook route addition (1.5) |
| `src-tauri/src/tool_loop.rs` | Progress callback parameter (1.1) |
| `src-tauri/src/channel.rs` | `agent_id` field on `IncomingMessage` (1.3) |

---

## Notes on OpenClaw's Stack vs NexiBot's

OpenClaw uses **grammY** (Node.js/TypeScript). NexiBot uses **teloxide** (Rust).
The feature parity goals above are framework-agnostic — they describe behaviors,
not API calls. Some OpenClaw config names are adapted to NexiBot's snake_case Rust
conventions.

Teloxide 0.13 does not yet expose `setMessageReaction` as a typed method (NexiBot
works around this with raw HTTP — line 2474). Similarly, some newer Bot API methods
(e.g., `sendMessageDraft`, Bot API 9.3) may require raw HTTP calls until teloxide
catches up.
