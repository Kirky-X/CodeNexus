// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit module for the Embed subsystem (T6/unified-architecture
//! Phase 2, Task 2.12; H10/D7 local ONNX support; v0.3.3 AsyncKit migration).
//!
//! Implements [`ModuleMeta`] + [`AsyncAutoBuilder`] for [`EmbedModule`],
//! wiring the existing [`EmbedClient`] trait (Strategy pattern) into the
//! unified Kit registry as `Arc<dyn EmbedClient>` under
//! [`EmbedModule`](crate::kit::EmbedModule).
//!
//! # Capability lifecycle (H10/D7)
//!
//! [`EmbedCapability`] owns an [`EmbeddingConfig`] and chooses the backend:
//!
//! - **Local mode** (`endpoint = None`, default): lazily loads a
//!   [`LocalEmbedClient`] (ort + arctic-embed-xs) on the first `embed()` call.
//!   The loaded client is cached behind a [`Mutex`] for reuse. If the model
//!   file is missing, `embed()` returns [`EmbedError::Unavailable`] with a
//!   clear message (Rule 12).
//! - **Remote mode** (`endpoint = Some(url)`): creates a fresh
//!   [`OpenAIEmbedClient`] per call (matches existing `search_cmd` semantics).
//!   Requires an API key — returns [`EmbedError::MissingApiKey`] if absent.
//!
//! # Degradation
//!
//! In local mode, if the model file is missing at `embed()` time, the error is
//! returned to the caller (e.g. `search_cmd`), which falls back to BM25. In
//! remote mode without an API key, the same fallback applies. The caller is
//! responsible for degradation — this mirrors the existing `semantic_search`
//! logic.
//!
//! # Hot reconfiguration (future work)
//!
//! The spec mentions `EmbedConfig` via `ConfigHandle` for hot-reloading the
//! endpoint/model. This is **not implemented** — the current capability takes
//! `EmbeddingConfig` as a construction-time constant. Hot reload would require
//! refactoring the capability to read from a shared `ConfigHandle`. Tracked as
//! future work; out of scope for the unified-registry migration.
//!
//! # Dependency note
//!
//! Conceptually the Embed subsystem depends on `StorageModule` (it writes
//! vectors to the `Embedding` table via [`EmbeddingStorage`]). The concrete
//! [`EmbedCapability`] is self-contained, however: it only owns the
//! embedding-service config and does not touch the database directly
//! (storage operations are orchestrated by `search_cmd` via
//! `QueryFacade::connection()`). Therefore `dependencies = &[]` at the type
//! level; the bootstrap (Task 2.13) enforces build ordering
//! (Storage → ... → Embed). This mirrors the
//! [`QueryModule`](crate::query::module::QueryModule),
//! [`TraceModule`](crate::trace::module::TraceModule), and
//! [`DaemonModule`](crate::daemon::module::DaemonModule) design — see
//! `design.md` D1 for the rationale.
//!
//! [`EmbeddingStorage`]: super::EmbeddingStorage
//! [`OpenAIEmbedClient`]: super::OpenAIEmbedClient
//! [`LocalEmbedClient`]: super::LocalEmbedClient
//! [`EmbeddingConfig`]: super::EmbeddingConfig

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::kit::{AsyncAutoBuilder, AsyncKit, ModuleMeta};

use super::client::{EmbedClient, LocalEmbedClient, OpenAIEmbedClient};
use super::{EmbedError, EmbeddingConfig, Result};

// ---------------------------------------------------------------------------
// Re-export
// ---------------------------------------------------------------------------

/// Re-export of [`EmbeddingConfig`] under the trait-kit convention name.
///
/// The spec calls this `EmbedConfig`, but the codebase has called it
/// `EmbeddingConfig` since SubTask 16.1. We follow the codebase convention
/// (Rule 11: convention beats novelty) and re-export under a shorter alias
/// so trait-kit consumers can write `embed::EmbedConfig`.
pub type EmbedConfig = EmbeddingConfig;

// ---------------------------------------------------------------------------
// Module (ModuleMeta + AsyncAutoBuilder)
// ---------------------------------------------------------------------------

/// trait-kit module tag for the Embed subsystem (Task 2.12).
///
/// Zero-sized marker — construction logic lives in
/// [`EmbedModule::build_cap`] (called from the [`AsyncAutoBuilder`] impl).
/// Register the capability in Kit via:
///
/// ```ignore
/// use codenexus::kit::{AsyncKit, EmbedModule};
/// use codenexus::embed::EmbeddingConfig;
///
/// let mut kit = AsyncKit::new();
/// kit.set_config(EmbeddingConfig::from_env());
/// kit.register::<EmbedModule>()?;
/// let kit = kit.build().await?;
/// let embed = kit.require::<EmbedModule>()?;
/// ```
pub struct EmbedModule;

impl ModuleMeta for EmbedModule {
    const NAME: &'static str = "embed";
    fn dependencies() -> &'static [(&'static str, TypeId)] {
        &[]
    }
}

impl AsyncAutoBuilder for EmbedModule {
    type Capability = Arc<dyn EmbedClient>;
    type Error = EmbedError;

    fn build<'a>(
        kit: &'a AsyncKit,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<Self::Capability, Self::Error>> + Send + 'a>>
    {
        Box::pin(async move {
            let config = kit
                .config::<EmbeddingConfig>()
                .map_err(|e| EmbedError::Unavailable(e.to_string()))?;
            Self::build_cap(&config)
        })
    }
}

