// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit module for the Embed subsystem (T6/unified-architecture
//! Phase 2, Task 2.12).
//!
//! Implements [`Module`] / [`ModuleBuilder`] / [`WithConfig`] for
//! [`EmbedModule`], wiring the existing [`EmbedClient`] trait (Strategy
//! pattern) into the unified Kit registry as `Arc<dyn EmbedClient>` under
//! [`EmbedKey`](crate::kit::EmbedKey).
//!
//! # Capability lifecycle
//!
//! [`EmbedCapability`] owns an [`EmbeddingConfig`] (immutable, `Send + Sync`).
//! Each [`EmbedClient::embed`] invocation constructs a fresh
//! [`OpenAIEmbedClient`] over the configured endpoint and delegates. This
//! matches the existing `search_cmd::semantic_search` semantics (one client
//! per call). A future optimization could pre-construct the HTTP client and
//! cache it behind the capability; out of scope for Task 2.12.
//!
//! # Degradation
//!
//! When `EmbeddingConfig::has_api_key()` is `false` (no API key in env),
//! [`EmbedCapability::embed`] returns [`EmbedError::MissingApiKey`]. The
//! caller (e.g. `search_cmd`) is responsible for falling back to BM25 —
//! this mirrors the existing `semantic_search` logic.
//!
//! # Hot reconfiguration (future work)
//!
//! The spec mentions `EmbedConfig` via `ConfigHandle` for hot-reloading the
//! endpoint/model. This is **not implemented** in Task 2.12 — the current
//! capability takes `EmbeddingConfig` as a construction-time constant. Hot
//! reload would require refactoring the capability to read from a shared
//! `ConfigHandle<EmbeddingConfig>`. Tracked as future work; out of scope for
//! the unified-registry migration.
//!
//! # Dependency note
//!
//! Conceptually the Embed subsystem depends on `StorageKey` (it writes
//! vectors to the `Embedding` table via [`EmbeddingStorage`]). The concrete
//! [`EmbedCapability`] is self-contained, however: it only owns the
//! embedding-service config and does not touch the database directly
//! (storage operations are orchestrated by `search_cmd` via
//! `QueryFacade::connection()`). Therefore `Requirements = NoRequirements`
//! at the type level; the bootstrap (Task 2.13) enforces build ordering
//! (Storage → ... → Embed). This mirrors the
//! [`QueryModule`](crate::query::module::QueryModule),
//! [`TraceModule`](crate::trace::module::TraceModule), and
//! [`DaemonModule`](crate::daemon::module::DaemonModule) design — see
//! `design.md` D1 for the rationale.
//!
//! [`Module`]: crate::kit::Module
//! [`ModuleBuilder`]: crate::kit::ModuleBuilder
//! [`WithConfig`]: crate::kit::WithConfig
//! [`EmbeddingStorage`]: super::EmbeddingStorage
//! [`OpenAIEmbedClient`]: super::OpenAIEmbedClient
//! [`EmbeddingConfig`]: super::EmbeddingConfig

use std::sync::Arc;

use crate::kit::{Module, ModuleBuilder, NoRequirements, WithConfig};

use super::client::{EmbedClient, OpenAIEmbedClient};
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
// Module + Builder
// ---------------------------------------------------------------------------

/// trait-kit module tag for the Embed subsystem (Task 2.12).
///
/// Zero-sized marker — construction logic lives in
/// [`EmbedModuleBuilder::build`]. Register in Kit via:
///
/// ```ignore
/// use codenexus::kit::{EmbedKey, IntoKitModuleBuilder, Kit};
/// use codenexus::embed::{EmbedModuleBuilder, EmbeddingConfig};
///
/// let kit = Kit::new();
/// let embed = EmbedModuleBuilder::new()
///     .config(EmbeddingConfig::from_env())
///     .kit(&kit)
///     .provide::<EmbedKey>()?;
/// ```
pub struct EmbedModule;

/// Builder for [`EmbedModule`] (Task 2.12).
///
/// Construct with [`EmbedModuleBuilder::new`], inject config with
/// [`WithConfig::config`], then attach to a [`Kit`](crate::kit::Kit) via
/// [`IntoKitModuleBuilder::kit`](crate::kit::IntoKitModuleBuilder::kit) and
/// call [`provide`](crate::kit::KitModuleBuilder::provide).
pub struct EmbedModuleBuilder {
    config: Option<EmbeddingConfig>,
}

impl EmbedModuleBuilder {
    /// Creates a builder with no config set. Call `.config(...)` before
    /// building.
    #[must_use]
    pub fn new() -> Self {
        Self { config: None }
    }
}

