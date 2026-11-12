// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! [`Nexus`] implementation — capability accessors over the built kit.

use std::sync::Arc;

#[cfg(feature = "cache")]
use crate::cache::CacheStore;
#[cfg(feature = "daemon")]
use crate::daemon::DaemonRunner;
#[cfg(feature = "embeddings")]
use crate::embed::EmbedClient;
use crate::index::capability::Indexer;
use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, KitError};
#[cfg(feature = "lsp")]
use crate::lsp::{LspRegistry, LspRegistryError, LspRegistryModule};
use crate::parse::capability::{ExtractorRegistry, ParserRegistry};
use crate::query::capability::QueryEngine;
use crate::resolve::capability::Resolver;
use crate::storage::capability::Storage;
use crate::trace::capability::TraceEngine as TraceEngineTrait;

/// Facade error surface.
///
/// Internal phase (design D-3): the single variant keeps the underlying kit
/// classification so callers can still distinguish lock contention from
/// build failure; the public-release switch to a self-owned 11-variant
/// mirror is task T014.
#[derive(Debug, thiserror::Error)]
pub enum NexusError {
    /// Capability resolution or kit assembly failed on the trait-kit side.
    #[error("kit error: {0}")]
    Kit(#[from] KitError),
}

/// The SDK facade: owns the built kit, hands out capability traits.
pub struct Nexus {
    kit: Arc<AsyncKit<AsyncReady>>,
}

impl Nexus {
    /// Wraps an already-built kit (tests, bootstrap call sites that assemble
    /// the kit themselves).
    #[must_use]
    pub fn from_kit(kit: AsyncKit<AsyncReady>) -> Self {
        Self { kit: Arc::new(kit) }
    }

    /// Arc-form constructor for the process-global singleton path (the
    /// global stores `Arc<AsyncKit<AsyncReady>>` and an `Arc` cannot be
    /// unwrapped while shared).
    #[must_use]
    pub(crate) fn from_kit_arc(kit: Arc<AsyncKit<AsyncReady>>) -> Self {
        Self { kit }
    }

    /// The process-global facade, `Some` once the kit is initialized.
    ///
    /// This is the one sanctioned global access point for service handlers:
    /// `run_*` core functions take `&Nexus` as an argument, and only forge
    /// wrappers (which cannot receive injected state) call this.
    #[must_use]
    pub fn from_runtime() -> Option<Self> {
        crate::service::runtime::kit().map(Self::from_kit_arc)
    }

    /// Assembles a kit from the full bootstrap config and wraps it.
    ///
    /// All configuration fields are preserved (`debounce_ms`, `verbose`,
    /// `notify_webhook`, `read_only`, …) — a reduced `(db_path, read_only)`
    /// signature would silently drop wiring the CLI relies on.
    ///
    /// # Errors
    ///
    /// [`NexusError::Kit`] when module assembly or capability wiring fails.
    pub async fn open(config: &KitBootstrapConfig) -> Result<Self, NexusError> {
        let built = build_kit(config).await?;
        Ok(Self::from_kit(built))
    }

    /// The underlying kit — for configuration reads
    /// (`config::<StorageConfig>()`) and health aggregation only. Callers
    /// must not re-enter `require` through it; that is what the accessors
    /// below are for.
    #[must_use]
    pub fn raw_kit(&self) -> Arc<AsyncKit<AsyncReady>> {
        Arc::clone(&self.kit)
    }

    /// Runs the kit's registered shutdown hooks in reverse-topological
    /// order.
    ///
    /// The `shutdown_async` future is **!Send** — it must be awaited
    /// in-place (or `block_on`-ed by the caller); spawning it is a compile
    /// error. Note that trait-kit's `shutdown_async` runs hooks but does not
    /// clear the capability registry: accessors keep resolving after
    /// shutdown, and module shutdown hooks decide per-module what a
    /// post-shutdown call does.
    pub async fn shutdown(&self) {
        self.kit.shutdown_async().await;
    }

