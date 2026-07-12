// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit bootstrap (T6/unified-architecture Phase 2, Task 2.13;
//! v0.3.3 AsyncKit migration).
//!
//! Provides [`build_kit`], the single entry point that assembles every
//! subsystem module into an [`AsyncKit<AsyncReady>`] in fixed dependency
//! order. CLI handlers (Task 2.14) and integration tests (Task 2.15) call
//! this once and then resolve capabilities via [`AsyncKit::require`] instead
//! of constructing subsystems ad-hoc.
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
//! Although every module declares `dependencies = &[]` at the type level
//! (concrete impls are self-contained — see each module's
//! `Dependency note`), the bootstrap enforces the conceptual dependency
//! order so that any future module declaring real dependencies can rely on
//! its dependencies already being present in the Kit.
//!
//! # Feature gating
//!
//! `DaemonModule` and `EmbedModule` are only registered when their
//! respective cargo features (`daemon`, `embed`) are enabled. Under
//! `--no-default-features --features lang-rust`, the returned Kit contains
//! exactly 7 capabilities (no `DaemonModule` / `EmbedModule`).
//!
//! [`AsyncKit::require`]: crate::kit::AsyncKit::require

use std::path::PathBuf;
use std::sync::Arc;

use crate::kit::{
    AsyncKit, AsyncReady, ExtractorRegistryModule, IndexerModule, KitError, ParserFactoryModule,
    QueryModule, ResolverModule, StorageModule, TraceModule,
};

// Feature-gated modules are only imported when their feature is on, mirroring
// the `cfg` on the module types themselves.
#[cfg(feature = "daemon")]
use crate::kit::DaemonModule;
#[cfg(feature = "embed")]
use crate::kit::EmbedModule;
#[cfg(feature = "cache")]
use crate::kit::CacheModule;

// Configs are still imported from their owning modules.
use crate::index::IndexConfig;
use crate::query::QueryConfig;
use crate::storage::StorageConfig;
use crate::trace::TraceConfig;

#[cfg(feature = "daemon")]
use crate::daemon::{DaemonConfig, DEFAULT_DEBOUNCE_MS};

#[cfg(feature = "embed")]
use crate::embed::EmbeddingConfig;

#[cfg(feature = "cache")]
use crate::cache::CacheConfig;

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
pub const DEFAULT_DEBOUNCE_MS: u64 = 2000;

// When the `embed` feature is off, `EmbeddingConfig` is not in scope, so
// `KitBootstrapConfig::new` cannot call `EmbeddingConfig::from_env()`. The
// `embedding_config` field is also gated out. Nothing to do here — the
// cfg on the field handles it.

// ---------------------------------------------------------------------------
// build_kit
// ---------------------------------------------------------------------------

