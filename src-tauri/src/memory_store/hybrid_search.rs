//! Hybrid search combining FTS5 text scores with vector cosine similarity.
//! Uses Reciprocal Rank Fusion (RRF) for robust score fusion and MMR
//! (Maximal Marginal Relevance) re-ranking to reduce redundancy.
//!
//! # How it works
//!
//! 1. **Text search** (FTS5 / word matching) produces `(record_id, raw_score)` pairs.
//! 2. **Vector search** (cosine similarity on embeddings) produces `(record_id, raw_score)`.
//! 3. Results are ranked independently, then merged using RRF:
//!    `rrf_score = text_weight / (k + text_rank) + vector_weight / (k + vector_rank)`
//! 4. A temporal decay factor optionally boosts recent memories.
//! 5. **MMR re-ranking** iteratively selects results that are relevant to the query
//!    while being diverse (dissimilar to already-selected results).
//!
//! # References
//!
//! - Cormack, Clarke & Buettcher (2009). "Reciprocal Rank Fusion outperforms
//!   Condorcet and individual Rank Learning Methods."
//! - Carbonell & Goldstein (1998). "The Use of MMR, Diversity-Based Reranking for
//!   Reordering Documents and Producing Summaries."

use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// RRF tuning parameter. Controls how much rank position matters.
/// Higher k values flatten score differences between ranks.
/// k=60 is the standard value from Cormack et al. (2009).
const RRF_K: f64 = 60.0;

/// Configuration for hybrid search.
#[derive(Debug, Clone)]
pub struct HybridSearchOptions {
    /// Weight multiplier for text (FTS) RRF scores. Default 1.0.
    pub text_weight: f64,
    /// Weight multiplier for vector similarity RRF scores. Default 1.0.
    pub vector_weight: f64,
    /// Exponential decay factor per day for temporal scoring.
    /// Higher values penalize older memories more aggressively.
    pub temporal_decay_factor: f64,
    /// Maximum number of results to return after re-ranking.
    pub max_results: usize,
    /// Lambda parameter for MMR. Higher values (closer to 1.0) prioritize
    /// relevance; lower values prioritize diversity.
    pub mmr_lambda: f64,
}

impl Default for HybridSearchOptions {
    fn default() -> Self {
        Self {
            text_weight: 1.0,
            vector_weight: 1.0,
            temporal_decay_factor: 0.01,
            max_results: 10,
            mmr_lambda: 0.7,
        }
    }
}

/// A single search result with breakdown of component scores.
#[derive(Debug, Clone)]
pub struct HybridSearchResult {
    /// The unique record ID.
    pub record_id: String,
    /// The memory content (for convenience -- avoids a second lookup).
    pub content: String,
    /// Normalized text/FTS score in [0, 1].
    #[allow(dead_code)]
    pub text_score: f64,
    /// Normalized vector similarity score in [0, 1].
    #[allow(dead_code)]
    pub vector_score: f64,
    /// Weighted combination of text, vector, and temporal scores.
    pub combined_score: f64,
    /// Temporal recency score in (0, 1].
    #[allow(dead_code)]
    pub temporal_score: f64,
}

// ---------------------------------------------------------------------------
// Scoring helpers
// ---------------------------------------------------------------------------

/// Compute cosine similarity between two embedding vectors.
///
/// Returns a value in [-1, 1]. If either vector has zero magnitude, returns 0.0.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f64 = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| (*x as f64) * (*y as f64))
        .sum();

    let norm_a: f64 = a
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt();
    let norm_b: f64 = b
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt();

    if norm_a > 0.0 && norm_b > 0.0 {
        dot / (norm_a * norm_b)
    } else {
        0.0
    }
}

/// Exponential temporal decay based on the age of a memory in days.
///
/// Returns a value in (0, 1] where 1.0 means "just created" and values
/// decay exponentially toward 0 for older memories.
///
/// Formula: `exp(-decay_factor * age_in_days)`
pub fn temporal_decay(created_at: DateTime<Utc>, decay_factor: f64) -> f64 {
    let age_days = (Utc::now() - created_at).num_seconds().max(0) as f64 / 86_400.0;
    (-decay_factor * age_days).exp()
}

