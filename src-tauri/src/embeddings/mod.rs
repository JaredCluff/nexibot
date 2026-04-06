//! ONNX-based text embeddings using MiniLM-L6-v2.
//!
//! Provides local, offline text embedding generation using ONNX Runtime.
//! The model is automatically downloaded on first use and cached as a singleton.

use anyhow::{Context, Result};
use lru::LruCache;
use ndarray::Array2;
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use sha2::{Digest, Sha256};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tokenizers::Tokenizer;
use tracing::{info, warn};

/// Embedding dimension for MiniLM-L6-v2
#[allow(dead_code)]
pub const EMBEDDING_DIM: usize = 384;

/// Maximum sequence length for the model
const MAX_SEQ_LENGTH: usize = 256;

/// HTTP timeout for model downloads in seconds
const MODEL_DOWNLOAD_TIMEOUT_SECS: u64 = 300;

/// HTTP connect timeout for model downloads in seconds
const MODEL_DOWNLOAD_CONNECT_TIMEOUT_SECS: u64 = 30;

/// Maximum number of cached embedding results (~3 MB at 384 dims * 4 bytes * 2048 entries).
const EMBEDDING_CACHE_CAPACITY: usize = 2048;

/// Global LRU cache for embedding results. Avoids redundant ~60ms ONNX inference
/// for previously-seen texts.
static EMBEDDING_CACHE: OnceLock<Mutex<LruCache<String, Vec<f32>>>> = OnceLock::new();

/// Get (or initialize) the global embedding cache.
fn get_embedding_cache() -> &'static Mutex<LruCache<String, Vec<f32>>> {
    EMBEDDING_CACHE.get_or_init(|| {
        Mutex::new(LruCache::new(
            NonZeroUsize::new(EMBEDDING_CACHE_CAPACITY).unwrap(),
        ))
    })
}

/// Global singleton for the embedding model (initialized lazily).
/// Uses Mutex because ONNX Session::run() requires &mut self.
static EMBEDDING_MODEL: std::sync::OnceLock<Result<std::sync::Mutex<EmbeddingModel>, String>> =
    std::sync::OnceLock::new();

/// Get the global embedding model instance, initializing if necessary.
///
/// Model initialization performs blocking I/O (HTTP download + ONNX session
/// construction).  When called from inside a Tokio async runtime we use
/// `block_in_place` so the scheduler can move other tasks off this thread
/// while we block.  When called from a plain sync context (e.g. unit tests
/// outside a runtime) we fall back to a direct call.
pub fn get_embedding_model() -> Result<&'static std::sync::Mutex<EmbeddingModel>> {
    let result = EMBEDDING_MODEL.get_or_init(|| {
        let init = || match EmbeddingModel::new() {
            Ok(model) => Ok(std::sync::Mutex::new(model)),
            Err(e) => Err(e.to_string()),
        };

        // If we are inside a Tokio multi-thread runtime, yield the thread to
        // the scheduler while we block on I/O so we don't starve other tasks.
        // `block_in_place` is a no-op on the current-thread scheduler and in
        // non-async contexts (it just calls the closure directly).
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(init)
        } else {
            init()
        }
    });

    match result {
        Ok(mutex) => Ok(mutex),
        Err(e) => Err(anyhow::anyhow!(
            "Failed to initialize embedding model: {}",
            e
        )),
    }
}

/// Generate an embedding for text using the global model.
///
/// Results are cached in a 2048-entry LRU cache to avoid redundant ONNX inference
/// (~60ms per call). Cache hits return in sub-microsecond time.
pub fn embed_text(text: &str) -> Result<Vec<f32>> {
    // Check cache first
    let cache = get_embedding_cache();
    {
        let mut cache_guard = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(cached) = cache_guard.get(text) {
            return Ok(cached.clone());
        }
    }

    // Cache miss — compute embedding
    let model_mutex = get_embedding_model()?;
    let mut model = match model_mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            warn!("Embedding model Mutex was poisoned, recovering");
            poisoned.into_inner()
        }
    };
    let embedding = model.embed_text(text)?;

    // Store in cache
    {
        let mut cache_guard = cache.lock().unwrap_or_else(|p| p.into_inner());
        cache_guard.put(text.to_string(), embedding.clone());
    }

    Ok(embedding)
}

