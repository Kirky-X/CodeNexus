// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Optional vector embedding subsystem (Strategy pattern).
//!
//! Gated behind the `embed` feature (ADR-004). Provides an HTTP client for
//! OpenAI-compatible embedding APIs, vector storage in LadybugDB, and
//! pluggable search strategies (BM25 / semantic / hybrid).
//!
//! # Architecture
//!
//! - [`client`]: [`EmbedClient`] trait + [`OpenAIEmbedClient`] (reqwest HTTP).
//!   API keys are read from environment variables and never persisted (TRD §6.1).
//! - [`storage`]: [`EmbeddingStorage`] stores/retrieves `FLOAT[384]` vectors in
//!   the LadybugDB `Embedding` table (DDD §5.9).
//! - [`search`]: [`SearchStrategy`] trait with [`Bm25Strategy`],
//!   [`SemanticStrategy`], and [`HybridStrategy`] (RRF fusion, AC-SEARCH-002).
//!
//! # Degradation
//!
//! On Windows the LadybugDB VECTOR extension is unavailable (R-003/TR-005);
//! [`search::is_vector_supported`] returns `false` and the search strategy
//! degrades to BM25-only. If the embedding service is unreachable, indexing
//! continues without embeddings (SubTask 16.4).
//!
//! # trait-kit integration (Task 2.12)
//!
//! When the `embed` feature is enabled, [`client::EmbedClient`] is the
//! capability trait stored in [`Kit`](crate::kit::Kit) under
//! [`EmbedKey`](crate::kit::EmbedKey). The concrete impl
//! ([`module::EmbedCapability`]) wraps the existing [`OpenAIEmbedClient`] so
//! the unified Kit can hand a pre-configured embedder to `search_cmd` instead
//! of having the CLI construct clients ad-hoc.

pub mod client;
pub mod module;
pub mod search;
pub mod storage;

pub use client::{EmbedClient, MockEmbedClient, OpenAIEmbedClient};
pub use module::{EmbedConfig, EmbedModule, EmbedModuleBuilder};
pub use search::{
    Bm25Strategy, HybridStrategy, SearchStrategy, SearchStrategyType, SemanticStrategy,
};
pub use storage::{EmbeddingRecord, EmbeddingStorage};

use thiserror::Error;

/// Expected embedding dimension (DDD §5.9: `FLOAT[384]`).
pub const EMBEDDING_DIM: usize = 384;

/// Environment variable name for the embedding API key (primary).
pub const API_KEY_ENV: &str = "CODENEXUS_EMBED_API_KEY";

/// Environment variable name for the embedding API key (fallback, OpenAI).
pub const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";

/// Configuration for the embedding subsystem.
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Base URL of the OpenAI-compatible embedding API.
    pub endpoint: String,
    /// Model name to use for embeddings.
    pub model: String,
    /// API key (read from environment, not persisted — TRD §6.1).
    pub api_key: Option<String>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            endpoint: "https://api.openai.com/v1".to_string(),
            model: "text-embedding-3-small".to_string(),
            api_key: None,
        }
    }
}

impl EmbeddingConfig {
    /// Creates a config from environment variables.
    ///
    /// Reads `CODENEXUS_EMBED_API_KEY` (preferred) or `OPENAI_API_KEY`
    /// (fallback). The key is held in memory only and never written to disk
    /// (TRD §6.1).
    #[must_use]
    pub fn from_env() -> Self {
        let api_key = std::env::var(API_KEY_ENV)
            .or_else(|_| std::env::var(OPENAI_API_KEY_ENV))
            .ok();
        Self {
            api_key,
            ..Self::default()
        }
    }

    /// Returns `true` if an API key is configured.
    #[must_use]
    pub fn has_api_key(&self) -> bool {
        self.api_key.is_some()
    }
}

/// Unified error type for the embedding subsystem.
#[derive(Debug, Error)]
pub enum EmbedError {
    /// HTTP transport error from the embedding service.
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    /// Storage layer error.
    #[error("storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),

