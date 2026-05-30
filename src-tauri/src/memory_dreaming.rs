//! Memory Dreaming & Consolidation Engine
//!
//! Consolidates memories during idle periods in three phases:
//! - Light: removes near-duplicate memories (cosine similarity > 0.97)
//! - Deep: LLM extracts insights from memory clusters into new Fact memories
//! - REM: recalculates importance scores for frequently-accessed memories
#![allow(dead_code)]

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Configuration for the dreaming engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamingConfig {
    /// Enable automatic background dreaming.
    pub enabled: bool,
    /// Minutes of inactivity before a dream cycle starts (default: 5).
    pub idle_minutes: u32,
    /// Enable deep phase (LLM insight extraction). Requires a working LLM.
    pub deep_dream_enabled: bool,
    /// Maximum memories processed per cycle (default: 100).
    pub max_memories_per_cycle: usize,
    /// Cosine similarity threshold for light-phase deduplication (default: 0.97).
    pub dedup_threshold: f32,
}

impl Default for DreamingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            idle_minutes: 5,
            deep_dream_enabled: true,
            max_memories_per_cycle: 100,
            dedup_threshold: 0.97,
        }
    }
}

/// Current phase of the dreaming engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DreamPhase {
    Idle,
    Light,
    Deep,
    Rem,
}

impl Default for DreamPhase {
    fn default() -> Self {
        DreamPhase::Idle
    }
}

/// Runtime status of the dreaming engine.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DreamStatus {
    pub phase: DreamPhase,
    pub last_cycle_at: Option<DateTime<Utc>>,
    pub last_cycle_duration_ms: u64,
    pub memories_processed: u64,
    pub memories_removed: u64,
    pub insights_created: u64,
    pub cycles_completed: u64,
    pub last_error: Option<String>,
}

/// One log entry per completed cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamLogEntry {
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub memories_processed: usize,
    pub duplicates_removed: usize,
    pub insights_created: usize,
    pub phase_reached: DreamPhase,
}

/// The dreaming engine — holds shared state accessed from commands and the bg task.
pub struct DreamingEngine {
    pub config: Arc<RwLock<DreamingConfig>>,
    pub status: Arc<RwLock<DreamStatus>>,
    pub log: Arc<RwLock<Vec<DreamLogEntry>>>,
}

impl DreamingEngine {
    pub fn new(config: DreamingConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            status: Arc::new(RwLock::new(DreamStatus::default())),
            log: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

/// Cosine similarity between two equal-length vectors.
/// Returns 0.0 if either vector is empty or zero-magnitude.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.len() != a.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag_a < 1e-9 || mag_b < 1e-9 {
        return 0.0;
    }
    (dot / (mag_a * mag_b)).clamp(-1.0, 1.0)
}

impl DreamingEngine {
    /// Light phase: scan memories for near-duplicates (cosine_similarity > threshold).
    /// Marks the lower-importance duplicate as superseded in the advanced memory manager.
    /// Returns the number of duplicates removed.
    pub async fn run_light_phase(
        &self,
        advanced_memory: &crate::memory_advanced::AdvancedMemoryManager,
        config: &DreamingConfig,
    ) -> Result<usize> {
        {
            let mut status = self.status.write().await;
            status.phase = DreamPhase::Light;
        }

        let memories = advanced_memory
            .get_all_with_embeddings(config.max_memories_per_cycle)
            .await?;
        let mut removed = 0usize;
        let n = memories.len();

        // O(n²) but bounded by max_memories_per_cycle (default 100 → 5_050 comparisons)
        for i in 0..n {
            if memories[i].1.is_none() {
                continue;
            }
            for j in (i + 1)..n {
                if memories[j].1.is_none() {
                    continue;
                }
                let emb_i = memories[i].1.as_ref().unwrap();
                let emb_j = memories[j].1.as_ref().unwrap();
                let sim = cosine_similarity(emb_i, emb_j);
                if sim >= config.dedup_threshold {
                    // Remove the one with lower importance
                    let (keep, drop_id) =
                        if memories[i].0.importance >= memories[j].0.importance {
                            (&memories[i].0.id, &memories[j].0.id)
                        } else {
                            (&memories[j].0.id, &memories[i].0.id)
                        };
                    info!(
                        "[DREAMING] Light: removing duplicate {} (similar to {}, sim={:.3})",
                        drop_id, keep, sim
                    );
                    if let Err(e) = advanced_memory.delete_memory(drop_id).await {
                        warn!("[DREAMING] Light: failed to remove {}: {}", drop_id, e);
                    } else {
                        removed += 1;
                    }
                }
            }
        }

        {
            let mut status = self.status.write().await;
            status.memories_removed += removed as u64;
        }

        Ok(removed)
    }
}

