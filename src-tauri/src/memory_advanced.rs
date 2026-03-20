//! Advanced Memory Features
//!
//! Extends the core memory system with:
//! - Memory importance/priority scoring
//! - Memory relationships and linking
//! - Memory export/import functionality
//! - Memory analytics and insights
//! - Intelligent memory cleanup and deduplication (wired on every insert)
//! - Advanced search filtering
//! - Memory decay and TTL management
//! - SQLite persistence (survives restarts)
//! - Background TTL expiry task
#![allow(dead_code)]

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

/// Memory importance score (0-100)
/// Higher = more important and less likely to be evicted
#[derive(Debug, Clone, Serialize, Deserialize, Copy, PartialEq, PartialOrd)]
pub struct Importance(pub f32);

impl Importance {
    pub fn new(score: f32) -> Self {
        Self(score.clamp(0.0, 100.0))
    }

    pub fn critical() -> Self {
        Self(100.0)
    }

    pub fn high() -> Self {
        Self(75.0)
    }

    pub fn normal() -> Self {
        Self(50.0)
    }

    pub fn low() -> Self {
        Self(25.0)
    }

    pub fn auto_calculate(access_count: u32, age_days: i64) -> Self {
        let access_score = (access_count as f32).min(100.0);
        let age_penalty = (age_days as f32 * 0.5).min(50.0);
        Self(((access_score + 50.0) - age_penalty).clamp(0.0, 100.0))
    }
}

/// Memory relationship type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RelationType {
    /// This memory relates to another memory
    Related,
    /// This memory supersedes another
    Supersedes,
    /// This memory complements another
    Complements,
    /// This memory contradicts another
    Contradicts,
    /// This memory references another
    References,
}

/// Link to another memory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryLink {
    pub target_memory_id: String,
    pub relation_type: RelationType,
    pub created_at: DateTime<Utc>,
}

/// Extended memory entry with advanced features
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedMemoryEntry {
    /// Memory ID
    pub id: String,
    /// Content
    pub content: String,
    /// Importance score
    pub importance: Importance,
    /// Links to other memories
    pub links: Vec<MemoryLink>,
    /// Time-to-live (None = permanent)
    pub ttl: Option<Duration>,
    /// When created
    pub created_at: DateTime<Utc>,
    /// When it expires
    pub expires_at: Option<DateTime<Utc>>,
    /// Source of this memory (user, assistant, system)
    pub source: String,
    /// Confidence score (0-100)
    pub confidence: f32,
    /// Whether this memory has been verified
    pub verified: bool,
}

/// Memory analytics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryAnalytics {
    /// Total memories stored
    pub total_memories: usize,
    /// Memories by importance distribution
    pub importance_distribution: HashMap<String, usize>,
    /// Memory access frequency (memories accessed per day)
    pub access_frequency: f32,
    /// Average memory age (days)
    pub average_age_days: f32,
    /// Memory redundancy score (0-100)
    pub redundancy_score: f32,
    /// Prediction on memory that will be evicted next
    pub predicted_eviction_count: usize,
    /// Memory retention rate
    pub retention_rate: f32,
}

/// Memory export format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryExport {
    /// Export timestamp
    pub timestamp: DateTime<Utc>,
    /// Memories
    pub memories: Vec<AdvancedMemoryEntry>,
    /// Memory relationships
    pub relationships: Vec<(String, String, RelationType)>,
    /// Metadata
    pub metadata: HashMap<String, String>,
}

/// Advanced Memory Manager with SQLite persistence and background TTL cleanup.
pub struct AdvancedMemoryManager {
    /// Memories with extended features (in-memory cache)
    memories: Arc<RwLock<HashMap<String, AdvancedMemoryEntry>>>,
    /// Memory relationships
    relationships: Arc<RwLock<Vec<MemoryLink>>>,
    /// Deduplication hashes (hash -> memory_id) — checked on every insert
    dedup_hashes: Arc<RwLock<HashMap<String, String>>>,
    /// Analytics cache
    analytics: Arc<RwLock<Option<MemoryAnalytics>>>,
    /// SQLite connection for persistence
    db: Arc<tokio::sync::Mutex<Connection>>,
}

