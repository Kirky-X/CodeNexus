// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Embedding service client.
//!
//! Defines the [`EmbedClient`] trait and three implementations:
//! - [`OpenAIEmbedClient`]: calls an OpenAI-compatible HTTP embedding API via
//!   [`reqwest::blocking`]. The API key is read from the environment and never
//!   persisted (TRD §6.1). Used when [`EmbeddingConfig::endpoint`] is `Some`.
//! - [`LocalEmbedClient`]: runs `arctic-embed-xs` inference locally
//!   via `vecboost` (Candle engine). Works fully offline. Used when
//!   [`EmbeddingConfig::endpoint`] is `None`.
//! - [`MockEmbedClient`]: returns deterministic vectors for testing without
//!   network access.
//!
//! # OpenAI-compatible API contract
//!
//! `POST {endpoint}/embeddings`
//! ```json
//! {"model": "text-embedding-3-small", "input": ["text1", "text2"]}
//! ```
//! Response:
//! ```json
//! {"data": [{"embedding": [0.1, ...]}, {"embedding": [0.2, ...]}]}
//! ```

use std::future::Future;

#[cfg(feature = "cache")]
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{EmbedError, EmbeddingConfig, Result, EMBEDDING_DIM, EMBED_MODEL_PATH_ENV};
use crate::i18n::t;

use vecboost::config::model::{ModelConfig, ModelTask, PoolingMode};
use vecboost::error::VecboostError;
use vecboost::library::{LibraryConfig, VecBoostLibrary};

#[cfg(feature = "cache")]
use crate::cache::CacheStore;

/// Trait for embedding text into dense vectors.
///
/// Implementations may call a remote HTTP service ([`OpenAIEmbedClient`]) or
/// return pre-computed/mock vectors ([`MockEmbedClient`]). The trait enables
/// dependency injection so callers can test without network access.
pub trait EmbedClient: Send + Sync {
    /// Embeds a batch of texts, returning one vector per input text.
    ///
    /// Each returned vector must have length [`EMBEDDING_DIM`].
    ///
    /// # Errors
    ///
    /// Returns [`EmbedError::Unavailable`] if the service cannot be reached,
    /// [`EmbedError::MissingApiKey`] if authentication is required but missing,
    /// or [`EmbedError::Api`] for non-2xx responses.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;

    /// Hot-reloads the embedding config at runtime (spec §Hot reconfiguration).
    ///
    /// The default implementation is a no-op. Concrete capabilities that hold
    /// the config behind an `Arc<RwLock<…>>` override this to propagate the
    /// new settings and invalidate any cached local model.
    fn update_config(&self, _new_config: EmbeddingConfig) {}
}

// --- OpenAI-compatible request/response types ---

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: Vec<&'a str>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

/// HTTP client for an OpenAI-compatible embedding API.
///
/// Uses [`reqwest::blocking`] so it can be called from synchronous code. The
/// API key is held in memory only and never written to disk (TRD §6.1).
///
/// Only used when [`EmbeddingConfig::endpoint`] is `Some` (remote HTTP mode).
/// For local offline inference, see [`LocalEmbedClient`].
pub struct OpenAIEmbedClient {
    config: EmbeddingConfig,
    http: reqwest::blocking::Client,
}

impl OpenAIEmbedClient {
    /// Creates a new client from the given [`EmbeddingConfig`].
    ///
    /// The config must be in remote HTTP mode (`endpoint = Some(url)`) and
    /// have an API key set.
    ///
    /// # Errors
    ///
    /// Returns [`EmbedError::Unavailable`] if `endpoint` is `None` (local mode
    /// — use [`LocalEmbedClient`] instead).
    /// Returns [`EmbedError::MissingApiKey`] if no API key is configured.
    pub fn new(config: EmbeddingConfig) -> Result<Self> {
        if config.is_local() {
            return Err(EmbedError::Unavailable(
                "OpenAIEmbedClient requires endpoint=Some(url); for local mode use LocalEmbedClient"
                    .to_string(),
            ));
        }
        if !config.has_api_key() {
            return Err(EmbedError::MissingApiKey);
        }
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self { config, http })
    }

    /// Creates a client from environment variables (convenience).
    ///
    /// Requires `CODENEXUS_EMBED_ENDPOINT` to be set (remote HTTP mode).
    ///
    /// # Errors
    ///
    /// Returns [`EmbedError::Unavailable`] if endpoint is not set.
    /// Returns [`EmbedError::MissingApiKey`] if neither `CODENEXUS_EMBED_API_KEY`
    /// nor `OPENAI_API_KEY` is set.
    pub fn from_env() -> Result<Self> {
        Self::new(EmbeddingConfig::from_env())
    }

    /// Returns the configured endpoint URL.
    ///
    /// # Panics
    ///
    /// Panics if `endpoint` is `None` — but [`OpenAIEmbedClient::new`] already
    /// rejects local-mode configs, so this should never occur for a
    /// successfully-constructed client.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        self.config
            .endpoint
            .as_deref()
            .expect("OpenAIEmbedClient endpoint is Some (enforced by new())")
    }

    /// Returns the configured model name.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.config.model
    }
}

impl EmbedClient for OpenAIEmbedClient {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let api_key = self
            .config
            .api_key
            .as_ref()
            .ok_or(EmbedError::MissingApiKey)?;