    /// Storage capability (queries, writes, transactions).
    ///
    /// # Errors
    ///
    /// [`NexusError::Kit`] when the capability is not registered.
    pub fn storage(&self) -> Result<Arc<dyn Storage>, NexusError> {
        self.kit
            .require::<crate::storage::StorageModule>()
            .map_err(NexusError::Kit)
    }

    /// Parser factory capability.
    ///
    /// # Errors
    ///
    /// [`NexusError::Kit`] when the capability is not registered.
    pub fn parser(&self) -> Result<Arc<dyn ParserRegistry>, NexusError> {
        self.kit
            .require::<crate::parse::ParserFactoryModule>()
            .map_err(NexusError::Kit)
    }

    /// Extractor registry capability.
    ///
    /// # Errors
    ///
    /// [`NexusError::Kit`] when the capability is not registered.
    pub fn extractor(&self) -> Result<Arc<dyn ExtractorRegistry>, NexusError> {
        self.kit
            .require::<crate::parse::ExtractorRegistryModule>()
            .map_err(NexusError::Kit)
    }

    /// Indexer capability.
    ///
    /// # Errors
    ///
    /// [`NexusError::Kit`] when the capability is not registered.
    pub fn indexer(&self) -> Result<Arc<dyn Indexer>, NexusError> {
        self.kit
            .require::<crate::index::IndexerModule>()
            .map_err(NexusError::Kit)
    }

    /// Resolver capability.
    ///
    /// # Errors
    ///
    /// [`NexusError::Kit`] when the capability is not registered.
    pub fn resolver(&self) -> Result<Arc<dyn Resolver>, NexusError> {
        self.kit
            .require::<crate::resolve::ResolverModule>()
            .map_err(NexusError::Kit)
    }

    /// Query engine capability.
    ///
    /// # Errors
    ///
    /// [`NexusError::Kit`] when the capability is not registered.
    pub fn query(&self) -> Result<Arc<dyn QueryEngine>, NexusError> {
        self.kit
            .require::<crate::query::QueryModule>()
            .map_err(NexusError::Kit)
    }

    /// Trace capability.
    ///
    /// # Errors
    ///
    /// [`NexusError::Kit`] when the capability is not registered.
    pub fn trace(&self) -> Result<Arc<dyn TraceEngineTrait>, NexusError> {
        self.kit
            .require::<crate::trace::TraceModule>()
            .map_err(NexusError::Kit)
    }

    /// LSP registry capability (extension → provider table).
    ///
    /// # Errors
    ///
    /// [`LspRegistryError::NotAvailable`] when the `lsp` feature is off or
    /// the module was not registered.
    #[cfg(feature = "lsp")]
    pub fn lsp(&self) -> Result<Arc<dyn LspRegistry>, LspRegistryError> {
        self.kit
            .require::<LspRegistryModule>()
            .map_err(|_| LspRegistryError::NotAvailable)
    }

    /// Daemon capability.
    ///
    /// # Errors
    ///
    /// [`NexusError::Kit`] when the capability is not registered.
    #[cfg(feature = "daemon")]
    pub fn daemon(&self) -> Result<Arc<dyn DaemonRunner>, NexusError> {
        self.kit
            .require::<crate::daemon::DaemonModule>()
            .map_err(NexusError::Kit)
    }

    /// Embed capability.
    ///
    /// # Errors
    ///
    /// [`NexusError::Kit`] when the capability is not registered.
    #[cfg(feature = "embeddings")]
    pub fn embed(&self) -> Result<Arc<dyn EmbedClient>, NexusError> {
        self.kit
            .require::<crate::embed::EmbedModule>()
            .map_err(NexusError::Kit)
    }

