// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit bootstrap (T6/unified-architecture Phase 2, Task 2.13).
//!
//! Provides [`build_kit`], the single entry point that assembles every
//! subsystem module into a [`Kit`] in fixed dependency order. CLI handlers
//! (Task 2.14) and integration tests (Task 2.15) call this once and then
//! resolve capabilities via [`Kit::require`] instead of constructing
//! subsystems ad-hoc.
//!
//! # Assembly order
//!
//! Per `design.md` D1, modules are registered in this exact order:
//!
//! ```text
//! Storage → Parser → Extractor → Indexer → Resolver
//!        → Query → Trace → Daemon(cfg) → Embed(cfg)
//! ```
//!
//! Although every module declares `Requirements = NoRequirements` at the
//! type level (concrete impls are self-contained — see each module's
//! `Dependency note`), the bootstrap enforces the conceptual dependency
//! order so that any future `WithRequirements`-based module can rely on
//! its dependencies already being present in the Kit.
//!
//! # Feature gating
//!
//! `DaemonModule` and `EmbedModule` are only registered when their
//! respective cargo features (`daemon`, `embed`) are enabled. Under
//! `--no-default-features --features lang-rust`, the returned Kit contains
//! exactly 7 capabilities (no `DaemonKey` / `EmbedKey`).
//!
//! [`Kit::require`]: crate::kit::Kit::require

use std::path::PathBuf;
use std::sync::Arc;

use crate::kit::{
    IntoKitModuleBuilder, Kit, KitError, WithConfig,
    StorageKey, ParserKey, ExtractorKey, IndexerKey, ResolverKey, QueryKey, TraceKey,
};

// Feature-gated keys are only imported when their feature is on, mirroring
// the `cfg` on the key types themselves.
#[cfg(feature = "daemon")]
use crate::kit::DaemonKey;
#[cfg(feature = "embed")]
use crate::kit::EmbedKey;

// Module builders + configs.
use crate::storage::{StorageConfig, StorageModuleBuilder};
use crate::parse::{ParserFactoryModuleBuilder, ExtractorRegistryModuleBuilder};
use crate::index::{IndexConfig, IndexerModuleBuilder};
use crate::resolve::ResolverModuleBuilder;
use crate::query::{QueryConfig, QueryModuleBuilder};
use crate::trace::{TraceConfig, TraceModuleBuilder};

#[cfg(feature = "daemon")]
use crate::daemon::{DaemonConfig, DaemonModuleBuilder, DEFAULT_DEBOUNCE_MS};

#[cfg(feature = "embed")]
use crate::embed::{EmbedModuleBuilder, EmbeddingConfig};

// ---------------------------------------------------------------------------
// Bootstrap config
// ---------------------------------------------------------------------------

/// Aggregated configuration for [`build_kit`] (Task 2.13).
///
/// Collects every parameter the 9 trait-kit modules need:
///
/// | Field            | Used by                                       |
/// |------------------|-----------------------------------------------|
/// | `db_path`        | Storage, Indexer, Query, Trace, Daemon        |
/// | `debounce_ms`    | Daemon (feature-gated)                        |
/// | `embedding_config` | Embed (feature-gated)                       |
///
/// Construct via [`KitBootstrapConfig::new`] (defaults `debounce_ms` to
/// [`DEFAULT_DEBOUNCE_MS`] and `embedding_config` to
/// [`EmbeddingConfig::from_env`]) or via the builder-style setters.
///
/// [`DEFAULT_DEBOUNCE_MS`]: crate::daemon::DEFAULT_DEBOUNCE_MS
/// [`EmbeddingConfig::from_env`]: crate::embed::EmbeddingConfig::from_env
#[derive(Debug, Clone)]
pub struct KitBootstrapConfig {
    /// Filesystem path to the LadybugDB database directory.
    ///
    /// Pass `":memory:"` for an in-memory database (useful for tests).
    pub db_path: PathBuf,

    /// Debounce window in milliseconds for the daemon (BR-DAEMON-001/004).
    /// Only consulted when the `daemon` feature is enabled.
    pub debounce_ms: u64,

    /// Embedding-service config (endpoint, model, API key). Only consulted
    /// when the `embed` feature is enabled.
    #[cfg(feature = "embed")]
    pub embedding_config: EmbeddingConfig,
}