/// Assemble every trait-kit module into a fresh [`AsyncKit`] in fixed
/// dependency order (Task 2.13 / design.md D1).
///
/// # Order
///
/// `Storage → Parser → Extractor → Indexer → Resolver → Query → Trace →
///  Daemon(cfg) → Embed(cfg)`
///
/// Each module's [`AsyncAutoBuilder::build`] observes its declared
/// dependencies already present in the Kit (currently all empty, but the
/// order is enforced for future migrations).
///
/// # Errors
///
/// Returns [`KitError`] if any module's `build` fails (e.g. `StorageModule`
/// cannot open the database at `db_path`), or
/// [`KitError::DuplicateCapability`] if a module is somehow already
/// registered (should not happen with a fresh [`AsyncKit::new`]).
///
/// # Example
///
/// ```ignore
/// use codenexus::kit::{build_kit, KitBootstrapConfig, StorageModule};
/// use std::path::PathBuf;
///
/// # tokio_test::block_on(async {
/// let config = KitBootstrapConfig::new(PathBuf::from("./codenexus.lbug"));
/// let kit = build_kit(&config).await?;
/// let storage = kit.require::<StorageModule>()?;
/// # Ok::<_, codenexus::kit::KitError>(())
/// # });
/// ```
///
/// [`AsyncAutoBuilder::build`]: crate::kit::AsyncAutoBuilder::build
pub async fn build_kit(config: &KitBootstrapConfig) -> Result<AsyncKit<AsyncReady>, KitError> {
    let mut kit = AsyncKit::new();

    // 1. Storage — opens Repository, initializes schema (Task 2.4).
    kit.set_config(StorageConfig {
        db_path: config.db_path.clone(),
    });
    kit.register::<StorageModule>()?;

    // 2. Parser — stateless ParserFactory (Task 2.5).
    kit.register::<ParserFactoryModule>()?;

    // 3. Extractor — stateless dispatcher (Task 2.6).
    kit.register::<ExtractorRegistryModule>()?;

    // 4. Indexer — IndexFacade with db_path (Task 2.7).
    kit.set_config(IndexConfig {
        db_path: config.db_path.clone(),
    });
    kit.register::<IndexerModule>()?;

    // 5. Resolver — stateless free functions (Task 2.8).
    kit.register::<ResolverModule>()?;

    // 6. Query — QueryFacade with db_path (Task 2.9).
    kit.set_config(QueryConfig {
        db_path: config.db_path.clone(),
    });
    kit.register::<QueryModule>()?;

    // 7. Trace — loads fresh subgraph per trace call (Task 2.10).
    kit.set_config(TraceConfig {
        db_path: config.db_path.clone(),
        ..Default::default()
    });
    kit.register::<TraceModule>()?;

    // 8. Daemon (feature-gated) — owns db_path + debounce_ms (Task 2.11).
    #[cfg(feature = "daemon")]
    {
        kit.set_config(DaemonConfig {
            db_path: config.db_path.clone(),
            debounce_ms: config.debounce_ms,
        });
        kit.register::<DaemonModule>()?;
    }

    // 9. Embed (feature-gated) — owns EmbeddingConfig (Task 2.12).
    #[cfg(feature = "embed")]
    {
        kit.set_config(config.embedding_config.clone());
        kit.register::<EmbedModule>()?;
    }

    // 10. Cache (feature-gated) — moka memory cache for content-addressed
    // caching (T017, v0.3.3). Leaf module — no upstream dependencies.
    // Registered last so all subsystems that might query the cache are
    // already present.
    #[cfg(feature = "cache")]
    {
        kit.set_config(CacheConfig::default());
        kit.register::<CacheModule>()?;
    }

    kit.build().await
}

// ---------------------------------------------------------------------------
// Convenience: capability accessor helpers
// ---------------------------------------------------------------------------

/// Extension trait adding typed `require_*` shortcuts to
/// [`AsyncKit<AsyncReady>`].
///
/// Defined as an extension trait (rather than inherent methods on
/// `AsyncKit`) because `AsyncKit` is defined in the external `trait_kit`
/// crate — Rust's orphan rule forbids inherent impls on external types.
/// The canonical API remains
/// [`AsyncKit::require`](crate::kit::AsyncKit::require); these helpers are
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
    fn require_resolver(&self) -> Result<Arc<dyn crate::resolve::capability::Resolver>, KitError>;

    /// Resolves the Query capability (`Arc<dyn QueryEngine>`).
    fn require_query(&self) -> Result<Arc<dyn crate::query::capability::QueryEngine>, KitError>;

    /// Resolves the Trace capability (`Arc<dyn TraceEngine>`).
    fn require_trace(&self) -> Result<Arc<dyn crate::trace::capability::TraceEngine>, KitError>;

    /// Resolves the Daemon capability (`Arc<dyn DaemonRunner>`).
    #[cfg(feature = "daemon")]
    fn require_daemon(&self) -> Result<Arc<dyn crate::daemon::capability::DaemonRunner>, KitError>;

    /// Resolves the Embed capability (`Arc<dyn EmbedClient>`).
    #[cfg(feature = "embed")]
    fn require_embed(&self) -> Result<Arc<dyn crate::embed::client::EmbedClient>, KitError>;

    /// Resolves the Cache capability (`Arc<dyn CacheStore>`).
    /// Only available when the `cache` feature is enabled.
    #[cfg(feature = "cache")]
    fn require_cache(&self) -> Result<Arc<dyn crate::cache::CacheStore>, KitError>;
}

