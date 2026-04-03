# NexiBot v0.9.0 — Coding Agent Capabilities Design Spec

**Date:** 2026-04-02
**Status:** Approved
**Scope:** Adopt coding agent capabilities from Claude Code architecture into NexiBot

## Context

NexiBot is a multi-channel autonomous agent (GUI, Telegram, Discord, WhatsApp, voice, headless) within the Paperclip ecosystem alongside Animus (persistent autonomous AI) and KN-Code (dedicated coding agent). NexiBot needs full-parity coding capabilities so users can leverage it for solid coding that won't make stupid mistakes. This may eventually make KN-Code unnecessary.

Key constraints:
- Must work across all channels (GUI, Telegram, Discord, voice, headless)
- Telegram: HTML formatting only (no markdown)
- Voice: Conversational natural language, no reading slashes/paths/code verbatim
- Headless: Can route output to Telegram for remote monitoring/approval
- Autonomous agent focus — minimize human-in-the-loop for trusted operations

## Architecture Overview

### Tool Trait Registry (Foundation)

New trait-based tool system for all new tools. Existing tools remain in the monolithic `execute_tool_call()` match statement. Dispatcher checks trait registry first, falls through to match.

**Future Phase A (not in v0.9.0):** Migrate all legacy tools from match statement to trait registry.

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> serde_json::Value;
    fn prompt_description(&self) -> String;
    fn is_read_only(&self, input: &Value) -> bool { false }
    fn is_concurrency_safe(&self) -> bool { false }

    async fn check_permissions(&self, input: &Value, ctx: &ToolContext) -> PermissionDecision;
    async fn call(&self, input: Value, ctx: ToolContext) -> ToolResult;
    async fn call_streaming(
        &self,
        input: Value,
        ctx: ToolContext,
        progress_tx: mpsc::UnboundedSender<ToolProgress>,
    ) -> ToolResult {
        self.call(input, ctx).await
    }
}

pub enum PermissionDecision {
    Allow,
    Deny(String),
    Ask { reason: String, details: Option<String> },
}

pub struct ToolResult {
    pub content: String,
    pub success: bool,
    pub progress_events: Vec<ToolProgress>,
}

pub enum ToolProgress {
    Stdout(String),
    Stderr(String),
    Status(String),
    PartialResult(String),
}
```

**Dispatcher integration** (top of `execute_tool_call()`):
```rust
if let Some(tool) = tool_registry.get(tool_name) {
    let ctx = ToolContext::from_state(state, session_key, agent_id, observer);
    let permission = tool.check_permissions(&tool_input, &ctx).await;
    match permission {
        PermissionDecision::Deny(msg) => return format!("BLOCKED: {msg}"),
        PermissionDecision::Ask { reason, details } => {
            if !observer.request_approval_with_details(tool_name, &reason, details.as_deref()).await {
                return "BLOCKED: User denied".to_string();
            }
        }
        PermissionDecision::Allow => {}
    }
    return tool.call(tool_input.clone(), ctx).await.content;
}
// Existing match statement follows as fallback
```

**Registry:** `HashMap<String, Box<dyn Tool>>` populated at startup.

**New file:** `src-tauri/src/tool_registry.rs`

---

## Feature 1: Structured File Edit Tool (`nexibot_file_edit`)

Targeted string replacement with diff output. Replaces whole-file rewrites.

**New file:** `src-tauri/src/tools/file_edit.rs`

### Input

```rust
pub struct FileEditInput {
    pub file_path: String,
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,  // default false
}
```

### 13-Step Validation

1. Reject if `old_string == new_string`
2. Check path against blocked/allowed paths (existing workspace config)
3. Check file size (max 1GB)
4. Read file with encoding detection (UTF-8 default, UTF-16LE via BOM)
5. Normalize line endings (`\r\n` to `\n` internally, preserve on write)
6. If `old_string` is empty and file doesn't exist: treat as new file creation
7. If `old_string` is empty and file exists and non-empty: reject
8. Read-before-edit enforcement: reject edits to files not read this session
9. File staleness check: compare mtime against last read timestamp
10. Find actual string: exact match first, then quote-normalized match (curly to straight quotes)
11. Uniqueness check: if multiple matches and `replace_all` is false, reject with count
12. Apply replacement, generate unified diff (using `similar` crate)
13. Write atomically, update read-state cache

### Output

Unified diff format showing exactly what changed:
```
The file /path/to/file.rs has been edited. Here's the diff:

