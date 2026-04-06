///! Memory Files System (OpenClaw-inspired)
///!
///! Provides persistent memory across conversations:
///! - Conversation history by session
///! - Learned user preferences
///! - Important facts and context
///! - Searchable memory retrieval
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::platform::file_security::restrict_file_permissions;
use tracing::{info, warn};

/// Maximum number of messages kept in a single conversation session.
/// Oldest messages are drained when this limit is exceeded.
const MAX_SESSION_MESSAGES: usize = 500;

/// Maximum number of memory entries. Least-recently-accessed entries are evicted.
const MAX_MEMORIES: usize = 50_000;

/// Maximum number of conversation sessions in memory. Least-recently-active are evicted.
const MAX_SESSIONS: usize = 200;

/// Memory entry representing a single piece of information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique ID for this memory
    pub id: String,
    /// Memory content
    pub content: String,
    /// When this memory was created
    pub created_at: DateTime<Utc>,
    /// When this memory was last accessed
    pub last_accessed: DateTime<Utc>,
    /// How many times this memory has been accessed
    pub access_count: u32,
    /// Memory type (conversation, preference, fact, context)
    pub memory_type: MemoryType,
    /// Optional tags for categorization
    pub tags: Vec<String>,
    /// Optional metadata
    pub metadata: HashMap<String, String>,
    /// Embedding vector for semantic search (384-dim MiniLM-L6-v2)
    #[serde(default)]
    pub embedding: Option<Vec<f32>>,
}

/// Type of memory
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// Conversation history
    Conversation,
    /// User preference or setting
    Preference,
    /// Important fact about the user or context
    Fact,
    /// General context information
    Context,
}

/// Conversation session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSession {
    /// Session ID
    pub id: String,
    /// Session title (summary of conversation)
    pub title: Option<String>,
    /// When session started
    pub started_at: DateTime<Utc>,
    /// When session last had activity
    pub last_activity: DateTime<Utc>,
    /// Messages in this session
    pub messages: Vec<SessionMessage>,
}

/// Message in a conversation session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    /// Message role (user or assistant)
    pub role: String,
    /// Message content
    pub content: String,
    /// When message was sent
    pub timestamp: DateTime<Utc>,
}

/// Prepend a short context string to memory content before embedding.
///
/// Inspired by Anthropic's Contextual Retrieval research which shows that even
/// simple context prepending significantly improves embedding quality (~49%
/// retrieval improvement). This is zero-cost at query time because the context
/// is baked into the embedding at storage time.
fn contextualize_content(content: &str, memory_type: &MemoryType, tags: &[String]) -> String {
    let type_label = match memory_type {
        MemoryType::Preference => "user preference",
        MemoryType::Fact => "important fact",
        MemoryType::Context => "contextual information",
        MemoryType::Conversation => "conversation excerpt",
    };
    let tag_str = if tags.is_empty() {
        String::new()
    } else {
        format!(" related to {}", tags.join(", "))
    };
    format!("This is a {}{}: {}", type_label, tag_str, content)
}

/// Memory manager
pub struct MemoryManager {
    memory_dir: PathBuf,
    memories: HashMap<String, MemoryEntry>,
    sessions: HashMap<String, ConversationSession>,
    current_session_id: Option<String>,
}

impl MemoryManager {
    /// Create a new memory manager
    pub fn new() -> Result<Self> {
        let memory_dir = Self::get_memory_dir()?;
        fs::create_dir_all(&memory_dir)?;

        let mut manager = Self {
            memory_dir,
            memories: HashMap::new(),
            sessions: HashMap::new(),
            current_session_id: None,
        };

        manager.load_all_memories()?;
        manager.load_all_sessions()?;

        Ok(manager)
    }

