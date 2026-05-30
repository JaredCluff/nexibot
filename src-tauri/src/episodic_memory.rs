/** Episodic memory: SQLite-backed self-improvement store.
 *
 * After each tool-loop run, a retrospective record is written summarising
 * what was tried, what worked, and what failed.  On the next similar task
 * those records are retrieved via vector similarity and injected into the
 * system prompt so the agent can learn from prior runs.
 */
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, info, warn};

use crate::embeddings;

/// Max records before LRU eviction.
const MAX_EPISODIC_RECORDS: usize = 10000;

/// Outcome of a completed task run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EpisodicOutcome {
    Success,
    Partial,
    Failure,
}

/// A single episodic retrospective record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodicRecord {
    pub id:              String,
    pub created_at:      DateTime<Utc>,
    pub goal:            String,
    pub outcome:         EpisodicOutcome,
    pub iterations:      usize,
    pub elapsed_ms:      u64,
    pub tools_used:      Vec<String>,
    pub what_worked:     Vec<String>,
    pub what_failed:     Vec<String>,
    pub embedding:       Option<Vec<f32>>,
}

/// Configuration controlling episodic-memory behaviour.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodicMemoryConfig {
    #[serde(default)]
    pub enabled:           bool,
    #[serde(default)]
    pub max_records:       usize,
    #[serde(default)]
    pub inject_limit:      usize,
    #[serde(default)]
    pub inject_threshold:  f32,
}

impl Default for EpisodicMemoryConfig {
    fn default() -> Self {
        Self {
            enabled:          true,
            max_records:      MAX_EPISODIC_RECORDS,
            inject_limit:     3,
            inject_threshold: 0.72,
        }
    }
}

pub struct EpisodicStore {
    db_path: PathBuf,
}

impl EpisodicStore {
    /// Open (or create) the episodic store at the given path.
    pub fn new(db_path: impl Into<PathBuf>) -> Result<Self> {
        let store = Self {
            db_path: db_path.into(),
        };
        store.init()?;
        Ok(store)
    }

    fn conn(&self) -> Result<Connection> {
        let c = Connection::open(&self.db_path)
            .context("open episodic memory db")?;
        c.pragma_update(None, "journal_mode", "WAL")?;
        c.pragma_update(None, "busy_timeout", 5000)?;
        Ok(c)
    }