/// Check whether the embedding model is available (already initialized or can be initialized).
#[allow(dead_code)]
pub fn is_model_available() -> bool {
    // If already initialized, check if it succeeded
    if let Some(result) = EMBEDDING_MODEL.get() {
        return result.is_ok();
    }
    // Otherwise check if model files exist
    let model_dir = get_models_dir();
    model_dir.join("minilm-l6-v2.onnx").exists() && model_dir.join("tokenizer.json").exists()
}

/// Cosine similarity between two embeddings.
///
/// Returns 0.0 immediately if the slices have different lengths or are empty,
/// preventing panics caused by mismatched embedding models.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a > 0.0 && norm_b > 0.0 {
        dot / (norm_a * norm_b)
    } else {
        0.0
    }
}

/// Get the models directory path.
fn get_models_dir() -> PathBuf {
    #[cfg(windows)]
    {
        dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("nexibot/models")
    }
    #[cfg(not(windows))]
    {
        dirs::home_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(".config/nexibot/models")
    }
}

/// Embedding model using ONNX Runtime.
pub struct EmbeddingModel {
    session: Session,
    tokenizer: Tokenizer,
}

impl EmbeddingModel {
    /// Create a new embedding model, downloading if necessary.
    pub fn new() -> Result<Self> {
        info!("[EMBEDDINGS] Initializing embedding model...");

        let model_dir = get_models_dir();
        std::fs::create_dir_all(&model_dir)?;

        let model_path = model_dir.join("minilm-l6-v2.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");

        if !model_path.exists() {
            info!("[EMBEDDINGS] Downloading MiniLM-L6-v2 model...");
            download_model(&model_path)?;
        }

        if !tokenizer_path.exists() {
            info!("[EMBEDDINGS] Downloading tokenizer...");
            download_tokenizer(&tokenizer_path)?;
        }

        let session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_intra_threads(4)?
            .commit_from_file(&model_path)
            .context("Failed to load ONNX model")?;

        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        info!("[EMBEDDINGS] Embedding model ready");
        Ok(Self { session, tokenizer })
    }

    /// Generate embedding for a single text.
    pub fn embed_text(&mut self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.embed_batch(&[text])?;
        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No embedding returned for text"))
    }

    /// Generate embeddings for multiple texts.
    #[allow(clippy::needless_range_loop)]
    pub fn embed_batch(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let batch_size = texts.len();

        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

        let max_len = encodings
            .iter()
            .map(|e| e.get_ids().len().min(MAX_SEQ_LENGTH))
            .max()
            .unwrap_or(0);

        let mut input_ids = vec![0i64; batch_size * max_len];
        let mut attention_mask = vec![0i64; batch_size * max_len];
        let token_type_ids = vec![0i64; batch_size * max_len];

        for (i, encoding) in encodings.iter().enumerate() {
            let ids = encoding.get_ids();
            let mask = encoding.get_attention_mask();

            let len = ids.len().min(max_len);
            for j in 0..len {
                input_ids[i * max_len + j] = ids[j] as i64;
                attention_mask[i * max_len + j] = mask[j] as i64;
            }
        }

        let input_ids_arr = Array2::from_shape_vec((batch_size, max_len), input_ids)?;
        let attention_mask_arr =
            Array2::from_shape_vec((batch_size, max_len), attention_mask.clone())?;
        let token_type_ids_arr = Array2::from_shape_vec((batch_size, max_len), token_type_ids)?;

        let input_ids_tensor = Tensor::from_array(input_ids_arr)?;
        let attention_mask_tensor = Tensor::from_array(attention_mask_arr)?;
        let token_type_ids_tensor = Tensor::from_array(token_type_ids_arr)?;

        let outputs = self.session.run(ort::inputs![
            "input_ids" => input_ids_tensor,
            "attention_mask" => attention_mask_tensor,
            "token_type_ids" => token_type_ids_tensor,
        ])?;

        let output = outputs["last_hidden_state"].try_extract_array::<f32>()?;

        // Mean pooling over sequence length
        let output_view = output.view();
        let shape = output_view.shape();
        let batch = shape[0];
        let seq_len = shape[1];
        let hidden_dim = shape[2];

        let mut embeddings = Vec::with_capacity(batch);
        for b in 0..batch {
            let mut pooled = vec![0f32; hidden_dim];
            let mut count = 0f32;

            for s in 0..seq_len.min(max_len) {
                let mask_val = attention_mask[b * max_len + s] as f32;
                if mask_val > 0.0 {
                    for h in 0..hidden_dim {
                        pooled[h] += output_view[[b, s, h]] * mask_val;
                    }
                    count += mask_val;
                }
            }

            if count > 0.0 {
                for h in 0..hidden_dim {
                    pooled[h] /= count;
                }
            }

            // L2 normalize
            let norm: f32 = pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for h in 0..hidden_dim {
                    pooled[h] /= norm;
                }
            }

            embeddings.push(pooled);
        }

