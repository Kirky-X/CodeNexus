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

/// Error-kind classification mirroring trait-kit's `ErrorKind` (self-owned
/// type — the SDK surface never re-exports trait-kit types, T014).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NexusErrorKind {
    /// The requested capability/config does not exist.
    Missing,
    /// The capability exists but its type does not match the request.
    TypeMismatch,
    /// Module build or lifecycle hook failed.
    InitFailed,
    /// Everything else (graph problems, registration conflicts, shutdown).
    Other,
}

/// Facade error surface — a self-owned mirror of trait-kit's
/// `TraitKitError` 11-variant classification (design D-3, release switch
/// T014): same taxonomy, `kind()` semantics, and message structure, but no
/// trait-kit type appears in this enum. Conversion from `KitError` is
/// variant-for-variant, so exit-code classification in
/// `CodeNexusError::Kit` stays exact.
#[derive(Debug)]
pub enum NexusError {
    /// Dependency graph contains a cycle.
    CycleDetected {
        /// Module names forming the cycle.
        cycle: Vec<&'static str>,
    },
    /// A dependency module was not registered.
    DependencyMissing {
        /// The module declaring the dependency.
        module: &'static str,
        /// The missing dependency.
        missing: &'static str,
    },
    /// The module was registered more than once.
    AlreadyRegistered {
        /// The duplicated module.
        module: &'static str,
    },
    /// A decorator targeted an unregistered module.
    DecoratorTargetMissing {
        /// The decorated module name.
        module: &'static str,
    },
    /// A dependency's capability version did not satisfy the requirement.
    VersionIncompatible {
        /// The module declaring the requirement.
        module: &'static str,
        /// The dependency providing the capability.
        dependency: &'static str,
        /// Required minimum version.
        required: &'static str,
        /// Provided version.
        provided: &'static str,
    },
    /// Module build failed.
    BuildFailed {
        /// Build-failure context (already localized by the failing module).
        context: String,
        /// Underlying error source.
        source: Box<dyn std::error::Error + Send + 'static>,
    },
    /// The requested capability does not exist.
    MissingCapability {
        /// Missing capability key.
        key: String,
    },
    /// The capability exists but its type does not match the request.
    CapabilityTypeMismatch {
        /// The capability key.
        key: String,
    },
    /// The requested config does not exist.
    MissingConfig {
        /// Missing config key.
        key: String,
    },
    /// A lifecycle hook failed.
    LifecycleFailed {
        /// Hook context (already localized).
        context: String,
        /// Underlying error source.
        source: Box<dyn std::error::Error + Send + 'static>,
    },
    /// Graceful shutdown timed out.
    ShutdownTimedOut {
        /// The phases that timed out (self-owned string mirror).
        phases: Vec<String>,
    },
}

impl std::fmt::Display for NexusError {
    // Localized through the same FTL catalog the rest of the CLI uses
    // (`sdk-error-*` keys, en + zh) — the mirror of trait-kit's hand-written
    // i18n Display. Fallback chain: current language → en → the key itself.
    #[allow(
        clippy::too_many_lines,
        reason = "11 mirrored variants each carry their own message template; splitting a shared helper would separate the template from its variant"
    )]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use crate::i18n::t as tr;
        match self {
            Self::CycleDetected { cycle } => {
                let cycle_text = if cycle.is_empty() {
                    tr("sdk-diag-unknown-cycle", &[])
                } else {
                    cycle.join(" → ")
                };
                write!(
                    f,
                    "{}",
                    tr("sdk-error-cycle-detected", &[("cycle", cycle_text)])
                )
            }
            Self::DependencyMissing { module, missing } => write!(
                f,
                "{}",
                tr(
                    "sdk-error-dependency-missing",
                    &[
                        ("module", (*module).to_string()),
                        ("missing", (*missing).to_string())
                    ],
                )
            ),
            Self::AlreadyRegistered { module } => write!(
                f,
                "{}",
                tr(
                    "sdk-error-already-registered",
                    &[("module", (*module).to_string())]
                )
            ),
            Self::DecoratorTargetMissing { module } => write!(
                f,
                "{}",
                tr(
                    "sdk-error-decorator-target-missing",
                    &[("module", (*module).to_string())]
                )
            ),
            Self::VersionIncompatible {
                module,
                dependency,
                required,
                provided,
            } => write!(
                f,
                "{}",
                tr(
                    "sdk-error-version-incompatible",
                    &[
                        ("module", (*module).to_string()),
                        ("dependency", (*dependency).to_string()),
                        ("required", (*required).to_string()),
                        ("provided", (*provided).to_string()),
                    ],
                )
            ),
            Self::BuildFailed { context, source } => write!(
                f,
                "{}",
                tr(
                    "sdk-error-build-failed",
                    &[("context", context.clone()), ("source", source.to_string())]
                )
            ),
            Self::MissingCapability { key } => write!(
                f,
                "{}",
                tr("sdk-error-missing-capability", &[("key", key.clone())])
            ),
            Self::CapabilityTypeMismatch { key } => write!(
                f,
                "{}",
                tr(
                    "sdk-error-capability-type-mismatch",
                    &[("key", key.clone())]
                )
            ),
            Self::MissingConfig { key } => write!(
                f,
                "{}",
                tr("sdk-error-missing-config", &[("key", key.clone())])
            ),
            Self::LifecycleFailed { context, source } => write!(
                f,
                "{}",
                tr(
                    "sdk-error-lifecycle-failed",
                    &[("context", context.clone()), ("source", source.to_string())]
                )
            ),
            Self::ShutdownTimedOut { phases } => write!(
                f,
                "{}",
                tr(
                    "sdk-error-shutdown-timed-out",
                    &[("phases", format!("{phases:?}"))]
                )
            ),
        }
    }
}