    /// JSON serialization/deserialization error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// The embedding API returned a non-2xx status.
    #[error("api error ({status}): {body}")]
    Api { status: u16, body: String },

    /// No API key found in environment variables.
    #[error("missing API key: set {API_KEY_ENV} or {OPENAI_API_KEY_ENV}")]
    MissingApiKey,

    /// Embedding dimension does not match `EMBEDDING_DIM`.
    #[error("embedding dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch { expected: usize, actual: usize },

    /// The embedding service is unavailable (network or config issue).
    #[error("embedding service unavailable: {0}")]
    Unavailable(String),

    /// The `Embedding` table is not available (VECTOR extension missing).
    #[error("embedding table not available (VECTOR extension may be missing)")]
    EmbeddingTableNotAvailable,
}

/// Convenience alias used throughout the embed subsystem.
pub type Result<T> = std::result::Result<T, EmbedError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_dim_is_384() {
        assert_eq!(EMBEDDING_DIM, 384, "DDD §5.9 requires FLOAT[384]");
    }

    #[test]
    fn config_default_has_openai_endpoint() {
        let cfg = EmbeddingConfig::default();
        assert!(cfg.endpoint.contains("openai.com"));
        assert!(!cfg.model.is_empty());
        assert!(!cfg.has_api_key());
    }

    #[test]
    fn config_from_env_reads_key() {
        // Set a key and verify it's picked up.
        std::env::set_var(API_KEY_ENV, "test-key-123");
        let cfg = EmbeddingConfig::from_env();
        assert_eq!(cfg.api_key.as_deref(), Some("test-key-123"));
        assert!(cfg.has_api_key());
        std::env::remove_var(API_KEY_ENV);
    }

    #[test]
    fn config_from_env_falls_back_to_openai_var() {
        std::env::remove_var(API_KEY_ENV);
        std::env::set_var(OPENAI_API_KEY_ENV, "openai-fallback");
        let cfg = EmbeddingConfig::from_env();
        assert_eq!(cfg.api_key.as_deref(), Some("openai-fallback"));
        std::env::remove_var(OPENAI_API_KEY_ENV);
    }

    #[test]
    fn config_from_env_no_key_returns_none() {
        std::env::remove_var(API_KEY_ENV);
        std::env::remove_var(OPENAI_API_KEY_ENV);
        let cfg = EmbeddingConfig::from_env();
        assert!(!cfg.has_api_key());
        assert!(cfg.api_key.is_none());
    }

    #[test]
    fn embed_error_missing_api_key_display() {
        let err = EmbedError::MissingApiKey;
        let msg = err.to_string();
        assert!(msg.contains("missing API key"), "got: {msg}");
        assert!(msg.contains(API_KEY_ENV), "got: {msg}");
    }

    #[test]
    fn embed_error_dimension_mismatch_display() {
        let err = EmbedError::DimensionMismatch {
            expected: 384,
            actual: 128,
        };
        let msg = err.to_string();
        assert!(msg.contains("384"), "got: {msg}");
        assert!(msg.contains("128"), "got: {msg}");
    }

    #[test]
    fn embed_error_unavailable_display() {
        let err = EmbedError::Unavailable("connection refused".to_string());
        let msg = err.to_string();
        assert!(msg.contains("unavailable"), "got: {msg}");
        assert!(msg.contains("connection refused"), "got: {msg}");
    }

    #[test]
    fn embed_error_api_display() {
        let err = EmbedError::Api {
            status: 401,
            body: "unauthorized".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("401"), "got: {msg}");
        assert!(msg.contains("unauthorized"), "got: {msg}");
    }

    #[test]
    fn embed_error_embedding_table_not_available_display() {
        let err = EmbedError::EmbeddingTableNotAvailable;
        let msg = err.to_string();
        assert!(msg.contains("not available"), "got: {msg}");
    }

    #[test]
    fn embed_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<EmbedError>();
    }

    #[test]
    fn embed_error_debug_includes_variant() {
        let err = EmbedError::MissingApiKey;
        let s = format!("{err:?}");
        assert!(s.contains("MissingApiKey"), "got: {s}");
    }
}