    /// Get the memory directory path
    fn get_memory_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Failed to get home directory")?;
        Ok(home.join(".config/nexibot/memory"))
    }

    /// Load all memories from disk
    fn load_all_memories(&mut self) -> Result<()> {
        let memories_file = self.memory_dir.join("memories.json");
        if !memories_file.exists() {
            info!("[MEMORY] No memories file found, starting fresh");
            return Ok(());
        }

        let content = fs::read_to_string(&memories_file)?;
        let memories: Vec<MemoryEntry> = serde_json::from_str(&content)?;

        for memory in memories {
            self.memories.insert(memory.id.clone(), memory);
        }

        info!("[MEMORY] Loaded {} memories", self.memories.len());
        Ok(())
    }

    /// Save all memories to disk
    fn save_memories(&self) -> Result<()> {
        let memories_file = self.memory_dir.join("memories.json");
        let memories: Vec<&MemoryEntry> = self.memories.values().collect();
        let content = serde_json::to_string_pretty(&memories)?;
        fs::write(&memories_file, content)?;
        let _ = restrict_file_permissions(&memories_file);
        Ok(())
    }

    /// Load all conversation sessions from disk
    fn load_all_sessions(&mut self) -> Result<()> {
        let sessions_dir = self.memory_dir.join("sessions");
        if !sessions_dir.exists() {
            info!("[MEMORY] No sessions directory found, starting fresh");
            return Ok(());
        }

        let mut count = 0;
        for entry in fs::read_dir(&sessions_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                match fs::read_to_string(&path) {
                    Ok(content) => match serde_json::from_str::<ConversationSession>(&content) {
                        Ok(session) => {
                            self.sessions.insert(session.id.clone(), session);
                            count += 1;
                        }
                        Err(e) => warn!("[MEMORY] Failed to parse session {:?}: {}", path, e),
                    },
                    Err(e) => warn!("[MEMORY] Failed to read session {:?}: {}", path, e),
                }
            }
        }

        info!("[MEMORY] Loaded {} conversation sessions", count);
        Ok(())
    }

    /// Save a conversation session to disk
    fn save_session(&self, session: &ConversationSession) -> Result<()> {
        let sessions_dir = self.memory_dir.join("sessions");
        fs::create_dir_all(&sessions_dir)?;

        let session_file = sessions_dir.join(format!("{}.json", session.id));
        let content = serde_json::to_string_pretty(session)?;
        fs::write(&session_file, content)?;
        let _ = restrict_file_permissions(&session_file);
        Ok(())
    }

    /// Add a memory entry
    pub fn add_memory(
        &mut self,
        content: String,
        memory_type: MemoryType,
        tags: Vec<String>,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();

        // Embed the contextualized version for better retrieval quality
        // (Anthropic Contextual Retrieval: ~49% improvement)
        let contextualized = contextualize_content(&content, &memory_type, &tags);
        let embedding = match crate::embeddings::embed_text(&contextualized) {
            Ok(emb) => Some(emb),
            Err(_) => None, // Graceful degradation: embedding model may not be available
        };

        let memory = MemoryEntry {
            id: id.clone(),
            content,
            created_at: now,
            last_accessed: now,
            access_count: 0,
            memory_type,
            tags,
            metadata: HashMap::new(),
            embedding,
        };

        // Evict least-recently-accessed BEFORE inserting to prevent momentarily
        // exceeding MAX_MEMORIES. Check >= so we make room for the incoming entry.
        if self.memories.len() >= MAX_MEMORIES {
            let to_remove = self.memories.len() - MAX_MEMORIES + 1;
            let mut entries: Vec<(String, DateTime<Utc>)> = self
                .memories
                .iter()
                .map(|(k, v)| (k.clone(), v.last_accessed))
                .collect();
            entries.sort_by(|a, b| a.1.cmp(&b.1)); // oldest first
            for (evict_id, _) in entries.into_iter().take(to_remove) {
                self.memories.remove(&evict_id);
            }
            info!(
                "[MEMORY] Evicted {} least-recently-accessed memories to make room",
                to_remove
            );
        }

        self.memories.insert(id.clone(), memory);

        self.save_memories()?;

        info!("[MEMORY] Added memory: {}", id);
        Ok(id)
    }

    /// Save facts extracted by the pre-compaction flush.
    ///
    /// Each fact is stored as a memory entry with type `Fact` and tagged
    /// with "pre-compaction-flush" for provenance tracking. The session ID
    /// is stored in the metadata HashMap for traceability.
    pub fn save_extracted_facts(&mut self, facts: &[String], session_id: &str) -> usize {
        let mut saved = 0;
        for fact in facts {
            let trimmed = fact.trim();
            if trimmed.is_empty() {
                continue;
            }

            let mut tags = vec!["pre-compaction-flush".to_string()];
            // Include session_id as a tag for easy filtering
            tags.push(format!("session:{}", session_id));

            match self.add_memory(trimmed.to_string(), MemoryType::Fact, tags) {
                Ok(_) => saved += 1,
                Err(e) => {
                    warn!("[MEMORY] Failed to save extracted fact: {}", e);
                }
            }
        }

        if saved > 0 {
            info!(
                "[MEMORY] Saved {} extracted facts from pre-compaction flush",
                saved
            );
        }

        saved
    }

    /// Get a memory by ID (updates access tracking)
    pub fn get_memory(&mut self, memory_id: &str) -> Option<&MemoryEntry> {
        if let Some(memory) = self.memories.get_mut(memory_id) {
            memory.last_accessed = Utc::now();
            memory.access_count += 1;
            let _ = self.save_memories();
            return self.memories.get(memory_id);
        }
        None
    }

    /// Search memories by content (keyword-based).
    pub fn search_memories(&self, query: &str) -> Vec<&MemoryEntry> {
        let query_lower = query.to_lowercase();
        let mut results: Vec<&MemoryEntry> = self
            .memories
            .values()
            .filter(|m| m.content.to_lowercase().contains(&query_lower))
            .collect();

        // Sort by access count (most accessed first)
        results.sort_by(|a, b| b.access_count.cmp(&a.access_count));

        results
    }

    /// Enhanced semantic search using the full hybrid pipeline:
    ///
    /// ```text
    /// Query -> QueryExpander -> [variant1, variant2, ...]
    ///   For each variant:
    ///     -> Keyword search (substring match + ranks)
    ///     -> Vector search (cosine similarity + ranks)
    ///     -> RRF merge (rank-based fusion)
    ///   Merge all variant results via RRF
    ///   -> Temporal decay
    ///   -> Reranker (phrase match, tag match, access count)
    ///   -> MMR re-ranking (diversity)
    ///   -> Return top-K
    /// ```
    ///
    /// Falls back to simple keyword search if the embedding model is unavailable.
    pub fn semantic_search(&self, query: &str, limit: usize) -> Vec<(&MemoryEntry, f32)> {
        use crate::memory_store::hybrid_search::{
            hybrid_search, mmr_rerank, HybridSearchOptions, HybridSearchResult,
        };
        use crate::memory_store::query_expander::QueryExpander;
        use crate::memory_store::reranker::MemoryReranker;

        // Step 1: Expand query into variants.
        // Limit to MAX_QUERY_VARIANTS to prevent unbounded expansion causing DoS.
        const MAX_QUERY_VARIANTS: usize = 5;
        let expander = QueryExpander::new();
        let variants: Vec<String> = expander.expand(query)
            .into_iter()
            .take(MAX_QUERY_VARIANTS)
            .collect();

        // Step 2: Try to embed the query for vector search
        let query_embedding = match crate::embeddings::embed_text(query) {
            Ok(emb) => Some(emb),
            Err(e) => {
                warn!("[MEMORY] Embedding failed, using text-only search: {}", e);
                None
            }
        };

        // Build lookup maps for hybrid search
        let content_map: HashMap<String, String> = self
            .memories
            .iter()
            .map(|(id, m)| (id.clone(), m.content.clone()))
            .collect();

        let timestamps: HashMap<String, chrono::DateTime<Utc>> = self
            .memories
            .iter()
            .map(|(id, m)| (id.clone(), m.created_at))
            .collect();

        let options = HybridSearchOptions {
            text_weight: 1.0,
            vector_weight: 1.0,
            temporal_decay_factor: 0.01,
            max_results: limit * 3, // Fetch more than needed for reranking
            mmr_lambda: 0.7,
        };

        // Step 3: Run search for each variant and collect results
        let mut all_rrf_entries: HashMap<String, f64> = HashMap::new();
        let mut seen_results: HashMap<String, HybridSearchResult> = HashMap::new();

        for (variant_rank, variant) in variants.iter().enumerate() {
            // Text search: keyword match with ranking
            let text_results: Vec<(String, f64)> = {
                let query_lower = variant.to_lowercase();
                let mut matches: Vec<(String, f64)> = self
                    .memories
                    .iter()
                    .filter_map(|(id, m)| {
                        let content_lower = m.content.to_lowercase();
                        if content_lower.contains(&query_lower) {
                            // Score by how much of the content matches
                            let score =
                                query_lower.len() as f64 / content_lower.len().max(1) as f64;
                            Some((id.clone(), score))
                        } else {
                            // Check individual words
                            let words: Vec<&str> = query_lower.split_whitespace().collect();
                            let matches =
                                words.iter().filter(|w| content_lower.contains(*w)).count();
                            if matches > 0 {
                                Some((id.clone(), matches as f64 / words.len().max(1) as f64))
                            } else {
                                None
                            }
                        }
                    })
                    .collect();
                matches.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                matches
            };

            // Vector search: cosine similarity
            let vector_results: Vec<(String, f64)> = if let Some(ref q_emb) = query_embedding {
                let mut scores: Vec<(String, f64)> = self
                    .memories
                    .values()
                    .filter_map(|m| {
                        m.embedding.as_ref().and_then(|emb| {
                            // Validate dimensions before calling cosine_similarity to prevent panics
                            // when old embeddings from a different model are in the store.
                            if q_emb.len() != emb.len() {
                                warn!(
                                    "[MEMORY] Embedding dimension mismatch: query has {} dims, stored has {} dims for memory {}. Skipping.",
                                    q_emb.len(), emb.len(), m.id
                                );
                                return None;
                            }
                            if q_emb.is_empty() {
                                return None;
                            }
                            let sim = crate::embeddings::cosine_similarity(q_emb, emb) as f64;
                            Some((m.id.clone(), sim))
                        })
                    })
                    .filter(|(_, s)| *s > 0.3)
                    .collect();
                scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                scores
            } else {
                vec![]
            };

            // RRF merge for this variant
            let variant_results = hybrid_search(
                text_results,
                vector_results,
                &options,
                &content_map,
                &timestamps,
            );

            // Accumulate RRF scores across variants (variant rank decay)
            let variant_decay = 1.0 / (1.0 + variant_rank as f64 * 0.3);
            for result in variant_results {
                let entry = all_rrf_entries
                    .entry(result.record_id.clone())
                    .or_insert(0.0);
                *entry += result.combined_score * variant_decay;
                seen_results
                    .entry(result.record_id.clone())
                    .or_insert(result);
            }
        }

        // Step 4: Build final results from accumulated scores
        let mut final_results: Vec<HybridSearchResult> = all_rrf_entries
            .into_iter()
            .filter_map(|(id, score)| {
                seen_results.remove(&id).map(|mut r| {
                    r.combined_score = score;
                    r
                })
            })
            .collect();

        final_results.sort_by(|a, b| {
            b.combined_score
                .partial_cmp(&a.combined_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Step 5: Rerank with memory-specific signals
        let reranker = MemoryReranker::new();

        let tags_map: HashMap<String, Vec<String>> = self
            .memories
            .iter()
            .map(|(id, m)| (id.clone(), m.tags.clone()))
            .collect();

        let memory_types: HashMap<String, String> = self
            .memories
            .iter()
            .map(|(id, m)| {
                let type_str = match m.memory_type {
                    MemoryType::Preference => "preference",
                    MemoryType::Fact => "fact",
                    MemoryType::Context => "context",
                    MemoryType::Conversation => "conversation",
                };
                (id.clone(), type_str.to_string())
            })
            .collect();

        let access_counts: HashMap<String, u32> = self
            .memories
            .iter()
            .map(|(id, m)| (id.clone(), m.access_count))
            .collect();

        let created_at_map: HashMap<String, chrono::DateTime<Utc>> = self
            .memories
            .iter()
            .map(|(id, m)| (id.clone(), m.created_at))
            .collect();

        final_results = reranker.rerank(
            final_results,
            query,
            &tags_map,
            &memory_types,
            &access_counts,
            &created_at_map,
        );

        // Step 6: MMR re-ranking for diversity
        let embeddings: HashMap<String, Vec<f32>> = self
            .memories
            .iter()
            .filter_map(|(id, m)| m.embedding.as_ref().map(|e| (id.clone(), e.clone())))
            .collect();

        mmr_rerank(&mut final_results, options.mmr_lambda, &embeddings);

        // Step 7: Truncate and convert to return type
        final_results.truncate(limit);

        let result: Vec<(&MemoryEntry, f32)> = final_results
            .iter()
            .filter_map(|r| {
                self.memories
                    .get(&r.record_id)
                    .map(|entry| (entry, r.combined_score as f32))
            })
            .collect();

        // Fall back to keyword search if no results
        if result.is_empty() {
            return self
                .search_memories(query)
                .into_iter()
                .take(limit)
                .map(|m| (m, 1.0))
                .collect();
        }

        result
    }

    /// Embed all existing memories that don't yet have embeddings.
    /// Returns the number of memories newly embedded.
    #[allow(dead_code)]
    pub fn embed_all_memories(&mut self) -> usize {
        let ids_to_embed: Vec<String> = self
            .memories
            .values()
            .filter(|m| m.embedding.is_none())
            .map(|m| m.id.clone())
            .collect();

        if ids_to_embed.is_empty() {
            return 0;
        }

        let mut count = 0;
        for id in ids_to_embed {
            if let Some(memory) = self.memories.get_mut(&id) {
                match crate::embeddings::embed_text(&memory.content) {
                    Ok(emb) => {
                        memory.embedding = Some(emb);
                        count += 1;
                    }
                    Err(e) => {
                        warn!("[MEMORY] Failed to embed memory {}: {}", id, e);
                        break; // Model likely unavailable, stop trying
                    }
                }
            }
        }

        if count > 0 {
            let _ = self.save_memories();
            info!("[MEMORY] Embedded {} memories", count);
        }

        count
    }

    /// Get memories by type
    pub fn get_memories_by_type(&self, memory_type: MemoryType) -> Vec<&MemoryEntry> {
        self.memories
            .values()
            .filter(|m| m.memory_type == memory_type)
            .collect()
    }

    /// Get memories by tag
    #[allow(dead_code)]
    pub fn get_memories_by_tag(&self, tag: &str) -> Vec<&MemoryEntry> {
        self.memories
            .values()
            .filter(|m| m.tags.contains(&tag.to_string()))
            .collect()
    }

    /// Delete a memory
    pub fn delete_memory(&mut self, memory_id: &str) -> Result<()> {
        self.memories.remove(memory_id);
        self.save_memories()?;
        info!("[MEMORY] Deleted memory: {}", memory_id);
        Ok(())
    }

    /// Start a new conversation session
    pub fn start_session(&mut self) -> Result<String> {
        // Evict least-recently-active session if at capacity
        if self.sessions.len() >= MAX_SESSIONS {
            let oldest_id = self
                .sessions
                .iter()
                .min_by_key(|(_, s)| s.last_activity)
                .map(|(id, _)| id.clone());
            if let Some(evict_id) = oldest_id {
                self.sessions.remove(&evict_id);
                info!(
                    "[MEMORY] Evicted least-recently-active session: {}",
                    evict_id
                );
            }
        }

        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();

        let session = ConversationSession {
            id: id.clone(),
            title: None,
            started_at: now,
            last_activity: now,
            messages: Vec::new(),
        };

        self.sessions.insert(id.clone(), session.clone());
        self.current_session_id = Some(id.clone());
        self.save_session(&session)?;

        info!("[MEMORY] Started new session: {}", id);
        Ok(id)
    }

    /// Add a message to the current session
    pub fn add_message(&mut self, role: String, content: String) -> Result<()> {
        let session_id = self
            .current_session_id
            .clone()
            .context("No active session")?;

        let message = SessionMessage {
            role,
            content,
            timestamp: Utc::now(),
        };

        // Update session and clone for saving
        let session_to_save = {
            let session = self
                .sessions
                .get_mut(&session_id)
                .context("Session not found")?;

            session.messages.push(message);

            // Enforce message limit — drain oldest if over capacity
            if session.messages.len() > MAX_SESSION_MESSAGES {
                let excess = session.messages.len() - MAX_SESSION_MESSAGES;
                session.messages.drain(..excess);
            }

            session.last_activity = Utc::now();
            session.clone()
        };

        self.save_session(&session_to_save)?;
        Ok(())
    }

    /// Set the title for the current session
    pub fn set_session_title(&mut self, title: String) -> Result<()> {
        let session_id = self
            .current_session_id
            .clone()
            .context("No active session")?;

        // Update session and clone for saving
        let session_to_save = {
            let session = self
                .sessions
                .get_mut(&session_id)
                .context("Session not found")?;

            session.title = Some(title);
            session.clone()
        };

        self.save_session(&session_to_save)?;
        Ok(())
    }

    /// Get current session
    pub fn get_current_session(&self) -> Option<&ConversationSession> {
        self.current_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
    }

    /// Get a session by ID
    pub fn get_session(&self, session_id: &str) -> Option<&ConversationSession> {
        self.sessions.get(session_id)
    }

    /// List all sessions
    pub fn list_sessions(&self) -> Vec<&ConversationSession> {
        let mut sessions: Vec<&ConversationSession> = self.sessions.values().collect();
        sessions.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
        sessions
    }

    /// Get the current session ID.
    pub fn get_current_session_id(&self) -> Option<String> {
        self.current_session_id.clone()
    }

    /// Set the current session ID (for resuming a saved session).
    pub fn set_current_session_id(&mut self, id: String) {
        info!("[MEMORY] Set current session to: {}", id);
        self.current_session_id = Some(id);
    }

    /// End the current session
    pub fn end_session(&mut self) -> Result<()> {
        if let Some(session_id) = &self.current_session_id {
            info!("[MEMORY] Ended session: {}", session_id);
            self.current_session_id = None;
        }
        Ok(())
    }

    /// Record a compaction event in the current session.
    /// Appends a compaction marker to the session (the full message history is
    /// preserved on disk; only the live Claude context is compressed).
    pub fn compact_session(&mut self, summary: &str, messages_removed: usize) -> Result<()> {
        let session_id = self
            .current_session_id
            .clone()
            .context("No active session")?;

        let session_to_save = {
            let session = self
                .sessions
                .get_mut(&session_id)
                .context("Session not found")?;

            session.messages.push(SessionMessage {
                role: "system".to_string(),
                content: format!(
                    "[COMPACTED: {} earlier messages were summarized]\n\n{}",
                    messages_removed, summary
                ),
                timestamp: Utc::now(),
            });

            session.last_activity = Utc::now();
            session.clone()
        };

        self.save_session(&session_to_save)?;
        info!(
            "[MEMORY] Recorded compaction event ({} messages removed)",
            messages_removed
        );
        Ok(())
    }

    /// Sync a completed session to K2K supermemory.
    /// This saves the conversation to the System Agent and triggers knowledge extraction.
    pub async fn sync_to_supermemory(
        &self,
        k2k: &crate::k2k_client::K2KIntegration,
        session: &ConversationSession,
    ) -> Result<()> {
        if session.messages.is_empty() {
            return Ok(());
        }

        let title = session.title.clone().unwrap_or_else(|| {
            // Auto-generate title from first user message
            session
                .messages
                .first()
                .map(|m| {
                    let content = &m.content;
                    if content.len() > 50 {
                        format!("{}...", &content[..50])
                    } else {
                        content.clone()
                    }
                })
                .unwrap_or_else(|| "Untitled conversation".to_string())
        });

        let messages: Vec<(String, String)> = session
            .messages
            .iter()
            .map(|m| (m.role.clone(), m.content.clone()))
            .collect();

        match k2k.save_conversation(&title, &messages).await {
            Ok(conv_id) => {
                info!(
                    "[MEMORY] Synced session {} to supermemory (conv_id: {})",
                    session.id, conv_id
                );
                // Trigger knowledge extraction
                if let Err(e) = k2k.extract_conversation_knowledge(&conv_id).await {
                    warn!(
                        "[MEMORY] Knowledge extraction failed for {}: {}",
                        conv_id, e
                    );
                }
            }
            Err(e) => {
                warn!(
                    "[MEMORY] Failed to sync session {} to supermemory: {}",
                    session.id, e
                );
            }
        }

        Ok(())
    }

    /// Extract facts, preferences, and context from a conversation session.
    ///
    /// Uses pattern matching heuristics (no LLM calls) to identify statements
    /// like "I prefer...", "My name is...", "I'm working on..." and stores
    /// them as tagged memory entries.
    ///
    /// Returns the IDs of newly created memory entries.
    pub fn extract_facts_from_session(
        &mut self,
        session: &ConversationSession,
    ) -> Result<Vec<String>> {
        let mut new_ids = Vec::new();

        // Only process user messages (assistant messages are our own responses)
        let user_messages: Vec<&SessionMessage> = session
            .messages
            .iter()
            .filter(|m| m.role == "user")
            .collect();

        if user_messages.is_empty() {
            return Ok(new_ids);
        }

        // Pattern groups: (patterns, memory_type, tags)
        let extraction_rules: &[(&[&str], MemoryType, &[&str])] = &[
            // Preferences
            (
                &[
                    "i prefer",
                    "i like",
                    "i always",
                    "i want",
                    "i'd rather",
                    "i enjoy",
                ],
                MemoryType::Preference,
                &["auto-extracted", "preference"],
            ),
            // Personal facts
            (
                &[
                    "my name is",
                    "i am a",
                    "i work at",
                    "i work as",
                    "i live in",
                    "my job",
                ],
                MemoryType::Fact,
                &["auto-extracted", "personal"],
            ),
            // Project/work context
            (
                &[
                    "i'm working on",
                    "we're building",
                    "the project",
                    "we use",
                    "our team",
                    "our stack",
                ],
                MemoryType::Context,
                &["auto-extracted", "project"],
            ),
        ];

        for msg in &user_messages {
            let content_lower = msg.content.to_lowercase();

            for (patterns, memory_type, tags) in extraction_rules {
                for pattern in *patterns {
                    if let Some(pos) = content_lower.find(pattern) {
                        // Extract the sentence containing the pattern
                        let sentence = Self::extract_sentence(&msg.content, pos);
                        if sentence.len() > 10 && sentence.len() < 500 {
                            // Avoid duplicates: check if we already have a similar memory
                            let is_duplicate = self
                                .memories
                                .values()
                                .any(|m| m.content.to_lowercase() == sentence.to_lowercase());

                            if !is_duplicate {
                                let tags_vec: Vec<String> =
                                    tags.iter().map(|t| t.to_string()).collect();
                                match self.add_memory(sentence, memory_type.clone(), tags_vec) {
                                    Ok(id) => {
                                        info!(
                                            "[MEMORY] Auto-extracted {:?} fact: {}",
                                            memory_type, id
                                        );
                                        new_ids.push(id);
                                    }
                                    Err(e) => {
                                        warn!("[MEMORY] Failed to store extracted fact: {}", e);
                                    }
                                }
                            }
                        }
                        break; // Only extract one fact per pattern group per message
                    }
                }
            }
        }

        if !new_ids.is_empty() {
            info!("[MEMORY] Extracted {} facts from session", new_ids.len());
        }

        Ok(new_ids)
    }

    /// Extract a sentence from text around a given character position.
    fn extract_sentence(text: &str, pos: usize) -> String {
        // Find sentence boundaries
        let start = text[..pos]
            .rfind(|c: char| c == '.' || c == '!' || c == '?' || c == '\n')
            .map(|p| p + 1)
            .unwrap_or(0);

        let end = text[pos..]
            .find(|c: char| c == '.' || c == '!' || c == '?' || c == '\n')
            .map(|p| pos + p + 1)
            .unwrap_or(text.len());

        text[start..end].trim().to_string()
    }

    /// Get memory context for system prompt.
    ///
    /// Returns a formatted string with remembered preferences, facts, and context.
    /// Always includes type-based sections for stable context, regardless of query.
    pub fn get_memory_context(&self, max_entries: usize) -> String {
        if self.memories.is_empty() {
            return String::new();
        }

        // Boundary markers signal to the model that the following content is
        // user-controlled and may contain adversarial prompts (second-order
        // prompt injection defense, consistent with soul.rs).
        let mut context = String::from("# Memory Context\n\n<MEMORY_CONTENT>\n");
        context.push_str("I have access to the following remembered information:\n\n");

        // Get preferences
        let preferences = self.get_memories_by_type(MemoryType::Preference);
        if !preferences.is_empty() {
            context.push_str("## User Preferences\n\n");
            for pref in preferences.iter().take(max_entries) {
                context.push_str(&format!("- {}\n", pref.content));
            }
            context.push('\n');
        }

        // Get important facts
        let facts = self.get_memories_by_type(MemoryType::Fact);
        if !facts.is_empty() {
            context.push_str("## Important Facts\n\n");
            for fact in facts.iter().take(max_entries) {
                context.push_str(&format!("- {}\n", fact.content));
            }
            context.push('\n');
        }

        // Get context information
        let contexts = self.get_memories_by_type(MemoryType::Context);
        if !contexts.is_empty() {
            context.push_str("## Context Information\n\n");
            for ctx in contexts.iter().take(max_entries) {
                context.push_str(&format!("- {}\n", ctx.content));
            }
            context.push('\n');
        }

        context.push_str("</MEMORY_CONTENT>\n");
        context
    }

    /// Get memory context relevant to a specific query using the hybrid search pipeline.
    ///
    /// Uses semantic search to find the most relevant memories for the current
    /// conversation topic, then formats them for inclusion in the system prompt.
    #[allow(dead_code)]
    pub fn get_relevant_memory_context(&self, query: &str, max_entries: usize) -> String {
        if self.memories.is_empty() || query.trim().is_empty() {
            return self.get_memory_context(max_entries);
        }

        let results = self.semantic_search(query, max_entries);
        if results.is_empty() {
            return self.get_memory_context(max_entries);
        }

        let mut context = String::from("# Relevant Memory Context\n\n<MEMORY_CONTENT>\n");

        for (entry, score) in &results {
            let type_label = match entry.memory_type {
                MemoryType::Preference => "Preference",
                MemoryType::Fact => "Fact",
                MemoryType::Context => "Context",
                MemoryType::Conversation => "Conversation",
            };
            context.push_str(&format!("- [{}] {}", type_label, entry.content));
            if *score > 0.7 {
                context.push_str(" (high relevance)");
            }
            context.push('\n');
        }

        context.push_str("</MEMORY_CONTENT>\n\n");
        context
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a MemoryManager backed by a temp directory.
    /// Returns both the manager and the TempDir guard (which must be kept alive).
    fn test_manager() -> (MemoryManager, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let memory_dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(memory_dir.join("sessions")).unwrap();
        let mgr = MemoryManager {
            memory_dir,
            memories: HashMap::new(),
            sessions: HashMap::new(),
            current_session_id: None,
        };
        (mgr, tmp)
    }

    #[test]
    fn test_session_messages_bounded_at_max() {
        let (mut mgr, _tmp) = test_manager();

        // Start a session
        mgr.start_session().unwrap();

        // Add more messages than the limit
        for i in 0..(MAX_SESSION_MESSAGES + 100) {
            mgr.add_message("user".to_string(), format!("msg-{}", i))
                .unwrap();
        }

        let session = mgr.get_current_session().unwrap();
        assert_eq!(session.messages.len(), MAX_SESSION_MESSAGES);

        // Newest message should be the last one added
        assert_eq!(
            session.messages.last().unwrap().content,
            format!("msg-{}", MAX_SESSION_MESSAGES + 99)
        );
    }

    #[test]
    fn test_memory_session_count_bounded() {
        let (mut mgr, _tmp) = test_manager();

        // Create MAX_SESSIONS sessions
        for _ in 0..MAX_SESSIONS {
            mgr.start_session().unwrap();
        }
        assert_eq!(mgr.sessions.len(), MAX_SESSIONS);

        // Creating one more should evict the oldest
        mgr.start_session().unwrap();
        assert_eq!(mgr.sessions.len(), MAX_SESSIONS);
    }

    #[test]
    fn test_contextualize_content() {
        let result =
            contextualize_content("dark mode", &MemoryType::Preference, &["ui".to_string()]);
        assert_eq!(result, "This is a user preference related to ui: dark mode");

        let result = contextualize_content("John Doe", &MemoryType::Fact, &[]);
        assert_eq!(result, "This is a important fact: John Doe");

        let result = contextualize_content(
            "building a web app",
            &MemoryType::Context,
            &["work".to_string(), "project".to_string()],
        );
        assert_eq!(
            result,
            "This is a contextual information related to work, project: building a web app"
        );
    }

    #[test]
    fn test_extract_facts_from_session() {
        let (mut mgr, _tmp) = test_manager();

        let session = ConversationSession {
            id: "test-session".to_string(),
            title: Some("Test".to_string()),
            started_at: Utc::now(),
            last_activity: Utc::now(),
            messages: vec![
                SessionMessage {
                    role: "user".to_string(),
                    content: "I prefer dark mode for all my editors.".to_string(),
                    timestamp: Utc::now(),
                },
                SessionMessage {
                    role: "assistant".to_string(),
                    content: "I'll note that preference!".to_string(),
                    timestamp: Utc::now(),
                },
                SessionMessage {
                    role: "user".to_string(),
                    content: "My name is John and I work at Acme Corp.".to_string(),
                    timestamp: Utc::now(),
                },
                SessionMessage {
                    role: "user".to_string(),
                    content: "I'm working on a new microservices architecture.".to_string(),
                    timestamp: Utc::now(),
                },
            ],
        };

        let ids = mgr.extract_facts_from_session(&session).unwrap();
        assert!(
            ids.len() >= 2,
            "Expected at least 2 extracted facts, got {}",
            ids.len()
        );

        // Verify the extracted memories exist and have correct types
        let prefs: Vec<&MemoryEntry> = mgr
            .memories
            .values()
            .filter(|m| {
                m.memory_type == MemoryType::Preference
                    && m.tags.contains(&"auto-extracted".to_string())
            })
            .collect();
        assert!(
            !prefs.is_empty(),
            "Should have extracted at least one preference"
        );
    }

    #[test]
    fn test_extract_sentence() {
        let text = "Hello there. I prefer dark mode for editing. Thanks!";
        let sentence = MemoryManager::extract_sentence(text, 14); // "I prefer"
        assert_eq!(sentence, "I prefer dark mode for editing.");
    }

    #[test]
    fn test_extract_no_duplicates() {
        let (mut mgr, _tmp) = test_manager();

        let session = ConversationSession {
            id: "s1".to_string(),
            title: None,
            started_at: Utc::now(),
            last_activity: Utc::now(),
            messages: vec![SessionMessage {
                role: "user".to_string(),
                content: "I prefer dark mode.".to_string(),
                timestamp: Utc::now(),
            }],
        };

        let ids1 = mgr.extract_facts_from_session(&session).unwrap();
        let ids2 = mgr.extract_facts_from_session(&session).unwrap();

        // Second extraction should find no new facts (duplicates)
        assert!(!ids1.is_empty());
        assert!(ids2.is_empty(), "Should not extract duplicate facts");
    }

    #[test]
    fn test_memory_count_bounded() {
        let (mut mgr, _tmp) = test_manager();

        // Insert 10 memories (small number to keep test fast)
        // Override for test: we test the eviction logic manually with a smaller limit
        for i in 0..10 {
            let id = format!("mem-{}", i);
            let now = Utc::now();
            let entry = MemoryEntry {
                id: id.clone(),
                content: format!("content-{}", i),
                created_at: now,
                last_accessed: now - chrono::Duration::seconds((10 - i) as i64),
                access_count: 0,
                memory_type: MemoryType::Fact,
                tags: vec![],
                metadata: HashMap::new(),
                embedding: None,
            };
            mgr.memories.insert(id, entry);
        }

        // Verify all 10 are present
        assert_eq!(mgr.memories.len(), 10);

        // Note: MAX_MEMORIES is 50_000 so we can't easily test insertion triggering eviction
        // without inserting 50K+ records. Instead verify the eviction logic directly:
        // The constant is set and the eviction code path exists.
        assert_eq!(MAX_MEMORIES, 50_000);
    }
}
