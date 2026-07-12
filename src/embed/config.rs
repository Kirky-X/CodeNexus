// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Configuration for the embedding subsystem.

use std::path::PathBuf;

/// Environment variable name for the embedding API key (primary).
pub const API_KEY_ENV: &str = "CODENEXUS_EMBED_API_KEY";

/// Environment variable name for the embedding API key (fallback, OpenAI).
pub const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";

/// Environment variable name for the embedding endpoint (H10/D7).
///
/// When set, forces remote HTTP mode. When unset, defaults to local ONNX
/// inference via `ort`.
pub const EMBED_ENDPOINT_ENV: &str = "CODENEXUS_EMBED_ENDPOINT";

/// Environment variable name for the local model file path (H10/D7).
///
/// Overrides [`DEFAULT_MODEL_PATH`] when set.
pub const EMBED_MODEL_PATH_ENV: &str = "CODENEXUS_EMBED_MODEL_PATH";

/// Default path to the bundled `arctic-embed-xs` ONNX model file (H10/D7).
///
/// Relative to the current working directory. The model is NOT bundled in the
/// repository (90 MB) — it must be downloaded separately and placed at this
/// path (or at a custom path via [`EmbeddingConfig::model_path`]).
///
/// Design D7 open question: the exact distribution method (git-lfs vs build.rs
/// download vs release asset) is unresolved. This default provides a stable
/// convention; users can override via `CODENEXUS_EMBED_MODEL_PATH`.
pub const DEFAULT_MODEL_PATH: &str = "assets/arctic-embed-xs.onnx";

/// Default path to the HuggingFace tokenizer JSON file (H10/D7).
///
/// Co-located with the model file (same directory, `tokenizer.json` filename).
/// Derived from [`DEFAULT_MODEL_PATH`] at runtime.
pub const DEFAULT_TOKENIZER_FILENAME: &str = "tokenizer.json";

/// Configuration for the embedding subsystem.
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Base URL of the OpenAI-compatible embedding API (H10/D7).
    ///
    /// `None` → local ONNX inference via `ort` (offline mode, default).
    /// `Some(url)` → remote HTTP via [`OpenAIEmbedClient`](crate::embed::client::OpenAIEmbedClient).
    pub endpoint: Option<String>,
    /// Model name (HTTP mode) or model identifier (local mode).
    pub model: String,
    /// API key for HTTP mode (read from environment, not persisted — TRD §6.1).
    ///
    /// Ignored in local mode.
    pub api_key: Option<String>,
    /// Path to the local ONNX model file (H10/D7).
    ///
    /// When `endpoint` is `None`, this points to the `arctic-embed-xs` ONNX
    /// model. Defaults to [`DEFAULT_MODEL_PATH`] when `None`.
    pub model_path: Option<PathBuf>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            // H10/D7: default to local (offline) inference.
            endpoint: None,
            model: "arctic-embed-xs".to_string(),
            api_key: None,
            model_path: None,
        }
    }
}

impl EmbeddingConfig {
    /// Creates a config from environment variables.
    ///
    /// Reads:
    /// - `CODENEXUS_EMBED_ENDPOINT` (optional — if set, enables remote HTTP
    ///   mode; if unset, defaults to local ONNX mode)
    /// - `CODENEXUS_EMBED_API_KEY` (preferred) or `OPENAI_API_KEY` (fallback)
    ///   for HTTP mode
    /// - `CODENEXUS_EMBED_MODEL_PATH` (optional — overrides default model path
    ///   for local mode)
    ///
    /// Keys are held in memory only and never written to disk (TRD §6.1).
    #[must_use]
    pub fn from_env() -> Self {
        let endpoint = std::env::var(EMBED_ENDPOINT_ENV).ok();
        let api_key = std::env::var(API_KEY_ENV)
            .or_else(|_| std::env::var(OPENAI_API_KEY_ENV))
            .ok();
        let model_path = std::env::var(EMBED_MODEL_PATH_ENV).ok().map(PathBuf::from);
        Self {
            endpoint,
            api_key,
            model_path,
            ..Self::default()
        }
    }

    /// Returns `true` if configured for local (offline) ONNX inference (H10/D7).
    #[must_use]
    pub fn is_local(&self) -> bool {
        self.endpoint.is_none()
    }

    /// Returns `true` if configured for remote HTTP inference (H10/D7).
    #[must_use]
    pub fn is_remote(&self) -> bool {
        self.endpoint.is_some()
    }

    /// Returns `true` if an API key is configured (HTTP mode only).
    ///
    /// In local mode, this is irrelevant — the API key is never used.
    #[must_use]
    pub fn has_api_key(&self) -> bool {
        self.api_key.is_some()
    }