impl std::error::Error for NexusError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BuildFailed { source, .. } | Self::LifecycleFailed { source, .. } => {
                Some(source.as_ref())
            }
            _ => None,
        }
    }
}

impl NexusError {
    /// Classifies the error — mirrors `TraitKitError::kind` exactly.
    #[must_use]
    pub fn kind(&self) -> NexusErrorKind {
        match self {
            Self::MissingCapability { .. } | Self::MissingConfig { .. } => NexusErrorKind::Missing,
            Self::CapabilityTypeMismatch { .. } => NexusErrorKind::TypeMismatch,
            Self::BuildFailed { .. } | Self::LifecycleFailed { .. } => NexusErrorKind::InitFailed,
            Self::CycleDetected { .. }
            | Self::DependencyMissing { .. }
            | Self::AlreadyRegistered { .. }
            | Self::DecoratorTargetMissing { .. }
            | Self::VersionIncompatible { .. }
            | Self::ShutdownTimedOut { .. } => NexusErrorKind::Other,
        }
    }
}

impl From<KitError> for NexusError {
    fn from(err: KitError) -> Self {
        // Variant-for-variant mirror: trait-kit's taxonomy maps 1:1, so
        // callers matching on `NexusError` see the same classification the
        // kit produced.
        match err {
            KitError::CycleDetected { cycle } => Self::CycleDetected { cycle },
            KitError::DependencyMissing { module, missing } => {
                Self::DependencyMissing { module, missing }
            }
            KitError::AlreadyRegistered { module } => Self::AlreadyRegistered { module },
            KitError::DecoratorTargetMissing { module } => Self::DecoratorTargetMissing { module },
            KitError::VersionIncompatible {
                module,
                dependency,
                required,
                provided,
            } => Self::VersionIncompatible {
                module,
                dependency,
                required,
                provided,
            },
            KitError::BuildFailed { context, source } => Self::BuildFailed { context, source },
            KitError::MissingCapability { key } => Self::MissingCapability { key },
            KitError::CapabilityTypeMismatch { key } => Self::CapabilityTypeMismatch { key },
            KitError::MissingConfig { key } => Self::MissingConfig { key },
            KitError::LifecycleFailed { context, source } => {
                Self::LifecycleFailed { context, source }
            }
            KitError::ShutdownTimedOut { phases } => Self::ShutdownTimedOut {
                phases: phases.into_iter().map(|p| p.as_str().to_string()).collect(),
            },
        }
    }
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
    /// [`NexusError`] (kit-classified) when the capability is not registered.
    pub fn storage(&self) -> Result<Arc<dyn Storage>, NexusError> {
        self.kit
            .require::<crate::storage::StorageModule>()
            .map_err(NexusError::from)
    }