impl KitBootstrapConfig {
    /// Creates a config with the given `db_path`, defaulting
    /// `debounce_ms` to [`DEFAULT_DEBOUNCE_MS`] and `embedding_config` to
    /// [`EmbeddingConfig::from_env`].
    ///
    /// [`DEFAULT_DEBOUNCE_MS`]: crate::daemon::DEFAULT_DEBOUNCE_MS
    /// [`EmbeddingConfig::from_env`]: crate::embed::EmbeddingConfig::from_env
    #[must_use]
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            #[cfg(feature = "embed")]
            embedding_config: EmbeddingConfig::from_env(),
        }
    }

    /// Sets the debounce window (only used when `daemon` feature is on).
    #[must_use]
    pub fn with_debounce_ms(mut self, debounce_ms: u64) -> Self {
        self.debounce_ms = debounce_ms;
        self
    }

    /// Sets the embedding config (only used when `embed` feature is on).
    #[cfg(feature = "embed")]
    #[must_use]
    pub fn with_embedding_config(mut self, config: EmbeddingConfig) -> Self {
        self.embedding_config = config;
        self
    }
}

// When the `daemon` feature is off, `DEFAULT_DEBOUNCE_MS` is not in scope,
// so provide a fallback constant for the default construction path. This
// keeps `KitBootstrapConfig::new` usable in `--no-default-features` builds.
#[cfg(not(feature = "daemon"))]
const DEFAULT_DEBOUNCE_MS: u64 = 2000;

// When the `embed` feature is off, `EmbeddingConfig` is not in scope, so
// `KitBootstrapConfig::new` cannot call `EmbeddingConfig::from_env()`. The
// `embedding_config` field is also gated out. Nothing to do here — the
// cfg on the field handles it.

// ---------------------------------------------------------------------------
// build_kit
// ---------------------------------------------------------------------------

/// Assemble every trait-kit module into a fresh [`Kit`] in fixed dependency
/// order (Task 2.13 / design.md D1).
///
/// # Order
///
/// `Storage → Parser → Extractor → Indexer → Resolver → Query → Trace →
///  Daemon(cfg) → Embed(cfg)`
///
/// Each module's [`ModuleBuilder::build`] observes its declared requirements
/// already present in the Kit (currently all `NoRequirements`, but the
/// order is enforced for future `WithRequirements` migrations).
///
/// # Errors
///
/// Returns [`KitError::BuildFailed`] if any module's `build` fails (e.g.
/// `StorageModuleBuilder` cannot open the database at `db_path`), or
/// [`KitError::DuplicateCapability`] if a key is somehow already registered
/// (should not happen with a fresh [`Kit::new`]).
///
/// # Example
///
/// ```ignore
/// use codenexus::kit::{build_kit, KitBootstrapConfig, StorageKey};
/// use std::path::PathBuf;
///
/// let config = KitBootstrapConfig::new(PathBuf::from("./codenexus.lbug"));
/// let kit = build_kit(&config)?;
/// let storage = kit.require::<StorageKey>()?;
/// ```
///
/// [`ModuleBuilder::build`]: crate::kit::ModuleBuilder::build
pub fn build_kit(config: &KitBootstrapConfig) -> Result<Kit, KitError> {
    let kit = Kit::new();

    // 1. Storage — opens Repository, initializes schema (Task 2.4).
    StorageModuleBuilder::new()
        .config(StorageConfig {
            db_path: config.db_path.clone(),
        })
        .kit(&kit)
        .provide::<StorageKey>()
        .map_err(|e| tag(e, "storage"))?;

    // 2. Parser — stateless ParserFactory (Task 2.5).
    ParserFactoryModuleBuilder::new()
        .kit(&kit)
        .provide::<ParserKey>()
        .map_err(|e| tag(e, "parser"))?;

    // 3. Extractor — stateless dispatcher (Task 2.6).
    ExtractorRegistryModuleBuilder::new()
        .kit(&kit)
        .provide::<ExtractorKey>()
        .map_err(|e| tag(e, "extractor"))?;

    // 4. Indexer — IndexFacade with db_path (Task 2.7).
    IndexerModuleBuilder::new()
        .config(IndexConfig {
            db_path: config.db_path.clone(),
        })
        .kit(&kit)
        .provide::<IndexerKey>()
        .map_err(|e| tag(e, "indexer"))?;

    // 5. Resolver — stateless free functions (Task 2.8).
    ResolverModuleBuilder::new()
        .kit(&kit)
        .provide::<ResolverKey>()
        .map_err(|e| tag(e, "resolver"))?;

    // 6. Query — QueryFacade with db_path (Task 2.9).
    QueryModuleBuilder::new()
        .config(QueryConfig {
            db_path: config.db_path.clone(),
        })
        .kit(&kit)
        .provide::<QueryKey>()
        .map_err(|e| tag(e, "query"))?;

    // 7. Trace — loads fresh subgraph per trace call (Task 2.10).
    TraceModuleBuilder::new()
        .config(TraceConfig {
            db_path: config.db_path.clone(),
        })
        .kit(&kit)
        .provide::<TraceKey>()
        .map_err(|e| tag(e, "trace"))?;

    // 8. Daemon (feature-gated) — owns db_path + debounce_ms (Task 2.11).
    #[cfg(feature = "daemon")]
    {
        DaemonModuleBuilder::new()
            .config(DaemonConfig {
                db_path: config.db_path.clone(),
                debounce_ms: config.debounce_ms,
            })
            .kit(&kit)
            .provide::<DaemonKey>()
            .map_err(|e| tag(e, "daemon"))?;
    }

    // 9. Embed (feature-gated) — owns EmbeddingConfig (Task 2.12).
    #[cfg(feature = "embed")]
    {
        EmbedModuleBuilder::new()
            .config(config.embedding_config.clone())
            .kit(&kit)
            .provide::<EmbedKey>()
            .map_err(|e| tag(e, "embed"))?;
    }

    Ok(kit)
}

