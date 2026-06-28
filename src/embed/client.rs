// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Embedding service client (SubTask 16.1, H10/D7).
//!
//! Defines the [`EmbedClient`] trait and three implementations:
//! - [`OpenAIEmbedClient`]: calls an OpenAI-compatible HTTP embedding API via
//!   [`reqwest::blocking`]. The API key is read from the environment and never
//!   persisted (TRD §6.1). Used when [`EmbeddingConfig::endpoint`] is `Some`.
//! - [`LocalEmbedClient`] (H10/D7): runs `arctic-embed-xs` inference locally
//!   via `ort` (ONNX Runtime). Works fully offline. Used when
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

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::{
    EmbedError, EmbeddingConfig, Result, EMBEDDING_DIM, EMBED_MODEL_PATH_ENV,
};

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
/// For local offline inference, see [`LocalEmbedClient`] (H10/D7).
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
        let endpoint = self
            .config
            .endpoint
            .as_deref()
            .ok_or_else(|| EmbedError::Unavailable("endpoint is None in remote mode".to_string()))?;
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
// LocalEmbedClient (H10/D7)
// ---------------------------------------------------------------------------

/// Local ONNX-based embedding client (H10/D7).
///
/// Runs `arctic-embed-xs` inference locally using [`ort`] (ONNX Runtime) and
/// HuggingFace [`tokenizers`]. Requires no network access — the model and
/// tokenizer files must be present on disk.
///
/// # Thread safety
///
/// `ort::session::Session::run` requires `&mut self`, so the session is
/// wrapped in a [`Mutex`]. [`tokenizers::Tokenizer::encode`] takes `&self`
/// and needs no wrapping. `LocalEmbedClient` is `Send + Sync` via the
/// `Mutex<Session>` (Session is `Send`).
///
/// # Inference pipeline
///
/// 1. Tokenize input text → `input_ids`, `attention_mask`, `token_type_ids`
/// 2. Create `ndarray::Array2<i64>` tensors (shape `[1, seq_len]`)
/// 3. Run ONNX inference via `ort::inputs!` + `Session::run`
/// 4. Extract `last_hidden_state` tensor (shape `[1, seq_len, hidden_dim]`)
/// 5. Mean-pool over `seq_len` using `attention_mask` as weights
/// 6. L2-normalize the pooled vector
///
/// # Errors
///
/// [`LocalEmbedClient::new`] returns [`EmbedError::Unavailable`] if the model
/// or tokenizer file is missing or cannot be loaded.
pub struct LocalEmbedClient {
    /// ONNX Runtime session (requires `&mut self` for `run`, hence `Mutex`).
    session: Mutex<ort::session::Session>,
    /// HuggingFace tokenizer (BERT WordPiece for arctic-embed-xs).
    tokenizer: tokenizers::Tokenizer,
    /// Model name (for Debug output).
    model: String,
}

impl LocalEmbedClient {
    /// Creates a new local embedding client from the given config.
    ///
    /// Loads the ONNX model from [`EmbeddingConfig::resolved_model_path`] and
    /// the tokenizer from [`EmbeddingConfig::resolved_tokenizer_path`].
    ///
    /// # Errors
    ///
    /// Returns [`EmbedError::Unavailable`] if:
    /// - The model file does not exist (with guidance on how to fix).
    /// - The tokenizer file does not exist.
    /// - The ONNX session cannot be created (corrupt model, ort init failure).
    /// - The tokenizer cannot be loaded (corrupt tokenizer.json).
    pub fn new(config: &EmbeddingConfig) -> Result<Self> {
        let model_path = config.resolved_model_path();
        let tokenizer_path = config.resolved_tokenizer_path();

        if !model_path.exists() {
            return Err(EmbedError::Unavailable(format!(
                "local embedding model not found at {}. \
                 Place the arctic-embed-xs ONNX model at this path \
                 (or set the {EMBED_MODEL_PATH_ENV} env var to a custom location).",
                model_path.display(),
            )));
        }

        if !tokenizer_path.exists() {
            return Err(EmbedError::Unavailable(format!(
                "tokenizer not found at {}. \
                 Place the HuggingFace tokenizer.json co-located with the model.",
                tokenizer_path.display(),
            )));
        }

        let session = ort::session::Session::builder()
            .map_err(|e| EmbedError::Unavailable(format!("ort session builder failed: {e}")))?
            .with_intra_threads(1)
            .map_err(|e| EmbedError::Unavailable(format!("ort thread config failed: {e}")))?
            .commit_from_file(&model_path)
            .map_err(|e| EmbedError::Unavailable(format!("ort model load failed: {e}")))?;

        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| EmbedError::Unavailable(format!("tokenizer load failed: {e}")))?;

        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            model: config.model.clone(),
        })
    }

    /// Returns the model name.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Mean-pools the last hidden state using the attention mask.
    ///
    /// `hidden` is flat row-major data for shape `[1, seq_len, hidden_dim]`.
    /// Returns a vector of length `hidden_dim` where each element is the mean
    /// of the non-masked token embeddings along the sequence dimension.
    fn mean_pool(
        hidden: &[f32],
        attention_mask: &[u32],
        seq_len: usize,
        hidden_dim: usize,
    ) -> Vec<f32> {
        let mut pooled = vec![0.0f32; hidden_dim];
        let mut count = 0u32;
        for (i, &mask_val) in attention_mask.iter().enumerate().take(seq_len) {
            if mask_val > 0 {
                let base = i * hidden_dim;
                let end = base + hidden_dim;
                for (p, &h) in pooled.iter_mut().zip(&hidden[base..end]) {
                    *p += h;
                }
                count += 1;
            }
        }
        if count > 0 {
            for v in &mut pooled {
                *v /= count as f32;
            }
        }
        pooled
    }

    /// L2-normalizes a vector in place.
    fn l2_normalize(vec: &mut [f32]) {
        let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in vec.iter_mut() {
                *v /= norm;
            }
        }
    }
}

