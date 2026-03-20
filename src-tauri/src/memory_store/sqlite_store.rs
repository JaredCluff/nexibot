//! SQLite-backed memory store with FTS5 full-text search.
//!
//! Uses `rusqlite` with WAL mode for concurrent reads and an FTS5 virtual table
//! for O(log n) ranked text search (vs the previous O(n*m*k) linear scan).
//!
//! If an existing `.json` memory file is found at `db_path`, records are migrated
//! into the SQLite database and the JSON file is renamed to `.json.bak`.
#![allow(dead_code)]

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{info, warn};

/// A single memory record stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecord {
    /// Unique identifier (UUID v4).
    pub id: String,
    /// The textual content of this memory.
    pub content: String,
    /// Category string (e.g. "conversation", "preference", "fact", "context").
    pub memory_type: String,
    /// Free-form tags for categorization.
    pub tags: Vec<String>,
    /// When this record was first created.
    pub created_at: DateTime<Utc>,
    /// When this record was last read / accessed.
    pub last_accessed: DateTime<Utc>,
    /// How many times this record has been accessed.
    pub access_count: u32,
    /// Arbitrary key-value metadata.
    pub metadata: HashMap<String, String>,
    /// Contextualized content for improved embedding quality.
    /// Format: "This is a [type] related to [tags]: [content]"
    #[serde(default)]
    pub contextualized_content: Option<String>,
    /// Embedding vector serialized as little-endian f32 bytes.
    #[serde(default)]
    pub embedding: Option<Vec<f32>>,
}

/// Persistent memory store backed by SQLite with FTS5 full-text search.
pub struct SqliteMemoryStore {
    /// SQLite connection (WAL mode for concurrent reads).
    conn: Connection,
}