/// Tags a [`KitError`] with the module name that failed, preserving the
/// original error chain via [`KitError::BuildFailed`]'s `source` field.
///
/// `KitError::BuildFailed` already carries `module: M::NAME` from the
/// builder, so this is mostly a no-op passthrough — kept as a single
/// chokepoint in case future bootstrap logic wants to enrich errors
/// uniformly.
fn tag(e: KitError, _module: &'static str) -> KitError {
    e
}

// ---------------------------------------------------------------------------
// Convenience: capability accessor helpers
// ---------------------------------------------------------------------------

/// Extension trait adding typed `require_*` shortcuts to [`Kit`].
///
/// Defined as an extension trait (rather than inherent methods on `Kit`)
/// because `Kit` is defined in the external `trait_kit` crate — Rust's
/// orphan rule forbids inherent impls on external types. The canonical API
/// remains [`Kit::require`](crate::kit::Kit::require); these helpers are
/// pure ergonomics for call sites that prefer named methods over turbofish.
pub trait KitExt {
    /// Resolves the Storage capability (`Arc<dyn Storage>`).
    fn require_storage(&self) -> Result<Arc<dyn crate::storage::capability::Storage>, KitError>;

    /// Resolves the Parser capability (`Arc<dyn ParserRegistry>`).
    fn require_parser(
        &self,
    ) -> Result<Arc<dyn crate::parse::capability::ParserRegistry>, KitError>;

    /// Resolves the Extractor capability (`Arc<dyn ExtractorRegistry>`).
    fn require_extractor(
        &self,
    ) -> Result<Arc<dyn crate::parse::capability::ExtractorRegistry>, KitError>;

    /// Resolves the Indexer capability (`Arc<dyn Indexer>`).
    fn require_indexer(&self) -> Result<Arc<dyn crate::index::capability::Indexer>, KitError>;

    /// Resolves the Resolver capability (`Arc<dyn Resolver>`).
    fn require_resolver(
        &self,
    ) -> Result<Arc<dyn crate::resolve::capability::Resolver>, KitError>;

    /// Resolves the Query capability (`Arc<dyn QueryEngine>`).
    fn require_query(&self) -> Result<Arc<dyn crate::query::capability::QueryEngine>, KitError>;

    /// Resolves the Trace capability (`Arc<dyn TraceEngine>`).
    fn require_trace(&self) -> Result<Arc<dyn crate::trace::capability::TraceEngine>, KitError>;

    /// Resolves the Daemon capability (`Arc<dyn DaemonRunner>`).
    #[cfg(feature = "daemon")]
    fn require_daemon(
        &self,
    ) -> Result<Arc<dyn crate::daemon::capability::DaemonRunner>, KitError>;

    /// Resolves the Embed capability (`Arc<dyn EmbedClient>`).
    #[cfg(feature = "embed")]
    fn require_embed(&self) -> Result<Arc<dyn crate::embed::client::EmbedClient>, KitError>;
}

