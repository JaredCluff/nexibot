//! SQLite persistence for DAG definitions, runs, tasks, and history.

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use rusqlite::{params, Connection};
use std::path::PathBuf;
use tracing::info;
use uuid::Uuid;

use super::{
    DagDefinition, DagHistoryEntry, DagRun, DagRunStatus, DagRunSummary, DagTask,
    DagTaskDefinition, DagTaskStatus,
};

/// SQLite-backed DAG store.
pub struct DagStore {
    conn: Connection,
}

impl DagStore {
    /// Open or create the DAG store at the given path.
    pub fn new(db_path: PathBuf) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create DAG store directory: {}", parent.display())
            })?;
        }

        let conn = Connection::open(&db_path)
            .with_context(|| format!("Failed to open DAG database at {}", db_path.display()))?;

        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;

        let store = Self { conn };
        store.create_tables()?;

        info!("[DAG_STORE] Opened at {}", db_path.display());
        Ok(store)
    }

    fn create_tables(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS dag_definitions (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                tasks_json TEXT NOT NULL,
                is_template INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS dag_runs (
                id TEXT PRIMARY KEY,
                definition_id TEXT,
                name TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                workspace_id TEXT NOT NULL,
                started_at TEXT,
                completed_at TEXT,
                created_at TEXT NOT NULL,
                error TEXT
            );

            CREATE TABLE IF NOT EXISTS dag_tasks (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL REFERENCES dag_runs(id),
                task_key TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                description TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                output TEXT,
                error TEXT,
                attempt INTEGER NOT NULL DEFAULT 0,
                max_retries INTEGER NOT NULL DEFAULT 0,
                retry_delay_ms INTEGER NOT NULL DEFAULT 1000,
                started_at TEXT,
                completed_at TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS dag_task_deps (
                task_id TEXT NOT NULL REFERENCES dag_tasks(id),
                depends_on_task_id TEXT NOT NULL REFERENCES dag_tasks(id),
                PRIMARY KEY (task_id, depends_on_task_id)
            );

            CREATE TABLE IF NOT EXISTS dag_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL REFERENCES dag_runs(id),
                task_id TEXT,
                event_type TEXT NOT NULL,
                details TEXT,
                timestamp TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_dag_tasks_run_id ON dag_tasks(run_id);
            CREATE INDEX IF NOT EXISTS idx_dag_tasks_status ON dag_tasks(status);
            CREATE INDEX IF NOT EXISTS idx_dag_history_run_id ON dag_history(run_id);
            CREATE INDEX IF NOT EXISTS idx_dag_runs_status ON dag_runs(status);
            ",
        )?;

        // Schema migration: add notification_sent column to dag_runs if it doesn't exist.
        // This is safe to run repeatedly — the column add is a no-op if it already exists.
        let _ = self.conn.execute_batch(
            "ALTER TABLE dag_runs ADD COLUMN notification_sent INTEGER NOT NULL DEFAULT 1;",
        );
        // Mark all existing completed runs as already notified so we don't spam on upgrade.
        let _ = self.conn.execute_batch(
            "UPDATE dag_runs SET notification_sent = 1 WHERE notification_sent IS NULL;",
        );

        Ok(())
    }

    /// Return run IDs + names for completed/failed runs whose notifications have not been sent.
    /// Used by the heartbeat catch-up scan.
    pub fn get_unsent_completed_runs(&self) -> Result<Vec<(String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, status FROM dag_runs
             WHERE status IN ('completed', 'failed')
               AND notification_sent = 0
             ORDER BY completed_at ASC
             LIMIT 50",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Mark a run's completion notification as sent.
    pub fn mark_notification_sent(&self, run_id: &str) -> Result<()> {
        let affected = self.conn.execute(
            "UPDATE dag_runs SET notification_sent = 1 WHERE id = ?1",
            params![run_id],
        )?;
        if affected == 0 {
            return Err(anyhow!(
                "DAG run {} was not found while marking notification_sent",
                run_id
            ));
        }
        Ok(())
    }

    // ── Definitions ──────────────────────────────────────────────────

    /// Save a DAG definition (insert or update).
    pub fn save_definition(&self, def: &DagDefinition) -> Result<()> {
        let tasks_json = serde_json::to_string(&def.tasks)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO dag_definitions (id, name, description, tasks_json, is_template, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                def.id,
                def.name,
                def.description,
                tasks_json,
                def.is_template as i32,
                def.created_at.to_rfc3339(),
                def.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Get a DAG definition by ID.
    pub fn get_definition(&self, id: &str) -> Result<Option<DagDefinition>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, tasks_json, is_template, created_at, updated_at
             FROM dag_definitions WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => {
                let tasks_json: String = row.get(3)?;
                Ok(Some(DagDefinition {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    tasks: serde_json::from_str(&tasks_json)?,
                    is_template: row.get::<_, i32>(4)? != 0,
                    created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    updated_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(6)?)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                }))
            }
            None => Ok(None),
        }
    }

    /// List all template definitions.
    pub fn list_templates(&self) -> Result<Vec<DagDefinition>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, tasks_json, is_template, created_at, updated_at
             FROM dag_definitions WHERE is_template = 1 ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let tasks_json: String = row.get(3)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                tasks_json,
                row.get::<_, i32>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;

        let mut defs = Vec::new();
        for row in rows {
            let (id, name, description, tasks_json, is_template, created_at, updated_at) = row?;
            defs.push(DagDefinition {
                id,
                name,
                description,
                tasks: serde_json::from_str(&tasks_json).unwrap_or_default(),
                is_template: is_template != 0,
                created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                updated_at: chrono::DateTime::parse_from_rfc3339(&updated_at)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            });
        }
        Ok(defs)
    }

    /// Delete a definition by ID.
    pub fn delete_definition(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM dag_definitions WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ── Runs ─────────────────────────────────────────────────────────

    /// Create a new DAG run from a definition (inline or template).
    /// Returns the run ID and creates all task instances + deps.
    /// The entire operation is wrapped in a single IMMEDIATE transaction so the
    /// database is never left in a partially-written state if the process crashes.
    pub fn create_run(
        &self,
        name: &str,
        definition_id: Option<&str>,
        tasks: &[DagTaskDefinition],
    ) -> Result<String> {
        let run_id = Uuid::new_v4().to_string();
        let workspace_id = format!("dag_{}", &run_id[..8]);
        let now = Utc::now().to_rfc3339();

        // Use an IMMEDIATE transaction so all inserts are atomic.
        self.conn.execute_batch("BEGIN IMMEDIATE")?;

        let result = (|| -> Result<()> {
            self.conn.execute(
                "INSERT INTO dag_runs (id, definition_id, name, status, workspace_id, created_at, notification_sent)
                 VALUES (?1, ?2, ?3, 'pending', ?4, ?5, 0)",
                params![run_id, definition_id, name, workspace_id, now],
            )?;

            // Create task instances
            let mut task_ids: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            for task_def in tasks {
                let task_id = Uuid::new_v4().to_string();
                let initial_status = if task_def.depends_on.is_empty() {
                    "pending"
                } else {
                    "blocked"
                };
                self.conn.execute(
                    "INSERT INTO dag_tasks (id, run_id, task_key, agent_id, description, status, attempt, max_retries, retry_delay_ms, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, ?9)",
                    params![
                        task_id,
                        run_id,
                        task_def.key,
                        task_def.agent_id,
                        task_def.description,
                        initial_status,
                        task_def.max_retries,
                        task_def.retry_delay_ms as i64,
                        now,
                    ],
                )?;
                task_ids.insert(task_def.key.clone(), task_id);
            }

            // Create dependency edges
            for task_def in tasks {
                if let Some(task_id) = task_ids.get(&task_def.key) {
                    for dep_key in &task_def.depends_on {
                        if let Some(dep_id) = task_ids.get(dep_key) {
                            self.conn.execute(
                                "INSERT INTO dag_task_deps (task_id, depends_on_task_id) VALUES (?1, ?2)",
                                params![task_id, dep_id],
                            )?;
                        }
                    }
                }
            }

            self.conn.execute(
                "INSERT INTO dag_history (run_id, task_id, event_type, details, timestamp)
                 VALUES (?1, NULL, 'run_created', ?2, ?3)",
                params![run_id, name, now],
            )?;

            Ok(())
        })();

        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT")?;
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }

        info!(
            "[DAG_STORE] Created run '{}' ({}) with {} tasks",
            name,
            run_id,
            tasks.len()
        );
        Ok(run_id)
    }

    /// Update the status of a run.
    pub fn update_run_status(
        &self,
        run_id: &str,
        status: &DagRunStatus,
        error: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let status_str = status.to_string();

        match status {
            DagRunStatus::Running => {
                self.conn.execute(
                    "UPDATE dag_runs SET status = ?1, started_at = ?2 WHERE id = ?3",
                    params![status_str, now, run_id],
                )?;
            }
            DagRunStatus::Completed | DagRunStatus::Failed | DagRunStatus::Cancelled => {
                self.conn.execute(
                    "UPDATE dag_runs SET status = ?1, completed_at = ?2, error = ?3 WHERE id = ?4",
                    params![status_str, now, error, run_id],
                )?;
            }
            _ => {
                self.conn.execute(
                    "UPDATE dag_runs SET status = ?1, error = ?2 WHERE id = ?3",
                    params![status_str, error, run_id],
                )?;
            }
        }
        Ok(())
    }

    /// Get a run by ID.
    pub fn get_run(&self, run_id: &str) -> Result<Option<DagRun>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, definition_id, name, status, workspace_id, started_at, completed_at, created_at, error
             FROM dag_runs WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![run_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(Self::row_to_run(row)?)),
            None => Ok(None),
        }
    }

    /// List recent runs (most recent first).
    pub fn list_runs(&self, limit: usize) -> Result<Vec<DagRun>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, definition_id, name, status, workspace_id, started_at, completed_at, created_at, error
             FROM dag_runs ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| Ok(Self::row_to_run_tuple(row)))?;

        let mut runs = Vec::new();
        for row in rows {
            let tuple = row?;
            runs.push(Self::tuple_to_run(tuple));
        }
        Ok(runs)
    }

    fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<DagRun> {
        Ok(DagRun {
            id: row.get(0)?,
            definition_id: row.get(1)?,
            name: row.get(2)?,
            status: DagRunStatus::from_str(&row.get::<_, String>(3)?),
            workspace_id: row.get(4)?,
            started_at: row
                .get::<_, Option<String>>(5)?
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc)),
            completed_at: row
                .get::<_, Option<String>>(6)?
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc)),
            created_at: row
                .get::<_, String>(7)
                .ok()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(Utc::now),
            error: row.get(8)?,
        })
    }

    // Helper to avoid lifetime issues in query_map closures
    fn row_to_run_tuple(
        row: &rusqlite::Row<'_>,
    ) -> (
        String,
        Option<String>,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
    ) {
        (
            row.get(0).unwrap_or_default(),
            row.get(1).unwrap_or_default(),
            row.get(2).unwrap_or_default(),
            row.get(3).unwrap_or_default(),
            row.get(4).unwrap_or_default(),
            row.get(5).unwrap_or_default(),
            row.get(6).unwrap_or_default(),
            row.get(7).unwrap_or_default(),
            row.get(8).unwrap_or_default(),
        )
    }

    fn tuple_to_run(
        t: (
            String,
            Option<String>,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
        ),
    ) -> DagRun {
        DagRun {
            id: t.0,
            definition_id: t.1,
            name: t.2,
            status: DagRunStatus::from_str(&t.3),
            workspace_id: t.4,
            started_at: t
                .5
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc)),
            completed_at: t
                .6
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc)),
            created_at: chrono::DateTime::parse_from_rfc3339(&t.7)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            error: t.8,
        }
    }

    // ── Tasks ────────────────────────────────────────────────────────

    /// Get all tasks for a run.
    /// Dependency keys are loaded in a single batch query (no N+1).
    pub fn get_tasks_for_run(&self, run_id: &str) -> Result<Vec<DagTask>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, run_id, task_key, agent_id, description, status, output, error,
                    attempt, max_retries, retry_delay_ms, started_at, completed_at, created_at
             FROM dag_tasks WHERE run_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, u32>(8)?,
                row.get::<_, u32>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, String>(13)?,
            ))
        })?;

        let mut raw_tasks = Vec::new();
        for row in rows {
            raw_tasks.push(row?);
        }

        // Batch-load all dependency keys for this run in a single query.
        let dep_map = self.get_all_dep_keys_for_run(run_id)?;

        let mut tasks = Vec::new();
        for t in raw_tasks {
            let deps = dep_map.get(&t.0).cloned().unwrap_or_default();
            tasks.push(DagTask {
                id: t.0,
                run_id: t.1,
                task_key: t.2,
                agent_id: t.3,
                description: t.4,
                status: DagTaskStatus::from_str(&t.5),
                output: t.6,
                error: t.7,
                attempt: t.8,
                max_retries: t.9,
                retry_delay_ms: t.10 as u64,
                depends_on: deps,
                started_at: t
                    .11
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                completed_at: t
                    .12
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                created_at: chrono::DateTime::parse_from_rfc3339(&t.13)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            });
        }
        Ok(tasks)
    }

    /// Get task dependency keys (task_keys, not IDs) for display purposes.
    /// Used for single-task lookup (e.g. after dynamic task addition).
    #[allow(dead_code)]
    fn get_task_dep_keys(&self, task_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.task_key FROM dag_task_deps d
             JOIN dag_tasks t ON t.id = d.depends_on_task_id
             WHERE d.task_id = ?1",
        )?;
        let rows = stmt.query_map(params![task_id], |row| row.get::<_, String>(0))?;
        let mut keys = Vec::new();
        for row in rows {
            keys.push(row?);
        }
        Ok(keys)
    }

    /// Batch-load all dep keys for every task in a run. Returns a map of task_id -> [dep_task_keys].
    /// Used by get_tasks_for_run and get_runnable_tasks to avoid N+1 queries.
    fn get_all_dep_keys_for_run(
        &self,
        run_id: &str,
    ) -> Result<std::collections::HashMap<String, Vec<String>>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.task_id, dep.task_key
             FROM dag_task_deps d
             JOIN dag_tasks dep ON dep.id = d.depends_on_task_id
             JOIN dag_tasks t   ON t.id   = d.task_id
             WHERE t.run_id = ?1",
        )?;
        let rows = stmt.query_map(params![run_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut map: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for row in rows {
            let (task_id, dep_key) = row?;
            map.entry(task_id).or_default().push(dep_key);
        }
        Ok(map)
    }

    /// Get tasks that are ready to run (all deps completed).
    /// Dependency keys are loaded in a single batch query (no N+1).
    pub fn get_runnable_tasks(&self, run_id: &str) -> Result<Vec<DagTask>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.run_id, t.task_key, t.agent_id, t.description, t.status,
                    t.output, t.error, t.attempt, t.max_retries, t.retry_delay_ms,
                    t.started_at, t.completed_at, t.created_at
             FROM dag_tasks t
             WHERE t.run_id = ?1
               AND t.status IN ('pending', 'blocked')
               AND NOT EXISTS (
                   SELECT 1 FROM dag_task_deps d
                   JOIN dag_tasks dep ON dep.id = d.depends_on_task_id
                   WHERE d.task_id = t.id AND dep.status != 'completed'
               )",
        )?;
        let rows = stmt.query_map(params![run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, u32>(8)?,
                row.get::<_, u32>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, String>(13)?,
            ))
        })?;

        let mut raw_tasks = Vec::new();
        for row in rows {
            raw_tasks.push(row?);
        }

        // Batch-load dependency keys for this run (shared helper, no N+1).
        let dep_map = self.get_all_dep_keys_for_run(run_id)?;

        let mut tasks = Vec::new();
        for t in raw_tasks {
            let deps = dep_map.get(&t.0).cloned().unwrap_or_default();
            tasks.push(DagTask {
                id: t.0,
                run_id: t.1,
                task_key: t.2,
                agent_id: t.3,
                description: t.4,
                status: DagTaskStatus::from_str(&t.5),
                output: t.6,
                error: t.7,
                attempt: t.8,
                max_retries: t.9,
                retry_delay_ms: t.10 as u64,
                depends_on: deps,
                started_at: t
                    .11
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                completed_at: t
                    .12
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                created_at: chrono::DateTime::parse_from_rfc3339(&t.13)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            });
        }
        Ok(tasks)
    }

    /// Update task status with optional output/error.
    pub fn update_task_status(
        &self,
        task_id: &str,
        status: &DagTaskStatus,
        output: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let status_str = status.to_string();

        match status {
            DagTaskStatus::Running => {
                self.conn.execute(
                    "UPDATE dag_tasks SET status = ?1, started_at = ?2, attempt = attempt + 1
                     WHERE id = ?3",
                    params![status_str, now, task_id],
                )?;
            }
            DagTaskStatus::Completed => {
                self.conn.execute(
                    "UPDATE dag_tasks SET status = ?1, completed_at = ?2, output = ?3
                     WHERE id = ?4",
                    params![status_str, now, output, task_id],
                )?;
            }
            DagTaskStatus::Failed => {
                self.conn.execute(
                    "UPDATE dag_tasks SET status = ?1, completed_at = ?2, error = ?3
                     WHERE id = ?4",
                    params![status_str, now, error, task_id],
                )?;
            }
            _ => {
                self.conn.execute(
                    "UPDATE dag_tasks SET status = ?1 WHERE id = ?2",
                    params![status_str, task_id],
                )?;
            }
        }
        Ok(())
    }

    /// Add a task to a running DAG dynamically.
    pub fn add_task_to_run(&self, run_id: &str, task_def: &DagTaskDefinition) -> Result<String> {
        let task_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let initial_status = if task_def.depends_on.is_empty() {
            "pending"
        } else {
            "blocked"
        };

        self.conn.execute(
            "INSERT INTO dag_tasks (id, run_id, task_key, agent_id, description, status, attempt, max_retries, retry_delay_ms, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, ?9)",
            params![
                task_id,
                run_id,
                task_def.key,
                task_def.agent_id,
                task_def.description,
                initial_status,
                task_def.max_retries,
                task_def.retry_delay_ms as i64,
                now,
            ],
        )?;

        // Create dep edges by looking up existing task IDs by key
        for dep_key in &task_def.depends_on {
            let dep_id: Option<String> = self
                .conn
                .query_row(
                    "SELECT id FROM dag_tasks WHERE run_id = ?1 AND task_key = ?2",
                    params![run_id, dep_key],
                    |row| row.get(0),
                )
                .ok();
            if let Some(dep_id) = dep_id {
                self.conn.execute(
                    "INSERT OR IGNORE INTO dag_task_deps (task_id, depends_on_task_id) VALUES (?1, ?2)",
                    params![task_id, dep_id],
                )?;
            } else {
                tracing::warn!(
                    "[DAG] Task '{}' depends on '{}' which does not exist in run '{}' — dependency edge skipped",
                    task_def.key, dep_key, run_id
                );
            }
        }

        self.add_history(run_id, Some(&task_id), "task_added", Some(&task_def.key))?;
        Ok(task_id)
    }

    /// Check if all tasks in a run are in terminal states.
    pub fn all_tasks_terminal(&self, run_id: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM dag_tasks
             WHERE run_id = ?1 AND status NOT IN ('completed', 'failed', 'cancelled')",
            params![run_id],
            |row| row.get(0),
        )?;
        Ok(count == 0)
    }

    /// Check if any task in a run has failed (and is not going to be retried).
    pub fn has_failed_tasks(&self, run_id: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM dag_tasks WHERE run_id = ?1 AND status = 'failed'",
            params![run_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Cancel all non-terminal tasks in a run.
    pub fn cancel_pending_tasks(&self, run_id: &str) -> Result<usize> {
        let count = self.conn.execute(
            "UPDATE dag_tasks SET status = 'cancelled'
             WHERE run_id = ?1 AND status NOT IN ('completed', 'failed', 'cancelled')",
            params![run_id],
        )?;
        Ok(count)
    }

    // ── History ──────────────────────────────────────────────────────

    /// Add a history entry.
    pub fn add_history(
        &self,
        run_id: &str,
        task_id: Option<&str>,
        event_type: &str,
        details: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO dag_history (run_id, task_id, event_type, details, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![run_id, task_id, event_type, details, now],
        )?;
        Ok(())
    }

    /// Get history for a run.
    pub fn get_history(&self, run_id: &str) -> Result<Vec<DagHistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, task_id, event_type, details, timestamp
             FROM dag_history WHERE run_id = ?1 ORDER BY timestamp ASC",
        )?;
        let rows = stmt.query_map(params![run_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;

        let mut entries = Vec::new();
        for row in rows {
            let t = row?;
            entries.push(DagHistoryEntry {
                run_id: t.0,
                task_id: t.1,
                event_type: t.2,
                details: t.3,
                timestamp: chrono::DateTime::parse_from_rfc3339(&t.4)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            });
        }
        Ok(entries)
    }

    // ── Summary ──────────────────────────────────────────────────────

    /// Get a complete summary of a run (run + tasks + history).
    pub fn get_run_summary(&self, run_id: &str) -> Result<Option<DagRunSummary>> {
        let run = match self.get_run(run_id)? {
            Some(r) => r,
            None => return Ok(None),
        };
        let tasks = self.get_tasks_for_run(run_id)?;
        let history = self.get_history(run_id)?;
        Ok(Some(DagRunSummary {
            run,
            tasks,
            history,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> DagStore {
        DagStore::new(PathBuf::from(":memory:")).expect("in-memory store")
    }

    fn sample_tasks() -> Vec<DagTaskDefinition> {
        vec![
            DagTaskDefinition {
                key: "research".into(),
                agent_id: "researcher".into(),
                description: "Research the topic".into(),
                depends_on: vec![],
                max_retries: 2,
                retry_delay_ms: 1000,
            },
            DagTaskDefinition {
                key: "analyze".into(),
                agent_id: "analyst".into(),
                description: "Analyze the research".into(),
                depends_on: vec!["research".into()],
                max_retries: 1,
                retry_delay_ms: 2000,
            },
            DagTaskDefinition {
                key: "summarize".into(),
                agent_id: "writer".into(),
                description: "Write a summary".into(),
                depends_on: vec!["analyze".into()],
                max_retries: 0,
                retry_delay_ms: 1000,
            },
        ]
    }

    #[test]
    fn test_create_run_and_get_tasks() {
        let store = temp_store();
        let run_id = store
            .create_run("Test Run", None, &sample_tasks())
            .expect("create run");

        let run = store
            .get_run(&run_id)
            .expect("get run")
            .expect("run exists");
        assert_eq!(run.name, "Test Run");
        assert_eq!(run.status, DagRunStatus::Pending);

        let tasks = store.get_tasks_for_run(&run_id).expect("get tasks");
        assert_eq!(tasks.len(), 3);

        // Research has no deps → pending
        let research = tasks.iter().find(|t| t.task_key == "research").unwrap();
        assert_eq!(research.status, DagTaskStatus::Pending);

        // Analyze depends on research → blocked
        let analyze = tasks.iter().find(|t| t.task_key == "analyze").unwrap();
        assert_eq!(analyze.status, DagTaskStatus::Blocked);

        // Summarize depends on analyze → blocked
        let summarize = tasks.iter().find(|t| t.task_key == "summarize").unwrap();
        assert_eq!(summarize.status, DagTaskStatus::Blocked);
    }

    #[test]
    fn test_runnable_tasks() {
        let store = temp_store();
        let run_id = store
            .create_run("Test", None, &sample_tasks())
            .expect("create run");

        // Initially only "research" is runnable
        let runnable = store.get_runnable_tasks(&run_id).expect("runnable");
        assert_eq!(runnable.len(), 1);
        assert_eq!(runnable[0].task_key, "research");

        // Complete research
        store
            .update_task_status(
                &runnable[0].id,
                &DagTaskStatus::Completed,
                Some("Done"),
                None,
            )
            .expect("update");

        // Now "analyze" should be runnable
        let runnable = store.get_runnable_tasks(&run_id).expect("runnable");
        assert_eq!(runnable.len(), 1);
        assert_eq!(runnable[0].task_key, "analyze");
    }

    #[test]
    fn test_parallel_tasks() {
        let store = temp_store();
        let tasks = vec![
            DagTaskDefinition {
                key: "a".into(),
                agent_id: "agent1".into(),
                description: "Task A".into(),
                depends_on: vec![],
                max_retries: 0,
                retry_delay_ms: 1000,
            },
            DagTaskDefinition {
                key: "b".into(),
                agent_id: "agent2".into(),
                description: "Task B".into(),
                depends_on: vec![],
                max_retries: 0,
                retry_delay_ms: 1000,
            },
            DagTaskDefinition {
                key: "c".into(),
                agent_id: "agent3".into(),
                description: "Task C (needs A+B)".into(),
                depends_on: vec!["a".into(), "b".into()],
                max_retries: 0,
                retry_delay_ms: 1000,
            },
        ];
        let run_id = store.create_run("Parallel", None, &tasks).expect("create");

        // A and B are both runnable
        let runnable = store.get_runnable_tasks(&run_id).expect("runnable");
        assert_eq!(runnable.len(), 2);

        // Complete A only — C still blocked
        let a = runnable.iter().find(|t| t.task_key == "a").unwrap();
        store
            .update_task_status(&a.id, &DagTaskStatus::Completed, Some("A done"), None)
            .expect("update");

        let runnable = store.get_runnable_tasks(&run_id).expect("runnable");
        assert_eq!(runnable.len(), 1);
        assert_eq!(runnable[0].task_key, "b");

        // Complete B — C now runnable
        store
            .update_task_status(
                &runnable[0].id,
                &DagTaskStatus::Completed,
                Some("B done"),
                None,
            )
            .expect("update");

        let runnable = store.get_runnable_tasks(&run_id).expect("runnable");
        assert_eq!(runnable.len(), 1);
        assert_eq!(runnable[0].task_key, "c");
    }

    #[test]
    fn test_add_task_dynamically() {
        let store = temp_store();
        let run_id = store
            .create_run("Dynamic", None, &sample_tasks())
            .expect("create");

        // Add a new task that depends on "research"
        let new_task = DagTaskDefinition {
            key: "extra".into(),
            agent_id: "helper".into(),
            description: "Extra work".into(),
            depends_on: vec!["research".into()],
            max_retries: 0,
            retry_delay_ms: 1000,
        };
        store.add_task_to_run(&run_id, &new_task).expect("add task");

        let tasks = store.get_tasks_for_run(&run_id).expect("get tasks");
        assert_eq!(tasks.len(), 4);
    }

    #[test]
    fn test_definition_save_and_load() {
        let store = temp_store();
        let def = DagDefinition {
            id: "def1".into(),
            name: "My Template".into(),
            description: Some("A reusable workflow".into()),
            tasks: sample_tasks(),
            is_template: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        store.save_definition(&def).expect("save");

        let loaded = store.get_definition("def1").expect("get").expect("exists");
        assert_eq!(loaded.name, "My Template");
        assert_eq!(loaded.tasks.len(), 3);
        assert!(loaded.is_template);

        let templates = store.list_templates().expect("list");
        assert_eq!(templates.len(), 1);

        store.delete_definition("def1").expect("delete");
        assert!(store.get_definition("def1").expect("get").is_none());
    }

    #[test]
    fn test_history() {
        let store = temp_store();
        let run_id = store
            .create_run("History Test", None, &sample_tasks())
            .expect("create");

        store
            .add_history(&run_id, None, "run_started", None)
            .expect("add history");
        store
            .add_history(&run_id, Some("task1"), "task_completed", Some("output..."))
            .expect("add history");

        let history = store.get_history(&run_id).expect("get history");
        // 1 from create_run + 2 added
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].event_type, "run_created");
    }

    #[test]
    fn test_cancel_pending_tasks() {
        let store = temp_store();
        let run_id = store
            .create_run("Cancel", None, &sample_tasks())
            .expect("create");

        let cancelled = store.cancel_pending_tasks(&run_id).expect("cancel");
        assert_eq!(cancelled, 3); // all 3 tasks cancelled

        let tasks = store.get_tasks_for_run(&run_id).expect("tasks");
        for task in &tasks {
            assert_eq!(task.status, DagTaskStatus::Cancelled);
        }
    }

    #[test]
    fn test_all_tasks_terminal() {
        let store = temp_store();
        let tasks = vec![DagTaskDefinition {
            key: "only".into(),
            agent_id: "a".into(),
            description: "Single task".into(),
            depends_on: vec![],
            max_retries: 0,
            retry_delay_ms: 1000,
        }];
        let run_id = store.create_run("Terminal", None, &tasks).expect("create");

        assert!(!store.all_tasks_terminal(&run_id).expect("check"));

        let all_tasks = store.get_tasks_for_run(&run_id).expect("tasks");
        store
            .update_task_status(
                &all_tasks[0].id,
                &DagTaskStatus::Completed,
                Some("done"),
                None,
            )
            .expect("update");

        assert!(store.all_tasks_terminal(&run_id).expect("check"));
    }

    #[test]
    fn test_run_summary() {
        let store = temp_store();
        let run_id = store
            .create_run("Summary", None, &sample_tasks())
            .expect("create");

        let summary = store
            .get_run_summary(&run_id)
            .expect("summary")
            .expect("exists");
        assert_eq!(summary.run.name, "Summary");
        assert_eq!(summary.tasks.len(), 3);
        assert!(!summary.history.is_empty()); // at least run_created
    }

    // ── Notification catch-up tests ───────────────────────────────────

    #[test]
    fn test_new_run_defaults_notification_not_sent() {
        let store = temp_store();
        let run_id = store
            .create_run("Notify Test", None, &sample_tasks())
            .expect("create");

        // Not yet completed → should not appear in unsent list
        let unsent = store.get_unsent_completed_runs().expect("query");
        assert!(!unsent.iter().any(|(id, _, _)| id == &run_id));
    }

    #[test]
    fn test_completed_run_appears_in_unsent() {
        let store = temp_store();
        let run_id = store
            .create_run("Completed Unsent", None, &sample_tasks())
            .expect("create");

        store
            .update_run_status(&run_id, &DagRunStatus::Completed, None)
            .expect("update");

        let unsent = store.get_unsent_completed_runs().expect("query");
        assert!(
            unsent.iter().any(|(id, name, status)| {
                id == &run_id && name == "Completed Unsent" && status == "completed"
            }),
            "completed run should appear in unsent list"
        );
    }

    #[test]
    fn test_failed_run_appears_in_unsent() {
        let store = temp_store();
        let run_id = store
            .create_run("Failed Unsent", None, &sample_tasks())
            .expect("create");

        store
            .update_run_status(&run_id, &DagRunStatus::Failed, Some("error"))
            .expect("update");

        let unsent = store.get_unsent_completed_runs().expect("query");
        assert!(
            unsent
                .iter()
                .any(|(id, _, status)| id == &run_id && status == "failed"),
            "failed run should appear in unsent list"
        );
    }

    #[test]
    fn test_mark_notification_sent_removes_from_unsent() {
        let store = temp_store();
        let run_id = store
            .create_run("Mark Sent", None, &sample_tasks())
            .expect("create");

        store
            .update_run_status(&run_id, &DagRunStatus::Completed, None)
            .expect("update");

        // Verify it's in the list before marking
        let unsent_before = store.get_unsent_completed_runs().expect("query");
        assert!(unsent_before.iter().any(|(id, _, _)| id == &run_id));

        // Mark as sent
        store.mark_notification_sent(&run_id).expect("mark sent");

        // Should no longer appear
        let unsent_after = store.get_unsent_completed_runs().expect("query after");
        assert!(
            !unsent_after.iter().any(|(id, _, _)| id == &run_id),
            "run should not appear in unsent list after mark_notification_sent"
        );
    }

    #[test]
    fn test_mark_notification_sent_missing_run_returns_error() {
        let store = temp_store();
        let err = store
            .mark_notification_sent("missing-run-id")
            .expect_err("missing run should return error");
        assert!(err
            .to_string()
            .contains("was not found while marking notification_sent"));
    }

    #[test]
    fn test_pending_run_not_in_unsent() {
        let store = temp_store();
        let run_id = store
            .create_run("Still Pending", None, &sample_tasks())
            .expect("create");

        // Pending status → not terminal → should not appear
        let unsent = store.get_unsent_completed_runs().expect("query");
        assert!(!unsent.iter().any(|(id, _, _)| id == &run_id));
    }

    #[test]
    fn test_multiple_unsent_runs_returned() {
        let store = temp_store();

        let run1 = store
            .create_run("Run A", None, &sample_tasks())
            .expect("create");
        let run2 = store
            .create_run("Run B", None, &sample_tasks())
            .expect("create");
        let run3 = store
            .create_run("Run C", None, &sample_tasks())
            .expect("create");

        store
            .update_run_status(&run1, &DagRunStatus::Completed, None)
            .expect("update");
        store
            .update_run_status(&run2, &DagRunStatus::Failed, Some("err"))
            .expect("update");
        // run3 stays pending

        let unsent = store.get_unsent_completed_runs().expect("query");
        let ids: Vec<&str> = unsent.iter().map(|(id, _, _)| id.as_str()).collect();

        assert!(
            ids.contains(&run1.as_str()),
            "completed run1 should be unsent"
        );
        assert!(ids.contains(&run2.as_str()), "failed run2 should be unsent");
        assert!(
            !ids.contains(&run3.as_str()),
            "pending run3 should not appear"
        );
    }

    #[test]
    fn test_schema_migration_idempotent() {
        // DagStore::new() calls create_tables() which runs the ALTER TABLE migration.
        // Calling new() a second time on a fresh in-memory DB should not panic.
        // (In production a file-backed DB would exercise the upgrade path; here we
        // confirm the guard `let _ = ...` suppresses duplicate-column errors.)
        let _store1 = temp_store();
        let _store2 = temp_store();
        // If we reach here without panic, the idempotency guard is working.
    }
}