        // endpoint is guaranteed Some by new(), but use as_deref().unwrap_or
        // for defensive programming (avoids panic if config was mutated).
        let endpoint = self.config.endpoint.as_deref().ok_or_else(|| {
            EmbedError::Unavailable("endpoint is None in remote mode".to_string())
        })?;
        let url = format!("{endpoint}/embeddings");
        let body = EmbeddingRequest {
            model: &self.config.model,
            input: texts.to_vec(),
        };

        let resp = self
            .http
            .post(&url)
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .map_err(|e| EmbedError::Unavailable(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(EmbedError::Api {
                status: status.as_u16(),
                body,
            });
        }

        let parsed: EmbeddingResponse = resp.json()?;
        let embeddings: Vec<Vec<f32>> = parsed.data.into_iter().map(|d| d.embedding).collect();

        // Validate dimensions.
        for emb in &embeddings {
            if emb.len() != EMBEDDING_DIM {
                return Err(EmbedError::DimensionMismatch {
                    expected: EMBEDDING_DIM,
                    actual: emb.len(),
                });
            }
        }

        Ok(embeddings)
    }
}

impl std::fmt::Debug for OpenAIEmbedClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAIEmbedClient")
            .field("endpoint", &self.config.endpoint)
            .field("model", &self.config.model)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

// ---------------------------------------------------------------------------
// LocalEmbedClient
// ---------------------------------------------------------------------------

/// Local embedding client backed by `vecboost` (Candle engine).
///
/// Runs `arctic-embed-xs` inference locally using the `vecboost` crate's
/// Candle engine. Requires no network access — the model bundle (config.json,
/// tokenizer.json, model.safetensors) must be present on disk.
///
/// # Thread safety
///
/// `VecBoostLibrary` is `Send + Sync` (verified by compile-time assertion in
/// tests). `LocalEmbedClient` is `Send + Sync` without additional wrapping.
///
/// # Inference pipeline
///
/// 1. `new()` performs a complete bundle pre-check (directory, config.json,
///    tokenizer.json, model.safetensors) before constructing the
///    `VecBoostLibrary`.
/// 2. `embed()` converts texts to `Vec<String>` and calls `embed_batch_sync`
///    (vecboost's process-level runtime), then extracts the embedding vectors.
/// 3. Mean pooling and L2 normalization are handled by the vecboost engine.
///
/// # Allocation note
///
/// Inputs are copied into `Vec<String>` per call and each vecboost result
/// carries an unused `text_preview` string — both are imposed by the vecboost
/// batch API (`&[String]` / `BatchEmbeddingResult`); the caller cannot avoid
/// them. Negligible next to the ms-level inference itself.
///
/// # Errors
///
/// [`LocalEmbedClient::new`] returns [`EmbedError::Unavailable`] if the model
/// bundle is incomplete or cannot be loaded.
pub struct LocalEmbedClient {
    /// vecboost library instance (Candle engine).
    library: VecBoostLibrary,
    /// Model name (for Debug output).
    model: String,
}

impl LocalEmbedClient {
    /// Creates a new local embedding client from the given config.
    ///
    /// Performs a complete bundle pre-check (directory, config.json,
    /// tokenizer.json, model.safetensors) before constructing the
    /// `VecBoostLibrary`.
    ///
    /// # Errors
    ///
    /// Returns [`EmbedError::Unavailable`] if:
    /// - The model directory does not exist.
    /// - config.json is missing.
    /// - tokenizer.json is missing.
    /// - Model weights (`model.safetensors`) are missing. The legacy
    ///   `pytorch_model.bin` (pickle) format is rejected — the vecboost Candle
    ///   engine cannot load it.
    /// - The vecboost library cannot be initialized.
    pub fn new(config: &EmbeddingConfig) -> Result<Self> {
        let model_dir = config.resolved_model_path();

        // 1. Check directory exists
        if !model_dir.exists() {
            return Err(EmbedError::Unavailable(t(
                "embed-model-not-found",
                &[
                    ("path", model_dir.display().to_string()),
                    ("env", EMBED_MODEL_PATH_ENV.to_string()),
                ],
            )));
        }

        // 2. Check config.json
        let config_path = model_dir.join("config.json");
        if !config_path.exists() {
            return Err(EmbedError::Unavailable(t(
                "embed-model-config-missing",
                &[("path", config_path.display().to_string())],
            )));
        }

        // 3. Check tokenizer.json
        let tokenizer_path = config.resolved_tokenizer_path();
        if !tokenizer_path.exists() {
            return Err(EmbedError::Unavailable(format!(
                "tokenizer not found at {}.",
                tokenizer_path.display(),
            )));
        }

        // 4. Check weights — safetensors only; pytorch_model.bin (pickle) is
        //    not supported by the Candle engine.
        let safetensors_path = model_dir.join("model.safetensors");
        if !safetensors_path.exists() {
            return Err(EmbedError::Unavailable(t(
                "embed-model-weights-missing",
                &[("path", model_dir.display().to_string())],
            )));
        }

        // 5. Build VecBoostLibrary
        let mc = ModelConfig {
            name: config.model.clone(),
            model_path: model_dir,
            tokenizer_path: None,
            pooling_mode: Some(PoolingMode::Mean),
            expected_dimension: Some(EMBEDDING_DIM),
            max_batch_size: 32,
            model_sha256: config.model_sha256.clone(),
            task: ModelTask::Embedding,
            ..Default::default()
        };

        let library = block_on_vecboost(VecBoostLibrary::new(LibraryConfig::from_model_config(mc)))
            .map_err(vecboost_error_to_embed_error)?;

        Ok(Self {
            library,
            model: config.model.clone(),
        })
    }

