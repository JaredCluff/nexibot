//! Reranker for memory search results — boosts results based on matching signals.
//!
//! Ported from the CLI agent's `src/retrieval/reranker.rs` with memory-specific
//! adaptations (access_count boost, memory_type priority, tag matching).

use chrono::Utc;

use super::hybrid_search::HybridSearchResult;

/// Reranks hybrid search results by applying boosting signals specific to
/// memory retrieval: exact phrase match, tag match, access frequency, and
/// memory type priority.
pub struct MemoryReranker;

impl MemoryReranker {
    pub fn new() -> Self {
        Self
    }

    /// Rerank results by applying boosting signals on top of existing combined scores.
    ///
    /// Boosts applied:
    /// - **Exact phrase match** in content: +0.15 to combined_score
    /// - **Tag keyword match**: +0.1 * (matched_tags / query_words)
    /// - **Access count boost**: +0.05 if accessed 5+ times, +0.02 if 2+ times
    /// - **Memory type priority**: +0.05 for Preference/Fact, +0.02 for Context
    /// - **Recency boost**: +0.05 if created < 7 days ago, +0.02 if < 30 days
    pub fn rerank(
        &self,
        results: Vec<HybridSearchResult>,
        query: &str,
        tags_map: &std::collections::HashMap<String, Vec<String>>,
        memory_types: &std::collections::HashMap<String, String>,
        access_counts: &std::collections::HashMap<String, u32>,
        created_at_map: &std::collections::HashMap<String, chrono::DateTime<Utc>>,
    ) -> Vec<HybridSearchResult> {
        let query_lower = query.to_lowercase();
        let query_words: Vec<String> = query_lower
            .split_whitespace()
            .filter(|w| w.len() > 2)
            .map(|w| w.to_string())
            .collect();

        let mut scored: Vec<(f64, HybridSearchResult)> = results
            .into_iter()
            .map(|result| {
                let mut boost = 0.0f64;

                // Exact phrase match in content
                if result.content.to_lowercase().contains(&query_lower) {
                    boost += 0.15;
                }

                // Tag keyword match
                if let Some(tags) = tags_map.get(&result.record_id) {
                    if !query_words.is_empty() {
                        let tags_lower: Vec<String> =
                            tags.iter().map(|t| t.to_lowercase()).collect();
                        let tag_matches = query_words
                            .iter()
                            .filter(|w| tags_lower.iter().any(|t| t.contains(w.as_str())))
                            .count();
                        boost += 0.1 * (tag_matches as f64 / query_words.len() as f64);
                    }
                }

                // Access count boost (frequently accessed = likely relevant)
                if let Some(&count) = access_counts.get(&result.record_id) {
                    if count >= 5 {
                        boost += 0.05;
                    } else if count >= 2 {
                        boost += 0.02;
                    }
                }

                // Memory type priority (preferences and facts are usually more useful)
                if let Some(mtype) = memory_types.get(&result.record_id) {
                    match mtype.as_str() {
                        "preference" | "fact" => boost += 0.05,
                        "context" => boost += 0.02,
                        _ => {} // conversation gets no boost
                    }
                }

                // Recency boost
                if let Some(created) = created_at_map.get(&result.record_id) {
                    let age_days = (Utc::now() - *created).num_days();
                    if age_days < 7 {
                        boost += 0.05;
                    } else if age_days < 30 {
                        boost += 0.02;
                    }
                }

                let final_score = result.combined_score + boost;
                (final_score, result)
            })
            .collect();

        // Sort by boosted score descending
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        scored
            .into_iter()
            .map(|(score, mut result)| {
                result.combined_score = score;
                result
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use std::collections::HashMap;

    fn make_result(id: &str, content: &str, score: f64) -> HybridSearchResult {
        HybridSearchResult {
            record_id: id.to_string(),
            content: content.to_string(),
            text_score: score,
            vector_score: score,
            combined_score: score,
            temporal_score: 1.0,
        }
    }

    #[test]
    fn test_exact_phrase_boost() {
        let reranker = MemoryReranker::new();
        let results = vec![
            make_result("a", "learn rust programming", 0.5),
            make_result("b", "rust is great for systems", 0.5),
        ];

        let tags = HashMap::new();
        let types = HashMap::new();
        let counts = HashMap::new();
        let created = HashMap::new();

        let reranked = reranker.rerank(
            results,
            "rust programming",
            &tags,
            &types,
            &counts,
            &created,
        );
        // "a" contains exact phrase "rust programming"
        assert_eq!(reranked[0].record_id, "a");
        assert!(reranked[0].combined_score > reranked[1].combined_score);
    }

    #[test]
    fn test_tag_match_boost() {
        let reranker = MemoryReranker::new();
        let results = vec![
            make_result("a", "some content", 0.5),
            make_result("b", "other content", 0.5),
        ];

        let tags: HashMap<String, Vec<String>> = [
            (
                "a".to_string(),
                vec!["programming".to_string(), "rust".to_string()],
            ),
            ("b".to_string(), vec!["cooking".to_string()]),
        ]
        .into();
        let types = HashMap::new();
        let counts = HashMap::new();
        let created = HashMap::new();

        let reranked = reranker.rerank(
            results,
            "rust programming guide",
            &tags,
            &types,
            &counts,
            &created,
        );
        assert_eq!(reranked[0].record_id, "a");
    }

    #[test]
    fn test_memory_type_priority() {
        let reranker = MemoryReranker::new();
        let results = vec![
            make_result("a", "content", 0.5),
            make_result("b", "content", 0.5),
        ];

        let tags = HashMap::new();
        let types: HashMap<String, String> = [
            ("a".to_string(), "conversation".to_string()),
            ("b".to_string(), "preference".to_string()),
        ]
        .into();
        let counts = HashMap::new();
        let created = HashMap::new();

        let reranked = reranker.rerank(results, "query", &tags, &types, &counts, &created);
        // Preference gets +0.05, conversation gets 0
        assert_eq!(reranked[0].record_id, "b");
    }

    #[test]
    fn test_access_count_boost() {
        let reranker = MemoryReranker::new();
        let results = vec![
            make_result("a", "content", 0.5),
            make_result("b", "content", 0.5),
        ];

        let tags = HashMap::new();
        let types = HashMap::new();
        let counts: HashMap<String, u32> = [("a".to_string(), 0), ("b".to_string(), 10)].into();
        let created = HashMap::new();

        let reranked = reranker.rerank(results, "query", &tags, &types, &counts, &created);
        assert_eq!(reranked[0].record_id, "b");
    }

    #[test]
    fn test_recency_boost() {
        let reranker = MemoryReranker::new();
        let results = vec![
            make_result("a", "content", 0.5),
            make_result("b", "content", 0.5),
        ];

        let tags = HashMap::new();
        let types = HashMap::new();
        let counts = HashMap::new();
        let now = Utc::now();
        let created: HashMap<String, chrono::DateTime<Utc>> = [
            ("a".to_string(), now - Duration::days(60)),
            ("b".to_string(), now - Duration::days(3)),
        ]
        .into();

        let reranked = reranker.rerank(results, "query", &tags, &types, &counts, &created);
        // "b" is recent (< 7 days), gets +0.05
        assert_eq!(reranked[0].record_id, "b");
    }

    #[test]
    fn test_preserves_relative_order_without_boosts() {
        let reranker = MemoryReranker::new();
        let results = vec![
            make_result("a", "content", 0.9),
            make_result("b", "content", 0.3),
        ];

        let tags = HashMap::new();
        let types = HashMap::new();
        let counts = HashMap::new();
        let created = HashMap::new();

        let reranked = reranker.rerank(
            results,
            "unrelated query xyz",
            &tags,
            &types,
            &counts,
            &created,
        );
        assert_eq!(reranked[0].record_id, "a");
    }
}