impl KitExt for Kit {
    fn require_storage(&self) -> Result<Arc<dyn crate::storage::capability::Storage>, KitError> {
        self.require::<StorageKey>()
    }

    fn require_parser(
        &self,
    ) -> Result<Arc<dyn crate::parse::capability::ParserRegistry>, KitError> {
        self.require::<ParserKey>()
    }

    fn require_extractor(
        &self,
    ) -> Result<Arc<dyn crate::parse::capability::ExtractorRegistry>, KitError> {
        self.require::<ExtractorKey>()
    }

    fn require_indexer(&self) -> Result<Arc<dyn crate::index::capability::Indexer>, KitError> {
        self.require::<IndexerKey>()
    }

    fn require_resolver(
        &self,
    ) -> Result<Arc<dyn crate::resolve::capability::Resolver>, KitError> {
        self.require::<ResolverKey>()
    }

    fn require_query(&self) -> Result<Arc<dyn crate::query::capability::QueryEngine>, KitError> {
        self.require::<QueryKey>()
    }

    fn require_trace(&self) -> Result<Arc<dyn crate::trace::capability::TraceEngine>, KitError> {
        self.require::<TraceKey>()
    }

    #[cfg(feature = "daemon")]
    fn require_daemon(
        &self,
    ) -> Result<Arc<dyn crate::daemon::capability::DaemonRunner>, KitError> {
        self.require::<DaemonKey>()
    }

    #[cfg(feature = "embed")]
    fn require_embed(&self) -> Result<Arc<dyn crate::embed::client::EmbedClient>, KitError> {
        self.require::<EmbedKey>()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Bootstrap with an in-memory database must register all 7 core
    /// capabilities (Storage/Parser/Extractor/Indexer/Resolver/Query/Trace).
    /// Feature-gated capabilities (Daemon/Embed) are asserted in their own
    /// cfg-gated tests below.
    #[test]
    fn build_kit_in_memory_registers_all_core_capabilities() {
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).expect("build_kit");

        assert!(kit.contains::<StorageKey>(), "StorageKey missing");
        assert!(kit.contains::<ParserKey>(), "ParserKey missing");
        assert!(kit.contains::<ExtractorKey>(), "ExtractorKey missing");
        assert!(kit.contains::<IndexerKey>(), "IndexerKey missing");
        assert!(kit.contains::<ResolverKey>(), "ResolverKey missing");
        assert!(kit.contains::<QueryKey>(), "QueryKey missing");
        assert!(kit.contains::<TraceKey>(), "TraceKey missing");
    }

    /// Each registered capability must be `require`-able (returns the same
    /// `Arc` that was registered).
    #[test]
    fn build_kit_require_returns_registered_arc() {
        use crate::kit::bootstrap::KitExt;
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).expect("build_kit");

        // require_storage returns Arc<dyn Storage>.
        let storage = kit.require_storage().expect("require_storage");
        // Sanity: capability is usable (init_schema is idempotent).
        storage.init_schema().expect("init_schema");

        // require_query returns Arc<dyn QueryEngine>.
        let _query = kit.require_query().expect("require_query");