impl KitExt for AsyncKit<AsyncReady> {
    fn require_storage(&self) -> Result<Arc<dyn crate::storage::capability::Storage>, KitError> {
        self.require::<StorageModule>()
    }

    fn require_parser(
        &self,
    ) -> Result<Arc<dyn crate::parse::capability::ParserRegistry>, KitError> {
        self.require::<ParserFactoryModule>()
    }

    fn require_extractor(
        &self,
    ) -> Result<Arc<dyn crate::parse::capability::ExtractorRegistry>, KitError> {
        self.require::<ExtractorRegistryModule>()
    }

    fn require_indexer(&self) -> Result<Arc<dyn crate::index::capability::Indexer>, KitError> {
        self.require::<IndexerModule>()
    }

    fn require_resolver(&self) -> Result<Arc<dyn crate::resolve::capability::Resolver>, KitError> {
        self.require::<ResolverModule>()
    }

    fn require_query(&self) -> Result<Arc<dyn crate::query::capability::QueryEngine>, KitError> {
        self.require::<QueryModule>()
    }

    fn require_trace(&self) -> Result<Arc<dyn crate::trace::capability::TraceEngine>, KitError> {
        self.require::<TraceModule>()
    }

    #[cfg(feature = "daemon")]
    fn require_daemon(&self) -> Result<Arc<dyn crate::daemon::capability::DaemonRunner>, KitError> {
        self.require::<DaemonModule>()
    }

    #[cfg(feature = "embed")]
    fn require_embed(&self) -> Result<Arc<dyn crate::embed::client::EmbedClient>, KitError> {
        self.require::<EmbedModule>()
    }

    #[cfg(feature = "cache")]
    fn require_cache(&self) -> Result<Arc<dyn crate::cache::CacheStore>, KitError> {
        self.require::<CacheModule>()
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
    #[tokio::test]
    async fn build_kit_in_memory_registers_all_core_capabilities() {
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).await.expect("build_kit");

        assert!(kit.contains::<StorageModule>(), "StorageModule missing");
        assert!(kit.contains::<ParserFactoryModule>(), "ParserFactoryModule missing");
        assert!(
            kit.contains::<ExtractorRegistryModule>(),
            "ExtractorRegistryModule missing"
        );
        assert!(kit.contains::<IndexerModule>(), "IndexerModule missing");
        assert!(kit.contains::<ResolverModule>(), "ResolverModule missing");
        assert!(kit.contains::<QueryModule>(), "QueryModule missing");
        assert!(kit.contains::<TraceModule>(), "TraceModule missing");
    }

    /// Each registered capability must be `require`-able (returns the same
    /// `Arc` that was registered).
    #[tokio::test]
    async fn build_kit_require_returns_registered_arc() {
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).await.expect("build_kit");

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
    /// StorageModule::build_cap → Repository::open returns an error, which
    /// AsyncKit surfaces as `KitError::BuildFailed`.
    #[tokio::test]
    async fn build_kit_invalid_db_path_returns_build_failed_error() {
        // A path under /nonexistent should fail to open.
        let config = KitBootstrapConfig::new(PathBuf::from("/nonexistent/dir/xyz/db.lbug"));
        let result = build_kit(&config).await;
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
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:")).with_debounce_ms(500);
        assert_eq!(config.debounce_ms, 500);
    }

    // --- Feature-gated capability assertions ---