impl SqliteMemoryStore {
    /// Open or create a memory store at the given path.
    ///
    /// If an existing `.json` file is found at the same path, records are migrated
    /// into the new SQLite database and the JSON file is renamed to `.json.bak`.
    pub fn new(db_path: PathBuf) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create parent directory for {}",
                    db_path.display()
                )
            })?;
        }

        // Determine the actual SQLite path (swap .json extension to .db if needed)
        let sqlite_path = if db_path.extension().and_then(|e| e.to_str()) == Some("json") {
            db_path.with_extension("db")
        } else {
            db_path.clone()
        };

        let conn = Connection::open(&sqlite_path).with_context(|| {
            format!(
                "Failed to open SQLite database at {}",
                sqlite_path.display()
            )
        })?;

        // Set restrictive file permissions (owner-only read/write) on Unix.
        // This prevents other users on the system from reading memory data.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(
                &sqlite_path,
                std::fs::Permissions::from_mode(0o600),
            ) {
                warn!(
                    "[MEMORY_STORE] Failed to set 0600 permissions on {}: {}",
                    sqlite_path.display(),
                    e
                );
            }
        }

        // Enable WAL mode for concurrent reads
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // Reasonable busy timeout for concurrent access
        conn.pragma_update(None, "busy_timeout", 5000)?;

        // Create tables
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                memory_type TEXT NOT NULL,
                tags TEXT,
                created_at TEXT NOT NULL,
                last_accessed TEXT NOT NULL,
                access_count INTEGER DEFAULT 0,
                metadata TEXT,
                contextualized_content TEXT,
                embedding BLOB
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                content,
                tags,
                content='memories',
                content_rowid='rowid'
            );

            -- Triggers to keep FTS index in sync
            CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, content, tags)
                VALUES (new.rowid, new.content, new.tags);
            END;

            CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, content, tags)
                VALUES ('delete', old.rowid, old.content, old.tags);
            END;

            CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, content, tags)
                VALUES ('delete', old.rowid, old.content, old.tags);
                INSERT INTO memories_fts(rowid, content, tags)
                VALUES (new.rowid, new.content, new.tags);
            END;",
        )?;

        // Migration: add contextualized_content column if missing (from older schema).
        // Uses BEGIN IMMEDIATE to acquire the write lock up front, preventing two concurrent
        // processes from both detecting the missing column and both attempting ALTER TABLE.
        conn.execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE memories ADD COLUMN IF NOT EXISTS contextualized_content TEXT;
             ALTER TABLE memories ADD COLUMN IF NOT EXISTS embedding BLOB;
             COMMIT;",
        )
        .unwrap_or_else(|e| {
            // ALTER TABLE fails if the column already exists (pre-3.37 SQLite lacks IF NOT
            // EXISTS for ADD COLUMN). Roll back cleanly and continue — schema is already current.
            let _ = conn.execute_batch("ROLLBACK;");
            // Only log if the error is not "duplicate column name" (expected on already-migrated DBs)
            let msg = e.to_string();
            if !msg.contains("duplicate column") {
                warn!("[MEMORY_STORE] Schema migration note: {}", msg);
            }
        });
        {
            let has_ctx_col: bool = conn
                .prepare("SELECT COUNT(*) FROM pragma_table_info('memories') WHERE name='contextualized_content'")?
                .query_row([], |row| row.get::<_, i64>(0))
                .unwrap_or(0) > 0;
            if has_ctx_col {
                info!("[MEMORY_STORE] Schema migration complete: contextualized_content and embedding columns present");
            }
        }

        let store = Self { conn };

        // Migrate from JSON if the old file exists
        let json_path = if db_path.extension().and_then(|e| e.to_str()) == Some("json") {
            db_path.clone()
        } else {
            db_path.with_extension("json")
        };

        if json_path.exists() {
            match store.migrate_from_json(&json_path) {
                Ok(count) => {
                    // Rename old JSON file to .bak
                    let bak_path = json_path.with_extension("json.bak");
                    if let Err(e) = std::fs::rename(&json_path, &bak_path) {
                        warn!("[MEMORY_STORE] Failed to rename JSON backup: {}", e);
                    } else {
                        info!(
                            "[MEMORY_STORE] Migrated {} records from JSON, backup at {}",
                            count,
                            bak_path.display()
                        );
                    }
                }
                Err(e) => {
                    warn!("[MEMORY_STORE] JSON migration failed (non-fatal): {}", e);
                }
            }
        }

        let count = store.len()?;
        info!(
            "[MEMORY_STORE] Opened SQLite store at {} ({} records)",
            sqlite_path.display(),
            count
        );

        Ok(store)
    }

    /// Migrate records from an existing JSON file into the SQLite database.
    fn migrate_from_json(&self, json_path: &PathBuf) -> Result<usize> {
        let content = std::fs::read_to_string(json_path)?;
        if content.trim().is_empty() {
            return Ok(0);
        }

        let records: Vec<MemoryRecord> = serde_json::from_str(&content)?;
        let mut count = 0;

        for record in &records {
            // Skip if already exists (idempotent migration)
            let exists: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM memories WHERE id = ?1)",
                params![record.id],
                |row| row.get(0),
            )?;

            if !exists {
                self.insert_record(record)?;
                count += 1;
            }
        }

        Ok(count)
    }

    /// Internal helper to insert a record via SQL.
    fn insert_record(&self, record: &MemoryRecord) -> Result<()> {
        let tags_json = serde_json::to_string(&record.tags)?;
        let metadata_json = serde_json::to_string(&record.metadata)?;

        // Serialize embedding as little-endian f32 bytes if present
        let embedding_blob: Option<Vec<u8>> = record
            .embedding
            .as_ref()
            .map(|emb| emb.iter().flat_map(|f| f.to_le_bytes()).collect());

        self.conn.execute(
            "INSERT OR REPLACE INTO memories (id, content, memory_type, tags, created_at, last_accessed, access_count, metadata, contextualized_content, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                record.id,
                record.content,
                record.memory_type,
                tags_json,
                record.created_at.to_rfc3339(),
                record.last_accessed.to_rfc3339(),
                record.access_count,
                metadata_json,
                record.contextualized_content,
                embedding_blob,
            ],
        )?;
        Ok(())
    }

    /// Parse a `MemoryRecord` from a SQLite row.
    /// Expects columns: id, content, memory_type, tags, created_at, last_accessed,
    /// access_count, metadata, contextualized_content, embedding
    fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRecord> {
        let id: String = row.get(0)?;
        let content: String = row.get(1)?;
        let memory_type: String = row.get(2)?;
        let tags_json: String = row.get::<_, String>(3).unwrap_or_else(|_| "[]".to_string());
        let created_at_str: String = row.get(4)?;
        let last_accessed_str: String = row.get(5)?;
        let access_count: u32 = row.get(6)?;
        let metadata_json: String = row.get::<_, String>(7).unwrap_or_else(|_| "{}".to_string());
        let contextualized_content: Option<String> =
            row.get::<_, Option<String>>(8).unwrap_or(None);

        // Deserialize embedding from little-endian f32 BLOB
        let embedding: Option<Vec<f32>> = row
            .get::<_, Option<Vec<u8>>>(9)
            .unwrap_or(None)
            .and_then(|blob| {
                if blob.len() % 4 != 0 {
                    return None;
                }
                Some(
                    blob.chunks_exact(4)
                        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                        .collect(),
                )
            });

        let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
        let metadata: HashMap<String, String> =
            serde_json::from_str(&metadata_json).unwrap_or_default();

        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let last_accessed = DateTime::parse_from_rfc3339(&last_accessed_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        Ok(MemoryRecord {
            id,
            content,
            memory_type,
            tags,
            created_at,
            last_accessed,
            access_count,
            metadata,
            contextualized_content,
            embedding,
        })
    }

    /// Insert a new memory record.
    pub fn insert(&self, record: MemoryRecord) -> Result<()> {
        info!("[MEMORY_STORE] Inserting record: {}", record.id);
        self.insert_record(&record)
    }

    /// Full-text search across memory records using FTS5.
    ///
    /// Returns up to `limit` results sorted by FTS5 BM25 rank.
    pub fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<(MemoryRecord, f64)>> {
        let query_words: Vec<String> = query
            .split_whitespace()
            .filter(|w| w.len() > 1)
            .map(|w| {
                // Escape FTS5 special characters and quote each term
                let escaped = w.replace('"', "\"\"");
                format!("\"{}\"", escaped)
            })
            .collect();

        if query_words.is_empty() {
            return Ok(Vec::new());
        }

        // Join with OR for partial matching
        let fts_query = query_words.join(" OR ");

        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.content, m.memory_type, m.tags, m.created_at,
                    m.last_accessed, m.access_count, m.metadata,
                    m.contextualized_content, m.embedding,
                    rank
             FROM memories_fts
             JOIN memories m ON memories_fts.rowid = m.rowid
             WHERE memories_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;

        let results: Vec<(MemoryRecord, f64)> = stmt
            .query_map(params![fts_query, limit as i64], |row| {
                let record = Self::row_to_record(row)?;
                let rank: f64 = row.get(10)?;
                // FTS5 rank is negative (lower = better match). Convert to positive score.
                Ok((record, -rank))
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    }

    /// Look up a record by its unique ID.
    pub fn get_by_id(&self, id: &str) -> Option<MemoryRecord> {
        self.conn
            .query_row(
                "SELECT id, content, memory_type, tags, created_at, last_accessed, access_count, metadata, contextualized_content, embedding
                 FROM memories WHERE id = ?1",
                params![id],
                Self::row_to_record,
            )
            .ok()
    }

    /// Delete a record by ID. Returns `true` if a record was removed.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let affected = self
            .conn
            .execute("DELETE FROM memories WHERE id = ?1", params![id])?;

        if affected > 0 {
            info!("[MEMORY_STORE] Deleted record: {}", id);
        } else {
            warn!("[MEMORY_STORE] Record not found for deletion: {}", id);
        }

        Ok(affected > 0)
    }

    /// Update access tracking for a record: bump `access_count` and set
    /// `last_accessed` to now.
    pub fn update_access(&self, id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let affected = self.conn.execute(
            "UPDATE memories SET access_count = access_count + 1, last_accessed = ?1 WHERE id = ?2",
            params![now, id],
        )?;

        if affected == 0 {
            anyhow::bail!("Record not found: {}", id);
        }

        Ok(())
    }

    /// Return all records in the store.
    pub fn get_all(&self) -> Result<Vec<MemoryRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content, memory_type, tags, created_at, last_accessed, access_count, metadata, contextualized_content, embedding
             FROM memories",
        )?;

        let records: Vec<MemoryRecord> = stmt
            .query_map([], Self::row_to_record)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(records)
    }

    /// Store an embedding for a memory record.
    pub fn store_embedding(
        &self,
        id: &str,
        embedding: &[f32],
        contextualized_content: Option<&str>,
    ) -> Result<()> {
        let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
        self.conn.execute(
            "UPDATE memories SET embedding = ?1, contextualized_content = ?2 WHERE id = ?3",
            params![blob, contextualized_content, id],
        )?;
        Ok(())
    }

    /// Retrieve the embedding for a memory record.
    pub fn get_embedding(&self, id: &str) -> Option<Vec<f32>> {
        self.conn
            .query_row(
                "SELECT embedding FROM memories WHERE id = ?1",
                params![id],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .ok()
            .flatten()
            .and_then(|blob| {
                if blob.len() % 4 != 0 {
                    return None;
                }
                Some(
                    blob.chunks_exact(4)
                        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                        .collect(),
                )
            })
    }

    /// Brute-force vector search: loads all embeddings, computes cosine similarity,
    /// returns top results above the similarity threshold.
    ///
    /// For NexiBot's scale (max 50K memories), brute-force over BLOB-stored vectors
    /// is fast enough (~10ms for 50K 384-dim vectors). No ANN indexing needed.
    pub fn vector_search(
        &self,
        query_embedding: &[f32],
        limit: usize,
        min_similarity: f64,
    ) -> Result<Vec<(MemoryRecord, f64)>> {
        let all = self.get_all()?;
        let mut scored: Vec<(MemoryRecord, f64)> = all
            .into_iter()
            .filter_map(|record| {
                record.embedding.as_ref().map(|emb| {
                    let sim = super::hybrid_search::cosine_similarity(query_embedding, emb);
                    (record.clone(), sim)
                })
            })
            .filter(|(_, sim)| *sim > min_similarity)
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        Ok(scored)
    }

    /// Persist — no-op with WAL mode (writes are already durable).
    pub fn save(&self) -> Result<()> {
        Ok(())
    }

    /// Return the number of records in the store.
    pub fn len(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Return whether the store is empty.
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    /// Helper: create a `MemoryRecord` with sensible defaults.
    fn make_record(id: &str, content: &str, tags: &[&str]) -> MemoryRecord {
        let now = Utc::now();
        MemoryRecord {
            id: id.to_string(),
            content: content.to_string(),
            memory_type: "fact".to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            created_at: now,
            last_accessed: now,
            access_count: 0,
            metadata: HashMap::new(),
            contextualized_content: None,
            embedding: None,
        }
    }

    /// Helper: create a store in a temp directory.
    fn temp_store() -> (SqliteMemoryStore, TempDir) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.db");
        let store = SqliteMemoryStore::new(path).unwrap();
        (store, tmp)
    }

    #[test]
    fn test_new_empty_store() {
        let (store, _tmp) = temp_store();
        assert!(store.is_empty().unwrap());
        assert_eq!(store.len().unwrap(), 0);
    }

    #[test]
    fn test_insert_and_get_by_id() {
        let (store, _tmp) = temp_store();
        let record = make_record("r1", "Rust is a systems language", &["programming", "rust"]);
        store.insert(record).unwrap();

        assert_eq!(store.len().unwrap(), 1);

        let found = store.get_by_id("r1").unwrap();
        assert_eq!(found.content, "Rust is a systems language");
        assert_eq!(found.tags, vec!["programming", "rust"]);
    }

    #[test]
    fn test_search_fts_basic() {
        let (store, _tmp) = temp_store();
        store
            .insert(make_record(
                "r1",
                "Rust is a fast systems programming language",
                &["rust"],
            ))
            .unwrap();
        store
            .insert(make_record(
                "r2",
                "Python is an interpreted language",
                &["python"],
            ))
            .unwrap();
        store
            .insert(make_record(
                "r3",
                "The weather is sunny today",
                &["weather"],
            ))
            .unwrap();

        // Search for "programming language" should match r1 and r2
        let results = store.search_fts("programming language", 10).unwrap();
        assert!(!results.is_empty());

        // r1 should rank higher (matches both "programming" and "language")
        assert_eq!(results[0].0.id, "r1");

        // r2 matches "language" only
        let r2_result = results.iter().find(|(r, _)| r.id == "r2");
        assert!(r2_result.is_some());
    }

    #[test]
    fn test_search_fts_tag_matching() {
        let (store, _tmp) = temp_store();
        store
            .insert(make_record("r1", "A short note", &["important", "rust"]))
            .unwrap();

        // "rust" is in the tags column
        let results = store.search_fts("rust", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.id, "r1");
    }

    #[test]
    fn test_search_fts_empty_query() {
        let (store, _tmp) = temp_store();
        store
            .insert(make_record("r1", "Some content", &[]))
            .unwrap();

        let results = store.search_fts("", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_fts_limit() {
        let (store, _tmp) = temp_store();
        for i in 0..20 {
            store
                .insert(make_record(
                    &format!("r{}", i),
                    &format!("Record number {} about rust", i),
                    &["rust"],
                ))
                .unwrap();
        }

        let results = store.search_fts("rust", 5).unwrap();
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn test_delete() {
        let (store, _tmp) = temp_store();
        store
            .insert(make_record("r1", "First record", &[]))
            .unwrap();
        store
            .insert(make_record("r2", "Second record", &[]))
            .unwrap();

        assert_eq!(store.len().unwrap(), 2);

        let removed = store.delete("r1").unwrap();
        assert!(removed);
        assert_eq!(store.len().unwrap(), 1);
        assert!(store.get_by_id("r1").is_none());
        assert!(store.get_by_id("r2").is_some());

        // Deleting non-existent ID returns false
        let removed = store.delete("r999").unwrap();
        assert!(!removed);
    }

    #[test]
    fn test_update_access() {
        let (store, _tmp) = temp_store();
        store
            .insert(make_record("r1", "Some content", &[]))
            .unwrap();

        assert_eq!(store.get_by_id("r1").unwrap().access_count, 0);

        store.update_access("r1").unwrap();
        assert_eq!(store.get_by_id("r1").unwrap().access_count, 1);

        store.update_access("r1").unwrap();
        assert_eq!(store.get_by_id("r1").unwrap().access_count, 2);
    }

    #[test]
    fn test_persistence_across_reloads() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("persist.db");

        // Create store and insert
        {
            let store = SqliteMemoryStore::new(path.clone()).unwrap();
            store
                .insert(make_record("r1", "Persisted content", &["test"]))
                .unwrap();
        }

        // Reopen and verify
        {
            let store = SqliteMemoryStore::new(path).unwrap();
            assert_eq!(store.len().unwrap(), 1);
            let record = store.get_by_id("r1").unwrap();
            assert_eq!(record.content, "Persisted content");
        }
    }

    #[test]
    fn test_get_all() {
        let (store, _tmp) = temp_store();
        store.insert(make_record("r1", "First", &[])).unwrap();
        store.insert(make_record("r2", "Second", &[])).unwrap();

        let all = store.get_all().unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_json_migration() {
        let tmp = TempDir::new().unwrap();
        let json_path = tmp.path().join("memory.json");

        // Write a JSON file with records
        let records = vec![
            make_record("m1", "Migrated record one", &["old"]),
            make_record("m2", "Migrated record two", &["old"]),
        ];
        let json = serde_json::to_string_pretty(&records).unwrap();
        std::fs::write(&json_path, &json).unwrap();

        // Open store at the JSON path — should trigger migration
        let store = SqliteMemoryStore::new(json_path.clone()).unwrap();
        assert_eq!(store.len().unwrap(), 2);

        // The original JSON should be renamed to .bak
        assert!(!json_path.exists());
        assert!(json_path.with_extension("json.bak").exists());

        // Records should be searchable via FTS
        let results = store.search_fts("migrated", 10).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_save_is_noop() {
        let (store, _tmp) = temp_store();
        // save() should succeed (it's a no-op with WAL mode)
        store.save().unwrap();
    }

    #[test]
    fn test_embedding_roundtrip() {
        let (store, _tmp) = temp_store();
        let mut record = make_record("r1", "Test embedding", &["test"]);
        let embedding = vec![0.1f32, 0.2, 0.3, 0.4, 0.5];
        record.embedding = Some(embedding.clone());
        record.contextualized_content =
            Some("This is a important fact: Test embedding".to_string());
        store.insert(record).unwrap();

        // Retrieve and verify embedding
        let retrieved = store.get_embedding("r1").unwrap();
        assert_eq!(retrieved.len(), 5);
        for (a, b) in retrieved.iter().zip(embedding.iter()) {
            assert!((a - b).abs() < 1e-6, "Embedding mismatch: {} vs {}", a, b);
        }

        // Verify via get_by_id too
        let rec = store.get_by_id("r1").unwrap();
        assert!(rec.embedding.is_some());
        assert!(rec.contextualized_content.is_some());
        assert_eq!(
            rec.contextualized_content.unwrap(),
            "This is a important fact: Test embedding"
        );
    }

    #[test]
    fn test_store_embedding_update() {
        let (store, _tmp) = temp_store();
        store.insert(make_record("r1", "Content", &[])).unwrap();

        // Initially no embedding
        assert!(store.get_embedding("r1").is_none());

        // Store embedding after the fact
        let emb = vec![1.0f32, 2.0, 3.0];
        store
            .store_embedding("r1", &emb, Some("Contextualized content"))
            .unwrap();

        let retrieved = store.get_embedding("r1").unwrap();
        assert_eq!(retrieved.len(), 3);
        assert!((retrieved[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_vector_search() {
        let (store, _tmp) = temp_store();

        // Insert records with embeddings (simple 3-dim vectors for testing)
        let mut r1 = make_record("r1", "Rust programming", &["rust"]);
        r1.embedding = Some(vec![1.0, 0.0, 0.0]);
        store.insert(r1).unwrap();

        let mut r2 = make_record("r2", "Python scripting", &["python"]);
        r2.embedding = Some(vec![0.0, 1.0, 0.0]);
        store.insert(r2).unwrap();

        let mut r3 = make_record("r3", "Rust systems", &["rust"]);
        r3.embedding = Some(vec![0.9, 0.1, 0.0]); // Similar to r1
        store.insert(r3).unwrap();

        // Search with query similar to r1 and r3
        let query_emb = vec![1.0f32, 0.0, 0.0];
        let results = store.vector_search(&query_emb, 10, 0.3).unwrap();

        assert!(!results.is_empty());
        // r1 should be first (exact match), r3 second (similar)
        assert_eq!(results[0].0.id, "r1");
        assert_eq!(results[1].0.id, "r3");
        // r2 is orthogonal, cosine similarity = 0, should be excluded (threshold 0.3)
        assert!(results.iter().all(|(r, _)| r.id != "r2"));
    }

    #[test]
    #[ignore] // Benchmark — run with: cargo test benchmark_fts5 -- --ignored --test-threads=1
    fn benchmark_fts5_search() {
        use std::time::Instant;

        let (store, _tmp) = temp_store();

        // Insert 10K records
        let insert_start = Instant::now();
        for i in 0..10_000 {
            store
                .insert(make_record(
                    &format!("bench-{}", i),
                    &format!(
                        "Record {} about {} topic with various keywords like rust programming memory search",
                        i,
                        if i % 3 == 0 { "science" } else if i % 3 == 1 { "technology" } else { "history" }
                    ),
                    &[if i % 2 == 0 { "even" } else { "odd" }],
                ))
                .unwrap();
        }
        let insert_elapsed = insert_start.elapsed();
        eprintln!(
            "[BENCHMARK] Inserted 10K records in {:?} ({:.1}ms/record)",
            insert_elapsed,
            insert_elapsed.as_millis() as f64 / 10_000.0
        );

        // Run 100 searches
        let search_start = Instant::now();
        let search_count = 100;
        for i in 0..search_count {
            let query = match i % 4 {
                0 => "rust programming",
                1 => "science technology",
                2 => "memory search",
                _ => "various keywords",
            };
            let results = store.search_fts(query, 20).unwrap();
            assert!(!results.is_empty());
        }
        let search_elapsed = search_start.elapsed();
        let ms_per_search = search_elapsed.as_millis() as f64 / search_count as f64;

        eprintln!(
            "[BENCHMARK] {} searches in {:?} ({:.1}ms/search)",
            search_count, search_elapsed, ms_per_search
        );

        // Assert < 20ms per search in debug mode (< 5ms in release)
        assert!(
            ms_per_search < 20.0,
            "FTS5 search too slow: {:.1}ms/search (expected < 20ms in debug)",
            ms_per_search
        );
    }
}