impl AdvancedMemoryManager {
    /// Create a new advanced memory manager backed by SQLite at `db_path`.
    /// Existing non-expired memories are loaded from the database on startup.
    pub fn new_with_db(db_path: PathBuf) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }

        let conn = Connection::open(&db_path).with_context(|| {
            format!(
                "Failed to open advanced memory DB at {}",
                db_path.display()
            )
        })?;

        // WAL mode for concurrent reads
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS advanced_memories (
                id          TEXT PRIMARY KEY,
                content     TEXT NOT NULL,
                importance  REAL NOT NULL DEFAULT 50.0,
                links_json  TEXT NOT NULL DEFAULT '[]',
                ttl_secs    INTEGER,
                created_at  TEXT NOT NULL,
                expires_at  TEXT,
                source      TEXT NOT NULL DEFAULT 'system',
                confidence  REAL NOT NULL DEFAULT 100.0,
                verified    INTEGER NOT NULL DEFAULT 0
            );",
        )?;

        // Load all non-expired memories
        let mut memories = HashMap::new();
        let mut dedup_hashes = HashMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT id, content, importance, links_json, ttl_secs,
                        created_at, expires_at, source, confidence, verified
                 FROM advanced_memories
                 WHERE expires_at IS NULL OR expires_at > ?1",
            )?;
            let now_str = Utc::now().to_rfc3339();
            let rows = stmt.query_map(params![now_str], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, f32>(8)?,
                    row.get::<_, bool>(9)?,
                ))
            })?;

            for row in rows {
                let (id, content, importance, links_json, ttl_secs, created_at_str,
                     expires_at_str, source, confidence, verified) = row?;

                let links: Vec<MemoryLink> =
                    serde_json::from_str(&links_json).unwrap_or_default();
                let created_at = created_at_str
                    .parse::<DateTime<Utc>>()
                    .unwrap_or_else(|_| Utc::now());
                let expires_at = expires_at_str.and_then(|s| s.parse::<DateTime<Utc>>().ok());
                let ttl = ttl_secs.map(Duration::seconds);

                let hash = content_hash(&content);
                dedup_hashes.insert(hash, id.clone());

                memories.insert(
                    id.clone(),
                    AdvancedMemoryEntry {
                        id,
                        content,
                        importance: Importance(importance),
                        links,
                        ttl,
                        created_at,
                        expires_at,
                        source,
                        confidence,
                        verified,
                    },
                );
            }
        }

        info!(
            "[MEMORY_ADVANCED] Loaded {} memories from {}",
            memories.len(),
            db_path.display()
        );

        Ok(Self {
            memories: Arc::new(RwLock::new(memories)),
            relationships: Arc::new(RwLock::new(Vec::new())),
            dedup_hashes: Arc::new(RwLock::new(dedup_hashes)),
            analytics: Arc::new(RwLock::new(None)),
            db: Arc::new(tokio::sync::Mutex::new(conn)),
        })
    }

    /// Create an in-memory-only manager (uses an in-memory SQLite database). Used for tests.
    pub fn new() -> Self {
        let conn = Connection::open_in_memory().expect("in-memory SQLite failed");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS advanced_memories (
                id          TEXT PRIMARY KEY,
                content     TEXT NOT NULL,
                importance  REAL NOT NULL DEFAULT 50.0,
                links_json  TEXT NOT NULL DEFAULT '[]',
                ttl_secs    INTEGER,
                created_at  TEXT NOT NULL,
                expires_at  TEXT,
                source      TEXT NOT NULL DEFAULT 'system',
                confidence  REAL NOT NULL DEFAULT 100.0,
                verified    INTEGER NOT NULL DEFAULT 0
            );",
        )
        .expect("schema creation failed");

        Self {
            memories: Arc::new(RwLock::new(HashMap::new())),
            relationships: Arc::new(RwLock::new(Vec::new())),
            dedup_hashes: Arc::new(RwLock::new(HashMap::new())),
            analytics: Arc::new(RwLock::new(None)),
            db: Arc::new(tokio::sync::Mutex::new(conn)),
        }
    }

    /// Start a background tokio task that periodically runs TTL expiry.
    /// Runs every `interval_secs` seconds; expired memories are removed from both
    /// the in-memory store and the database.
    pub fn start_ttl_cleanup_task(
        self: &Arc<Self>,
        interval_secs: u64,
    ) -> tokio::task::JoinHandle<()> {
        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            let interval = tokio::time::Duration::from_secs(interval_secs);
            loop {
                tokio::time::sleep(interval).await;
                match mgr.cleanup_expired().await {
                    Ok(removed) if removed > 0 => {
                        info!(
                            "[MEMORY_ADVANCED] TTL task removed {} expired memories",
                            removed
                        );
                    }
                    Err(e) => {
                        warn!("[MEMORY_ADVANCED] TTL cleanup error: {}", e);
                    }
                    _ => {}
                }
            }
        })
    }

    /// Add a memory with importance scoring.
    ///
    /// Deduplication is checked on every insert:
    /// - Exact hash match → rejected, returns `DuplicateOf:<existing_id>`
    /// - Word-overlap similarity > 0.85 → rejected, returns `DuplicateOf:<similar_id>`
    pub async fn add_memory(
        &self,
        content: String,
        importance: Importance,
        source: String,
        confidence: f32,
        ttl: Option<Duration>,
    ) -> Result<String> {
        // --- Exact deduplication via content hash ---
        let hash = content_hash(&content);
        {
            let hashes = self.dedup_hashes.read().await;
            if let Some(existing_id) = hashes.get(&hash) {
                info!(
                    "[MEMORY_ADVANCED] Exact duplicate detected, rejecting in favour of {}",
                    existing_id
                );
                return Ok(format!("DuplicateOf:{}", existing_id));
            }
        }

        // --- Near-duplicate detection via word overlap ---
        let similar = self.find_similar(&content, 0.85).await?;
        if !similar.is_empty() {
            info!(
                "[MEMORY_ADVANCED] Near-duplicate detected (similar to {:?}), rejecting",
                similar
            );
            return Ok(format!("DuplicateOf:{}", similar[0]));
        }

        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let expires_at = ttl.map(|d| now + d);

        let memory = AdvancedMemoryEntry {
            id: id.clone(),
            content: content.clone(),
            importance,
            links: Vec::new(),
            ttl,
            created_at: now,
            expires_at,
            source: source.clone(),
            confidence,
            verified: false,
        };

        // Persist to database first
        {
            let db = self.db.lock().await;
            db.execute(
                "INSERT INTO advanced_memories
                 (id, content, importance, links_json, ttl_secs, created_at, expires_at, source, confidence, verified)
                 VALUES (?1, ?2, ?3, '[]', ?4, ?5, ?6, ?7, ?8, 0)",
                params![
                    id,
                    content,
                    importance.0,
                    ttl.map(|d| d.num_seconds()),
                    now.to_rfc3339(),
                    expires_at.map(|t| t.to_rfc3339()),
                    source,
                    confidence,
                ],
            )
            .context("Failed to persist advanced memory")?;
        }

        // Update in-memory stores
        {
            let mut hashes = self.dedup_hashes.write().await;
            hashes.insert(hash, id.clone());
        }
        {
            let mut memories = self.memories.write().await;
            memories.insert(id.clone(), memory);
        }

        info!(
            "[MEMORY_ADVANCED] Added memory {} with importance {}",
            id, importance.0
        );
        Ok(id)
    }

    /// Link two memories
    pub async fn link_memories(
        &self,
        source_id: &str,
        target_id: &str,
        relation_type: RelationType,
    ) -> Result<()> {
        let link = MemoryLink {
            target_memory_id: target_id.to_string(),
            relation_type: relation_type.clone(),
            created_at: Utc::now(),
        };

        // Update in-memory entry and capture the new links JSON
        let links_json = {
            let mut memories = self.memories.write().await;
            if let Some(memory) = memories.get_mut(source_id) {
                memory.links.push(link);
                serde_json::to_string(&memory.links)?
            } else {
                return Err(anyhow!("Source memory not found: {}", source_id));
            }
        };

        // Persist updated links
        {
            let db = self.db.lock().await;
            db.execute(
                "UPDATE advanced_memories SET links_json = ?1 WHERE id = ?2",
                params![links_json, source_id],
            )
            .context("Failed to persist memory links")?;
        }

        info!("[MEMORY_ADVANCED] Linked {} to {}", source_id, target_id);
        Ok(())
    }

    /// Find similar memories (potential duplicates) using word overlap
    pub async fn find_similar(&self, content: &str, threshold: f32) -> Result<Vec<String>> {
        let content_lower = content.to_lowercase();
        let memories = self.memories.read().await;

        let similar: Vec<String> = memories
            .iter()
            .filter_map(|(id, mem)| {
                let mem_lower = mem.content.to_lowercase();
                // Simple similarity check: substring match or word overlap
                if mem_lower.contains(&content_lower) || content_lower.contains(&mem_lower) {
                    Some(id.clone())
                } else {
                    let word_overlap = calculate_word_overlap(&content_lower, &mem_lower);
                    if word_overlap > threshold {
                        Some(id.clone())
                    } else {
                        None
                    }
                }
            })
            .collect();

        Ok(similar)
    }

    /// Mark memory as verified
    pub async fn verify_memory(&self, memory_id: &str) -> Result<()> {
        {
            let mut memories = self.memories.write().await;
            if let Some(memory) = memories.get_mut(memory_id) {
                memory.verified = true;
            } else {
                return Err(anyhow!("Memory not found: {}", memory_id));
            }
        }

        {
            let db = self.db.lock().await;
            db.execute(
                "UPDATE advanced_memories SET verified = 1 WHERE id = ?1",
                params![memory_id],
            )
            .context("Failed to persist memory verification")?;
        }

        info!("[MEMORY_ADVANCED] Verified memory {}", memory_id);
        Ok(())
    }

    /// Update importance score
    pub async fn set_importance(&self, memory_id: &str, importance: Importance) -> Result<()> {
        {
            let mut memories = self.memories.write().await;
            if let Some(memory) = memories.get_mut(memory_id) {
                memory.importance = importance;
            } else {
                return Err(anyhow!("Memory not found: {}", memory_id));
            }
        }

        {
            let db = self.db.lock().await;
            db.execute(
                "UPDATE advanced_memories SET importance = ?1 WHERE id = ?2",
                params![importance.0, memory_id],
            )
            .context("Failed to persist importance update")?;
        }

        info!(
            "[MEMORY_ADVANCED] Updated importance for {}: {}",
            memory_id, importance.0
        );
        Ok(())
    }

    /// Get related memories
    pub async fn get_related_memories(&self, memory_id: &str) -> Result<Vec<AdvancedMemoryEntry>> {
        let memories = self.memories.read().await;

        if let Some(memory) = memories.get(memory_id) {
            let related: Vec<AdvancedMemoryEntry> = memory
                .links
                .iter()
                .filter_map(|link| memories.get(&link.target_memory_id).cloned())
                .collect();

            Ok(related)
        } else {
            Err(anyhow!("Memory not found: {}", memory_id))
        }
    }

    /// Clean up expired memories from both the in-memory cache and the database.
    pub async fn cleanup_expired(&self) -> Result<usize> {
        let now = Utc::now();
        let now_str = now.to_rfc3339();

        // Collect expired IDs from in-memory store
        let expired_ids: Vec<String> = {
            let memories = self.memories.read().await;
            memories
                .iter()
                .filter_map(|(id, mem)| {
                    if let Some(expires_at) = mem.expires_at {
                        if expires_at <= now {
                            return Some(id.clone());
                        }
                    }
                    None
                })
                .collect()
        };

        if expired_ids.is_empty() {
            return Ok(0);
        }

        // Remove from database first
        {
            let db = self.db.lock().await;
            db.execute(
                "DELETE FROM advanced_memories WHERE expires_at IS NOT NULL AND expires_at <= ?1",
                params![now_str],
            )
            .context("Failed to delete expired memories from DB")?;
        }

        // Remove from in-memory cache and dedup hashes
        {
            let mut memories = self.memories.write().await;
            let mut hashes = self.dedup_hashes.write().await;
            for id in &expired_ids {
                if let Some(mem) = memories.remove(id) {
                    let hash = content_hash(&mem.content);
                    hashes.remove(&hash);
                }
            }
        }

        let removed = expired_ids.len();
        info!("[MEMORY_ADVANCED] Cleaned up {} expired memories", removed);
        Ok(removed)
    }

    /// Calculate memory analytics
    pub async fn calculate_analytics(&self) -> Result<MemoryAnalytics> {
        let memories = self.memories.read().await;
        let now = Utc::now();

        let total_memories = memories.len();

        // Importance distribution
        let mut importance_distribution = HashMap::new();
        for memory in memories.values() {
            let bucket = match memory.importance.0 {
                x if x >= 75.0 => "high",
                x if x >= 50.0 => "normal",
                x if x >= 25.0 => "low",
                _ => "critical",
            };
            *importance_distribution
                .entry(bucket.to_string())
                .or_insert(0) += 1;
        }

        // Average age
        let average_age_days = if !memories.is_empty() {
            memories
                .values()
                .map(|m| {
                    let age = now.signed_duration_since(m.created_at);
                    age.num_days() as f32
                })
                .sum::<f32>()
                / memories.len() as f32
        } else {
            0.0
        };

        let analytics = MemoryAnalytics {
            total_memories,
            importance_distribution,
            access_frequency: 0.0, // Would be calculated from usage logs
            average_age_days,
            redundancy_score: 0.0, // Would be calculated from similarity analysis
            predicted_eviction_count: 0,
            retention_rate: 100.0,
        };

        *self.analytics.write().await = Some(analytics.clone());
        Ok(analytics)
    }

    /// Export memories to a portable format
    pub async fn export_memories(&self, include_relationships: bool) -> Result<MemoryExport> {
        let memories: Vec<AdvancedMemoryEntry> =
            self.memories.read().await.values().cloned().collect();

        let relationships = if include_relationships {
            memories
                .iter()
                .flat_map(|mem| {
                    mem.links.iter().map(|link| {
                        (
                            mem.id.clone(),
                            link.target_memory_id.clone(),
                            link.relation_type.clone(),
                        )
                    })
                })
                .collect()
        } else {
            Vec::new()
        };

        let export = MemoryExport {
            timestamp: Utc::now(),
            memories,
            relationships,
            metadata: HashMap::new(),
        };

        info!(
            "[MEMORY_ADVANCED] Exported {} memories",
            export.memories.len()
        );
        Ok(export)
    }

    /// Import memories from export
    pub async fn import_memories(&self, export: MemoryExport) -> Result<usize> {
        let count = export.memories.len();

        for memory in export.memories {
            let links_json = serde_json::to_string(&memory.links)?;
            {
                let db = self.db.lock().await;
                db.execute(
                    "INSERT OR REPLACE INTO advanced_memories
                     (id, content, importance, links_json, ttl_secs, created_at, expires_at, source, confidence, verified)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        memory.id,
                        memory.content,
                        memory.importance.0,
                        links_json,
                        memory.ttl.map(|d| d.num_seconds()),
                        memory.created_at.to_rfc3339(),
                        memory.expires_at.map(|t| t.to_rfc3339()),
                        memory.source,
                        memory.confidence,
                        memory.verified,
                    ],
                )?;
            }
            let hash = content_hash(&memory.content);
            {
                let mut hashes = self.dedup_hashes.write().await;
                hashes.insert(hash, memory.id.clone());
            }
            {
                let mut memories = self.memories.write().await;
                memories.insert(memory.id.clone(), memory);
            }
        }

        info!("[MEMORY_ADVANCED] Imported {} memories", count);
        Ok(count)
    }

    /// Get memory by ID
    pub async fn get_memory(&self, memory_id: &str) -> Result<AdvancedMemoryEntry> {
        let memories = self.memories.read().await;
        memories
            .get(memory_id)
            .cloned()
            .ok_or_else(|| anyhow!("Memory not found: {}", memory_id))
    }

    /// Get analytics
    pub async fn get_analytics(&self) -> Option<MemoryAnalytics> {
        self.analytics.read().await.clone()
    }

    /// Search with filters
    pub async fn search(
        &self,
        query: &str,
        min_importance: Option<f32>,
        include_unverified: bool,
    ) -> Result<Vec<AdvancedMemoryEntry>> {
        let memories = self.memories.read().await;
        let query_lower = query.to_lowercase();

        let results: Vec<AdvancedMemoryEntry> = memories
            .values()
            .filter(|mem| {
                // Content match
                let content_match = mem.content.to_lowercase().contains(&query_lower);

                // Importance filter
                let importance_match = min_importance.map_or(true, |min| mem.importance.0 >= min);

                // Verification filter
                let verification_match = include_unverified || mem.verified;

                content_match && importance_match && verification_match
            })
            .cloned()
            .collect();

        Ok(results)
    }
}