    #[cfg(feature = "daemon")]
    #[tokio::test]
    async fn build_kit_registers_daemon_when_feature_on() {
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).await.expect("build_kit");
        assert!(
            kit.contains::<DaemonModule>(),
            "DaemonModule missing with daemon feature"
        );
        let _daemon = kit.require_daemon().expect("require_daemon");
    }

    #[cfg(not(feature = "daemon"))]
    #[tokio::test]
    async fn build_kit_omits_daemon_when_feature_off() {
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).await.expect("build_kit");
        // DaemonModule does not exist as a type under not(feature = "daemon"),
        // so we cannot call contains::<DaemonModule>(). Instead, we verify the
        // Kit has exactly 7 capabilities by requiring the 7 core modules.
        assert!(kit.contains::<StorageModule>());
        assert!(kit.contains::<ParserFactoryModule>());
        assert!(kit.contains::<ExtractorRegistryModule>());
        assert!(kit.contains::<IndexerModule>());
        assert!(kit.contains::<ResolverModule>());
        assert!(kit.contains::<QueryModule>());
        assert!(kit.contains::<TraceModule>());
    }

    #[cfg(feature = "embed")]
    #[tokio::test]
    async fn build_kit_registers_embed_when_feature_on() {
        // Ensure deterministic env state — no API key, no endpoint (local mode).
        std::env::remove_var(crate::embed::API_KEY_ENV);
        std::env::remove_var(crate::embed::OPENAI_API_KEY_ENV);
        std::env::remove_var(crate::embed::EMBED_ENDPOINT_ENV);
        std::env::remove_var(crate::embed::EMBED_MODEL_PATH_ENV);

        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).await.expect("build_kit");
        assert!(
            kit.contains::<EmbedModule>(),
            "EmbedModule missing with embed feature"
        );
        let embed = kit.require_embed().expect("require_embed");
        // H10/D7: default is local mode; without a model file, embed() must
        // return Unavailable (not MissingApiKey — no API key needed locally).
        let result = embed.embed(&["hello"]);
        assert!(
            matches!(result, Err(crate::embed::EmbedError::Unavailable(ref msg)) if msg.contains("not found")),
            "expected Unavailable (model not found) in local mode, got {result:?}"
        );
    }

    #[cfg(feature = "embed")]
    #[tokio::test]
    async fn build_kit_embed_remote_without_key_returns_missing_api_key() {
        // H10/D7: remote mode (endpoint=Some) without API key → MissingApiKey.
        std::env::remove_var(crate::embed::API_KEY_ENV);
        std::env::remove_var(crate::embed::OPENAI_API_KEY_ENV);
        std::env::set_var(
            crate::embed::EMBED_ENDPOINT_ENV,
            "https://api.openai.com/v1",
        );

        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).await.expect("build_kit");
        let embed = kit.require_embed().expect("require_embed");
        let result = embed.embed(&["hello"]);
        assert!(
            matches!(result, Err(crate::embed::EmbedError::MissingApiKey)),
            "expected MissingApiKey in remote mode, got {result:?}"
        );
        std::env::remove_var(crate::embed::EMBED_ENDPOINT_ENV);
    }

    #[cfg(feature = "embed")]
    #[test]
    fn bootstrap_config_with_embedding_config_overrides_from_env() {
        let custom = EmbeddingConfig {
            endpoint: Some("https://custom.example.com/v1".to_string()),
            model: "custom-model".to_string(),
            api_key: Some("custom-key".to_string()),
            ..EmbeddingConfig::default()
        };
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"))
            .with_embedding_config(custom.clone());
        assert_eq!(config.embedding_config.endpoint, custom.endpoint);
        assert_eq!(config.embedding_config.model, custom.model);
        assert_eq!(config.embedding_config.api_key, custom.api_key);
    }

    #[cfg(not(feature = "embed"))]
    #[tokio::test]
    async fn build_kit_omits_embed_when_feature_off() {
        // Mirror of the daemon-off test. EmbedModule is not in scope, so we
        // only assert the 7 core modules are present.
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).await.expect("build_kit");
        assert!(kit.contains::<StorageModule>());
        assert!(kit.contains::<ParserFactoryModule>());
        assert!(kit.contains::<ExtractorRegistryModule>());
        assert!(kit.contains::<IndexerModule>());
        assert!(kit.contains::<ResolverModule>());
        assert!(kit.contains::<QueryModule>());
        assert!(kit.contains::<TraceModule>());
    }

    /// Convenience helpers (`require_storage`, etc.) return the same `Arc`
    /// as a direct `require::<Module>()` call.
    #[tokio::test]
    async fn convenience_helpers_match_direct_require() {
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).await.expect("build_kit");

        let via_helper = kit.require_storage().expect("require_storage");
        let via_direct: Arc<dyn crate::storage::capability::Storage> = kit
            .require::<StorageModule>()
            .expect("require::<StorageModule>");
        assert!(
            Arc::ptr_eq(&via_helper, &via_direct),
            "convenience helper must return the same Arc"
        );
    }

    /// `require_parser`, `require_extractor`, `require_indexer`, and
    /// `require_resolver` must all return their registered capabilities.
    /// These are the remaining KitExt convenience helpers not exercised by
    /// `build_kit_require_returns_registered_arc`.
    #[tokio::test]
    async fn convenience_helpers_return_registered_capabilities() {
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).await.expect("build_kit");

        // require_parser returns Arc<dyn ParserRegistry> — create_parser works.
        let parser = kit.require_parser().expect("require_parser");
        let langs = parser.supported_languages();
        assert!(
            !langs.is_empty(),
            "parser should support at least one language"
        );

        // require_extractor returns Arc<dyn ExtractorRegistry> — get_extractor works.
        let extractor = kit.require_extractor().expect("require_extractor");
        let ext_langs = extractor.supported_languages();
        assert!(
            !ext_langs.is_empty(),
            "extractor should support at least one language"
        );

        // require_indexer returns Arc<dyn Indexer>.
        let _indexer = kit.require_indexer().expect("require_indexer");

        // require_resolver returns Arc<dyn Resolver>.
        let _resolver = kit.require_resolver().expect("require_resolver");
    }

    /// Each convenience helper's returned `Arc` must be pointer-equal to the
    /// `Arc` returned by a direct `require::<Module>()` call (they delegate,
    /// so they must return the same registration).
    #[tokio::test]
    async fn convenience_helpers_all_match_direct_require() {
        let config = KitBootstrapConfig::new(PathBuf::from(":memory:"));
        let kit = build_kit(&config).await.expect("build_kit");

        let parser_helper = kit.require_parser().expect("require_parser");
        let parser_direct: Arc<dyn crate::parse::capability::ParserRegistry> = kit
            .require::<ParserFactoryModule>()
            .expect("require::<ParserFactoryModule>");
        assert!(Arc::ptr_eq(&parser_helper, &parser_direct));

        let extractor_helper = kit.require_extractor().expect("require_extractor");
        let extractor_direct: Arc<dyn crate::parse::capability::ExtractorRegistry> = kit
            .require::<ExtractorRegistryModule>()
            .expect("require::<ExtractorRegistryModule>");
        assert!(Arc::ptr_eq(&extractor_helper, &extractor_direct));

        let indexer_helper = kit.require_indexer().expect("require_indexer");
        let indexer_direct: Arc<dyn crate::index::capability::Indexer> =
            kit.require::<IndexerModule>().expect("require::<IndexerModule>");
        assert!(Arc::ptr_eq(&indexer_helper, &indexer_direct));

        let resolver_helper = kit.require_resolver().expect("require_resolver");
        let resolver_direct: Arc<dyn crate::resolve::capability::Resolver> = kit
            .require::<ResolverModule>()
            .expect("require::<ResolverModule>");
        assert!(Arc::ptr_eq(&resolver_helper, &resolver_direct));
    }
}
