// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Configuration for the embedding subsystem.

use std::path::PathBuf;

/// Environment variable name for the embedding API key (primary).
pub const API_KEY_ENV: &str = "CODENEXUS_EMBED_API_KEY";

/// Environment variable name for the embedding API key (fallback, OpenAI).
pub const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";

/// Environment variable name for the embedding endpoint.
///
/// When set, forces remote HTTP mode. When unset, defaults to local
/// inference via `vecboost` (Candle).
pub const EMBED_ENDPOINT_ENV: &str = "CODENEXUS_EMBED_ENDPOINT";

/// Environment variable name for the local model file path.
///
/// Overrides [`DEFAULT_MODEL_PATH`] when set.
pub const EMBED_MODEL_PATH_ENV: &str = "CODENEXUS_EMBED_MODEL_PATH";

/// Environment variable name for the expected SHA-256 of the model weights.
///
/// When set, the local engine verifies `model.safetensors` against this
/// digest at load time and rejects a mismatch (`ModelFileCorrupted`).
pub const EMBED_MODEL_SHA256_ENV: &str = "CODENEXUS_EMBED_MODEL_SHA256";

/// Default path to the bundled `arctic-embed-xs` model directory.
///
/// Relative to the current working directory. The model is NOT bundled in the
/// repository (90 MB) — it must be downloaded separately and placed at this
/// path (or at a custom path via [`EmbeddingConfig::model_path`]).
///
/// Design D7 open question: the exact distribution method (git-lfs vs build.rs
/// download vs release asset) is unresolved. This default provides a stable
/// convention; users can override via `CODENEXUS_EMBED_MODEL_PATH`.
pub const DEFAULT_MODEL_PATH: &str = "assets/arctic-embed-xs";

/// Default filename for the HuggingFace tokenizer JSON file.
///
/// Located inside the model directory. Derived from [`DEFAULT_MODEL_PATH`] at
/// runtime.
pub const DEFAULT_TOKENIZER_FILENAME: &str = "tokenizer.json";

/// Whether `value` is a well-formed SHA-256 hex digest (exactly 64 hex
/// characters). Digests are normalized to lowercase before storage so the
/// engine's lowercase comparison matches regardless of the operator's casing.
fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit())
}

/// Configuration for the embedding subsystem.
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Base URL of the OpenAI-compatible embedding API.
    ///
    /// `None` → local inference via `vecboost` (Candle, offline mode, default).
    /// `Some(url)` → remote HTTP via [`OpenAIEmbedClient`](crate::embed::client::OpenAIEmbedClient).
    pub endpoint: Option<String>,
    /// Model name (HTTP mode) or model identifier (local mode).
    pub model: String,
    /// API key for HTTP mode (read from environment, not persisted — TRD §6.1).
    ///
    /// Ignored in local mode.
    pub api_key: Option<String>,
    /// Path to the local model directory.
    ///
    /// When `endpoint` is `None`, this points to the `arctic-embed-xs` model
    /// directory. Defaults to [`DEFAULT_MODEL_PATH`] when `None`.
    pub model_path: Option<PathBuf>,
    /// Expected SHA-256 (hex, case-insensitive) of `model.safetensors`.
    ///
    /// When `Some`, the local engine verifies the weights file against this
    /// digest at load time (tampering / supply-chain check) and rejects a
    /// mismatch. `None` skips verification.
    pub model_sha256: Option<String>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            // Default to local (offline) inference.
            endpoint: None,
            model: "arctic-embed-xs".to_string(),
            api_key: None,
            model_path: None,
            model_sha256: None,
        }
    }
}