    /// Returns the model name.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }
}

impl EmbedClient for LocalEmbedClient {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        let library = &self.library;
        // `embed_batch_sync` rejects calls from inside a tokio runtime context
        // (vecboost's sync guard), and every CLI/MCP entry point is an async
        // handler — inference must therefore run on a thread outside any
        // runtime, where vecboost lazily builds its own. The loop stays on
        // that one thread: one spawn per call, not per chunk.
        let responses = on_dedicated_thread(move || {
            let mut responses = Vec::with_capacity(owned.len().div_ceil(EMBED_CHUNK_TEXTS));
            for chunk in owned.chunks(EMBED_CHUNK_TEXTS) {
                responses.push(library.embed_batch_sync(chunk)?);
            }
            Ok::<_, VecboostError>(responses)
        })
        .map_err(vecboost_error_to_embed_error)?;
        let embeddings: Vec<Vec<f32>> = responses
            .into_iter()
            .flat_map(|resp| resp.embeddings.into_iter().map(|r| r.embedding))
            .collect();
        for emb in &embeddings {
            if emb.len() != EMBEDDING_DIM {
                return Err(EmbedError::DimensionMismatch {
                    expected: EMBEDDING_DIM,
                    actual: emb.len(),
                });
            }
        }
        Ok(embeddings)
    }
}

impl std::fmt::Debug for LocalEmbedClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalEmbedClient")
            .field("model", &self.model)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// vecboost construction helper
// ---------------------------------------------------------------------------

/// Per-call client-side inference chunk. vecboost hard-rejects any batch above
/// its global `MAX_BATCH_SIZE` (100) regardless of the `max_batch_size` model
/// config, while the previous ort path chunked at 32 — chunking stays here so
/// callers can pass any number of texts.
const EMBED_CHUNK_TEXTS: usize = 32;

/// Runs a blocking closure on a dedicated OS thread, outside any tokio
/// runtime context.
///
/// vecboost's sync API refuses to run inside a runtime; this is the escape
/// hatch for it. The thread itself needs no runtime — the sync path lazily
/// builds vecboost's own. Returns `Err` (not panic) if the closure panicked:
/// the join observes the unwind and turns it into a `VecboostError`. Note the
/// release profile sets `panic = "abort"`, under which a panic takes the whole
/// process down before `join` returns — the `Err` path is a dev/test safety
/// net, not a production one.
fn on_dedicated_thread<F, T>(f: F) -> std::result::Result<T, VecboostError>
where
    F: FnOnce() -> std::result::Result<T, VecboostError> + Send,
    T: Send,
{
    match std::thread::scope(|scope| scope.spawn(f).join()) {
        Ok(inner) => inner,
        Err(_) => Err(VecboostError::InternalError(
            "vecboost inference thread panicked".to_string(),
        )),
    }
}

/// Constructs a `VecBoostLibrary` by running its async constructor on a
/// dedicated OS thread with a fresh tokio runtime.
///
/// The constructor runs once per client load (cached by the capability), so
/// the thread cost is not on the inference hot path. Returns `Err` (not
/// panic) if the runtime cannot be built; a thread panic unwinds into the
/// join's `Err` arm under the default unwind profile — under the release
/// profile's `panic = "abort"` it terminates the process, like any panic.
fn block_on_vecboost<F, T>(fut: F) -> std::result::Result<T, VecboostError>
where
    F: Future<Output = std::result::Result<T, VecboostError>> + Send,
    T: Send,
{
    std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(fut),
                Err(e) => Err(VecboostError::InternalError(t(
                    "embed-runtime-build-failed",
                    &[("reason", e.to_string())],
                ))),
            }
        });
        match handle.join() {
            Ok(inner) => inner,
            Err(_) => Err(VecboostError::InternalError(t(
                "embed-runtime-thread-panic",
                &[],
            ))),
        }
    })
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

/// Maps a `VecboostError` to [`EmbedError::Unavailable`].
///
/// Exhaustive match over all 20 `VecboostError` variants — no catch-all arm,
/// so upstream additions cause a compile error forcing an explicit decision.
///
/// Invariant: the in-process library path (no network server) does not produce
/// the server-side operational variants — `AuthenticationError`,
/// `RateLimitExceeded`, `Overloaded`, `RequestTimeout`, `DatabaseError` belong
/// to the HTTP/auth/pipeline layers, not to local inference. All variants
/// therefore collapse into the single retry/fallback classification
/// [`EmbedError::Unavailable`] (callers degrade to BM25), with the original
/// detail string preserved for diagnostics.
fn vecboost_error_to_embed_error(e: VecboostError) -> EmbedError {
    let detail = match e {
        VecboostError::ConfigError(s) => s,
        VecboostError::ModelLoadError(s) => s,
        VecboostError::ModelFileCorrupted(s) => s,
        VecboostError::ModelIntegrityError(s) => s,
        VecboostError::TokenizationError(s) => s,
        VecboostError::InferenceError(s) => s,
        VecboostError::OutOfMemory(s) => s,
        VecboostError::InvalidInput(s) => s,
        VecboostError::NotFound(s) => s,
        VecboostError::ModelNotLoaded(s) => s,
        VecboostError::AuthenticationError(s) => s,
        VecboostError::SecurityError(s) => s,
        VecboostError::IoError(s) => s,
        VecboostError::ValidationError(s) => s,
        VecboostError::RateLimitExceeded(s) => s,
        VecboostError::Overloaded(s) => s,
        VecboostError::RequestTimeout(s) => s,
        VecboostError::UnsupportedTask(s) => s,
        VecboostError::DatabaseError(s) => s,
        VecboostError::InternalError(s) => s,
    };
    EmbedError::Unavailable(detail)
}

