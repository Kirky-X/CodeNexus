// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Embedding service client (SubTask 16.1).
//!
//! Defines the [`EmbedClient`] trait and two implementations:
//! - [`OpenAIEmbedClient`]: calls an OpenAI-compatible HTTP embedding API via
//!   [`reqwest::blocking`]. The API key is read from the environment and never
//!   persisted (TRD §6.1).
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

use serde::{Deserialize, Serialize};

use super::{EmbedError, EmbeddingConfig, Result, EMBEDDING_DIM};

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
pub struct OpenAIEmbedClient {
    config: EmbeddingConfig,
    http: reqwest::blocking::Client,
}

impl OpenAIEmbedClient {
    /// Creates a new client from the given [`EmbeddingConfig`].
    ///
    /// # Errors
    ///
    /// Returns [`EmbedError::MissingApiKey`] if no API key is configured.
    pub fn new(config: EmbeddingConfig) -> Result<Self> {
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
    /// # Errors
    ///
    /// Returns [`EmbedError::MissingApiKey`] if neither `CODENEXUS_EMBED_API_KEY`
    /// nor `OPENAI_API_KEY` is set.
    pub fn from_env() -> Result<Self> {
        Self::new(EmbeddingConfig::from_env())
    }

    /// Returns the configured endpoint URL.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.config.endpoint
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

        let url = format!("{}/embeddings", self.config.endpoint);
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
    fn openai_client_new_without_key_returns_error() {
        std::env::remove_var("CODENEXUS_EMBED_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");
        let cfg = EmbeddingConfig::from_env();
        let result = OpenAIEmbedClient::new(cfg);
        assert!(result.is_err(), "should error without API key");
        assert!(matches!(result.unwrap_err(), EmbedError::MissingApiKey));
    }

    #[test]
    fn openai_client_new_with_key_succeeds() {
        let cfg = EmbeddingConfig {
            api_key: Some("test-key".to_string()),
            ..EmbeddingConfig::default()
        };
        let client = OpenAIEmbedClient::new(cfg).expect("should succeed with key");
        assert!(client.endpoint().contains("openai.com"));
        assert!(!client.model().is_empty());
    }

    #[test]
    fn openai_client_from_env_without_key_errors() {
        std::env::remove_var("CODENEXUS_EMBED_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");
        let result = OpenAIEmbedClient::from_env();
        assert!(result.is_err());
    }

    #[test]
    fn openai_client_from_env_with_key_succeeds() {
        std::env::set_var("CODENEXUS_EMBED_API_KEY", "env-key");
        let client = OpenAIEmbedClient::from_env().expect("should succeed");
        assert_eq!(client.endpoint(), "https://api.openai.com/v1");
        std::env::remove_var("CODENEXUS_EMBED_API_KEY");
    }

    #[test]
    fn openai_client_debug_redacts_api_key() {
        let cfg = EmbeddingConfig {
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
        // Create a client with a key, then test the embed path.
        // We can't make a real HTTP call, but we can verify the client
        // is constructed correctly.
        let cfg = EmbeddingConfig {
            api_key: Some("test".to_string()),
            endpoint: "http://localhost:1".to_string(), // unreachable port
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