impl EmbeddingConfig {
    /// Creates a config from environment variables.
    ///
    /// Reads:
    /// - `CODENEXUS_EMBED_ENDPOINT` (optional — if set, enables remote HTTP
    ///   mode; if unset, defaults to local vecboost mode)
    /// - `CODENEXUS_EMBED_API_KEY` (preferred) or `OPENAI_API_KEY` (fallback)
    ///   for HTTP mode
    /// - `CODENEXUS_EMBED_MODEL_PATH` (optional — overrides default model path
    ///   for local mode)
    /// - `CODENEXUS_EMBED_MODEL_SHA256` (optional — enables weights integrity
    ///   verification for local mode; must be a 64-character hex digest, an
    ///   ill-formed value is reported to stderr and ignored, leaving
    ///   verification disabled rather than silently misconfigured)
    ///
    /// Keys are held in memory only and never written to disk (TRD §6.1).
    #[must_use]
    pub fn from_env() -> Self {
        let endpoint = std::env::var(EMBED_ENDPOINT_ENV).ok();
        let api_key = std::env::var(API_KEY_ENV)
            .or_else(|_| std::env::var(OPENAI_API_KEY_ENV))
            .ok();
        let model_path = std::env::var(EMBED_MODEL_PATH_ENV).ok().map(PathBuf::from);
        let model_sha256 = std::env::var(EMBED_MODEL_SHA256_ENV)
            .ok()
            .and_then(|value| {
                let normalized = value.trim().to_ascii_lowercase();
                if is_sha256_hex(&normalized) {
                    Some(normalized)
                } else {
                    let display: String =
                        value.chars().filter(|c| !c.is_control()).take(80).collect();
                    eprintln!(
                        "{}",
                        crate::i18n::t("embed-invalid_model_sha256", &[("value", display)])
                    );
                    None
                }
            });
        Self {
            endpoint,
            api_key,
            model_path,
            model_sha256,
            ..Self::default()
        }
    }

    /// Returns `true` if configured for local (offline) inference.
    #[must_use]
    pub fn is_local(&self) -> bool {
        self.endpoint.is_none()
    }

    /// Returns `true` if configured for remote HTTP inference.
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

    /// Returns the resolved model file path.
    ///
    /// Uses [`EmbeddingConfig::model_path`] if set, otherwise falls back to
    /// [`DEFAULT_MODEL_PATH`].
    #[must_use]
    pub fn resolved_model_path(&self) -> PathBuf {
        self.model_path
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_MODEL_PATH))
    }

    /// Returns the resolved tokenizer file path.
    ///
    /// Derived from the model directory: [`DEFAULT_TOKENIZER_FILENAME`] inside
    /// the model directory.
    #[must_use]
    pub fn resolved_tokenizer_path(&self) -> PathBuf {
        self.resolved_model_path().join(DEFAULT_TOKENIZER_FILENAME)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_is_local_offline() {
        // Default is local (offline) — no endpoint, no API key.
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
            PathBuf::from("assets/arctic-embed-xs/tokenizer.json")
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
            PathBuf::from("/custom/model.onnx/tokenizer.json")
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
    fn config_from_env_reads_model_sha256() {
        let _lock = crate::embed::ENV_TEST_LOCK.lock().unwrap();
        std::env::remove_var(EMBED_ENDPOINT_ENV);
        // Format fixture only: any 64-char hex works, so a low-entropy
        // pattern keeps detect-secrets from flagging a realistic digest.
        std::env::set_var(EMBED_MODEL_SHA256_ENV, "AB".repeat(32));
        let cfg = EmbeddingConfig::from_env();
        assert_eq!(
            cfg.model_sha256.as_deref(),
            Some("ab".repeat(32).as_str()),
            "a well-formed digest is accepted and normalized to lowercase"
        );
        std::env::remove_var(EMBED_MODEL_SHA256_ENV);
    }

    /// An ill-formed digest must be dropped (verification stays off) with a
    /// stderr report — never forwarded to the engine, which would fail only
    /// at load time with an opaque mismatch.
    #[test]
    fn config_from_env_drops_ill_formed_model_sha256() {
        let _lock = crate::embed::ENV_TEST_LOCK.lock().unwrap();
        std::env::remove_var(EMBED_ENDPOINT_ENV);
        for bad in ["abc123", "", "  ", &"g".repeat(64), &"a".repeat(63)] {
            std::env::set_var(EMBED_MODEL_SHA256_ENV, bad);
            let cfg = EmbeddingConfig::from_env();
            assert!(
                cfg.model_sha256.is_none(),
                "ill-formed digest {bad:?} must be dropped"
            );
        }
        std::env::remove_var(EMBED_MODEL_SHA256_ENV);
    }

    #[test]
    fn is_sha256_hex_accepts_only_64_hex_chars() {
        assert!(is_sha256_hex(&"0a1b".repeat(16)));
        assert!(is_sha256_hex(&"A1F2".repeat(16)));
        assert!(!is_sha256_hex(&"a".repeat(63)));
        assert!(!is_sha256_hex(&"a".repeat(65)));
        assert!(!is_sha256_hex(&"g".repeat(64)));
        assert!(!is_sha256_hex(""));
    }

    #[test]
    fn config_default_has_no_model_sha256() {
        let cfg = EmbeddingConfig::default();
        assert!(cfg.model_sha256.is_none());
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