impl Default for AdvancedMemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute a simple content hash for exact-duplicate detection.
fn content_hash(content: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.trim().to_lowercase().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Calculate word overlap similarity
fn calculate_word_overlap(text1: &str, text2: &str) -> f32 {
    let words1: HashSet<&str> = text1.split_whitespace().collect();
    let words2: HashSet<&str> = text2.split_whitespace().collect();

    if words1.is_empty() || words2.is_empty() {
        return 0.0;
    }

    let intersection = words1.intersection(&words2).count();
    let union = words1.union(&words2).count();

    intersection as f32 / union as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_importance_scoring() {
        assert_eq!(Importance::critical().0, 100.0);
        assert_eq!(Importance::high().0, 75.0);
        assert_eq!(Importance::normal().0, 50.0);
        assert_eq!(Importance::low().0, 25.0);
    }

    #[test]
    fn test_importance_clamping() {
        let imp = Importance::new(150.0);
        assert_eq!(imp.0, 100.0);
        let imp = Importance::new(-10.0);
        assert_eq!(imp.0, 0.0);
    }

    #[tokio::test]
    async fn test_memory_manager() {
        let manager = AdvancedMemoryManager::new();
        let id = manager
            .add_memory(
                "Test memory".to_string(),
                Importance::normal(),
                "test".to_string(),
                100.0,
                None,
            )
            .await
            .unwrap();

        let memory = manager.get_memory(&id).await.unwrap();
        assert_eq!(memory.content, "Test memory");
    }

    #[tokio::test]
    async fn test_deduplication() {
        let manager = AdvancedMemoryManager::new();
        let id1 = manager
            .add_memory(
                "Duplicate content".to_string(),
                Importance::normal(),
                "test".to_string(),
                100.0,
                None,
            )
            .await
            .unwrap();

        // Second insert with identical content should be rejected
        let id2 = manager
            .add_memory(
                "Duplicate content".to_string(),
                Importance::normal(),
                "test".to_string(),
                100.0,
                None,
            )
            .await
            .unwrap();

        assert!(
            id2.starts_with("DuplicateOf:"),
            "Expected duplicate rejection, got {}",
            id2
        );
        assert!(id2.contains(&id1));
    }

    #[test]
    fn test_word_overlap() {
        let overlap = calculate_word_overlap("hello world", "hello there");
        assert!(overlap > 0.0 && overlap < 1.0);
    }
}
