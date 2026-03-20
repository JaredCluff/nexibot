//! Enhanced memory storage with SQLite FTS5 and hybrid search.
//!
//! This module provides four components:
//!
//! - **`sqlite_store`**: A persistent memory store backed by SQLite with FTS5
//!   full-text search. O(log n) ranked text search via BM25 scoring. WAL mode
//!   for concurrent reads. Automatic migration from legacy JSON files.
//!
//! - **`hybrid_search`**: Combines text-based (FTS) scores with vector cosine similarity
//!   scores using Reciprocal Rank Fusion (RRF), then applies MMR (Maximal Marginal
//!   Relevance) re-ranking to reduce redundancy in search results.
//!
//! - **`query_expander`**: Generates query variants via stop-word removal, compound term
//!   splitting, synonym expansion, and memory-type-aware term expansion.
//!
//! - **`reranker`**: Boosts search results based on exact phrase match, tag match,
//!   access frequency, memory type priority, and recency.

pub mod hybrid_search;
pub mod query_expander;
pub mod reranker;
pub mod sqlite_store;