impl Default for EmbedModuleBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for EmbedModule {
    type Config = EmbeddingConfig;
    type Requirements = NoRequirements;
    type Capability = Arc<dyn EmbedClient>;
    type Error = EmbedError;
    type Builder = EmbedModuleBuilder;
    const NAME: &'static str = "embed";
}

impl ModuleBuilder<EmbedModule> for EmbedModuleBuilder {
    fn build(self) -> Result<Arc<dyn EmbedClient>> {
        let config = self.config.ok_or_else(|| {
            EmbedError::Unavailable(
                "EmbedModuleBuilder requires config — call .config(EmbeddingConfig::from_env()) before build".to_string(),
            )
        })?;
        Ok(Arc::new(EmbedCapability { config }))
    }
}

impl WithConfig<EmbedModule> for EmbedModuleBuilder {
    fn config(self, config: EmbeddingConfig) -> Self {
        Self {
            config: Some(config),
        }
    }
}

// ---------------------------------------------------------------------------
// Concrete dyn EmbedClient implementation
// ---------------------------------------------------------------------------

/// Concrete implementation of [`dyn EmbedClient`] that constructs a fresh
/// [`OpenAIEmbedClient`] on every [`EmbedClient::embed`] call.
///
/// The capability owns only an [`EmbeddingConfig`] (immutable,
/// `Send + Sync`). Each `embed` invocation:
///
/// 1. Checks `config.has_api_key()` — returns [`EmbedError::MissingApiKey`]
///    if absent.
/// 2. Constructs a fresh [`OpenAIEmbedClient`] from the config.
/// 3. Delegates to `client.embed(texts)`.
///
/// This matches the existing `search_cmd::semantic_search` semantics (one
/// client per call). A future optimization could pre-construct and cache the
/// HTTP client; out of scope for Task 2.12.
struct EmbedCapability {
    /// Embedding-service config (endpoint, model, API key).
    config: EmbeddingConfig,
}

impl EmbedClient for EmbedCapability {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if !self.config.has_api_key() {
            return Err(EmbedError::MissingApiKey);
        }
        let client = OpenAIEmbedClient::new(self.config.clone())?;
        client.embed(texts)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::EmbedKey;

    #[test]
    fn builder_requires_config() {
        let result = EmbedModuleBuilder::new().build();
        assert!(result.is_err());
        let err = result.err().unwrap();
        let msg = err.to_string();
        assert!(
            msg.contains("config"),
            "missing-config error should mention config: {msg}"
        );
    }

    #[test]
    fn build_returns_send_sync_capability() {
        let cap = EmbedModuleBuilder::new()
            .config(EmbeddingConfig::default())
            .build()
            .expect("EmbedModuleBuilder::build");
        // If this compiles, EmbedCapability is Send + Sync (the dyn
        // EmbedClient bound requires it). The Arc<dyn EmbedClient> is also
        // Send + Sync.
        fn _assert_send_sync<T: Send + Sync>(_: &T) {}
        _assert_send_sync(&cap);
    }

    /// `embed` without an API key must return `EmbedError::MissingApiKey`
    /// (non-blocking failure path — no network access attempted).
    ///
    /// This is the only `embed` code path that is safe to exercise in a unit
    /// test: all other paths attempt real HTTP calls. End-to-end coverage
    /// lives in the `kit_bootstrap` integration test (Task 1.7).
    #[test]
    fn capability_embed_without_api_key_returns_missing_api_key() {
        // Ensure no env vars leak in from the host.
        std::env::remove_var(crate::embed::API_KEY_ENV);
        std::env::remove_var(crate::embed::OPENAI_API_KEY_ENV);

        let cap = EmbedModuleBuilder::new()
            .config(EmbeddingConfig::default()) // no api_key
            .build()
            .expect("EmbedModuleBuilder::build");
        let result = cap.embed(&["hello"]);
        assert!(
            matches!(result, Err(EmbedError::MissingApiKey)),
            "expected MissingApiKey, got {result:?}"
        );
    }

    /// Verify the full Kit registration flow works end-to-end.
    #[test]
    fn kit_registration_flow() {
        use crate::kit::{IntoKitModuleBuilder, Kit};

        let kit = Kit::new();
        let embed = EmbedModuleBuilder::new()
            .config(EmbeddingConfig::default())
            .kit(&kit)
            .provide::<EmbedKey>()
            .expect("provide::<EmbedKey>");

        assert!(kit.contains::<EmbedKey>());

        let required = kit.require::<EmbedKey>().expect("require::<EmbedKey>");
        assert!(Arc::ptr_eq(&embed, &required));
    }

    /// `EmbedConfig` is a re-export of `EmbeddingConfig` (type alias).
    #[test]
    fn embed_config_alias_matches_embedding_config() {
        let cfg: EmbedConfig = EmbeddingConfig::default();
        assert!(!cfg.has_api_key());
    }
}