    /// Parser factory capability.
    ///
    /// # Errors
    ///
    /// [`NexusError`] (kit-classified) when the capability is not registered.
    pub fn parser(&self) -> Result<Arc<dyn ParserRegistry>, NexusError> {
        self.kit
            .require::<crate::parse::ParserFactoryModule>()
            .map_err(NexusError::from)
    }

    /// Extractor registry capability.
    ///
    /// # Errors
    ///
    /// [`NexusError`] (kit-classified) when the capability is not registered.
    pub fn extractor(&self) -> Result<Arc<dyn ExtractorRegistry>, NexusError> {
        self.kit
            .require::<crate::parse::ExtractorRegistryModule>()
            .map_err(NexusError::from)
    }

    /// Indexer capability.
    ///
    /// # Errors
    ///
    /// [`NexusError`] (kit-classified) when the capability is not registered.
    pub fn indexer(&self) -> Result<Arc<dyn Indexer>, NexusError> {
        self.kit
            .require::<crate::index::IndexerModule>()
            .map_err(NexusError::from)
    }

    /// Resolver capability.
    ///
    /// # Errors
    ///
    /// [`NexusError`] (kit-classified) when the capability is not registered.
    pub fn resolver(&self) -> Result<Arc<dyn Resolver>, NexusError> {
        self.kit
            .require::<crate::resolve::ResolverModule>()
            .map_err(NexusError::from)
    }

    /// Query engine capability.
    ///
    /// # Errors
    ///
    /// [`NexusError`] (kit-classified) when the capability is not registered.
    pub fn query(&self) -> Result<Arc<dyn QueryEngine>, NexusError> {
        self.kit
            .require::<crate::query::QueryModule>()
            .map_err(NexusError::from)
    }