    fn init(&self) -> Result<()> {
        let conn = self.conn()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS episodic_records (
                id         TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                goal       TEXT    NOT NULL,
                outcome    TEXT    NOT NULL,
                iterations INTEGER NOT NULL,
                elapsed_ms INTEGER NOT NULL,
                tools_used TEXT    NOT NULL,
                what_worked TEXT  NOT NULL,
                what_failed TEXT  NOT NULL,
                embedding  BLOB
            );
            CREATE INDEX IF NOT EXISTS idx_episodic_created ON episodic_records(created_at);
            CREATE VIRTUAL TABLE IF NOT EXISTS episodic_fts USING fts5(goal, content='', content_rowid=rowid);
            "
        ).context("episodic memory schema")?;
        // Triggers keep FTS5 in sync.
        conn.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS episodic_insert AFTER INSERT ON episodic_records BEGIN
              INSERT INTO episodic_fts(rowid, goal) VALUES (new.rowid, new.goal);
            END;
            CREATE TRIGGER IF NOT EXISTS episodic_delete AFTER DELETE ON episodic_records BEGIN
              INSERT INTO episodic_fts(episodic_fts, rowid, goal) VALUES ('delete', old.rowid, old.goal);
            END;
            CREATE TRIGGER IF NOT EXISTS episodic_update AFTER UPDATE ON episodic_records BEGIN
              INSERT INTO episodic_fts(episodic_fts, rowid, goal) VALUES ('delete', old.rowid, old.goal);
              INSERT INTO episodic_fts(rowid, goal) VALUES (new.rowid, new.goal);
            END;"
        ).context("episodic triggers")?;
        info!("[EPISODIC] Store initialised at {:?}", self.db_path);
        Ok(())
    }

    /// Persist a new retrospective record.  Computes an embedding on the goal text
    /// before insertion so similarity search works.
    pub async fn write(&self, record: &EpisodicRecord) -> Result<()> {
        let embedding = match embeddings::embed_text(&record.goal) {
            Ok(vec) => Some(to_blob(&vec)),
            Err(e) => {
                warn!("[EPISODIC] Embedding failed: {}", e);
                None
            }
        };

        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO episodic_records (id, created_at, goal, outcome, iterations,
                elapsed_ms, tools_used, what_worked, what_failed, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
               goal=excluded.goal, outcome=excluded.outcome, iterations=excluded.iterations,
               elapsed_ms=excluded.elapsed_ms, tools_used=excluded.tools_used,
               what_worked=excluded.what_worked, what_failed=excluded.what_failed,
               embedding=excluded.embedding",
            params![
                record.id, record.created_at.timestamp_millis(),
                &record.goal, outcome_str(&record.outcome),
                record.iterations as i64, record.elapsed_ms as i64,
                json_str(&record.tools_used), json_str(&record.what_worked),
                json_str(&record.what_failed), embedding,
            ],
        ).context("insert episodic record")?;
        debug!("[EPISODIC] Wrote record {}", record.id);
        Ok(())
    }

    /// Search for records whose goal embedding is cosine-similar to the goal text.
    /// Returns up to `limit` records, sorted highest-first by similarity.
    pub async fn search_similar(&self, goal: &str, limit: usize) -> Result<Vec<(EpisodicRecord, f32)>> {
        let query_emb = match embeddings::embed_text(goal) {
            Ok(v) => v,
            Err(e) => {
                warn!("[EPISODIC] Cannot embed query: {}", e);
                return Ok(Vec::new());
            }
        };
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, created_at, goal, outcome, iterations, elapsed_ms,
                    tools_used, what_worked, what_failed, embedding
             FROM episodic_records"
        )?;
        let rows = stmt.query_map([], |row| {
            let emb_blob: Option<Vec<u8>> = row.get(9)?;
            let emb = emb_blob.as_deref().map(from_blob);
            Ok((
                EpisodicRecord {
                    id: row.get(0)?,
                    created_at: DateTime::from_timestamp_millis(row.get(1)?).unwrap_or_else(|| Utc::now()),
                    goal: row.get(2)?,
                    outcome: parse_outcome(row.get::<_, String>(3)?.as_str()),
                    iterations: row.get::<_, i64>(4)? as usize,
                    elapsed_ms: row.get::<_, i64>(5)? as u64,
                    tools_used: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
                    what_worked: serde_json::from_str(&row.get::<_, String>(7)?).unwrap_or_default(),
                    what_failed: serde_json::from_str(&row.get::<_, String>(8)?).unwrap_or_default(),
                    embedding: emb,
                },
                emb_blob,
            ))
        })?;

        let mut scored = Vec::new();
        for row in rows {
            let (rec, emb_blob) = row?;
            let sim = emb_blob
                .map(|b| cosine_similarity(&query_emb, &from_blob(&b)))
                .unwrap_or(0.0);
            scored.push((rec, sim));
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        scored.truncate(limit);
        Ok(scored.into_iter().map(|(r, s)| (r, s)).collect())
    }

    /// Evict oldest records to keep total at most `keep`.
    pub async fn prune_old(&self, keep: usize) -> Result<()> {
        let conn = self.conn()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM episodic_records", [], |row| row.get(0))?;
        let excess = count.saturating_sub(keep as i64);
        if excess > 0 {
            let n = conn.execute(
                "DELETE FROM episodic_records WHERE id IN (
                    SELECT id FROM episodic_records ORDER BY created_at ASC LIMIT ?1
                 )",
                [excess],
            )?;
            info!("[EPISODIC] Pruned {} oldest records", n);
        }
        Ok(())
    }
}

fn outcome_str(o: &EpisodicOutcome) -> &'static str {
    match o {
        EpisodicOutcome::Success => "success",
        EpisodicOutcome::Partial => "partial",
        EpisodicOutcome::Failure => "failure",
    }
}

fn parse_outcome(s: &str) -> EpisodicOutcome {
    match s {
        "success" => EpisodicOutcome::Success,
        "partial" => EpisodicOutcome::Partial,
        _ => EpisodicOutcome::Failure,
    }
}

fn json_str<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".into())
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn from_blob(v: &[u8]) -> Vec<f32> {
    v.chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let (dot, norm_a, norm_b) = a.iter().zip(b.iter()).fold((0.0_f32, 0.0_f32, 0.0_f32), |(d, na, nb), (x, y)| {
        (d + x * y, na + x * x, nb + y * y)
    });
    let denom = (norm_a.sqrt() * norm_b.sqrt()).max(f32::EPSILON);
    (dot / denom).clamp(0.0, 1.0)
}

/// Format a retrospective record as a compact text block for injection.
/// Target: ≤400 tokens.
pub fn format_retrospective(r: &EpisodicRecord) -> String {
    format!(
        "Prior task on '{}' ({} in {}ms): succeeded with {}; struggled with {}. Tools: {}.",
        r.goal.chars().take(120).collect::<String>(),
        outcome_str(&r.outcome),
        r.elapsed_ms,
        r.what_worked.iter().take(3).cloned().collect::<Vec<_>>().join(", "),
        r.what_failed.iter().take(3).cloned().collect::<Vec<_>>().join(", "),
        r.tools_used.iter().take(5).cloned().collect::<Vec<_>>().join(", "),
    )
}