/// Build the prompt sent to the LLM during deep dreaming.
pub fn build_deep_dream_prompt(memories: &[String]) -> String {
    let bullet_list = memories
        .iter()
        .enumerate()
        .map(|(i, m)| format!("{}. {}", i + 1, m))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "You are a memory consolidation system. Below are raw memory entries.\n\
        Synthesize them into 1-3 concise, high-value insights. Each insight should be\n\
        a standalone fact that can be retrieved independently. Output only the insights,\n\
        one per line, with no numbering or extra commentary.\n\n\
        Memory entries:\n{}\n\nInsights:",
        bullet_list
    )
}

impl DreamingEngine {
    /// REM phase: recalculate importance scores for all memories based on
    /// age. Persists updated scores via the public set_importance() method.
    pub async fn run_rem_phase(
        &self,
        advanced_memory: &crate::memory_advanced::AdvancedMemoryManager,
        config: &DreamingConfig,
    ) -> Result<usize> {
        {
            let mut status = self.status.write().await;
            status.phase = DreamPhase::Rem;
        }

        let now = Utc::now();

        // Collect (id, created_at) using the public API
        let entries: Vec<(String, chrono::DateTime<Utc>)> = advanced_memory
            .get_all_with_embeddings(config.max_memories_per_cycle)
            .await?
            .into_iter()
            .map(|(e, _)| (e.id, e.created_at))
            .collect();

        let mut updated = 0usize;
        for (id, created_at) in entries {
            let age_days = (now - created_at).num_days().max(0);
            let new_importance =
                crate::memory_advanced::Importance::auto_calculate(0u32, age_days);
            if let Err(e) = advanced_memory.set_importance(&id, new_importance).await {
                warn!("[DREAMING] REM: failed to update importance for {}: {}", id, e);
            } else {
                updated += 1;
            }
        }

        Ok(updated)
    }

    /// Run a full dream cycle: light → deep → REM.
    pub async fn run_cycle(
        &self,
        advanced_memory: &crate::memory_advanced::AdvancedMemoryManager,
        claude_client: Option<&crate::claude::ClaudeClient>,
    ) -> Result<DreamLogEntry> {
        let config = self.config.read().await.clone();
        let started_at = Utc::now();

        let duplicates_removed = self
            .run_light_phase(advanced_memory, &config)
            .await
            .unwrap_or_else(|e| {
                warn!("[DREAMING] Light phase error: {}", e);
                0
            });

        let insights_created = if let Some(client) = claude_client {
            self.run_deep_phase(advanced_memory, client, &config)
                .await
                .unwrap_or_else(|e| {
                    warn!("[DREAMING] Deep phase error: {}", e);
                    0
                })
        } else {
            0
        };

        let memories_processed = self
            .run_rem_phase(advanced_memory, &config)
            .await
            .unwrap_or_else(|e| {
                warn!("[DREAMING] REM phase error: {}", e);
                0
            });

        let completed_at = Utc::now();
        let duration_ms = (completed_at - started_at).num_milliseconds().max(0) as u64;

        let entry = DreamLogEntry {
            started_at,
            completed_at,
            duration_ms,
            memories_processed,
            duplicates_removed,
            insights_created,
            phase_reached: DreamPhase::Rem,
        };

        {
            let mut status = self.status.write().await;
            status.phase = DreamPhase::Idle;
            status.last_cycle_at = Some(completed_at);
            status.last_cycle_duration_ms = duration_ms;
            status.memories_processed += memories_processed as u64;
            status.cycles_completed += 1;
        }

        {
            let mut log = self.log.write().await;
            log.push(entry.clone());
            // Keep only last 100 entries
            let len = log.len();
            if len > 100 {
                log.drain(0..len - 100);
            }
        }

        info!(
            "[DREAMING] Cycle complete: {}ms, {} dupes removed, {} insights, {} rem-updated",
            duration_ms, duplicates_removed, insights_created, memories_processed
        );

        Ok(entry)
    }