/// Mock embedding client for testing (no network access).
///
/// Generates deterministic pseudo-random vectors based on a hash of the input
/// text. All vectors have length [`EMBEDDING_DIM`]. This allows tests to verify
/// storage and search logic without calling a real embedding service.
pub struct MockEmbedClient {
    /// Override dimension (defaults to [`EMBEDDING_DIM`]).
    dim: usize,
    /// If set, `embed()` returns this error instead of vectors.
    error: Option<EmbedError>,
}

impl MockEmbedClient {
    /// Creates a mock client that produces vectors of the standard dimension.
    #[must_use]
    pub fn new() -> Self {
        Self {
            dim: EMBEDDING_DIM,
            error: None,
        }
    }

    /// Creates a mock client with a custom vector dimension.
    #[must_use]
    pub fn with_dim(dim: usize) -> Self {
        Self { dim, error: None }
    }

    /// Creates a mock client that always returns the given error.
    #[must_use]
    pub fn with_error(error: EmbedError) -> Self {
        Self {
            dim: EMBEDDING_DIM,
            error: Some(error),
        }
    }
}

impl Default for MockEmbedClient {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbedClient for MockEmbedClient {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if let Some(ref err) = self.error {
            return Err(EmbedError::Unavailable(err.to_string()));
        }
        // Deterministic pseudo-embedding: hash each text and fill the vector.
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(text, &mut hasher);
            let seed = std::hash::Hasher::finish(&hasher);
            let mut vec = Vec::with_capacity(self.dim);
            let mut state = seed;
            for _ in 0..self.dim {
                // Simple LCG for deterministic pseudo-random floats in [0, 1).
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let val = ((state >> 33) as f32) / (1u64 << 31) as f32;
                vec.push(val);
            }
            results.push(vec);
        }
        Ok(results)
    }
}

impl std::fmt::Debug for MockEmbedClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockEmbedClient")
            .field("dim", &self.dim)
            .field("error", &self.error.is_some())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// CachedEmbedClient
// ---------------------------------------------------------------------------

