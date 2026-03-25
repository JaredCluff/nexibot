//! Skill Lifecycle Manager
//!
//! Implements the self-learning skills pipeline:
//!
//! 1. **Auto-creation**: After each conversation turn a `TurnSummary` is sent over an
//!    async channel.  A background task scores the turn (tool diversity, length,
//!    recurrence) and — when the score crosses the configured threshold — fires an
//!    async LLM sub-call asking the model to synthesise a reusable skill.  The result
//!    is written to `~/.config/nexibot/skills/agent-generated/<slug>/` and the existing
//!    hot-reload watcher picks it up automatically.
//!
//! 2. **Improvement loop**: Every skill invocation is recorded in `skill_usage_log`.
//!    After every N uses (default 5) an improvement job is queued.  The improvement
//!    sub-call rewrites the `SKILL.md` based on the last 10 outcomes, then overwrites
//!    the file on disk.
//!
//! 3. **Explicit capture**: `/save-as-skill [name]` sends a `TurnSummary` with
//!    `score_override: Some(1.0)`, bypassing the heuristic entirely.
//!
//! All generated skills pass through the existing `skill_scanner` before going live.
//! Skills flagged as `Dangerous` are discarded; `Warning`-level skills are written
//! but tagged `source: agent-generated-flagged` in their frontmatter.

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn};
use uuid::Uuid;

use crate::skill_security::RiskLevel;
use crate::skills::SkillsManager;

// ─────────────────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────────────────

/// Summary of a completed conversation turn, sent to the lifecycle manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnSummary {
    /// Unique turn identifier (UUID v4).
    pub turn_id: String,
    /// Session this turn belongs to.
    pub session_id: String,
    /// All tool names used during the turn (may contain duplicates).
    pub tools_used: Vec<String>,
    /// Total assistant messages in the turn.
    pub assistant_message_count: usize,
    /// Full conversation text for the turn (user + assistant combined).
    pub conversation_text: String,
    /// When set, bypasses scoring and forces creation at this score (0.0–1.0).
    pub score_override: Option<f32>,
    /// Optional user-provided name hint for the skill.
    pub name_hint: Option<String>,
}

/// Outcome of a single skill invocation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillOutcome {
    Success,
    Error,
    Skipped,
}