    /// Returns the resolved model file path (H10/D7).
    ///
    /// Uses [`EmbeddingConfig::model_path`] if set, otherwise falls back to
    /// [`DEFAULT_MODEL_PATH`].
    #[must_use]
    pub fn resolved_model_path(&self) -> PathBuf {
        self.model_path
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_MODEL_PATH))
    }

    /// Returns the resolved tokenizer file path (H10/D7).
    ///
    /// Derived from the model path: same directory, [`DEFAULT_TOKENIZER_FILENAME`].
    #[must_use]
    pub fn resolved_tokenizer_path(&self) -> PathBuf {
        self.resolved_model_path()
            .with_file_name(DEFAULT_TOKENIZER_FILENAME)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_is_local_offline() {
        // H10/D7: default is local (offline) — no endpoint, no API key.
        let cfg = EmbeddingConfig::default();
        assert!(cfg.is_local(), "default should be local (offline)");
        assert!(!cfg.is_remote(), "default should not be remote");
        assert!(cfg.endpoint.is_none(), "default endpoint should be None");
        assert!(!cfg.model.is_empty());
        assert!(!cfg.has_api_key());
    }

    #[test]
    fn config_default_uses_arctic_model_name() {
        let cfg = EmbeddingConfig::default();
        assert_eq!(cfg.model, "arctic-embed-xs");
    }

    #[test]
    fn config_default_resolves_default_model_path() {
        let cfg = EmbeddingConfig::default();
        assert_eq!(cfg.resolved_model_path(), PathBuf::from(DEFAULT_MODEL_PATH));
        assert_eq!(
            cfg.resolved_tokenizer_path(),
            PathBuf::from("assets/tokenizer.json")
        );
    }

    #[test]
    fn config_with_custom_model_path() {
        let cfg = EmbeddingConfig {
            model_path: Some(PathBuf::from("/custom/model.onnx")),
            ..EmbeddingConfig::default()
        };
        assert_eq!(
            cfg.resolved_model_path(),
            PathBuf::from("/custom/model.onnx")
        );
        assert_eq!(
            cfg.resolved_tokenizer_path(),
            PathBuf::from("/custom/tokenizer.json")
        );
    }

    #[test]
    fn config_remote_mode_when_endpoint_set() {
        let cfg = EmbeddingConfig {
            endpoint: Some("https://api.openai.com/v1".to_string()),
            ..EmbeddingConfig::default()
        };
        assert!(cfg.is_remote());
        assert!(!cfg.is_local());
    }

    #[test]
    fn config_from_env_reads_key() {
        let _lock = crate::embed::ENV_TEST_LOCK.lock().unwrap();
        // Set a key and verify it's picked up.
        std::env::set_var(API_KEY_ENV, "test-key-123");
        std::env::remove_var(EMBED_ENDPOINT_ENV);
        std::env::remove_var(EMBED_MODEL_PATH_ENV);
        let cfg = EmbeddingConfig::from_env();
        assert_eq!(cfg.api_key.as_deref(), Some("test-key-123"));
        assert!(cfg.has_api_key());
        std::env::remove_var(API_KEY_ENV);
    }

    #[test]
    fn config_from_env_falls_back_to_openai_var() {
        let _lock = crate::embed::ENV_TEST_LOCK.lock().unwrap();
        std::env::remove_var(API_KEY_ENV);
        std::env::set_var(OPENAI_API_KEY_ENV, "openai-fallback");
        std::env::remove_var(EMBED_ENDPOINT_ENV);
        let cfg = EmbeddingConfig::from_env();
        assert_eq!(cfg.api_key.as_deref(), Some("openai-fallback"));
        std::env::remove_var(OPENAI_API_KEY_ENV);
    }

    #[test]
    fn config_from_env_no_key_returns_none() {
        let _lock = crate::embed::ENV_TEST_LOCK.lock().unwrap();
        std::env::remove_var(API_KEY_ENV);
        std::env::remove_var(OPENAI_API_KEY_ENV);
        std::env::remove_var(EMBED_ENDPOINT_ENV);
        let cfg = EmbeddingConfig::from_env();
        assert!(!cfg.has_api_key());
        assert!(cfg.api_key.is_none());
    }

    #[test]
    fn config_from_env_reads_endpoint_for_remote_mode() {
        let _lock = crate::embed::ENV_TEST_LOCK.lock().unwrap();
        std::env::set_var(EMBED_ENDPOINT_ENV, "https://custom.example.com/v1");
        std::env::remove_var(API_KEY_ENV);
        std::env::remove_var(OPENAI_API_KEY_ENV);
        let cfg = EmbeddingConfig::from_env();
        assert!(
            cfg.is_remote(),
            "setting endpoint should enable remote mode"
        );
        assert_eq!(
            cfg.endpoint.as_deref(),
            Some("https://custom.example.com/v1")
        );
        std::env::remove_var(EMBED_ENDPOINT_ENV);
    }

    #[test]
    fn config_from_env_reads_model_path() {
        let _lock = crate::embed::ENV_TEST_LOCK.lock().unwrap();
        std::env::set_var(EMBED_MODEL_PATH_ENV, "/custom/from/env/model.onnx");
        std::env::remove_var(EMBED_ENDPOINT_ENV);
        let cfg = EmbeddingConfig::from_env();
        assert_eq!(
            cfg.resolved_model_path(),
            PathBuf::from("/custom/from/env/model.onnx")
        );
        std::env::remove_var(EMBED_MODEL_PATH_ENV);
    }

    #[test]
    fn config_from_env_defaults_to_local_when_endpoint_unset() {
        let _lock = crate::embed::ENV_TEST_LOCK.lock().unwrap();
        std::env::remove_var(EMBED_ENDPOINT_ENV);
        let cfg = EmbeddingConfig::from_env();
        assert!(
            cfg.is_local(),
            "should default to local when endpoint unset"
        );
    }
}