@@ -42,7 +42,7 @@
 fn handle_request(req: Request) -> Response {
-    let result = process(req.body);
+    let result = process_validated(req.body);
     Response::ok(result)
 }
```

### Shared State

```rust
pub struct FileReadState {
    read_files: HashMap<PathBuf, FileReadRecord>,
}

pub struct FileReadRecord {
    pub mtime: SystemTime,
    pub was_full_read: bool,
}
```

Shared between `nexibot_file_read` and `nexibot_file_edit`. Session-scoped.

### Quote Normalization

- Curly quotes (U+2018, U+2019, U+201C, U+201D) normalized to straight quotes for matching
- Exact match attempted first, normalized match as fallback
- When matched via normalization, file's original quote style preserved in replacement

### Optional Git Diff

After successful edit, if file is git-tracked: run `git diff HEAD -- <file>`, parse and include in output.

---

## Feature 2: Enhanced File Read (`nexibot_file_read`)

Upgrades existing `read_file` action.

**New file:** `src-tauri/src/tools/file_read.rs`

### Input

```rust
pub struct FileReadInput {
    pub path: String,
    pub offset: Option<u64>,    // Line number to start from (1-indexed)
    pub limit: Option<u64>,     // Number of lines to read
    pub pages: Option<String>,  // PDF page range: "1-5", "3", "10-20"
}
```

### New Capabilities

1. **Line-range reading** — Read lines N through M. Output in `cat -n` format (line numbers prefixed). Updates `FileReadState` with `was_full_read: false`.

2. **Image support** — Detect by extension + magic bytes (first 8KB). Read as base64, resize to fit token budget. Return as image content block to API. Channel fallback: return metadata description for Telegram/voice.

3. **PDF support** — Small PDFs (<64KB): base64 inline as document block. Large PDFs: extract requested page range to images. `pages` parameter required for large PDFs (max 20 pages per request).

4. **Jupyter notebook support** — Parse `.ipynb` JSON, return cells with type (code/markdown), content, execution count, and outputs.

5. **Read deduplication** — Same file + same offset/limit + unchanged mtime = return stub ("File unchanged since last read") instead of re-sending.

6. **Binary file detection** — Check first 8KB for null bytes, not just extension. Return `"[Binary file, N bytes, type: ELF/Mach-O/etc]"`.

7. **Device file blocking** — Reject `/dev/*`, `/proc/*`, named pipes. These can hang indefinitely.

### Read-State Integration

Every successful read updates the `FileReadState` map that `nexibot_file_edit` depends on for staleness checks.

---

## Feature 3: Streaming Tool Output

**New file:** `src-tauri/src/tool_streaming.rs`

### Tool Trait Extension

`call_streaming()` method accepts `mpsc::UnboundedSender<ToolProgress>` for real-time progress.

### Flow

1. Dispatcher creates `mpsc::unbounded_channel()` per tool call
2. Calls `tool.call_streaming(input, ctx, progress_tx)`
3. Reader task drains `progress_rx`, forwards to observer
4. Observer emits per-channel: `chat:tool-progress` (GUI), periodic edits (Telegram), status summaries (voice)
5. Final result collected as single `String` for LLM — streaming is for the user

### Observer Trait Expansion

```rust
async fn on_tool_progress(&self, name: &str, id: &str, progress: &ToolProgress) {}
```

### Tools That Use Streaming

- `nexibot_execute` (run_command): streams stdout/stderr
- Git operations via execute: streams for long clone/fetch
- Any tool taking >2s benefits

### Channel Adaptation

- **GUI**: Real-time `chat:tool-progress` Tauri events, collapsible terminal panel
- **Telegram**: Edit progress message every 5 seconds with latest output tail
- **Voice**: Brief spoken status at intervals ("Still running the tests...")
- **Headless/background**: Logged to task output file

---

## Feature 4: Git Integration

### 4a. System Prompt Context Injection

**New file:** `src-tauri/src/git_context.rs`

At conversation start, run 5 git commands in parallel:
```
git branch --show-current
git log --oneline -n 5
git status --short          (capped at 2000 chars)
git config user.name
git rev-parse --abbrev-ref @{u}
```

Injected as snapshot in system prompt. Explicitly noted as point-in-time snapshot.

### 4b. Git Safety Rules (System Prompt)

- NEVER update git config
- NEVER run destructive git commands without explicit user request
- NEVER skip hooks (--no-verify) unless user explicitly asks
- NEVER force push to main/master — warn the user
- Always create NEW commits rather than amending
- Don't commit secret files (.env, credentials.json)
- Use HEREDOC syntax for commit messages

### 4c. No Dedicated Git Tool

Git commands run through `nexibot_execute` with guardrails enforcement. The safety rules in the system prompt guide LLM behavior. Git-specific additions:
- System prompt context injection
- `nexibot_file_edit` optional git diff in output
- Guardrails enhanced with git-specific destructive patterns

---

## Feature 5: LSP Integration (`nexibot_lsp`)

**New directory:** `src-tauri/src/tools/lsp/`

### Supported Operations (9)

| Operation | LSP Method | Purpose |
|-----------|-----------|---------|
| goToDefinition | textDocument/definition | Find symbol definition |
| findReferences | textDocument/references | Find all references |
| hover | textDocument/hover | Type info and docs |
| documentSymbol | textDocument/documentSymbol | List file symbols |
| workspaceSymbol | workspace/symbol | Search project symbols |
| goToImplementation | textDocument/implementation | Find trait implementations |
| prepareCallHierarchy | textDocument/prepareCallHierarchy | Get call hierarchy |
| incomingCalls | callHierarchy/incomingCalls | What calls this? |
| outgoingCalls | callHierarchy/outgoingCalls | What does this call? |

### Input

```rust
pub struct LspInput {
    pub operation: LspOperation,
    pub file_path: String,
    pub line: u32,       // 1-based, converted to 0-based for protocol
    pub character: u32,  // 1-based
}
```

### Architecture

- **Connection:** stdio via JSON-RPC 2.0 (using `tower-lsp` or `lsp-types` crate)
- **Server discovery:** Configuration-driven in `config.yaml`:
  ```yaml
  lsp:
    servers:
      rust:
        command: "rust-analyzer"
        extensions: [".rs"]
      typescript:
        command: "typescript-language-server"
        args: ["--stdio"]
        extensions: [".ts", ".tsx", ".js", ".jsx"]
      python:
        command: "pylsp"
        extensions: [".py"]
  ```
- **File routing:** Extension to server mapping. Files opened via `textDocument/didOpen` before operations.
- **Health management:** Per-server state tracking (stopped/starting/running/error), max 3 restart attempts with exponential backoff (500ms/1s/2s)
- **Request timeout:** 30s for startup, 3 retries for transient errors
- **Availability:** Only when NexiBot has filesystem access. `is_enabled()` returns false if no servers configured.

### Output Format

```
goToDefinition: src/tool_loop.rs:42:10
  pub async fn run_tool_loop(config: ToolLoopConfig, ...) -> ToolLoopResult

findReferences (3 results in 2 files):
  src/commands/chat.rs:156:20 - let result = run_tool_loop(config, client).await;
  src/commands/chat.rs:289:16 - run_tool_loop(telegram_config, client).await;
  src/headless.rs:44:12 - run_tool_loop(headless_config, client).await;
```

---

## Feature 6: Worktree Isolation

**New file:** `src-tauri/src/tools/worktree.rs`

### 6a. Sub-Agent Worktrees

When `nexibot_orchestrate` spawns with `isolation: "worktree"`:

```rust
pub enum Isolation {
    Worktree,
}
```

1. Find git root from current directory
2. Create branch: `worktree-{agent_id_short}` (using `-B` for orphan handling)
3. Create worktree: `git worktree add {git_root}/.nexibot/worktrees/{slug} {branch}`
4. Set sub-agent's working directory to worktree path
5. On completion: changes exist = return path + branch; no changes = auto-cleanup

Agent worktrees don't mutate global session state.

### 6b. Session Worktrees (User-Initiated)

```rust
pub struct WorktreeInput {
    pub action: WorktreeAction,
}

pub enum WorktreeAction {
    Enter { name: String },
    Exit { discard_changes: bool },
    Status,
}
```

**Safety:**
- Change detection before exit: `git status --porcelain` + `git rev-list --count`
- Explicit `discard_changes: true` required to remove worktrees with work
- Never deletes main/master branch
- Stale worktree pruning on startup via `git worktree prune`

**State:**
```rust
pub struct WorktreeState {
    pub active: bool,
    pub original_cwd: PathBuf,
    pub worktree_path: PathBuf,
    pub branch: String,
    pub original_branch: String,
}
```

---

## Feature 7: Plan Mode

**New file:** `src-tauri/src/tools/plan_mode.rs`

### Tools

```rust
// nexibot_enter_plan_mode
pub struct EnterPlanInput {
    pub description: Option<String>,
}

// nexibot_exit_plan_mode
pub struct ExitPlanInput {
    pub plan_content: Option<String>,
}
```

### Flow

1. LLM calls `nexibot_enter_plan_mode` — system injects read-only constraint
2. Permission model changes: read-only tools auto-approved, write/execute tools blocked (exception: plan file at `.nexibot/plans/{name}.md`)
3. LLM explores codebase and writes plan
4. LLM calls `nexibot_exit_plan_mode` — triggers approval
5. On approval — permissions restored, LLM receives plan and begins execution

### Channel-Specific Approval

- **GUI**: Plan in review panel, inline editing, approve/reject buttons
- **Telegram**: Plan sent as HTML message(s), inline keyboard Approve/Reject, user can reply with edits
- **Voice**: Plan summarized conversationally, verbal "approve" triggers execution
- **Headless**: Auto-approved if autonomous mode enabled, can route to Telegram for remote approval

### State

```rust
pub struct PlanModeState {
    pub active: bool,
    pub pre_plan_approval_mode: ApprovalMode,
    pub plan_file_path: Option<PathBuf>,
    pub entered_at: Instant,
}
```

---

## Feature 8: Background Tasks with Output Streaming

**New file:** `src-tauri/src/tools/tasks.rs`

### Task Model

```rust
pub struct BackgroundTask {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    pub task_type: TaskType,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub output_path: PathBuf,
    pub metadata: HashMap<String, Value>,
}

pub enum TaskType {
    BackgroundCommand,
    BackgroundAgent,
    Scheduled,
}
```

### LLM Tools

| Tool | Purpose |
|------|---------|
| `nexibot_task_create` | Spawn background command or agent |
| `nexibot_task_list` | List tasks with status filtering |
| `nexibot_task_get` | Task details + last 4KB of output |
| `nexibot_task_output` | Full output with offset/limit |
| `nexibot_task_stop` | Kill a running task |

### Output Streaming

- Commands redirect stdout/stderr to `~/.config/nexibot/tasks/{task_id}/output.log`
- `nexibot_task_get` returns last 4KB (quick check)
- `nexibot_task_output` supports offset/limit for specific portions
- Output files cleaned up after 24h TTL

### Persistence

- Task metadata as JSON in `~/.config/nexibot/tasks/{task_id}/task.json`
- Survives restart. Orphaned `Running` tasks marked `Failed` on startup.
- Monotonic IDs with high-water-mark file.

---

## Feature 9: Inter-Agent Direct Messaging (`nexibot_send_message`)

**New file:** `src-tauri/src/tools/send_message.rs`

### Input

```rust
pub struct SendMessageInput {
    pub to: String,       // Agent name, ID, or "*" for broadcast
    pub message: String,
}
```

### Addressing Resolution

1. By name: `agent_name_registry.get(name)` to agent ID
2. By ID: direct lookup in task/orchestration manager
3. Broadcast: `"*"` to all active agents in current orchestration

### Delivery

```rust
pub struct AgentMessage {
    pub from: String,
    pub to: String,
    pub text: String,
    pub timestamp: DateTime<Utc>,
}
```

- **In-process agents**: `mpsc::unbounded_channel` per agent, drained at each tool-loop iteration, injected as user message
- **Background agents**: Written to `~/.config/nexibot/tasks/{agent_id}/inbox/` as JSON files, scanned/consumed at each iteration

### Agent Lifecycle

- Running: queued, delivered at next iteration
- Completed: error returned
- Failed/cancelled: error with status

### Name Registry

```rust
pub struct AgentNameRegistry {
    names: HashMap<String, String>,  // name -> agent_id
}
```

Populated on spawn, cleared on completion. Available via `ToolContext`.

All messages are plain text. No structured message subtypes.

---

## Feature 10: Permission Classifier Improvements

**New file:** `src-tauri/src/security/llm_classifier.rs`

### Three-Tier System

**Tier 1: Fast allowlist (existing)**
- Read-only tools: auto-approve
- Known-safe commands: auto-approve
- Blocked paths/commands: auto-deny

**Tier 2: Pattern-based DCG (existing, enhanced)**
- Existing destructive command guard
- Enhanced with git-specific patterns: force push, skip hooks, amend, reset --hard

**Tier 3: LLM classifier (new, ambiguous cases only)**

```rust
pub struct ClassifierRequest {
    pub tool_name: String,
    pub tool_input: Value,
    pub recent_context: Vec<String>,
    pub working_directory: String,
}

pub struct ClassifierResult {
    pub allow: bool,
    pub reason: String,
    pub confidence: f32,
}
```

- Uses cheap/fast model (Haiku) via side query
- Only invoked for Tier 2 ambiguous flags
- Cached per command pattern for session
- 5-second timeout, falls back to asking user on failure
- Configurable: `classifier.llm_enabled: false` disables Tier 3

### Approval Tracking

```rust
pub enum ApprovalSource {
    Allowlist,
    PatternMatch,
    LlmClassifier(String),
    UserApproved,
    AutonomousMode,
}
```

---

## Feature 11: Notebook Editing (`nexibot_notebook_edit`)

**New file:** `src-tauri/src/tools/notebook_edit.rs`

### Input

```rust
pub struct NotebookEditInput {
    pub notebook_path: String,
    pub cell_id: Option<String>,
    pub action: NotebookAction,
}

pub enum NotebookAction {
    Replace { content: String },
    Insert { content: String, cell_type: CellType, after_cell_id: Option<String> },
    Delete,
}

pub enum CellType { Code, Markdown }
```

### Behavior

- **Replace**: Updates cell source, resets execution_count, clears outputs for code cells
- **Insert**: Creates new cell after target (or at end), auto-generates cell ID for nbformat >= 4.5
- **Delete**: Removes cell by ID

### Safety

- Read-before-edit enforced via `FileReadState`
- File staleness check (mtime)
- Validates notebook JSON before and after edit
- Preserves nbformat, metadata, kernelspec
- `nexibot_file_edit` rejects `.ipynb` with redirect message

---

## Feature 12: Cost/Token Tracking

**New file:** `src-tauri/src/cost_tracker.rs`

### Tracker

```rust
pub struct CostTracker {
    pub session_id: String,
    pub model_usage: HashMap<String, ModelUsage>,
    pub total_cost_usd: f64,
    pub total_api_duration: Duration,
    pub total_tool_duration: Duration,
    pub session_start: Instant,
}

pub struct ModelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: f64,
    pub request_count: u32,
}
```

### Context Compaction

```rust
pub struct ContextManager {
    pub context_window: u64,
    pub compaction_threshold: f64,  // 0.8
    pub token_count: u64,
}
```

- Uses actual `input_tokens` from API response (not character-based estimate)
- Auto-compact at 80% context window usage
- Compaction: summarize older messages, keep recent turns + system prompt + landmark messages
- Post-compaction: clear `FileReadState`, clear memoized git context

### Budget Limits

```yaml
limits:
  max_session_cost_usd: 5.00
  warn_session_cost_usd: 2.00
  max_tokens_per_turn: 32000
```

Channel behavior on budget exceeded:
- GUI: warning banner then hard stop
- Telegram/channels: cost summary message, agent stops
- Autonomous: respects hard stop, logs reason

---

## Feature 13: System Prompt Assembly

**New file:** `src-tauri/src/system_prompt.rs`

### Section Architecture

```rust
pub struct SystemPromptBuilder {
    sections: Vec<PromptSection>,
}

pub struct PromptSection {
    pub key: String,
    pub content: String,
    pub cacheable: bool,
    pub priority: u8,
}
```

### Section Order

1. Identity and role (cacheable)
2. Core instructions (cacheable)
3. Tool descriptions (cacheable) — auto-generated from trait registry + static map for legacy tools
4. Channel-specific rules (cacheable) — HTML for Telegram, conversational for voice
5. Git context (session-cached)
6. Git safety rules (cacheable)
7. Agent context (dynamic) — soul/personality
8. Memory/CLAUDE.md (session-cached)
9. MCP server instructions (dynamic, cache-breaking)
10. Plan mode constraints (conditional)

### Caching

- Sections 1-4, 6: static, API-level prompt caching
- Sections 5, 8: computed once per session
- Sections 7, 9-10: recomputed each turn
- `[DYNAMIC_BOUNDARY]` marker separates cacheable prefix from dynamic sections

### Token Budget

Target: ~4K tokens cacheable, ~2K dynamic. Total ~6K.

---

## Channel Rendering Rules (Cross-Cutting)

| Channel | Format | Approval | Progress |
|---------|--------|----------|----------|
| GUI | Markdown + rich UI | Dialog panels | Real-time Tauri events |
| Telegram | HTML only (`<b>`, `<i>`, `<code>`, `<pre>`) | Inline keyboard buttons | Edit message every 5s |
| Voice | Conversational natural language | Verbal approve/reject | Spoken status at intervals |
| Discord | Markdown | Reaction-based or text | Embed updates |
| Headless | Plain text, can route to Telegram | Auto-approve or route to channel | Log to task output |

---

## New Files Summary

| File | Feature |
|------|---------|
| `src-tauri/src/tool_registry.rs` | Tool trait + registry |
| `src-tauri/src/tools/file_edit.rs` | Structured file edit |
| `src-tauri/src/tools/file_read.rs` | Enhanced file read |
| `src-tauri/src/tool_streaming.rs` | Streaming infrastructure |
| `src-tauri/src/git_context.rs` | Git system prompt context |
| `src-tauri/src/tools/lsp/mod.rs` | LSP tool |
| `src-tauri/src/tools/lsp/server_manager.rs` | LSP server lifecycle |
| `src-tauri/src/tools/lsp/client.rs` | LSP JSON-RPC client |
| `src-tauri/src/tools/lsp/formatters.rs` | LSP result formatting |
| `src-tauri/src/tools/worktree.rs` | Worktree isolation |
| `src-tauri/src/tools/plan_mode.rs` | Plan mode |
| `src-tauri/src/tools/tasks.rs` | Background task tools |
| `src-tauri/src/tools/send_message.rs` | Inter-agent messaging |
| `src-tauri/src/security/llm_classifier.rs` | LLM permission classifier |
| `src-tauri/src/tools/notebook_edit.rs` | Notebook editing |
| `src-tauri/src/cost_tracker.rs` | Cost/token tracking |
| `src-tauri/src/system_prompt.rs` | System prompt builder |

## New LLM-Callable Tools (13 total)

`nexibot_file_edit`, `nexibot_file_read` (upgraded), `nexibot_lsp`, `nexibot_worktree`, `nexibot_enter_plan_mode`, `nexibot_exit_plan_mode`, `nexibot_task_create`, `nexibot_task_list`, `nexibot_task_get`, `nexibot_task_output`, `nexibot_task_stop`, `nexibot_send_message`, `nexibot_notebook_edit`

## Future Work (Not in v0.9.0)

- **Phase A**: Migrate all legacy tools from match statement to trait registry
- **Shared crate**: Extract tool system as reusable crate for KN-Code consumption
