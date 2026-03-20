//! Semantic search index for MCP tools.
//!
//! Instead of sending ALL MCP tools to the LLM on every message (wasting context),
//! this module indexes tool descriptions as embeddings and returns only the most
//! relevant tools for a given user query.

use std::collections::HashMap;

use tracing::{debug, info, warn};

use crate::config::ToolSearchConfig;
use crate::embeddings;

/// A single indexed tool with its pre-computed embedding.
struct IndexedTool {
    /// Prefixed tool name (e.g., "server__toolname")
    name: String,
    /// Pre-computed embedding of "{name}: {description}"
    embedding: Vec<f32>,
    /// Full tool JSON definition for the LLM
    definition: serde_json::Value,
}

/// Semantic search index over MCP tool descriptions.
pub struct ToolSearchIndex {
    tools: HashMap<String, IndexedTool>,
    config: ToolSearchConfig,
}

impl ToolSearchIndex {
    /// Create a new empty index with the given config.
    pub fn new(config: ToolSearchConfig) -> Self {
        Self {
            tools: HashMap::new(),
            config,
        }
    }

    /// Update the search config (e.g., after hot-reload).
    pub fn update_config(&mut self, config: ToolSearchConfig) {
        self.config = config;
    }

    /// Index a batch of tool definitions. Each tool JSON should have
    /// `name` and `description` fields. Pre-computes embeddings.
    pub fn index_tools(&mut self, tools: &[serde_json::Value]) {
        for tool in tools {
            let name = match tool.get("name").and_then(|n| n.as_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let description = tool
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("");

            let text = format!("{}: {}", name, description);
            match embeddings::embed_text(&text) {
                Ok(embedding) => {
                    self.tools.insert(
                        name.clone(),
                        IndexedTool {
                            name,
                            embedding,
                            definition: tool.clone(),
                        },
                    );
                }
                Err(e) => {
                    warn!(
                        "[TOOL_SEARCH] Failed to embed tool '{}': {} — including without index",
                        name, e
                    );
                    // Store with empty embedding so get_tool_by_name still works
                    self.tools.insert(
                        name.clone(),
                        IndexedTool {
                            name,
                            embedding: vec![],
                            definition: tool.clone(),
                        },
                    );
                }
            }
        }
        info!(
            "[TOOL_SEARCH] Indexed {} tools ({} with embeddings)",
            self.tools.len(),
            self.tools
                .values()
                .filter(|t| !t.embedding.is_empty())
                .count()
        );
    }

    /// Search for the most relevant tools given a user query.
    /// Returns up to `top_k` tool definitions sorted by relevance.
    ///
    /// If search is disabled or embedding fails, returns ALL tools (graceful degradation).
    pub fn search(&self, query: &str) -> Vec<serde_json::Value> {
        if !self.config.enabled || self.tools.is_empty() {
            return self.all_tools();
        }

        let query_embedding = match embeddings::embed_text(query) {
            Ok(emb) => emb,
            Err(e) => {
                warn!(
                    "[TOOL_SEARCH] Failed to embed query: {} — returning all tools",
                    e
                );
                return self.all_tools();
            }
        };

        let mut scored: Vec<(&str, f32, &serde_json::Value)> = self
            .tools
            .values()
            .filter_map(|tool| {
                if tool.embedding.is_empty() {
                    // Tools without embeddings always included
                    Some((tool.name.as_str(), f32::MAX, &tool.definition))
                } else {
                    let sim = embeddings::cosine_similarity(&query_embedding, &tool.embedding);
                    if sim >= self.config.similarity_threshold as f32 {
                        Some((tool.name.as_str(), sim, &tool.definition))
                    } else {
                        None
                    }
                }
            })
            .collect();

        // Sort by similarity descending (MAX first for tools without embeddings)
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Take top-K
        let results: Vec<serde_json::Value> = scored
            .into_iter()
            .take(self.config.top_k)
            .map(|(name, sim, def)| {
                if sim < f32::MAX {
                    debug!(
                        "[TOOL_SEARCH] Selected tool '{}' (similarity: {:.3})",
                        name, sim
                    );
                }
                def.clone()
            })
            .collect();

        info!(
            "[TOOL_SEARCH] Query matched {} of {} MCP tools (top_k={}, threshold={:.2})",
            results.len(),
            self.tools.len(),
            self.config.top_k,
            self.config.similarity_threshold
        );

        results
    }

    /// Look up a tool by its prefixed name (exact match).
    /// Used for dynamic tool addition when the LLM requests a tool
    /// that wasn't in the filtered set.
    pub fn get_tool_by_name(&self, name: &str) -> Option<serde_json::Value> {
        self.tools.get(name).map(|t| t.definition.clone())
    }

    /// Remove all tools belonging to a specific MCP server prefix.
    #[allow(dead_code)]
    pub fn remove_server_tools(&mut self, server_prefix: &str) {
        let prefix_with_separator = format!("{}_", server_prefix);
        self.tools
            .retain(|name, _| !name.starts_with(&prefix_with_separator));
    }

    /// Remove all indexed tools.
    pub fn clear(&mut self) {
        self.tools.clear();
    }

    /// Return all tool definitions (used when search is disabled).
    fn all_tools(&self) -> Vec<serde_json::Value> {
        self.tools.values().map(|t| t.definition.clone()).collect()
    }

    /// Number of indexed tools.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the index is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ToolSearchConfig;

    fn test_config() -> ToolSearchConfig {
        ToolSearchConfig {
            enabled: true,
            top_k: 3,
            similarity_threshold: 0.0, // accept everything for tests
        }
    }

    fn make_tool(name: &str, desc: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "description": desc,
            "input_schema": { "type": "object", "properties": {} }
        })
    }

    #[test]
    fn test_index_and_lookup() {
        let mut index = ToolSearchIndex::new(test_config());
        let tools = vec![
            make_tool("fs_read", "Read a file from the filesystem"),
            make_tool("fs_write", "Write content to a file on disk"),
            make_tool("web_search", "Search the web using a query"),
        ];
        index.index_tools(&tools);
        assert_eq!(index.len(), 3);

        // Exact lookup
        assert!(index.get_tool_by_name("fs_read").is_some());
        assert!(index.get_tool_by_name("nonexistent").is_none());
    }

    #[test]
    fn test_remove_server_tools() {
        let mut index = ToolSearchIndex::new(test_config());
        let tools = vec![
            make_tool("serverA_read", "Read files"),
            make_tool("serverA_write", "Write files"),
            make_tool("serverB_search", "Search things"),
        ];
        index.index_tools(&tools);
        assert_eq!(index.len(), 3);

        index.remove_server_tools("serverA");
        assert_eq!(index.len(), 1);
        assert!(index.get_tool_by_name("serverB_search").is_some());
    }

    #[test]
    fn test_clear() {
        let mut index = ToolSearchIndex::new(test_config());
        index.index_tools(&[make_tool("test", "A test tool")]);
        assert_eq!(index.len(), 1);
        index.clear();
        assert!(index.is_empty());
    }

    #[test]
    fn test_disabled_returns_all() {
        let config = ToolSearchConfig {
            enabled: false,
            top_k: 1,
            similarity_threshold: 0.99,
        };
        let mut index = ToolSearchIndex::new(config);
        let tools = vec![
            make_tool("a", "Tool A"),
            make_tool("b", "Tool B"),
            make_tool("c", "Tool C"),
        ];
        index.index_tools(&tools);

        // Even with top_k=1 and high threshold, disabled search returns all
        let results = index.search("anything");
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_search_respects_top_k() {
        let config = ToolSearchConfig {
            enabled: true,
            top_k: 2,
            similarity_threshold: 0.0, // accept all
        };
        let mut index = ToolSearchIndex::new(config);
        let tools = vec![
            make_tool("a", "Tool A"),
            make_tool("b", "Tool B"),
            make_tool("c", "Tool C"),
            make_tool("d", "Tool D"),
        ];
        index.index_tools(&tools);

        let results = index.search("some query");
        assert!(
            results.len() <= 2,
            "Should respect top_k=2, got {}",
            results.len()
        );
    }

    #[test]
    fn test_search_semantic_relevance() {
        // This test verifies that semantically relevant tools rank higher.
        // It requires the embedding model to be available.
        if !embeddings::is_model_available() {
            eprintln!("Skipping semantic test — embedding model not available");
            return;
        }

        let config = ToolSearchConfig {
            enabled: true,
            top_k: 2,
            similarity_threshold: 0.0,
        };
        let mut index = ToolSearchIndex::new(config);
        let tools = vec![
            make_tool(
                "filesystem_read",
                "Read a file from the local filesystem and return its contents",
            ),
            make_tool(
                "web_search",
                "Search the internet using a search engine query",
            ),
            make_tool("database_query", "Execute a SQL query against the database"),
        ];
        index.index_tools(&tools);

        let results = index.search("read my home directory files");
        assert!(!results.is_empty());
        // The filesystem tool should rank first for a filesystem-related query
        let first_name = results[0]
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("");
        assert_eq!(
            first_name, "filesystem_read",
            "Filesystem tool should rank first for file-related query, got '{}'",
            first_name
        );
    }

    #[test]
    fn test_update_config() {
        let mut index = ToolSearchIndex::new(test_config());
        index.index_tools(&[make_tool("a", "Tool A")]);

        // Change to disabled
        index.update_config(ToolSearchConfig {
            enabled: false,
            ..test_config()
        });
        let results = index.search("anything");
        assert_eq!(results.len(), 1, "Disabled search should return all");
    }
}