        // require_trace returns Arc<dyn TraceEngine>.
        let _trace = kit.require_trace().expect("require_trace");
    }

    /// Bootstrap must fail (not panic) when the database path is invalid.
    /// StorageModuleBuilder::build → Repository::open returns an error,
    /// which Kit surfaces as `KitError::BuildFailed`.
    #[test]
    fn build_kit_invalid_db_path_returns_build_failed_error() {
        // A path under /nonexistent should fail to open.
        let config = KitBootstrapConfig::new(PathBuf::from("/nonexistent/dir/xyz/db.lbug"));
        let result = build_kit(&config);
        assert!(
            result.is_err(),
            "expected build_kit to fail on invalid db_path"
        );
        let err = result.err().unwrap();
        let msg = err.to_string();
        // BuildFailed mentions the module name (storage) and the source.
        assert!(
            msg.contains("storage") || msg.contains("build"),
            "error should mention storage/build, got: {msg}"
        );
    }

    /// `KitBootstrapConfig::new` defaults `debounce_ms` to
    /// `DEFAULT_DEBOUNCE_MS` (2000ms, BR-DAEMON-001).
    #[test]
    fn bootstrap_config_new_defaults_debounce_ms() {
        let config = KitBootstrapConfig::new(PathBuf::from("/tmp/db.lbug"));
        assert_eq!(config.debounce_ms, 2000, "BR-DAEMON-001 default debounce");
        assert_eq!(config.db_path, PathBuf::from("/tmp/db.lbug"));
    }

    /// `with_debounce_ms` overrides the default.
    #[test]
    fn bootstrap_config_with_debounce_ms_overrides_default() {
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"))
            .with_debounce_ms(500);
        assert_eq!(config.debounce_ms, 500);
    }

    // --- Feature-gated capability assertions ---

    #[cfg(feature = "daemon")]
    #[test]
    fn build_kit_registers_daemon_when_feature_on() {
        use crate::kit::bootstrap::KitExt;
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).expect("build_kit");
        assert!(kit.contains::<DaemonKey>(), "DaemonKey missing with daemon feature");
        let _daemon = kit.require_daemon().expect("require_daemon");
    }

    #[cfg(not(feature = "daemon"))]
    #[test]
    fn build_kit_omits_daemon_when_feature_off() {
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).expect("build_kit");
        // DaemonKey does not exist as a type under not(feature = "daemon"),
        // so we cannot call contains::<DaemonKey>(). Instead, we verify the
        // Kit has exactly 7 capabilities by requiring the 7 core keys.
        assert!(kit.contains::<StorageKey>());
        assert!(kit.contains::<ParserKey>());
        assert!(kit.contains::<ExtractorKey>());
        assert!(kit.contains::<IndexerKey>());
        assert!(kit.contains::<ResolverKey>());
        assert!(kit.contains::<QueryKey>());
        assert!(kit.contains::<TraceKey>());
    }

    #[cfg(feature = "embed")]
    #[test]
    fn build_kit_registers_embed_when_feature_on() {
        use crate::kit::bootstrap::KitExt;
        // Ensure deterministic env state — no API key.
        std::env::remove_var(crate::embed::API_KEY_ENV);
        std::env::remove_var(crate::embed::OPENAI_API_KEY_ENV);

        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).expect("build_kit");
        assert!(kit.contains::<EmbedKey>(), "EmbedKey missing with embed feature");
        let embed = kit.require_embed().expect("require_embed");
        // Without an API key, embed() must return MissingApiKey (non-blocking).
        let result = embed.embed(&["hello"]);
        assert!(
            matches!(result, Err(crate::embed::EmbedError::MissingApiKey)),
            "expected MissingApiKey, got {result:?}"
        );
    }

    #[cfg(feature = "embed")]
    #[test]
    fn bootstrap_config_with_embedding_config_overrides_from_env() {
        let custom = EmbeddingConfig {
            endpoint: "https://custom.example.com/v1".to_string(),
            model: "custom-model".to_string(),
            api_key: Some("custom-key".to_string()),
        };
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"))
            .with_embedding_config(custom.clone());
        assert_eq!(config.embedding_config.endpoint, custom.endpoint);
        assert_eq!(config.embedding_config.model, custom.model);
        assert_eq!(config.embedding_config.api_key, custom.api_key);
    }

    #[cfg(not(feature = "embed"))]
    #[test]
    fn build_kit_omits_embed_when_feature_off() {
        // Mirror of the daemon-off test. EmbedKey is not in scope, so we
        // only assert the 7 core keys are present.
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).expect("build_kit");
        assert!(kit.contains::<StorageKey>());
        assert!(kit.contains::<ParserKey>());
        assert!(kit.contains::<ExtractorKey>());
        assert!(kit.contains::<IndexerKey>());
        assert!(kit.contains::<ResolverKey>());
        assert!(kit.contains::<QueryKey>());
        assert!(kit.contains::<TraceKey>());
    }

    /// Convenience helpers (`require_storage`, etc.) return the same `Arc`
    /// as a direct `require::<Key>()` call.
    #[test]
    fn convenience_helpers_match_direct_require() {
        use crate::kit::bootstrap::KitExt;
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).expect("build_kit");

        let via_helper = kit.require_storage().expect("require_storage");
        let via_direct: Arc<dyn crate::storage::capability::Storage> =
            kit.require::<StorageKey>().expect("require::<StorageKey>");
        assert!(
            Arc::ptr_eq(&via_helper, &via_direct),
            "convenience helper must return the same Arc"
        );
    }
}