    /// Trace capability.
    ///
    /// # Errors
    ///
    /// [`NexusError`] (kit-classified) when the capability is not registered.
    pub fn trace(&self) -> Result<Arc<dyn TraceEngineTrait>, NexusError> {
        self.kit
            .require::<crate::trace::TraceModule>()
            .map_err(NexusError::from)
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

    /// Instance-scoped DB identity: the bootstrap config's `db_path` — the
    /// same string the indexer's cache invalidation derives its namespace
    /// from. Replaces the retired process-global `DB_KEY` (T012 instance
    /// isolation): two `Nexus` instances over different databases carry
    /// different identities and never overwrite each other.
    ///
    /// # Errors
    ///
    /// [`NexusError::Kit`] when the storage config is not set.
    pub fn db_key(&self) -> Result<String, NexusError> {
        let config = self
            .raw_kit()
            .config::<crate::storage::StorageConfig>()
            .map_err(NexusError::from)?;
        Ok(config.db_path.display().to_string())
    }

    /// Query-cache namespace for the `#[cached]` macro cache:
    /// `cypher:{db_path}.g{generation}`.
    ///
    /// The generation counter makes graph-mutation invalidations precise —
    /// bumping `cypher:{db_path}` strands this instance's older entries
    /// without touching another instance's namespace (`T012` acceptance: A
    /// invalidates, B still hits). Falls back to generation 0 when the
    /// cache capability is absent (cache-feature-off builds never consult
    /// the macro cache anyway).
    ///
    /// # Errors
    ///
    /// [`NexusError::Kit`] when the storage config is not set.
    #[cfg(feature = "cache")]
    pub fn query_cache_ns(&self) -> Result<String, NexusError> {
        let db_key = self.db_key()?;
        let ns = format!("cypher:{db_key}");
        let generation = match self.cache() {
            Ok(cache) => cache.namespace_generation(&ns),
            Err(_) => 0,
        };
        Ok(format!("{ns}.g{generation}"))
    }

    /// Daemon capability.
    ///
    /// # Errors
    ///
    /// [`NexusError`] (kit-classified) when the capability is not registered.
    #[cfg(feature = "daemon")]
    pub fn daemon(&self) -> Result<Arc<dyn DaemonRunner>, NexusError> {
        self.kit
            .require::<crate::daemon::DaemonModule>()
            .map_err(NexusError::from)
    }

    /// Embed capability.
    ///
    /// # Errors
    ///
    /// [`NexusError`] (kit-classified) when the capability is not registered.
    #[cfg(feature = "embeddings")]
    pub fn embed(&self) -> Result<Arc<dyn EmbedClient>, NexusError> {
        self.kit
            .require::<crate::embed::EmbedModule>()
            .map_err(NexusError::from)
    }

    /// Cache capability.
    ///
    /// # Errors
    ///
    /// [`NexusError`] (kit-classified) when the capability is not registered.
    #[cfg(feature = "cache")]
    pub fn cache(&self) -> Result<Arc<dyn CacheStore>, NexusError> {
        self.kit
            .require::<crate::cache::CacheModule>()
            .map_err(NexusError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "daemon")]
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
    #[serial_test::serial(kit_init)]
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

    /// T012 acceptance ①: two `Nexus` instances over different databases,
    /// built sequentially, carry their own DB identities — the second
    /// build never overwrites the first (the identity is instance state,
    /// not process state).
    #[tokio::test]
    async fn instance_identities_are_isolated() {
        let dir = tempfile::TempDir::new().expect("tmpdir");
        let db_a = dir.path().join("iso_db_a");
        let db_b = dir.path().join("iso_db_b");

        let nexus_a = open_nexus(&db_a).await;
        let key_a = nexus_a.db_key().expect("a identity");
        assert_eq!(
            key_a,
            db_a.display().to_string(),
            "A's identity is A's path"
        );

        let nexus_b = open_nexus(&db_b).await;
        let key_b = nexus_b.db_key().expect("b identity");
        assert_eq!(
            key_b,
            db_b.display().to_string(),
            "B's identity is B's path"
        );

        assert_ne!(key_a, key_b, "distinct DBs must hold distinct identities");
        // Sequential build must not retroactively change A's identity.
        assert_eq!(
            nexus_a.db_key().expect("a identity after B"),
            db_a.display().to_string(),
            "B's build must not overwrite A's identity"
        );
    }

    /// T014: the self-owned mirror classifies exactly like
    /// `TraitKitError::kind` — spot-check the three non-trivial buckets.
    #[test]
    fn kind_mirrors_the_kit_taxonomy() {
        use crate::kit::KitError;
        // The FTL catalog is registered by `i18n::init` (main.rs); tests must
        // register it explicitly or `t()` falls back to the key itself.
        crate::i18n::init();
        let missing = NexusError::from(KitError::MissingCapability {
            key: "storage".to_string(),
        });
        assert_eq!(missing.kind(), NexusErrorKind::Missing);
        assert!(
            missing.to_string().contains("storage"),
            "localized Display carries the capability key: {missing}"
        );

        let mismatch = NexusError::from(KitError::CapabilityTypeMismatch {
            key: "cache".to_string(),
        });
        assert_eq!(mismatch.kind(), NexusErrorKind::TypeMismatch);

        let build = NexusError::from(KitError::BuildFailed {
            context: "storage open".to_string(),
            source: Box::new(std::io::Error::other("boom")),
        });
        assert_eq!(build.kind(), NexusErrorKind::InitFailed);
        assert!(build.to_string().contains("boom"), "source text survives");
    }

    /// T014 / R-rel-001 acceptance: EVERY mirrored variant round-trips
    /// `KitError → NexusError → CodeNexusError::Kit(KitError)` with exact
    /// field fidelity, and `kind()` matches the kit's classification
    /// bucket-for-bucket. Table-driven — one row per variant.
    #[test]
    fn kit_error_mirror_round_trips_every_variant() {
        use crate::kit::KitError;
        use crate::service::error::CodeNexusError;

        // `Box<dyn Error + Send>` is not `Clone` (no Sync bound) — each
        // Build/Lifecycle row boxes a fresh io error instead.
        fn fresh_boxed_source() -> Box<dyn std::error::Error + Send + 'static> {
            Box::new(std::io::Error::other("boom"))
        }
        let _source = fresh_boxed_source();
        let cases: Vec<(&'static str, KitError, NexusError, NexusErrorKind, bool)> = vec![
            (
                "cycle",
                KitError::CycleDetected {
                    cycle: vec!["a", "b"],
                },
                NexusError::CycleDetected {
                    cycle: vec!["a", "b"],
                },
                NexusErrorKind::Other,
                true,
            ),
            (
                "dep-missing",
                KitError::DependencyMissing {
                    module: "m",
                    missing: "d",
                },
                NexusError::DependencyMissing {
                    module: "m",
                    missing: "d",
                },
                NexusErrorKind::Other,
                true,
            ),
            (
                "already-reg",
                KitError::AlreadyRegistered { module: "m" },
                NexusError::AlreadyRegistered { module: "m" },
                NexusErrorKind::Other,
                true,
            ),
            (
                "decorator",
                KitError::DecoratorTargetMissing { module: "m" },
                NexusError::DecoratorTargetMissing { module: "m" },
                NexusErrorKind::Other,
                true,
            ),
            (
                "version",
                KitError::VersionIncompatible {
                    module: "m",
                    dependency: "d",
                    required: "1",
                    provided: "0",
                },
                NexusError::VersionIncompatible {
                    module: "m",
                    dependency: "d",
                    required: "1",
                    provided: "0",
                },
                NexusErrorKind::Other,
                true,
            ),
            (
                "build",
                KitError::BuildFailed {
                    context: "ctx".into(),
                    source: fresh_boxed_source(),
                },
                NexusError::BuildFailed {
                    context: "ctx".into(),
                    source: fresh_boxed_source(),
                },
                NexusErrorKind::InitFailed,
                true,
            ),
            (
                "missing-cap",
                KitError::MissingCapability { key: "k".into() },
                NexusError::MissingCapability { key: "k".into() },
                NexusErrorKind::Missing,
                true,
            ),
            (
                "type-mismatch",
                KitError::CapabilityTypeMismatch { key: "k".into() },
                NexusError::CapabilityTypeMismatch { key: "k".into() },
                NexusErrorKind::TypeMismatch,
                true,
            ),
            (
                "missing-config",
                KitError::MissingConfig { key: "k".into() },
                NexusError::MissingConfig { key: "k".into() },
                NexusErrorKind::Missing,
                true,
            ),
            (
                "lifecycle",
                KitError::LifecycleFailed {
                    context: "ctx".into(),
                    source: fresh_boxed_source(),
                },
                NexusError::LifecycleFailed {
                    context: "ctx".into(),
                    source: fresh_boxed_source(),
                },
                NexusErrorKind::InitFailed,
                true,
            ),
            (
                "shutdown",
                KitError::ShutdownTimedOut { phases: vec![] },
                NexusError::ShutdownTimedOut { phases: vec![] },
                NexusErrorKind::Other,
                // deliberate: the string mirror cannot reconstruct typed
                // phases, so the reverse map lands on Internal (exit 1 —
                // the same code the kit's Other kind produces). See the
                // From<NexusError> comment in service::error.
                false,
            ),
        ];
        assert_eq!(cases.len(), 11, "one row per mirrored variant");
        for (name, kit_err, nexus_err, expected_kind, expect_kit) in cases {
            let mirrored = NexusError::from(kit_err);
            assert_eq!(
                std::mem::discriminant(&mirrored),
                std::mem::discriminant(&nexus_err),
                "{name}: mirror variant mismatch"
            );
            assert_eq!(
                mirrored.kind(),
                expected_kind,
                "{name}: kind must mirror the kit bucket"
            );
            // Reverse map (service::error) must re-encode into the matching
            // CodeNexusError::Kit variant with the same exit classification.
            let unified = CodeNexusError::from(mirrored);
            if expect_kit {
                match unified {
                    CodeNexusError::Kit(back) => {
                        let re = NexusError::from(back);
                        assert_eq!(
                            std::mem::discriminant(&re),
                            std::mem::discriminant(&nexus_err),
                            "{name}: reverse map lost the variant"
                        );
                        assert_eq!(
                            re.to_string(),
                            nexus_err.to_string(),
                            "{name}: Display text diverged across the round trip"
                        );
                    }
                    other => panic!("{name}: expected Kit classification, got {other:?}"),
                }
            } else {
                assert!(
                    !matches!(unified, CodeNexusError::Kit(_)),
                    "{name}: deliberately non-Kit classification"
                );
            }
        }
    }

    /// T014: `From<NexusError> for CodeNexusError` re-encodes each mirrored
    /// classification into the matching `CodeNexusError::Kit` variant, so
    /// exit-code classification stays exact. (The mapping lives in
    /// service::error — asserted there via the Kit round-trip here.)
    #[test]
    fn kit_error_mirror_is_lossless_for_missing_capability() {
        use crate::kit::KitError;
        let mirrored = NexusError::from(KitError::MissingCapability {
            key: "trace".to_string(),
        });
        match mirrored {
            NexusError::MissingCapability { key } => assert_eq!(key, "trace"),
            other => panic!("unexpected mirror variant: {other:?}"),
        }
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