impl std::fmt::Display for SkillOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::Error => write!(f, "error"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

/// A recorded invocation of a skill (written to `skill_usage_log`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillUsageRecord {
    pub skill_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub outcome: SkillOutcome,
    pub tools_used: Vec<String>,
    pub timestamp: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Scoring
// ─────────────────────────────────────────────────────────────────────────────

/// Default score threshold above which a skill is created.
const DEFAULT_CREATION_THRESHOLD: f32 = 0.65;

/// Default number of skill uses before an improvement job is queued.
const DEFAULT_IMPROVEMENT_USES: i64 = 5;

/// Score a turn for skill-worthiness.  Returns 0.0–1.0.
fn score_turn(summary: &TurnSummary, recent_tool_sets: &[Vec<String>]) -> f32 {
    if let Some(override_score) = summary.score_override {
        return override_score.clamp(0.0, 1.0);
    }

    let mut score: f32 = 0.0;

    // Distinct tools used (≥4 distinct tools → +0.30)
    let distinct: std::collections::HashSet<&String> = summary.tools_used.iter().collect();
    if distinct.len() >= 4 {
        score += 0.30;
    } else if distinct.len() >= 2 {
        score += 0.10;
    }

    // Turn length (≥6 assistant messages → +0.20)
    if summary.assistant_message_count >= 6 {
        score += 0.20;
    } else if summary.assistant_message_count >= 3 {
        score += 0.08;
    }

    // Multi-step file + exec + search combination → +0.15
    let has_file = distinct.iter().any(|t| {
        t.contains("file") || t.contains("read") || t.contains("write") || t.contains("filesystem")
    });
    let has_exec = distinct.iter().any(|t| {
        t.contains("exec") || t.contains("shell") || t.contains("bash") || t.contains("run")
    });
    let has_search = distinct.iter().any(|t| {
        t.contains("search") || t.contains("fetch") || t.contains("browser") || t.contains("web")
    });
    if (has_file as u8) + (has_exec as u8) + (has_search as u8) >= 2 {
        score += 0.15;
    }

    // Recurrence: similar tool set seen in recent turns → +0.25
    let current_set: std::collections::HashSet<&String> = summary.tools_used.iter().collect();
    for prior_tools in recent_tool_sets {
        let prior_set: std::collections::HashSet<&String> = prior_tools.iter().collect();
        let intersection = current_set.intersection(&prior_set).count();
        let union = current_set.union(&prior_set).count();
        if union > 0 {
            let jaccard = intersection as f32 / union as f32;
            if jaccard >= 0.6 {
                score += 0.25;
                break;
            }
        }
    }

    score.min(1.0)
}

// ─────────────────────────────────────────────────────────────────────────────
// SkillLifecycleManager
// ─────────────────────────────────────────────────────────────────────────────

/// Manages skill creation, usage tracking, and improvement.
pub struct SkillLifecycleManager {
    /// SQLite connection for `skill_usage_log` and `skill_creation_queue`.
    conn: Connection,
    /// Sender half — cloned into AppState so callers can submit `TurnSummary` events.
    pub tx: mpsc::Sender<TurnSummary>,
    /// Receiver half — consumed by the background task in `start_background_task`.
    rx: Option<mpsc::Receiver<TurnSummary>>,
    /// Score threshold for auto-creating skills.
    creation_threshold: f32,
    /// Uses before triggering an improvement job.
    improvement_uses: i64,
}

impl SkillLifecycleManager {
    /// Create a new manager and open / migrate the SQLite database.
    pub fn new() -> Result<Self> {
        let db_path = Self::db_path()?;
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&db_path)
            .with_context(|| format!("Failed to open skill lifecycle DB at {:?}", db_path))?;

        // WAL mode + busy timeout
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;

        // Owner-only permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&db_path, fs::Permissions::from_mode(0o600));
        }

        Self::migrate(&conn)?;

        let (tx, rx) = mpsc::channel(256);

        Ok(Self {
            conn,
            tx,
            rx: Some(rx),
            creation_threshold: DEFAULT_CREATION_THRESHOLD,
            improvement_uses: DEFAULT_IMPROVEMENT_USES,
        })
    }

    /// Path to the lifecycle SQLite database.
    pub fn db_path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Failed to get home directory")?;
        Ok(home.join(".config/nexibot/skills/lifecycle.db"))
    }

    /// Create tables if they do not exist.
    fn migrate(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "BEGIN IMMEDIATE;

            CREATE TABLE IF NOT EXISTS skill_usage_log (
                id          TEXT PRIMARY KEY,
                skill_id    TEXT NOT NULL,
                session_id  TEXT NOT NULL,
                turn_id     TEXT NOT NULL,
                outcome     TEXT NOT NULL,
                tools_used  TEXT NOT NULL,
                timestamp   TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_usage_skill_id
                ON skill_usage_log(skill_id);

            CREATE TABLE IF NOT EXISTS skill_creation_queue (
                id                  TEXT PRIMARY KEY,
                turn_id             TEXT NOT NULL,
                session_id          TEXT NOT NULL,
                score               REAL NOT NULL,
                status              TEXT NOT NULL DEFAULT 'pending',
                generated_skill_id  TEXT,
                timestamp           TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_creation_status
                ON skill_creation_queue(status);

            -- Rolling window: last 100 turn tool-sets for recurrence detection
            CREATE TABLE IF NOT EXISTS recent_turns (
                id          TEXT PRIMARY KEY,
                turn_id     TEXT NOT NULL,
                tools_json  TEXT NOT NULL,
                timestamp   TEXT NOT NULL
            );

            COMMIT;",
        )
        .unwrap_or_else(|e| {
            let _ = conn.execute_batch("ROLLBACK;");
            warn!("[SKILL_LIFECYCLE] Migration note: {}", e);
        });
        Ok(())
    }

    // ── Usage logging ────────────────────────────────────────────────────────

    /// Record a skill invocation.  Call this whenever a skill is used.
    pub fn record_usage(&self, record: &SkillUsageRecord) -> Result<()> {
        let tools_json = serde_json::to_string(&record.tools_used)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO skill_usage_log
             (id, skill_id, session_id, turn_id, outcome, tools_used, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                Uuid::new_v4().to_string(),
                record.skill_id,
                record.session_id,
                record.turn_id,
                record.outcome.to_string(),
                tools_json,
                record.timestamp,
            ],
        )?;
        Ok(())
    }

    /// Return the last N usage records for a skill.
    pub fn recent_usage(&self, skill_id: &str, limit: usize) -> Vec<SkillUsageRecord> {
        let mut stmt = match self.conn.prepare(
            "SELECT skill_id, session_id, turn_id, outcome, tools_used, timestamp
             FROM skill_usage_log
             WHERE skill_id = ?1
             ORDER BY timestamp DESC
             LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map(params![skill_id, limit as i64], |row| {
            let tools_json: String = row.get(4)?;
            let tools_used: Vec<String> =
                serde_json::from_str(&tools_json).unwrap_or_default();
            let outcome_str: String = row.get(3)?;
            let outcome = match outcome_str.as_str() {
                "success" => SkillOutcome::Success,
                "error" => SkillOutcome::Error,
                _ => SkillOutcome::Skipped,
            };
            Ok(SkillUsageRecord {
                skill_id: row.get(0)?,
                session_id: row.get(1)?,
                turn_id: row.get(2)?,
                outcome,
                tools_used,
                timestamp: row.get(5)?,
            })
        })
        .ok()
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    /// Total usage count for a skill.
    pub fn usage_count(&self, skill_id: &str) -> i64 {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM skill_usage_log WHERE skill_id = ?1",
                params![skill_id],
                |row| row.get(0),
            )
            .unwrap_or(0)
    }

    // ── Recent turn window ───────────────────────────────────────────────────

    fn store_recent_turn(&self, turn_id: &str, tools: &[String]) {
        let tools_json = serde_json::to_string(tools).unwrap_or_default();
        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO recent_turns (id, turn_id, tools_json, timestamp)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                Uuid::new_v4().to_string(),
                turn_id,
                tools_json,
                Utc::now().to_rfc3339(),
            ],
        );
        // Keep only last 100 entries
        let _ = self.conn.execute(
            "DELETE FROM recent_turns WHERE id NOT IN (
                SELECT id FROM recent_turns ORDER BY timestamp DESC LIMIT 100
             )",
            [],
        );
    }

    fn load_recent_tool_sets(&self) -> Vec<Vec<String>> {
        let mut stmt = match self.conn.prepare(
            "SELECT tools_json FROM recent_turns ORDER BY timestamp DESC LIMIT 30",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        stmt.query_map([], |row| row.get::<_, String>(0))
            .ok()
            .map(|rows| {
                rows.flatten()
                    .filter_map(|j| serde_json::from_str::<Vec<String>>(&j).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    // ── Background task ──────────────────────────────────────────────────────

    /// Consume the receiver and start the background processing loop.
    ///
    /// `claude_client` and `skills_manager` are needed for LLM sub-calls and
    /// writing generated skills to disk.  Both are Arc<RwLock<_>> so we can hold
    /// them for the lifetime of the task without blocking the main thread.
    pub fn start_background_task(
        mut self,
        claude_client: Arc<RwLock<crate::claude::ClaudeClient>>,
        skills_manager: Arc<RwLock<SkillsManager>>,
    ) -> Arc<mpsc::Sender<TurnSummary>> {
        let tx = Arc::new(self.tx.clone());
        let rx = self.rx.take().expect("receiver already consumed");
        let threshold = self.creation_threshold;
        let improvement_uses = self.improvement_uses;

        // We need the manager fields accessible in the async task.
        // Move the manager into a Mutex so the task can call &self methods.
        let manager = Arc::new(tokio::sync::Mutex::new(self));

        tokio::spawn(async move {
            let mut rx = rx;
            while let Some(summary) = rx.recv().await {
                let mgr = manager.lock().await;
                mgr.process_turn(
                    &summary,
                    threshold,
                    improvement_uses,
                    &claude_client,
                    &skills_manager,
                )
                .await;
            }
        });

        tx
    }

    /// Process one incoming `TurnSummary`.
    async fn process_turn(
        &self,
        summary: &TurnSummary,
        threshold: f32,
        improvement_uses: i64,
        claude_client: &Arc<RwLock<crate::claude::ClaudeClient>>,
        skills_manager: &Arc<RwLock<SkillsManager>>,
    ) {
        // Store turn tools for future recurrence detection
        self.store_recent_turn(&summary.turn_id, &summary.tools_used);
        let recent = self.load_recent_tool_sets();

        // Score the turn
        let score = score_turn(summary, &recent);
        info!(
            "[SKILL_LIFECYCLE] Turn {} scored {:.2} (threshold {:.2})",
            summary.turn_id, score, threshold
        );

        if score < threshold {
            return;
        }

        // Queue the creation
        if let Err(e) = self.queue_creation(summary, score) {
            warn!("[SKILL_LIFECYCLE] Failed to queue creation: {}", e);
            return;
        }

        // Fire the creation sub-call
        self.run_creation_job(summary, claude_client, skills_manager)
            .await;

        // Check all skills for pending improvement
        self.run_improvement_jobs(improvement_uses, claude_client, skills_manager)
            .await;
    }

    fn queue_creation(&self, summary: &TurnSummary, score: f32) -> Result<()> {
        self.conn.execute(
            "INSERT INTO skill_creation_queue
             (id, turn_id, session_id, score, status, timestamp)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
            params![
                Uuid::new_v4().to_string(),
                summary.turn_id,
                summary.session_id,
                score,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    // ── LLM sub-calls ────────────────────────────────────────────────────────

    async fn run_creation_job(
        &self,
        summary: &TurnSummary,
        claude_client: &Arc<RwLock<crate::claude::ClaudeClient>>,
        skills_manager: &Arc<RwLock<SkillsManager>>,
    ) {
        let name_hint = summary.name_hint.as_deref().unwrap_or("").to_string();
        let prompt = build_creation_prompt(&summary.conversation_text, &name_hint, &summary.tools_used);

        let response = {
            let client = claude_client.read().await;
            match client.send_simple_message(&prompt).await {
                Ok(r) => r,
                Err(e) => {
                    warn!("[SKILL_LIFECYCLE] Creation LLM call failed: {}", e);
                    return;
                }
            }
        };

        match parse_and_write_skill(&response, skills_manager, false).await {
            Ok(skill_id) => {
                info!("[SKILL_LIFECYCLE] Auto-created skill: {}", skill_id);
                // Update queue status
                let _ = self.conn.execute(
                    "UPDATE skill_creation_queue
                     SET status = 'created', generated_skill_id = ?1
                     WHERE turn_id = ?2 AND status = 'pending'",
                    params![skill_id, summary.turn_id],
                );
            }
            Err(e) => {
                warn!("[SKILL_LIFECYCLE] Failed to write skill: {}", e);
                let _ = self.conn.execute(
                    "UPDATE skill_creation_queue SET status = 'failed'
                     WHERE turn_id = ?1 AND status = 'pending'",
                    params![summary.turn_id],
                );
            }
        }
    }

    async fn run_improvement_jobs(
        &self,
        improvement_uses: i64,
        claude_client: &Arc<RwLock<crate::claude::ClaudeClient>>,
        skills_manager: &Arc<RwLock<SkillsManager>>,
    ) {
        // Find agent-generated skills that hit the improvement threshold
        let candidates: Vec<(String, i64)> = {
            let mut stmt = match self.conn.prepare(
                "SELECT skill_id, COUNT(*) as cnt
                 FROM skill_usage_log
                 GROUP BY skill_id
                 HAVING cnt >= ?1 AND cnt % ?1 = 0",
            ) {
                Ok(s) => s,
                Err(_) => return,
            };
            stmt.query_map(params![improvement_uses], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .ok()
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
        };

        for (skill_id, _count) in candidates {
            let usage_log = self.recent_usage(&skill_id, 10);
            let skill_content = {
                let mgr = skills_manager.read().await;
                mgr.get_skill_content(&skill_id)
            };
            let Some(current_content) = skill_content else {
                continue;
            };

            let prompt = build_improvement_prompt(&skill_id, &current_content, &usage_log);

            let response = {
                let client = claude_client.read().await;
                match client.send_simple_message(&prompt).await {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("[SKILL_LIFECYCLE] Improvement LLM call for '{}' failed: {}", skill_id, e);
                        continue;
                    }
                }
            };

            if let Err(e) = apply_skill_improvement(&skill_id, &response, skills_manager).await {
                warn!("[SKILL_LIFECYCLE] Failed to apply improvement to '{}': {}", skill_id, e);
            } else {
                info!("[SKILL_LIFECYCLE] Improved skill: {}", skill_id);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Prompt builders
// ─────────────────────────────────────────────────────────────────────────────

fn build_creation_prompt(conversation: &str, name_hint: &str, tools_used: &[String]) -> String {
    let name_line = if name_hint.is_empty() {
        String::new()
    } else {
        format!("The user suggested the name: \"{}\"\n\n", name_hint)
    };

    let distinct_tools: std::collections::BTreeSet<&String> = tools_used.iter().collect();
    let tools_list = distinct_tools.iter()
        .map(|t| format!("- {}", t))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"You are NexiBot's skill writer. Based on the conversation below, create a reusable skill that can be used to handle similar tasks in the future.

{name_line}Tools used in this conversation:
{tools_list}

CONVERSATION:
---
{conversation}
---

Output a complete skill bundle with exactly this structure (XML-fenced sections):

<SKILL_MD>
---
name: "Skill Name"
description: "One-sentence description"
user-invocable: true
version: "1.0.0"
author: "agent-generated"
source: "agent-generated"
---

## Overview

[What this skill does and when to use it]

## Steps

[Step-by-step instructions the agent should follow]

## Examples

[One or two example invocations]
</SKILL_MD>

If the task required a script, also include:

<SCRIPT_TYPE>python</SCRIPT_TYPE>
<SCRIPT_CONTENT>
[python script content]
</SCRIPT_CONTENT>

If no script is needed, omit the SCRIPT_TYPE and SCRIPT_CONTENT sections entirely.
Output ONLY the fenced sections above, no other text."#,
        name_line = name_line,
        tools_list = tools_list,
        conversation = &conversation[..conversation.len().min(8000)],
    )
}

fn build_improvement_prompt(skill_id: &str, current_content: &str, usage: &[SkillUsageRecord]) -> String {
    let usage_summary = usage
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "{}. [{}] outcome={} tools={:?}",
                i + 1,
                &r.timestamp[..r.timestamp.len().min(19)],
                r.outcome,
                r.tools_used,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"You are NexiBot's skill editor. The skill below has been used {count} times. Based on the usage log, improve the skill's instructions to make it more effective.

SKILL ID: {skill_id}

CURRENT SKILL.MD:
---
{content}
---

RECENT USAGE LOG (newest first):
{usage_summary}

Rules:
- Output ONLY the improved SKILL.md content (including YAML frontmatter), nothing else.
- Increment the version field (e.g. "1.0.0" → "1.0.1").
- Keep the `author`, `source`, and `name` fields unchanged.
- If the usage log shows consistent errors, fix the instructions.
- If the usage log shows consistent success, clarify and tighten the instructions.
- Do not change the skill if usage shows it is already working well and the improvements would be minor."#,
        count = usage.len(),
        skill_id = skill_id,
        content = current_content,
        usage_summary = usage_summary,
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Skill writing helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Parse the LLM creation response, security-scan it, and write it to disk.
/// Returns the generated skill ID on success.
async fn parse_and_write_skill(
    response: &str,
    skills_manager: &Arc<RwLock<SkillsManager>>,
    _is_improvement: bool,
) -> Result<String> {
    // Extract SKILL_MD
    let skill_md = extract_between(response, "<SKILL_MD>", "</SKILL_MD>")
        .context("Response missing <SKILL_MD> section")?;

    // Extract optional script
    let script_type = extract_between(response, "<SCRIPT_TYPE>", "</SCRIPT_TYPE>");
    let script_content = extract_between(response, "<SCRIPT_CONTENT>", "</SCRIPT_CONTENT>");

    // Extract name from frontmatter for the directory slug
    let skill_name = extract_yaml_field(skill_md, "name").unwrap_or_else(|| "unnamed-skill".to_string());
    let skill_id = slugify(&skill_name);

    // Security scan
    let scan = crate::skill_security::analyze_skill_content(
        &skill_id,
        skill_md,
        None,
        None,
    );

    match scan.risk_level {
        RiskLevel::Dangerous => {
            return Err(anyhow::anyhow!(
                "Generated skill '{}' failed security scan (Dangerous): {}",
                skill_id,
                scan.summary
            ));
        }
        RiskLevel::Warning => {
            warn!(
                "[SKILL_LIFECYCLE] Skill '{}' flagged at Warning level: {}",
                skill_id, scan.summary
            );
            // Write but tag as flagged — done by injecting into frontmatter below
        }
        _ => {}
    }

    // Annotate flagged skills
    let final_skill_md = if scan.risk_level == RiskLevel::Warning {
        skill_md.replacen(
            "source: \"agent-generated\"",
            "source: \"agent-generated-flagged\"",
            1,
        )
    } else {
        skill_md.to_string()
    };

    // Write files via SkillsManager
    let mut mgr = skills_manager.write().await;
    mgr.write_agent_generated_skill(
        &skill_id,
        &final_skill_md,
        script_type.as_deref(),
        script_content.as_deref(),
    )?;

    Ok(skill_id)
}

/// Apply an improvement: overwrite the SKILL.md for an existing skill, re-scan,
/// then hot-reload.
async fn apply_skill_improvement(
    skill_id: &str,
    improved_content: &str,
    skills_manager: &Arc<RwLock<SkillsManager>>,
) -> Result<()> {
    // Security scan the improved content
    let scan = crate::skill_security::analyze_skill_content(
        skill_id,
        improved_content,
        None,
        None,
    );
    if scan.risk_level == RiskLevel::Dangerous {
        return Err(anyhow::anyhow!(
            "Improved skill content flagged as Dangerous: {}",
            scan.summary
        ));
    }

    let mut mgr = skills_manager.write().await;
    mgr.update_skill_content(skill_id, improved_content)?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// String utilities
// ─────────────────────────────────────────────────────────────────────────────

fn extract_between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let start_pos = s.find(start)? + start.len();
    let end_pos = s[start_pos..].find(end)? + start_pos;
    Some(s[start_pos..end_pos].trim())
}

fn extract_yaml_field(content: &str, field: &str) -> Option<String> {
    let prefix = format!("{}:", field);
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&prefix) {
            let value = trimmed[prefix.len()..]
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn slugify(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() { c.to_lowercase().next().unwrap_or(c) } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

// ─────────────────────────────────────────────────────────────────────────────
// SkillsManager extensions required by SkillLifecycleManager
// ─────────────────────────────────────────────────────────────────────────────
// These are added to SkillsManager in skills.rs (see below).
// Declared here as trait extensions so we can call them without touching the
// SkillsManager file's existing public API surface.

pub trait SkillLifecycleExt {
    fn write_agent_generated_skill(
        &mut self,
        skill_id: &str,
        skill_md: &str,
        script_type: Option<&str>,
        script_content: Option<&str>,
    ) -> Result<()>;

    fn update_skill_content(&mut self, skill_id: &str, new_content: &str) -> Result<()>;

    fn get_skill_content(&self, skill_id: &str) -> Option<String>;
}

impl SkillLifecycleExt for SkillsManager {
    fn write_agent_generated_skill(
        &mut self,
        skill_id: &str,
        skill_md: &str,
        script_type: Option<&str>,
        script_content: Option<&str>,
    ) -> Result<()> {
        let base = SkillsManager::get_skills_dir()
            .context("Cannot get skills directory")?
            .join("agent-generated")
            .join(skill_id);
        fs::create_dir_all(&base)?;

        let skill_md_path = base.join("SKILL.md");
        fs::write(&skill_md_path, skill_md)?;
        info!("[SKILL_LIFECYCLE] Wrote {}", skill_md_path.display());

        if let (Some(stype), Some(scontent)) = (script_type, script_content) {
            let scripts_dir = base.join("scripts");
            fs::create_dir_all(&scripts_dir)?;
            let ext = if stype.contains("python") || stype == "py" { "py" } else { "sh" };
            let script_path = scripts_dir.join(format!("run.{}", ext));
            fs::write(&script_path, scontent)?;

            // Make shell scripts executable on Unix
            #[cfg(unix)]
            if ext == "sh" {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755));
            }
            info!("[SKILL_LIFECYCLE] Wrote script {}", script_path.display());
        }

        // Hot-reload: add the new skill to the in-memory registry
        if let Err(e) = self.load_all_skills() {
            warn!("[SKILL_LIFECYCLE] Hot-reload after write failed: {}", e);
        }

        Ok(())
    }

    fn update_skill_content(&mut self, skill_id: &str, new_content: &str) -> Result<()> {
        let skill_md_path = SkillsManager::get_skills_dir()
            .context("Cannot get skills directory")?
            .join("agent-generated")
            .join(skill_id)
            .join("SKILL.md");

        if !skill_md_path.exists() {
            return Err(anyhow::anyhow!(
                "Cannot update '{}': SKILL.md not found at {:?}",
                skill_id,
                skill_md_path
            ));
        }

        fs::write(&skill_md_path, new_content)?;
        if let Err(e) = self.load_all_skills() {
            warn!("[SKILL_LIFECYCLE] Hot-reload after update failed: {}", e);
        }
        Ok(())
    }

    fn get_skill_content(&self, skill_id: &str) -> Option<String> {
        let skill_md_path = SkillsManager::get_skills_dir()
            .ok()?
            .join("agent-generated")
            .join(skill_id)
            .join("SKILL.md");
        fs::read_to_string(&skill_md_path).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_score_low() {
        let summary = TurnSummary {
            turn_id: "t1".into(),
            session_id: "s1".into(),
            tools_used: vec!["fetch".into()],
            assistant_message_count: 1,
            conversation_text: "short".into(),
            score_override: None,
            name_hint: None,
        };
        let score = score_turn(&summary, &[]);
        assert!(score < DEFAULT_CREATION_THRESHOLD, "score={}", score);
    }

    #[test]
    fn test_score_high() {
        let summary = TurnSummary {
            turn_id: "t2".into(),
            session_id: "s1".into(),
            tools_used: vec![
                "fetch".into(), "search".into(), "filesystem_write".into(),
                "execute_command".into(), "browser".into(),
            ],
            assistant_message_count: 7,
            conversation_text: "complex workflow".into(),
            score_override: None,
            name_hint: None,
        };
        let score = score_turn(&summary, &[]);
        assert!(score >= DEFAULT_CREATION_THRESHOLD, "score={}", score);
    }

    #[test]
    fn test_score_override() {
        let summary = TurnSummary {
            turn_id: "t3".into(),
            session_id: "s1".into(),
            tools_used: vec![],
            assistant_message_count: 0,
            conversation_text: "".into(),
            score_override: Some(1.0),
            name_hint: None,
        };
        assert_eq!(score_turn(&summary, &[]), 1.0);
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("My Cool Skill!"), "my-cool-skill");
        assert_eq!(slugify("web-search"), "web-search");
    }

    #[test]
    fn test_extract_between() {
        let s = "foo <TAG>content here</TAG> bar";
        assert_eq!(extract_between(s, "<TAG>", "</TAG>"), Some("content here"));
    }
}