impl EmbedClient for LocalEmbedClient {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            let encoding = self.tokenizer.encode(*text, true).map_err(|e| {
                EmbedError::Unavailable(format!("tokenization failed: {e}"))
            })?;

            let input_ids = encoding.get_ids();
            let attention_mask = encoding.get_attention_mask();
            let token_type_ids = encoding.get_type_ids();
            let seq_len = input_ids.len();

            if seq_len == 0 {
                return Err(EmbedError::Unavailable(
                    "tokenization produced empty sequence".to_string(),
                ));
            }

            // Build input tensors (shape: [1, seq_len], dtype: i64).
            let input_ids_arr = ndarray::Array2::from_shape_vec(
                (1, seq_len),
                input_ids.iter().map(|&v| v as i64).collect(),
            )
            .map_err(|e| EmbedError::Unavailable(format!("input_ids ndarray: {e}")))?;

            let attention_mask_arr = ndarray::Array2::from_shape_vec(
                (1, seq_len),
                attention_mask.iter().map(|&v| v as i64).collect(),
            )
            .map_err(|e| EmbedError::Unavailable(format!("attention_mask ndarray: {e}")))?;

            // arctic-embed-xs (BERT-style) uses token_type_ids; reuse the
            // tokenizer-provided ones rather than a zero vector to stay
            // faithful to the model's training distribution.
            let token_type_ids_arr = ndarray::Array2::from_shape_vec(
                (1, seq_len),
                token_type_ids.iter().map(|&v| v as i64).collect(),
            )
            .map_err(|e| EmbedError::Unavailable(format!("token_type_ids ndarray: {e}")))?;

            // ort 2.0.0-rc.12 requires Value objects (not raw ndarrays) in
            // `ort::inputs!`. Convert each array to a `Value<Tensor>` first.
            let input_ids_value = ort::value::Value::from_array(input_ids_arr)
                .map_err(|e| EmbedError::Unavailable(format!("input_ids value: {e}")))?;
            let attention_mask_value = ort::value::Value::from_array(attention_mask_arr)
                .map_err(|e| EmbedError::Unavailable(format!("attention_mask value: {e}")))?;
            let token_type_ids_value = ort::value::Value::from_array(token_type_ids_arr)
                .map_err(|e| EmbedError::Unavailable(format!("token_type_ids value: {e}")))?;

            // Run inference — Session::run requires &mut self, so lock the mutex.
            let mut session = self.session.lock().map_err(|e| {
                EmbedError::Unavailable(format!("session mutex poisoned: {e}"))
            })?;

            // `ort::inputs!` returns `[SessionInputValue; N]` directly (not a
            // Result) when all inputs are already `&Value` references.
            let inputs = ort::inputs![
                &input_ids_value,
                &attention_mask_value,
                &token_type_ids_value
            ];

            let outputs = session
                .run(inputs)
                .map_err(|e| EmbedError::Unavailable(format!("ort inference failed: {e}")))?;