        Ok(embeddings)
    }
}

/// Expected SHA-256 of the official MiniLM-L6-v2 ONNX model
/// (sentence-transformers/all-MiniLM-L6-v2, onnx/model.onnx, Git LFS OID).
const MODEL_ONNX_SHA256: &str =
    "6fd5d72fe4589f189f8ebc006442dbb529bb7ce38f8082112682524616046452";

/// Expected SHA-256 of the official tokenizer.json.
/// Run `sha256sum tokenizer.json` on the verified file to update this constant.
const TOKENIZER_SHA256: &str =
    "be50c3628f2bf5bb5e3a7f17b1f74611b2561a3a27eeab05e5aa30f411572037";

/// Download the ONNX model and verify its SHA-256.
fn download_model(path: &PathBuf) -> Result<()> {
    const MODEL_URL: &str = "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx";
    download_file(MODEL_URL, path, MODEL_ONNX_SHA256)
}

/// Download the tokenizer and verify its SHA-256.
fn download_tokenizer(path: &PathBuf) -> Result<()> {
    const TOKENIZER_URL: &str =
        "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json";
    download_file(TOKENIZER_URL, path, TOKENIZER_SHA256)
}

/// Download a file from URL and verify its SHA-256 against the expected digest.
///
/// Uses `reqwest::blocking::Client`.  This function must only be called from
/// within `get_embedding_model`, which already wraps the entire
/// `EmbeddingModel::new()` call in `tokio::task::block_in_place` so that the
/// async runtime is not starved while the HTTP download or ONNX session
/// construction is in progress.
fn download_file(url: &str, path: &PathBuf, expected_sha256: &str) -> Result<()> {
    info!("[EMBEDDINGS] Downloading: {}", url);

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(MODEL_DOWNLOAD_TIMEOUT_SECS))
        .connect_timeout(std::time::Duration::from_secs(
            MODEL_DOWNLOAD_CONNECT_TIMEOUT_SECS,
        ))
        .build()
        .context("Failed to create HTTP client")?;

    let response = client.get(url).send().context("Failed to download file")?;

    if !response.status().is_success() {
        anyhow::bail!("Download failed with status: {}", response.status());
    }

    let bytes = response.bytes()?;

    // Verify SHA-256 before writing to disk — guards against supply-chain
    // attacks where HuggingFace or the CDN is compromised and delivers a
    // malicious model file.
    if !expected_sha256.starts_with("TODO") {
        let actual = hex::encode(Sha256::digest(&bytes));
        if actual != expected_sha256 {
            anyhow::bail!(
                "SHA-256 mismatch for {}: expected {}, got {}. \
                 Refusing to use potentially tampered file.",
                path.display(),
                expected_sha256,
                actual
            );
        }
        info!("[EMBEDDINGS] SHA-256 verified for {}", path.display());
    } else {
        anyhow::bail!(
            "SHA-256 not configured for {} — refusing to write unverified file. \
             Replace the TODO constant in embeddings/mod.rs with the expected SHA-256.",
            path.display()
        );
    }

    // Write to temp file first, then rename (atomic)
    let temp_path = path.with_extension("tmp");
    std::fs::write(&temp_path, &bytes)?;
    std::fs::rename(&temp_path, path)?;

    info!(
        "[EMBEDDINGS] Downloaded to: {} ({} bytes)",
        path.display(),
        bytes.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim + 1.0).abs() < 0.001);
    }

    #[test]
    fn test_models_dir() {
        let dir = get_models_dir();
        assert!(dir.to_string_lossy().contains("nexibot/models"));
    }

    #[test]
    #[ignore] // Requires model download
    fn test_embedding_generation() {
        let mut model = EmbeddingModel::new().unwrap();
        let embedding = model.embed_text("Hello, world!").unwrap();
        assert_eq!(embedding.len(), EMBEDDING_DIM);

        // Embedding should be normalized
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }

    #[test]
    #[ignore] // Requires model download
    fn test_semantic_similarity() {
        let mut model = EmbeddingModel::new().unwrap();

        let emb1 = model.embed_text("The cat sat on the mat").unwrap();
        let emb2 = model.embed_text("A kitten is resting on a rug").unwrap();
        let emb3 = model
            .embed_text("Python is a programming language")
            .unwrap();

        let sim_related = cosine_similarity(&emb1, &emb2);
        let sim_unrelated = cosine_similarity(&emb1, &emb3);

        assert!(sim_related > sim_unrelated);
    }

    #[test]
    fn test_embedding_cache_hit_and_miss() {
        // Verify the cache returns consistent results and stores entries
        let cache = get_embedding_cache();

        // Insert a known value
        {
            let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
            guard.put("test-key".to_string(), vec![1.0, 2.0, 3.0]);
        }

        // Should get a hit
        {
            let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
            let result = guard.get("test-key");
            assert!(result.is_some());
            assert_eq!(result.unwrap(), &vec![1.0, 2.0, 3.0]);
        }

        // Should miss on unknown key
        {
            let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
            assert!(guard.get("nonexistent").is_none());
        }
    }

    #[test]
    fn test_embedding_cache_eviction() {
        // Create a small cache to test eviction
        let small_cache = Mutex::new(LruCache::<String, Vec<f32>>::new(
            NonZeroUsize::new(2).unwrap(),
        ));

        {
            let mut guard = small_cache.lock().unwrap_or_else(|e| e.into_inner());
            guard.put("a".to_string(), vec![1.0]);
            guard.put("b".to_string(), vec![2.0]);
            guard.put("c".to_string(), vec![3.0]); // evicts "a"
        }

        {
            let mut guard = small_cache.lock().unwrap_or_else(|e| e.into_inner());
            assert!(guard.get("a").is_none()); // evicted
            assert!(guard.get("b").is_some());
            assert!(guard.get("c").is_some());
        }
    }

    #[test]
    #[ignore] // Requires model download — benchmark
    fn benchmark_embedding_cache_lookups() {
        use std::time::Instant;

        let cache = get_embedding_cache();

        // Populate cache with some entries
        {
            let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
            for i in 0..100 {
                guard.put(format!("bench-key-{}", i), vec![0.0; EMBEDDING_DIM]);
            }
        }

        // Benchmark 100K cache lookups
        let start = Instant::now();
        let iterations = 100_000;
        for i in 0..iterations {
            let key = format!("bench-key-{}", i % 100);
            let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
            let _ = guard.get(&key);
        }
        let elapsed = start.elapsed();

        let per_lookup_ns = elapsed.as_nanos() / iterations as u128;
        eprintln!(
            "[BENCHMARK] {} cache lookups in {:?} ({} ns/lookup)",
            iterations, elapsed, per_lookup_ns
        );

        // Assert sub-microsecond per lookup (1000 ns)
        assert!(
            per_lookup_ns < 1000,
            "Cache lookup too slow: {} ns/lookup (expected < 1000 ns)",
            per_lookup_ns
        );
    }
}