impl EmbedModule {
    /// Constructs an EmbedCapability from the given config.
    ///
    /// Shared between [`AsyncAutoBuilder::build`] and tests so that
    /// capability-level tests can run without an async runtime.
    pub(crate) fn build_cap(config: &EmbeddingConfig) -> Result<Arc<dyn EmbedClient>> {
        Ok(Arc::new(EmbedCapability {
            config: config.clone(),
            local_client: Mutex::new(None),
        }))
    }
}

// ---------------------------------------------------------------------------
// Concrete dyn EmbedClient implementation
// ---------------------------------------------------------------------------

/// Concrete implementation of [`dyn EmbedClient`] that routes to either
/// [`LocalEmbedClient`] (offline ONNX) or [`OpenAIEmbedClient`] (remote HTTP)
/// based on [`EmbeddingConfig::endpoint`] (H10/D7).
///
/// # Local mode (endpoint = None, default)
///
/// The [`LocalEmbedClient`] is lazily loaded on the first `embed()` call and
/// cached behind a [`Mutex`] for subsequent calls. If the model file is
/// missing, `embed()` returns [`EmbedError::Unavailable`] — `build()` always
/// succeeds so that kit bootstrap doesn't fail when the model isn't present.
///
/// # Remote mode (endpoint = Some)
///
/// A fresh [`OpenAIEmbedClient`] is constructed on every `embed()` call
/// (matching the existing `search_cmd::semantic_search` semantics). Requires
/// an API key — returns [`EmbedError::MissingApiKey`] if absent.
struct EmbedCapability {
    /// Embedding-service config (endpoint, model, API key, model path).
    config: EmbeddingConfig,
    /// Lazily-loaded local ONNX client (H10/D7).
    ///
    /// `None` = not yet loaded (or local mode not in use).
    /// `Some(client)` = loaded and cached for reuse.
    local_client: Mutex<Option<LocalEmbedClient>>,
}

impl EmbedClient for EmbedCapability {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if self.config.is_local() {
            // H10/D7: local ONNX inference — lazy-load the model on first use.
            let mut guard = self.local_client.lock().map_err(|e| {
                EmbedError::Unavailable(format!("local_client mutex poisoned: {e}"))
            })?;
            if guard.is_none() {
                let client = LocalEmbedClient::new(&self.config)?;
                *guard = Some(client);
            }
            // unwrap is safe: we just ensured it's Some.
            guard
                .as_ref()
                .expect("local_client initialized")
                .embed(texts)
        } else {
            // Remote HTTP mode — create a fresh OpenAIEmbedClient per call.
            if !self.config.has_api_key() {
                return Err(EmbedError::MissingApiKey);
            }
            let client = OpenAIEmbedClient::new(self.config.clone())?;
            client.embed(texts)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{AsyncKit, EmbedModule};

    #[test]
    fn build_returns_send_sync_capability() {
        let cap =
            EmbedModule::build_cap(&EmbeddingConfig::default()).expect("EmbedModule::build_cap");
        // If this compiles, EmbedCapability is Send + Sync (the dyn
        // EmbedClient bound requires it). The Arc<dyn EmbedClient> is also
        // Send + Sync.
        fn _assert_send_sync<T: Send + Sync>(_: &T) {}
        _assert_send_sync(&cap);
    }

    /// `embed` in local mode without a model file returns `Unavailable`
    /// (non-blocking failure path — build succeeds, embed fails clearly).
    ///
    /// This verifies the lazy-loading path: `build()` succeeds even when the
    /// model is missing, and `embed()` returns a clear error.
    #[test]
    fn capability_embed_local_without_model_returns_unavailable() {
        // Ensure no env vars override the default local mode.
        std::env::remove_var(crate::embed::EMBED_ENDPOINT_ENV);
        std::env::remove_var(crate::embed::EMBED_MODEL_PATH_ENV);

        let cap =
            EmbedModule::build_cap(&EmbeddingConfig::default()).expect("EmbedModule::build_cap");

        let result = cap.embed(&["hello"]);
        assert!(result.is_err(), "should error without model file");
        let err = result.unwrap_err();
        assert!(
            matches!(err, EmbedError::Unavailable(ref msg) if msg.contains("not found")),
            "expected Unavailable with 'not found', got: {err}"
        );
    }

    /// `embed` in remote mode without an API key returns `MissingApiKey`
    /// (non-blocking failure path — no network access attempted).
    #[test]
    fn capability_embed_remote_without_api_key_returns_missing_api_key() {
        // Ensure no env vars leak in from the host.
        std::env::remove_var(crate::embed::API_KEY_ENV);
        std::env::remove_var(crate::embed::OPENAI_API_KEY_ENV);

        let cap = EmbedModule::build_cap(&EmbeddingConfig {
            endpoint: Some("https://api.openai.com/v1".to_string()),
            ..EmbeddingConfig::default()
        })
        .expect("EmbedModule::build_cap");
        let result = cap.embed(&["hello"]);
        assert!(
            matches!(result, Err(EmbedError::MissingApiKey)),
            "expected MissingApiKey, got {result:?}"
        );
    }

    /// Verify the full AsyncKit registration flow works end-to-end.
    #[tokio::test]
    async fn kit_registration_flow() {
        let mut kit = AsyncKit::new();
        kit.set_config(EmbeddingConfig::default());
        kit.register::<EmbedModule>()
            .expect("register::<EmbedModule>");
        let kit = kit.build().await.expect("build");

        assert!(kit.contains::<EmbedModule>(), "EmbedModule missing");

        let _required = kit
            .require::<EmbedModule>()
            .expect("require::<EmbedModule>");
    }

    /// `EmbedConfig` is a re-export of `EmbeddingConfig` (type alias).
    #[test]
    fn embed_config_alias_matches_embedding_config() {
        let cfg: EmbedConfig = EmbeddingConfig::default();
        assert!(cfg.is_local());
    }
}