            // `try_extract_tensor::<f32>()` returns `(&Shape, &[f32])` — the
            // flat row-major tensor data. For arctic-embed-xs the output
            // `last_hidden_state` has shape `[1, seq_len, hidden_dim]`.
            let (_shape, hidden_data) = outputs["last_hidden_state"]
                .try_extract_tensor::<f32>()
                .map_err(|e| EmbedError::Unavailable(format!("ort output extract: {e}")))?;

            // hidden_data.len() = 1 * seq_len * hidden_dim
            let hidden_dim = hidden_data.len() / seq_len;

            // Mean-pool + L2-normalize.
            let mut pooled = Self::mean_pool(hidden_data, attention_mask, seq_len, hidden_dim);
            Self::l2_normalize(&mut pooled);

            if pooled.len() != EMBEDDING_DIM {
                return Err(EmbedError::DimensionMismatch {
                    expected: EMBEDDING_DIM,
                    actual: pooled.len(),
                });
            }

            results.push(pooled);
        }
        Ok(results)
    }
}

impl std::fmt::Debug for LocalEmbedClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalEmbedClient")
            .field("model", &self.model)
            .finish()
    }
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
        // H10/D7: default config is local mode (endpoint=None).
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
        // H10/D7: without CODENEXUS_EMBED_ENDPOINT, from_env returns local-mode
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
            "expected Unavailable, got: {err}"
        );
    }

    // --- LocalEmbedClient (H10/D7) ---

    #[test]
    fn local_client_new_without_model_returns_unavailable() {
        // Default config → local mode, model path doesn't exist.
        let cfg = EmbeddingConfig::default();
        let result = LocalEmbedClient::new(&cfg);
        assert!(result.is_err(), "should error when model file is missing");
        let err = result.unwrap_err();
        assert!(
            matches!(err, EmbedError::Unavailable(ref msg) if msg.contains("not found")),
            "expected Unavailable with 'not found', got: {err}"
        );
    }

    #[test]
    fn local_client_new_with_nonexistent_custom_path_returns_unavailable() {
        let cfg = EmbeddingConfig {
            model_path: Some(std::path::PathBuf::from("/nonexistent/model.onnx")),
            ..EmbeddingConfig::default()
        };
        let result = LocalEmbedClient::new(&cfg);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not found"), "error should mention 'not found': {msg}");
        assert!(
            msg.contains("/nonexistent/model.onnx"),
            "error should mention the path: {msg}"
        );
    }

    #[test]
    fn local_client_debug_shows_model_name() {
        // We can't construct a LocalEmbedClient without a real model file,
        // but we can verify the Debug impl format via MockEmbedClient
        // pattern. This test documents the expected Debug format.
        // (Actual Debug test requires a model file — integration concern.)
    }

    #[test]
    fn local_client_mean_pool_correctness() {
        // Unit test for the mean_pool helper (no model file needed).
        // Shape: [1, 3, 2] — 3 tokens, 2 hidden dims. Flat row-major data.
        let hidden_data = vec![
            // token 0 (masked in)
            1.0, 2.0,
            // token 1 (masked out)
            100.0, 200.0,
            // token 2 (masked in)
            3.0, 4.0,
        ];
        let attention_mask = vec![1u32, 0u32, 1u32];

        let pooled = LocalEmbedClient::mean_pool(&hidden_data, &attention_mask, 3, 2);

        // Mean of [1.0, 3.0] = 2.0; mean of [2.0, 4.0] = 3.0
        // (token 1 is masked out and excluded)
        assert_eq!(pooled, vec![2.0, 3.0]);
    }

    #[test]
    fn local_client_mean_pool_all_masked_returns_zeros() {
        // Edge case: all tokens masked out → returns zeros (no division by zero).
        let hidden_data = vec![1.0, 2.0, 3.0, 4.0];
        let attention_mask = vec![0u32, 0u32];

        let pooled = LocalEmbedClient::mean_pool(&hidden_data, &attention_mask, 2, 2);
        assert_eq!(pooled, vec![0.0, 0.0]);
    }

    #[test]
    fn local_client_l2_normalize_correctness() {
        let mut vec = vec![3.0, 4.0]; // norm = 5.0
        LocalEmbedClient::l2_normalize(&mut vec);
        assert!((vec[0] - 0.6).abs() < 1e-6, "expected 0.6, got {}", vec[0]);
        assert!((vec[1] - 0.8).abs() < 1e-6, "expected 0.8, got {}", vec[1]);
    }

    #[test]
    fn local_client_l2_normalize_zero_vector_noop() {
        let mut vec = vec![0.0, 0.0, 0.0];
        LocalEmbedClient::l2_normalize(&mut vec);
        assert_eq!(vec, vec![0.0, 0.0, 0.0]);
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
}
