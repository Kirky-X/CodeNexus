// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Error types for the embedding subsystem.

use thiserror::Error;

use crate::embed::config::{API_KEY_ENV, OPENAI_API_KEY_ENV};

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