    /// Deep phase: groups memories by semantic similarity, calls LLM to extract
    /// insights, and stores each insight as a new Fact memory.
    pub async fn run_deep_phase(
        &self,
        advanced_memory: &crate::memory_advanced::AdvancedMemoryManager,
        claude_client: &crate::claude::ClaudeClient,
        config: &DreamingConfig,
    ) -> Result<usize> {
        if !config.deep_dream_enabled {
            return Ok(0);
        }

        {
            let mut status = self.status.write().await;
            status.phase = DreamPhase::Deep;
        }

        let memories = advanced_memory
            .get_all_with_embeddings(config.max_memories_per_cycle)
            .await?
            .into_iter()
            .map(|(e, _)| e.content)
            .collect::<Vec<_>>();

        if memories.is_empty() {
            return Ok(0);
        }

        // Chunk into groups of 10 for manageable prompts
        let mut insights_created = 0usize;
        for chunk in memories.chunks(10) {
            let prompt = build_deep_dream_prompt(&chunk.to_vec());
            match claude_client.send_message(&prompt).await {
                Ok(response) => {
                    for line in response.lines() {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        let entry = crate::memory_advanced::AdvancedMemoryEntry {
                            id: uuid::Uuid::new_v4().to_string(),
                            content: line.to_string(),
                            importance: crate::memory_advanced::Importance::high(),
                            links: vec![],
                            ttl: None,
                            created_at: Utc::now(),
                            expires_at: None,
                            source: "dreaming_deep".to_string(),
                            confidence: 90.0,
                            verified: false,
                        };
                        if let Err(e) = advanced_memory.store_memory(entry).await {
                            warn!("[DREAMING] Deep: failed to store insight: {}", e);
                        } else {
                            insights_created += 1;
                        }
                    }
                }
                Err(e) => {
                    warn!("[DREAMING] Deep phase LLM call failed: {}", e);
                }
            }
        }

        {
            let mut status = self.status.write().await;
            status.insights_created += insights_created as u64;
        }

        Ok(insights_created)
    }
}

/// Spawn a background task that triggers a dream cycle after the system has
/// been idle for at least `config.idle_minutes` minutes.
/// The watcher polls every 60 seconds and skips if dreaming is disabled.
pub fn spawn_idle_watcher(
    engine: Arc<DreamingEngine>,
    advanced_memory: Arc<crate::memory_advanced::AdvancedMemoryManager>,
    last_activity: Arc<tokio::sync::RwLock<std::time::Instant>>,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            let config = engine.config.read().await.clone();
            if !config.enabled {
                continue;
            }
            let idle = { last_activity.read().await.elapsed() };
            let required =
                std::time::Duration::from_secs(config.idle_minutes as u64 * 60);
            if idle >= required {
                if let Err(e) = engine.run_cycle(&advanced_memory, None).await {
                    warn!("[DREAMING] Background cycle error: {}", e);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dreaming_config_defaults() {
        let c = DreamingConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.idle_minutes, 5);
        assert!(c.deep_dream_enabled);
        assert_eq!(c.max_memories_per_cycle, 100);
    }

    #[test]
    fn dream_status_initial() {
        let s = DreamStatus::default();
        assert!(matches!(s.phase, DreamPhase::Idle));
        assert!(s.last_cycle_at.is_none());
        assert_eq!(s.cycles_completed, 0);
    }

    #[test]
    fn cosine_similarity_identical() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![1.0f32, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-6, "identical vectors: {}", sim);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![0.0f32, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6, "orthogonal vectors: {}", sim);
    }

    #[test]
    fn cosine_similarity_empty_returns_zero() {
        let a: Vec<f32> = vec![];
        let b: Vec<f32> = vec![];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn rem_score_boosted_for_high_access() {
        use crate::memory_advanced::Importance;
        // auto_calculate: score = min(access_count, 100) + 50 - age_days * 0.5, clamped 0-100
        let score = Importance::auto_calculate(50, 0);
        // 50 + 50 - 0 = 100.0, clamped to 100
        assert!((score.0 - 100.0).abs() < 1e-3, "score: {}", score.0);
    }

    #[test]
    fn rem_score_decays_with_age() {
        use crate::memory_advanced::Importance;
        let score = Importance::auto_calculate(0, 40);
        // 0 + 50 - 20 = 30.0
        assert!((score.0 - 30.0).abs() < 1e-3, "score: {}", score.0);
    }

    #[test]
    fn build_deep_dream_prompt_contains_memories() {
        let memories = vec![
            "I prefer dark mode".to_string(),
            "I use Rust for backend work".to_string(),
            "I work on AI projects".to_string(),
        ];
        let prompt = build_deep_dream_prompt(&memories);
        assert!(prompt.contains("dark mode"), "prompt: {}", prompt);
        assert!(prompt.contains("Rust"), "prompt: {}", prompt);
        assert!(prompt.contains("Synthesize"), "missing Synthesize: {}", prompt);
    }
}