// ---------------------------------------------------------------------------
// MMR Re-ranking
// ---------------------------------------------------------------------------

/// Apply Maximal Marginal Relevance re-ranking to search results.
///
/// MMR balances relevance (how well a result matches the query) against
/// diversity (how different it is from already-selected results). The
/// `lambda` parameter controls this trade-off:
///
/// - `lambda = 1.0` => pure relevance ranking (no diversity bonus).
/// - `lambda = 0.0` => pure diversity ranking (no relevance).
/// - `lambda = 0.7` (default) => 70% relevance, 30% diversity.
///
/// `embeddings` maps `record_id -> embedding vector` and is used to compute
/// inter-result similarity for the diversity penalty. Results without embeddings
/// receive no diversity penalty (treated as maximally diverse).
pub fn mmr_rerank(
    results: &mut Vec<HybridSearchResult>,
    lambda: f64,
    embeddings: &HashMap<String, Vec<f32>>,
) {
    if results.len() <= 1 {
        return;
    }

    let n = results.len();
    let mut selected: Vec<HybridSearchResult> = Vec::with_capacity(n);
    let mut remaining: Vec<HybridSearchResult> = std::mem::take(results);

    // Greedily select the highest-scoring result first
    remaining.sort_by(|a, b| {
        b.combined_score
            .partial_cmp(&a.combined_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    selected.push(remaining.remove(0));

    while !remaining.is_empty() {
        let mut best_idx = 0;
        let mut best_mmr = f64::NEG_INFINITY;

        for (i, candidate) in remaining.iter().enumerate() {
            let relevance = candidate.combined_score;

            // Max similarity to any already-selected result
            let max_sim = selected
                .iter()
                .map(|sel| {
                    match (
                        embeddings.get(&candidate.record_id),
                        embeddings.get(&sel.record_id),
                    ) {
                        (Some(emb_c), Some(emb_s)) => cosine_similarity(emb_c, emb_s),
                        _ => 0.0, // No embedding => assume maximally diverse
                    }
                })
                .fold(f64::NEG_INFINITY, f64::max);

            let mmr_score = lambda * relevance - (1.0 - lambda) * max_sim;

            if mmr_score > best_mmr {
                best_mmr = mmr_score;
                best_idx = i;
            }
        }

        selected.push(remaining.remove(best_idx));
    }

    *results = selected;
}

// ---------------------------------------------------------------------------
// Hybrid search combiner
// ---------------------------------------------------------------------------

/// Combine text search results and vector search results into a unified,
/// scored, and ranked list using Reciprocal Rank Fusion (RRF).
///
/// Both inputs are lists of `(record_id, raw_score)`. Results are ranked
/// independently by raw score, then merged using RRF: for each record,
/// `rrf_score = sum(weight_i / (k + rank_i))` across the search modalities
/// it appears in. This is more robust than min-max normalization because
/// it depends only on rank positions, not raw score distributions.
///
/// `content_map` provides text content for each record ID.
/// `timestamps` enables temporal recency decay as a multiplier on the RRF score.
pub fn hybrid_search(
    text_results: Vec<(String, f64)>,
    vector_results: Vec<(String, f64)>,
    options: &HybridSearchOptions,
    content_map: &HashMap<String, String>,
    timestamps: &HashMap<String, DateTime<Utc>>,
) -> Vec<HybridSearchResult> {
    // Sort text results by raw score descending to establish ranks
    let mut text_ranked = text_results;
    text_ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Sort vector results by raw score descending to establish ranks
    let mut vector_ranked = vector_results;
    vector_ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Compute RRF scores: score = weight / (k + rank + 1)
    // Using rank+1 so the top result gets 1/(k+1) not 1/k
    let mut rrf_scores: HashMap<String, (f64, f64, f64)> = HashMap::new(); // (total_rrf, text_rrf, vector_rrf)

    for (rank, (id, _raw_score)) in text_ranked.iter().enumerate() {
        let rrf = options.text_weight / (RRF_K + rank as f64 + 1.0);
        let entry = rrf_scores.entry(id.clone()).or_insert((0.0, 0.0, 0.0));
        entry.0 += rrf;
        entry.1 = rrf;
    }

    for (rank, (id, _raw_score)) in vector_ranked.iter().enumerate() {
        let rrf = options.vector_weight / (RRF_K + rank as f64 + 1.0);
        let entry = rrf_scores.entry(id.clone()).or_insert((0.0, 0.0, 0.0));
        entry.0 += rrf;
        entry.2 = rrf;
    }

    // Build results with temporal decay
    let mut results: Vec<HybridSearchResult> = rrf_scores
        .into_iter()
        .map(|(id, (total_rrf, text_rrf, vector_rrf))| {
            let temporal_score = timestamps
                .get(&id)
                .map(|ts| temporal_decay(*ts, options.temporal_decay_factor))
                .unwrap_or(1.0);

            let combined = total_rrf * temporal_score;
            let content = content_map.get(&id).cloned().unwrap_or_default();

            HybridSearchResult {
                record_id: id,
                content,
                text_score: text_rrf,
                vector_score: vector_rrf,
                combined_score: combined,
                temporal_score,
            }
        })
        .collect();

    // Sort by combined score descending
    results.sort_by(|a, b| {
        b.combined_score
            .partial_cmp(&a.combined_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Truncate to max_results
    results.truncate(options.max_results);

    results
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    // -- cosine_similarity tests --

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0f32, 0.0, 0.0];
        let sim = cosine_similarity(&a, &a);
        assert!((sim - 1.0).abs() < 1e-6, "Expected 1.0, got {}", sim);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![0.0f32, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6, "Expected ~0.0, got {}", sim);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0f32, 0.0];
        let b = vec![-1.0f32, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim + 1.0).abs() < 1e-6, "Expected -1.0, got {}", sim);
    }

    #[test]
    fn test_cosine_similarity_empty() {
        let a: Vec<f32> = vec![];
        let b: Vec<f32> = vec![];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_cosine_similarity_mismatched_lengths() {
        let a = vec![1.0f32, 0.0];
        let b = vec![1.0f32, 0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = vec![0.0f32, 0.0, 0.0];
        let b = vec![1.0f32, 2.0, 3.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    // -- temporal_decay tests --

    #[test]
    fn test_temporal_decay_now() {
        let score = temporal_decay(Utc::now(), 0.01);
        // Just created => should be very close to 1.0
        assert!(
            (score - 1.0).abs() < 0.001,
            "Expected ~1.0 for now, got {}",
            score
        );
    }

    #[test]
    fn test_temporal_decay_old() {
        let one_year_ago = Utc::now() - Duration::days(365);
        let score = temporal_decay(one_year_ago, 0.01);
        // exp(-0.01 * 365) = exp(-3.65) ~ 0.026
        assert!(
            score < 0.1,
            "Expected small value for 1-year-old memory, got {}",
            score
        );
        assert!(score > 0.0, "Score should be positive, got {}", score);
    }

    #[test]
    fn test_temporal_decay_zero_factor() {
        let old = Utc::now() - Duration::days(1000);
        let score = temporal_decay(old, 0.0);
        // decay_factor = 0 => exp(0) = 1.0, no decay at all
        assert!(
            (score - 1.0).abs() < 1e-6,
            "Expected 1.0 with zero decay, got {}",
            score
        );
    }

    #[test]
    fn test_temporal_decay_high_factor() {
        let yesterday = Utc::now() - Duration::days(1);
        let score = temporal_decay(yesterday, 1.0);
        // exp(-1.0 * 1) = exp(-1) ~ 0.368
        assert!(
            (score - 0.368).abs() < 0.01,
            "Expected ~0.368, got {}",
            score
        );
    }

    // -- RRF hybrid_search tests --

    #[test]
    fn test_rrf_scoring_basic() {
        // Verify RRF formula: score = weight / (k + rank + 1)
        // With k=60, rank 0: score = 1.0 / 61 ≈ 0.01639
        let text_results = vec![("a".to_string(), 0.9)];
        let content_map: HashMap<String, String> =
            [("a".to_string(), "Content A".to_string())].into();
        let timestamps: HashMap<String, DateTime<Utc>> = HashMap::new();

        let options = HybridSearchOptions {
            text_weight: 1.0,
            vector_weight: 1.0,
            temporal_decay_factor: 0.0,
            max_results: 10,
            mmr_lambda: 0.7,
        };

        let results = hybrid_search(text_results, vec![], &options, &content_map, &timestamps);
        assert_eq!(results.len(), 1);
        let expected = 1.0 / 61.0; // 1/(60+0+1)
        assert!(
            (results[0].combined_score - expected).abs() < 1e-6,
            "Expected {}, got {}",
            expected,
            results[0].combined_score
        );
    }

    #[test]
    fn test_hybrid_search_text_only() {
        let text_results = vec![("a".to_string(), 0.9), ("b".to_string(), 0.5)];
        let vector_results: Vec<(String, f64)> = vec![];

        let content_map: HashMap<String, String> = [
            ("a".to_string(), "Content A".to_string()),
            ("b".to_string(), "Content B".to_string()),
        ]
        .into();

        let timestamps: HashMap<String, DateTime<Utc>> = HashMap::new();

        let options = HybridSearchOptions {
            text_weight: 1.0,
            vector_weight: 1.0,
            temporal_decay_factor: 0.0,
            max_results: 10,
            mmr_lambda: 0.7,
        };

        let results = hybrid_search(
            text_results,
            vector_results,
            &options,
            &content_map,
            &timestamps,
        );
        assert_eq!(results.len(), 2);
        // "a" has higher raw score => rank 0, "b" gets rank 1
        assert_eq!(results[0].record_id, "a");
        assert!(results[0].combined_score > results[1].combined_score);
    }

    #[test]
    fn test_hybrid_search_vector_only() {
        let text_results: Vec<(String, f64)> = vec![];
        let vector_results = vec![("x".to_string(), 0.95), ("y".to_string(), 0.60)];

        let content_map: HashMap<String, String> = [
            ("x".to_string(), "Content X".to_string()),
            ("y".to_string(), "Content Y".to_string()),
        ]
        .into();

        let timestamps: HashMap<String, DateTime<Utc>> = HashMap::new();

        let options = HybridSearchOptions {
            text_weight: 1.0,
            vector_weight: 1.0,
            temporal_decay_factor: 0.0,
            max_results: 10,
            mmr_lambda: 0.7,
        };

        let results = hybrid_search(
            text_results,
            vector_results,
            &options,
            &content_map,
            &timestamps,
        );
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].record_id, "x");
    }

    #[test]
    fn test_hybrid_search_combined_rrf() {
        // Record "a" ranks 1st in text, 2nd in vector
        // Record "b" ranks 2nd in text, 1st in vector
        // With equal weights, both get the same combined RRF score
        let text_results = vec![("a".to_string(), 1.0), ("b".to_string(), 0.2)];
        let vector_results = vec![("b".to_string(), 1.0), ("a".to_string(), 0.2)];

        let content_map: HashMap<String, String> = [
            ("a".to_string(), "Content A".to_string()),
            ("b".to_string(), "Content B".to_string()),
        ]
        .into();

        let timestamps: HashMap<String, DateTime<Utc>> = HashMap::new();

        let options = HybridSearchOptions {
            text_weight: 1.0,
            vector_weight: 1.0,
            temporal_decay_factor: 0.0,
            max_results: 10,
            mmr_lambda: 0.7,
        };

        let results = hybrid_search(
            text_results,
            vector_results,
            &options,
            &content_map,
            &timestamps,
        );
        assert_eq!(results.len(), 2);

        // a: text_rrf = 1/(60+0+1) = 1/61, vector_rrf = 1/(60+1+1) = 1/62
        // b: text_rrf = 1/(60+1+1) = 1/62, vector_rrf = 1/(60+0+1) = 1/61
        // Both get 1/61 + 1/62 => equal scores
        let diff = (results[0].combined_score - results[1].combined_score).abs();
        assert!(
            diff < 1e-10,
            "Expected identical RRF scores for symmetric ranks, got diff = {}",
            diff
        );
    }

    #[test]
    fn test_hybrid_search_rrf_both_modalities_beat_single() {
        // Record "a" appears in both text and vector results
        // Record "b" appears only in text results
        // "a" should score higher because it gets RRF from two sources
        let text_results = vec![("b".to_string(), 1.0), ("a".to_string(), 0.8)];
        let vector_results = vec![("a".to_string(), 0.9)];

        let content_map: HashMap<String, String> = [
            ("a".to_string(), "Content A".to_string()),
            ("b".to_string(), "Content B".to_string()),
        ]
        .into();

        let timestamps: HashMap<String, DateTime<Utc>> = HashMap::new();

        let options = HybridSearchOptions {
            text_weight: 1.0,
            vector_weight: 1.0,
            temporal_decay_factor: 0.0,
            max_results: 10,
            mmr_lambda: 0.7,
        };

        let results = hybrid_search(
            text_results,
            vector_results,
            &options,
            &content_map,
            &timestamps,
        );
        assert_eq!(results.len(), 2);
        // "a" gets RRF from both modalities, "b" only from text
        assert_eq!(results[0].record_id, "a");
        assert!(results[0].combined_score > results[1].combined_score);
    }

    #[test]
    fn test_hybrid_search_respects_max_results() {
        let text_results: Vec<(String, f64)> = (0..20)
            .map(|i| (format!("r{}", i), i as f64 / 20.0))
            .collect();

        let content_map: HashMap<String, String> = text_results
            .iter()
            .map(|(id, _)| (id.clone(), format!("Content {}", id)))
            .collect();

        let timestamps: HashMap<String, DateTime<Utc>> = HashMap::new();

        let options = HybridSearchOptions {
            max_results: 5,
            ..Default::default()
        };

        let results = hybrid_search(text_results, vec![], &options, &content_map, &timestamps);
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn test_hybrid_search_with_temporal_decay() {
        let now = Utc::now();
        let old = now - Duration::days(100);

        // Both at same rank position in text, so RRF base scores are equal
        // Temporal decay differentiates them
        let text_results = vec![("new".to_string(), 0.8), ("old".to_string(), 0.8)];

        let content_map: HashMap<String, String> = [
            ("new".to_string(), "New content".to_string()),
            ("old".to_string(), "Old content".to_string()),
        ]
        .into();

        let timestamps: HashMap<String, DateTime<Utc>> =
            [("new".to_string(), now), ("old".to_string(), old)].into();

        let options = HybridSearchOptions {
            text_weight: 1.0,
            vector_weight: 1.0,
            temporal_decay_factor: 0.05,
            max_results: 10,
            mmr_lambda: 0.7,
        };

        let results = hybrid_search(text_results, vec![], &options, &content_map, &timestamps);
        assert_eq!(results.len(), 2);

        // The newer result should score higher due to temporal decay
        assert_eq!(results[0].record_id, "new");
        assert!(results[0].combined_score > results[1].combined_score);
        assert!(results[0].temporal_score > results[1].temporal_score);
    }

    // -- MMR re-ranking tests --

    #[test]
    fn test_mmr_rerank_single_result() {
        let mut results = vec![HybridSearchResult {
            record_id: "a".to_string(),
            content: "A".to_string(),
            text_score: 1.0,
            vector_score: 1.0,
            combined_score: 1.0,
            temporal_score: 1.0,
        }];

        let embeddings: HashMap<String, Vec<f32>> = HashMap::new();
        mmr_rerank(&mut results, 0.7, &embeddings);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].record_id, "a");
    }

    #[test]
    fn test_mmr_rerank_promotes_diversity() {
        // Three results where b and c are similar to each other but different from a.
        // a has moderate relevance, b and c have high relevance but are near-duplicates.
        let mut results = vec![
            HybridSearchResult {
                record_id: "a".to_string(),
                content: "A".to_string(),
                text_score: 0.6,
                vector_score: 0.6,
                combined_score: 0.6,
                temporal_score: 1.0,
            },
            HybridSearchResult {
                record_id: "b".to_string(),
                content: "B".to_string(),
                text_score: 0.9,
                vector_score: 0.9,
                combined_score: 0.9,
                temporal_score: 1.0,
            },
            HybridSearchResult {
                record_id: "c".to_string(),
                content: "C".to_string(),
                text_score: 0.85,
                vector_score: 0.85,
                combined_score: 0.85,
                temporal_score: 1.0,
            },
        ];

        // b and c have nearly identical embeddings; a is different
        let embeddings: HashMap<String, Vec<f32>> = [
            ("a".to_string(), vec![1.0, 0.0, 0.0]),
            ("b".to_string(), vec![0.0, 1.0, 0.01]),
            ("c".to_string(), vec![0.0, 1.0, 0.02]), // very similar to b
        ]
        .into();

        mmr_rerank(&mut results, 0.5, &embeddings);

        // b should be selected first (highest combined), then a (diverse from b),
        // then c (similar to b, penalized)
        assert_eq!(results[0].record_id, "b");
        assert_eq!(results[1].record_id, "a"); // diverse pick
        assert_eq!(results[2].record_id, "c"); // similar to b, ranked last
    }

    #[test]
    fn test_mmr_rerank_high_lambda_preserves_relevance_order() {
        let mut results = vec![
            HybridSearchResult {
                record_id: "a".to_string(),
                content: "A".to_string(),
                text_score: 0.3,
                vector_score: 0.3,
                combined_score: 0.3,
                temporal_score: 1.0,
            },
            HybridSearchResult {
                record_id: "b".to_string(),
                content: "B".to_string(),
                text_score: 0.9,
                vector_score: 0.9,
                combined_score: 0.9,
                temporal_score: 1.0,
            },
            HybridSearchResult {
                record_id: "c".to_string(),
                content: "C".to_string(),
                text_score: 0.6,
                vector_score: 0.6,
                combined_score: 0.6,
                temporal_score: 1.0,
            },
        ];

        // All different embeddings
        let embeddings: HashMap<String, Vec<f32>> = [
            ("a".to_string(), vec![1.0, 0.0, 0.0]),
            ("b".to_string(), vec![0.0, 1.0, 0.0]),
            ("c".to_string(), vec![0.0, 0.0, 1.0]),
        ]
        .into();

        // lambda = 1.0 means pure relevance, no diversity penalty
        mmr_rerank(&mut results, 1.0, &embeddings);

        assert_eq!(results[0].record_id, "b");
        assert_eq!(results[1].record_id, "c");
        assert_eq!(results[2].record_id, "a");
    }

    #[test]
    fn test_rrf_weight_multipliers() {
        // Higher text_weight should boost text-only results
        let text_results = vec![("a".to_string(), 0.9)];
        let vector_results = vec![("b".to_string(), 0.9)];

        let content_map: HashMap<String, String> = [
            ("a".to_string(), "Content A".to_string()),
            ("b".to_string(), "Content B".to_string()),
        ]
        .into();

        let timestamps: HashMap<String, DateTime<Utc>> = HashMap::new();

        let options = HybridSearchOptions {
            text_weight: 2.0, // Double text weight
            vector_weight: 1.0,
            temporal_decay_factor: 0.0,
            max_results: 10,
            mmr_lambda: 0.7,
        };

        let results = hybrid_search(
            text_results,
            vector_results,
            &options,
            &content_map,
            &timestamps,
        );
        assert_eq!(results.len(), 2);
        // "a" (text only) should score higher because text_weight=2.0
        assert_eq!(results[0].record_id, "a");
        // a: 2.0/61 ≈ 0.0328, b: 1.0/61 ≈ 0.0164
        assert!(results[0].combined_score > results[1].combined_score);
    }
}