    /// Cache capability.
    ///
    /// # Errors
    ///
    /// [`NexusError::Kit`] when the capability is not registered.
    #[cfg(feature = "cache")]
    pub fn cache(&self) -> Result<Arc<dyn CacheStore>, NexusError> {
        self.kit
            .require::<crate::cache::CacheModule>()
            .map_err(NexusError::Kit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::DaemonConfig;

    /// Assembles a full-feature kit over a throwaway DB and wraps it.
    async fn open_nexus(db: &std::path::Path) -> Nexus {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        Nexus::open(&config).await.expect("open")
    }

    /// R-sdk-001 acceptance: after `open`, every capability accessor
    /// resolves (each asserted individually — a single `?` chain would hide
    /// which wiring broke).
    #[tokio::test]
    async fn open_resolves_every_capability_accessor() {
        let dir = tempfile::TempDir::new().expect("tmpdir");
        let db = dir.path().join("nexus_test_db");
        let nexus = open_nexus(&db).await;

        assert!(nexus.storage().is_ok(), "storage must resolve");
        assert!(nexus.parser().is_ok(), "parser must resolve");
        assert!(nexus.extractor().is_ok(), "extractor must resolve");
        assert!(nexus.indexer().is_ok(), "indexer must resolve");
        assert!(nexus.resolver().is_ok(), "resolver must resolve");
        assert!(nexus.query().is_ok(), "query must resolve");
        assert!(nexus.trace().is_ok(), "trace must resolve");
        #[cfg(feature = "daemon")]
        assert!(nexus.daemon().is_ok(), "daemon must resolve");
        #[cfg(feature = "embeddings")]
        assert!(nexus.embed().is_ok(), "embed must resolve");
        #[cfg(feature = "cache")]
        assert!(nexus.cache().is_ok(), "cache must resolve");
        #[cfg(feature = "lsp")]
        assert!(nexus.lsp().is_ok(), "lsp registry must resolve");
    }

    /// R-sdk-001: the full config survives the facade — a reduced
    /// `(db_path, read_only)` signature would lose `debounce_ms`.
    #[cfg(feature = "daemon")]
    #[tokio::test]
    async fn open_preserves_daemon_config_fields() {
        let dir = tempfile::TempDir::new().expect("tmpdir");
        let db = dir.path().join("nexus_cfg_db");
        let mut config = KitBootstrapConfig::new(db.clone());
        config.debounce_ms = 4321;
        let nexus = Nexus::open(&config).await.expect("open");
        let daemon_config = nexus
            .raw_kit()
            .config::<DaemonConfig>()
            .expect("daemon config set by bootstrap");
        assert_eq!(
            daemon_config.debounce_ms, 4321,
            "debounce_ms must pass through"
        );
        assert_eq!(daemon_config.db_path, db, "db_path must pass through");
    }

    /// R-sdk-001: `from_runtime` is `None` before initialization and `Some`
    /// after (explicit init/reset cycle — the global is process state).
    #[test]
    fn from_runtime_tracks_initialization() {
        crate::service::runtime::reset_kit_for_testing();
        assert!(Nexus::from_runtime().is_none(), "uninitialized → None");

        let dir = tempfile::TempDir::new().expect("tmpdir");
        let db = dir.path().join("nexus_runtime_db");
        let config = KitBootstrapConfig::new(db);
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let kit = rt.block_on(crate::kit::build_kit(&config)).expect("build");
        crate::service::runtime::init_kit(kit).expect("init");
        assert!(Nexus::from_runtime().is_some(), "initialized → Some");

        crate::service::runtime::reset_kit_for_testing();
        assert!(Nexus::from_runtime().is_none(), "reset → None");
    }

    /// R-sdk-003: open → capability call → shutdown completes in one
    /// runtime (the !Send shutdown future must be awaited in place).
    #[tokio::test]
    async fn open_call_shutdown_round_trip() {
        let dir = tempfile::TempDir::new().expect("tmpdir");
        let db = dir.path().join("nexus_shutdown_db");
        let nexus = open_nexus(&db).await;
        let storage = nexus.storage().expect("storage before shutdown");
        assert!(storage.query("RETURN 1").is_ok(), "capability usable");
        nexus.shutdown().await;
    }
}