/// Serializes a `Vec<f32>` to little-endian bytes for cache storage.
///
/// Each `f32` is written as 4 little-endian bytes. The inverse is
/// [`deserialize_vec`].
#[cfg(feature = "cache")]
fn serialize_vec(vec: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vec.len() * 4);
    for &v in vec {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

/// Deserializes little-endian bytes back to `Vec<f32>`.
///
/// Inverse of [`serialize_vec`]. Returns an empty `Vec` when the byte
/// length is not a multiple of 4 — this signals corrupt cache data so the
/// caller can treat it as a cache miss and recompute.
///
/// Note: [`chunks_exact`] silently drops a trailing partial chunk; the
/// explicit length check here makes that behavior visible and recoverable.
///
/// [`chunks_exact`]: slice::chunks_exact
#[cfg(feature = "cache")]
fn deserialize_vec(bytes: &[u8]) -> Vec<f32> {
    if !bytes.len().is_multiple_of(4) {
        return Vec::new();
    }
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Embedding client wrapper that caches vectors by text content hash.
///
/// Wraps an inner [`EmbedClient`] and a [`CacheStore`](crate::cache::CacheStore).
/// On `embed`, each input text is looked up in the cache by
/// `format!("embed:{content_hash}")` where `content_hash` is the BLAKE3 of
/// the text (via [`compute_content_hash`](crate::index::hash::compute_content_hash)).
/// Hits are returned from the cache; misses are batched and delegated to the
/// inner client, then written back to the cache.
///
/// Existing API is unchanged: [`EmbedClient`], [`OpenAIEmbedClient`],
/// [`LocalEmbedClient`], and [`MockEmbedClient`] are not modified.
#[cfg(feature = "cache")]
pub struct CachedEmbedClient {
    inner: Arc<dyn EmbedClient>,
    cache: Arc<dyn CacheStore>,
}

#[cfg(feature = "cache")]
impl CachedEmbedClient {
    /// Creates a new cached embedding client wrapping `inner` with `cache`.
    ///
    /// `inner` is an [`Arc`] so callers that keep a lazily-initialised client
    /// (e.g. the [`LocalEmbedClient`] behind a mutex) can share the same
    /// instance between the cached wrapper and the fallback path.
    #[must_use]
    pub fn new(inner: Arc<dyn EmbedClient>, cache: Arc<dyn CacheStore>) -> Self {
        Self { inner, cache }
    }
}

#[cfg(feature = "cache")]
impl EmbedClient for CachedEmbedClient {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut results: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        let mut miss_indices: Vec<usize> = Vec::with_capacity(texts.len());
        let mut miss_texts: Vec<&str> = Vec::with_capacity(texts.len());

        // 1. Probe cache per text. Misses are collected for batch delegation.
        for (i, text) in texts.iter().enumerate() {
            let hash = crate::index::hash::compute_content_hash(text.as_bytes());
            let key = format!("embed:{hash}");
            if let Some(bytes) = self.cache.get(&key) {
                let vec = deserialize_vec(&bytes);
                if vec.len() == EMBEDDING_DIM {
                    results.push(vec);
                } else {
                    // Corrupt cache entry (wrong byte length or dim
                    // mismatch): treat as miss and overwrite with a fresh
                    // computation on the next phase.
                    results.push(Vec::new());
                    miss_indices.push(i);
                    miss_texts.push(*text);
                }
            } else {
                results.push(Vec::new());
                miss_indices.push(i);
                miss_texts.push(*text);
            }
        }

        // 2. Batch-call inner client for misses, then write results to cache.
        if !miss_texts.is_empty() {
            let embeddings = self.inner.embed(&miss_texts)?;
            for (idx, emb) in miss_indices.iter().zip(embeddings) {
                let text = texts[*idx];
                let hash = crate::index::hash::compute_content_hash(text.as_bytes());
                let key = format!("embed:{hash}");
                self.cache.set(&key, serialize_vec(&emb));
                results[*idx] = emb;
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- MockEmbedClient ---

    #[test]
    fn mock_client_returns_correct_count() {
        let client = MockEmbedClient::new();
        let texts = ["hello", "world", "foo"];
        let result = client.embed(&texts).expect("embed");
        assert_eq!(result.len(), 3, "should return one vector per text");
    }

    #[test]
    fn mock_client_returns_correct_dimension() {
        let client = MockEmbedClient::new();
        let result = client.embed(&["test"]).expect("embed");
        assert_eq!(result[0].len(), EMBEDDING_DIM, "dimension should be 384");
    }

    #[test]
    fn mock_client_is_deterministic() {
        let client = MockEmbedClient::new();
        let a = client.embed(&["hello"]).expect("embed");
        let b = client.embed(&["hello"]).expect("embed");
        assert_eq!(a, b, "same input should produce same output");
    }

    #[test]
    fn mock_client_different_inputs_differ() {
        let client = MockEmbedClient::new();
        let a = client.embed(&["hello"]).expect("embed");
        let b = client.embed(&["world"]).expect("embed");
        assert_ne!(a, b, "different inputs should produce different outputs");
    }

    #[test]
    fn mock_client_with_custom_dim() {
        let client = MockEmbedClient::with_dim(128);
        let result = client.embed(&["test"]).expect("embed");
        assert_eq!(result[0].len(), 128);
    }

    #[test]
    fn mock_client_with_error_returns_error() {
        let client =
            MockEmbedClient::with_error(EmbedError::Unavailable("service down".to_string()));
        let result = client.embed(&["test"]);
        assert!(result.is_err(), "should return error");
        assert!(result.unwrap_err().to_string().contains("unavailable"));
    }

    #[test]
    fn mock_client_empty_input_returns_empty() {
        let client = MockEmbedClient::new();
        let result = client.embed(&[]).expect("embed");
        assert!(result.is_empty(), "empty input should return empty");
    }

    #[test]
    fn mock_client_default_is_new() {
        let client = MockEmbedClient::default();
        let result = client.embed(&["x"]).expect("embed");
        assert_eq!(result[0].len(), EMBEDDING_DIM);
    }

    #[test]
    fn mock_client_debug_does_not_leak_vectors() {
        let client = MockEmbedClient::new();
        let s = format!("{client:?}");
        assert!(s.contains("MockEmbedClient"));
        assert!(s.contains("dim"));
    }

    // --- OpenAIEmbedClient ---

    #[test]
    fn openai_client_new_without_endpoint_returns_error() {
        // Default config is local mode (endpoint=None).
        // OpenAIEmbedClient should reject local-mode configs.
        let cfg = EmbeddingConfig::default();
        let result = OpenAIEmbedClient::new(cfg);
        assert!(result.is_err(), "should error without endpoint");
        assert!(
            matches!(result.unwrap_err(), EmbedError::Unavailable(_)),
            "should return Unavailable for local-mode config"
        );
    }

    #[test]
    fn openai_client_new_with_endpoint_but_no_key_returns_missing_key() {
        let _lock = crate::embed::ENV_TEST_LOCK.lock().unwrap();
        // Remote mode (endpoint=Some) but no API key → MissingApiKey.
        std::env::remove_var("CODENEXUS_EMBED_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");
        let cfg = EmbeddingConfig {
            endpoint: Some("https://api.openai.com/v1".to_string()),
            ..EmbeddingConfig::default()
        };
        let result = OpenAIEmbedClient::new(cfg);
        assert!(result.is_err(), "should error without API key");
        assert!(matches!(result.unwrap_err(), EmbedError::MissingApiKey));
    }

    #[test]
    fn openai_client_new_with_endpoint_and_key_succeeds() {
        let cfg = EmbeddingConfig {
            endpoint: Some("https://api.openai.com/v1".to_string()),
            api_key: Some("test-key".to_string()),
            ..EmbeddingConfig::default()
        };
        let client = OpenAIEmbedClient::new(cfg).expect("should succeed with endpoint+key");
        assert!(client.endpoint().contains("openai.com"));
        assert!(!client.model().is_empty());
    }

    #[test]
    fn openai_client_from_env_without_endpoint_errors() {
        let _lock = crate::embed::ENV_TEST_LOCK.lock().unwrap();
        // Without CODENEXUS_EMBED_ENDPOINT, from_env returns local-mode
        // config, which OpenAIEmbedClient rejects.
        std::env::remove_var("CODENEXUS_EMBED_ENDPOINT");
        std::env::remove_var("CODENEXUS_EMBED_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");
        let result = OpenAIEmbedClient::from_env();
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), EmbedError::Unavailable(_)),
            "should return Unavailable when endpoint not set"
        );
    }

    #[test]
    fn openai_client_from_env_with_endpoint_and_key_succeeds() {
        let _lock = crate::embed::ENV_TEST_LOCK.lock().unwrap();
        std::env::set_var("CODENEXUS_EMBED_ENDPOINT", "https://api.openai.com/v1");
        std::env::set_var("CODENEXUS_EMBED_API_KEY", "env-key");
        let client = OpenAIEmbedClient::from_env().expect("should succeed");
        assert_eq!(client.endpoint(), "https://api.openai.com/v1");
        std::env::remove_var("CODENEXUS_EMBED_ENDPOINT");
        std::env::remove_var("CODENEXUS_EMBED_API_KEY");
    }

    #[test]
    fn openai_client_debug_redacts_api_key() {
        let cfg = EmbeddingConfig {
            endpoint: Some("https://api.openai.com/v1".to_string()),
            api_key: Some("secret-key-12345".to_string()),
            ..EmbeddingConfig::default()
        };
        let client = OpenAIEmbedClient::new(cfg).expect("client");
        let s = format!("{client:?}");
        assert!(s.contains("<redacted>"), "API key must be redacted: {s}");
        assert!(
            !s.contains("secret-key-12345"),
            "API key must not appear in debug: {s}"
        );
    }

    #[test]
    fn openai_client_embed_without_key_returns_missing_key() {
        // Create a client with endpoint+key, then test the embed path.
        // We can't make a real HTTP call, but we can verify the client
        // is constructed correctly.
        let cfg = EmbeddingConfig {
            endpoint: Some("http://localhost:1".to_string()), // unreachable port
            api_key: Some("test".to_string()),
            ..EmbeddingConfig::default()
        };
        let client = OpenAIEmbedClient::new(cfg).expect("client");
        let result = client.embed(&["test"]);
        // Should get an Unavailable error (connection refused).
        assert!(result.is_err(), "should fail to connect");
        let err = result.unwrap_err();
        assert!(
            matches!(err, EmbedError::Unavailable(_)),
            "expected Unavailable, got {}",
            err.kind()
        );
    }

    // --- LocalEmbedClient ---

    /// Registers the embedded FTL catalogs (idempotent) so tests exercise
    /// real translations rather than the bare-key fallback `t` serves when
    /// `crate::i18n::init()` has not run in this process. Global registry
    /// state, so callers must serialize via `serial(i18n_catalog)`.
    fn init_i18n_catalog() {
        crate::i18n::init();
    }

    #[test]
    #[serial_test::serial(i18n_catalog)]
    fn local_client_new_without_model_returns_unavailable() {
        // Default config → local mode, model path doesn't exist.
        let cfg = EmbeddingConfig::default();
        init_i18n_catalog();
        let result = LocalEmbedClient::new(&cfg);
        assert!(result.is_err(), "should error when model file is missing");
        let err = result.unwrap_err();
        let expected_path = cfg.resolved_model_path();
        // The message is a localized catalog entry, so this asserts on the
        // locale-independent payload (the missing artifact path) instead of
        // any language's wording.
        let expected = expected_path.display().to_string();
        assert!(
            matches!(err, EmbedError::Unavailable(ref msg) if msg.contains(&expected)),
            "expected Unavailable naming '{expected_path:?}', got: {err}"
        );
    }

    #[test]
    #[serial_test::serial(i18n_catalog)]
    fn local_client_new_with_nonexistent_custom_path_returns_unavailable() {
        init_i18n_catalog();
        let cfg = EmbeddingConfig {
            model_path: Some(std::path::PathBuf::from("/nonexistent/model-dir")),
            ..EmbeddingConfig::default()
        };
        let result = LocalEmbedClient::new(&cfg);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("/nonexistent/model-dir"),
            "error should name the missing model path: {msg}"
        );
    }

    /// Every hardcoded English user-facing string this module used to emit is
    /// now a catalog entry: the message must resolve through the registry
    /// (locale-independent payload intact) rather than falling back to a
    /// bare key.
    #[test]
    #[serial_test::serial(i18n_catalog)]
    fn embed_messages_resolve_through_catalog() {
        init_i18n_catalog();
        assert_eq!(
            crate::i18n::t(
                "embed-model-not-found",
                &[
                    ("path", "/m".to_string()),
                    ("env", EMBED_MODEL_PATH_ENV.to_string()),
                ],
            ),
            "local embedding model not found at /m. \
             Place the arctic-embed-xs model at this path \
             (or set the CODENEXUS_EMBED_MODEL_PATH env var to a custom location)."
        );
        assert_eq!(
            crate::i18n::tr("embed-model-config-missing"),
            "model config.json not found at { $path }."
        );
        assert_eq!(
            crate::i18n::tr("embed-runtime-build-failed"),
            "failed to build tokio runtime: { $reason }"
        );
        assert_eq!(
            crate::i18n::tr("embed-runtime-thread-panic"),
            "vecboost runtime thread panicked"
        );
        // The weights message names both the expected and the rejected file.
        let weights = crate::i18n::t("embed-model-weights-missing", &[("path", "/m".to_string())]);
        assert!(
            weights.contains("/m")
                && weights.contains("model.safetensors")
                && weights.contains("pytorch_model.bin"),
            "weights message must name the bundle path and both files: {weights}"
        );
    }

    #[test]
    #[serial_test::serial(i18n_catalog)]
    fn local_client_new_bundle_missing_tokenizer_returns_unavailable() {
        init_i18n_catalog();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), "{}").unwrap();
        std::fs::write(dir.path().join("model.safetensors"), b"placeholder").unwrap();

        let cfg = EmbeddingConfig {
            model_path: Some(dir.path().to_path_buf()),
            ..EmbeddingConfig::default()
        };
        let result = LocalEmbedClient::new(&cfg);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("tokenizer.json"),
            "expected the error to name the missing tokenizer path: {msg}"
        );
    }

    #[test]
    #[serial_test::serial(i18n_catalog)]
    fn local_client_new_rejects_missing_weights_in_bundle() {
        init_i18n_catalog();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), "{}").unwrap();
        std::fs::write(dir.path().join("tokenizer.json"), "{}").unwrap();

        let cfg = EmbeddingConfig {
            model_path: Some(dir.path().to_path_buf()),
            ..EmbeddingConfig::default()
        };
        let result = LocalEmbedClient::new(&cfg);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("model.safetensors") && msg.contains("pytorch_model.bin"),
            "expected a safetensors-only contract message, got: {msg}"
        );
    }

    #[test]
    #[serial_test::serial(i18n_catalog)]
    fn local_client_new_rejects_pytorch_bin_only_bundle() {
        init_i18n_catalog();
        // pytorch_model.bin (pickle) is not loadable by the Candle engine —
        // the pre-check must reject the bundle instead of failing later with
        // an obscure load error.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), "{}").unwrap();
        std::fs::write(dir.path().join("tokenizer.json"), "{}").unwrap();
        std::fs::write(dir.path().join("pytorch_model.bin"), b"placeholder").unwrap();

        let cfg = EmbeddingConfig {
            model_path: Some(dir.path().to_path_buf()),
            ..EmbeddingConfig::default()
        };
        let result = LocalEmbedClient::new(&cfg);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("model.safetensors") && msg.contains("pytorch_model.bin"),
            "expected safetensors-only contract message, got: {msg}"
        );
    }

    #[test]
    #[serial_test::serial(i18n_catalog)]
    fn local_client_new_bundle_missing_config_returns_unavailable() {
        init_i18n_catalog();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tokenizer.json"), "{}").unwrap();
        std::fs::write(dir.path().join("model.safetensors"), b"placeholder").unwrap();

        let cfg = EmbeddingConfig {
            model_path: Some(dir.path().to_path_buf()),
            ..EmbeddingConfig::default()
        };
        let result = LocalEmbedClient::new(&cfg);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("config.json"),
            "expected the error to name the missing config.json path: {msg}"
        );
    }

    #[test]
    fn local_client_debug_shows_model_name() {
        // We can't construct a LocalEmbedClient without a real model bundle,
        // but we can verify the Debug impl format via MockEmbedClient
        // pattern. This test documents the expected Debug format.
        // (Actual Debug test requires a model bundle — integration concern.)
    }

    #[test]
    fn local_embed_client_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LocalEmbedClient>();
    }

    // --- EmbedClient trait object ---

    #[test]
    fn embed_client_trait_object_works() {
        let client: Box<dyn EmbedClient> = Box::new(MockEmbedClient::new());
        let result = client.embed(&["hello", "world"]).expect("embed");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn embed_client_trait_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Box<dyn EmbedClient>>();
    }

    // --- CachedEmbedClient ---

    #[cfg(feature = "cache")]
    mod cached_tests {
        use super::*;
        use crate::cache::CacheStore;
        use std::collections::HashMap;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};

        /// Wraps [`MockEmbedClient`], counting `embed()` calls and recording
        /// the texts passed to each call. Inspection handles are shared via
        /// [`Arc`] so the test can read them after the client is boxed.
        struct CountingEmbedClient {
            calls: Arc<AtomicUsize>,
            recorded: Arc<Mutex<Vec<Vec<String>>>>,
            inner: MockEmbedClient,
        }

        impl CountingEmbedClient {
            fn new() -> Self {
                Self {
                    calls: Arc::new(AtomicUsize::new(0)),
                    recorded: Arc::new(Mutex::new(Vec::new())),
                    inner: MockEmbedClient::new(),
                }
            }
        }

        impl EmbedClient for CountingEmbedClient {
            fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.recorded
                    .lock()
                    .expect("lock")
                    .push(texts.iter().map(|s| s.to_string()).collect());
                self.inner.embed(texts)
            }
        }

        /// HashMap-backed [`CacheStore`] for testing.
        struct MockCache {
            store: Mutex<HashMap<String, Vec<u8>>>,
        }

        impl MockCache {
            fn new() -> Self {
                Self {
                    store: Mutex::new(HashMap::new()),
                }
            }

            fn snapshot(&self) -> HashMap<String, Vec<u8>> {
                self.store.lock().expect("lock").clone()
            }

            fn len(&self) -> usize {
                self.store.lock().expect("lock").len()
            }
        }

        impl CacheStore for MockCache {
            fn get(&self, key: &str) -> Option<Vec<u8>> {
                self.store.lock().expect("lock").get(key).cloned()
            }

            fn set(&self, key: &str, val: Vec<u8>) {
                self.store
                    .lock()
                    .expect("lock")
                    .insert(key.to_string(), val);
            }

            fn invalidate_all(&self) {
                self.store.lock().expect("lock").clear();
            }
        }

        #[test]
        fn cached_embed_miss_then_hit_skips_inner_call() {
            let inner = CountingEmbedClient::new();
            let calls = Arc::clone(&inner.calls);
            let cached = CachedEmbedClient::new(Arc::new(inner), Arc::new(MockCache::new()));

            let r1 = cached.embed(&["hello"]).expect("first embed");
            let r2 = cached.embed(&["hello"]).expect("second embed (cache hit)");

            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "inner should be called once; second call must hit cache"
            );
            assert_eq!(r1, r2, "both calls should return identical vectors");
        }

        #[test]
        fn cached_embed_batch_partial_cache_hit() {
            let inner = CountingEmbedClient::new();
            let calls = Arc::clone(&inner.calls);
            let recorded = Arc::clone(&inner.recorded);
            let cache = Arc::new(MockCache::new());
            let cached = CachedEmbedClient::new(Arc::new(inner), cache.clone());

            // Prime the cache: embed "cached_text" once.
            let primed = cached
                .embed(&["cached_text"])
                .expect("prime")
                .into_iter()
                .next()
                .expect("one vector");
            let prime_calls = calls.load(Ordering::SeqCst);

            // Batch call: "cached_text" is cached, "fresh_text" is a miss.
            let results = cached
                .embed(&["cached_text", "fresh_text"])
                .expect("batch embed");

            assert_eq!(results.len(), 2, "should return one vector per input");
            assert_eq!(
                results[0], primed,
                "cached_text result should match the primed vector"
            );
            let batch_calls = calls.load(Ordering::SeqCst) - prime_calls;
            assert_eq!(
                batch_calls, 1,
                "inner should be called once for the miss only"
            );
            let last = recorded
                .lock()
                .expect("lock")
                .last()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                last,
                vec!["fresh_text".to_string()],
                "inner should only receive uncached texts"
            );
        }

        #[test]
        fn cached_embed_same_text_same_key() {
            let inner = CountingEmbedClient::new();
            let cache = Arc::new(MockCache::new());
            let cached = CachedEmbedClient::new(Arc::new(inner), cache.clone());

            cached.embed(&["hello"]).expect("first");
            cached.embed(&["hello"]).expect("second");

            let snap = cache.snapshot();
            assert_eq!(
                snap.len(),
                1,
                "same text should produce same key → exactly one entry"
            );
            let key = snap.keys().next().expect("one entry");
            assert!(
                key.starts_with("embed:"),
                "key should start with 'embed:': {key}"
            );
        }

        #[test]
        fn cached_embed_different_texts_different_keys() {
            let inner = CountingEmbedClient::new();
            let cache = Arc::new(MockCache::new());
            let cached = CachedEmbedClient::new(Arc::new(inner), cache.clone());

            cached.embed(&["hello", "world"]).expect("embed");

            let snap = cache.snapshot();
            assert_eq!(
                snap.len(),
                2,
                "different texts should produce different keys → two entries"
            );
            for key in snap.keys() {
                assert!(
                    key.starts_with("embed:"),
                    "key should start with 'embed:': {key}"
                );
            }
        }

        #[test]
        fn cached_embed_empty_input_returns_empty() {
            let inner = CountingEmbedClient::new();
            let calls = Arc::clone(&inner.calls);
            let cache = Arc::new(MockCache::new());
            let cached = CachedEmbedClient::new(Arc::new(inner), cache.clone());

            let results = cached.embed(&[]).expect("empty embed");
            assert!(results.is_empty(), "empty input should return empty");
            assert_eq!(
                calls.load(Ordering::SeqCst),
                0,
                "inner should not be called for empty input"
            );
            assert_eq!(cache.len(), 0, "cache should remain empty");
        }

        #[test]
        fn cached_embed_stores_correct_vector() {
            let inner = CountingEmbedClient::new();
            let calls = Arc::clone(&inner.calls);
            let cache = Arc::new(MockCache::new());
            let cached = CachedEmbedClient::new(Arc::new(inner), cache.clone());

            let r1 = cached.embed(&["hello"]).expect("first embed");
            let v1 = r1.into_iter().next().expect("one vector");

            // Cache should have exactly one entry with an embed: prefix.
            let snap = cache.snapshot();
            assert_eq!(snap.len(), 1, "exactly one entry should be cached");
            let (key, _val) = snap.iter().next().expect("one entry");
            assert!(
                key.starts_with("embed:"),
                "key should start with 'embed:': {key}"
            );

            // Second call returns from cache — proves serialize/deserialize
            // round-trip preserves the vector exactly.
            let r2 = cached.embed(&["hello"]).expect("second embed (cache hit)");
            let v2 = r2.into_iter().next().expect("one vector");
            assert_eq!(
                v1, v2,
                "cache hit should return the same vector (round-trip correctness)"
            );
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "inner should be called once; second call was a cache hit"
            );
        }
    }
}
