// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! trait-kit module for the Daemon subsystem.
//!
//! Implements [`ModuleMeta`] + [`AsyncAutoBuilder`] for [`DaemonModule`],
//! wiring the existing [`Daemon`] + [`IndexObserver`] (Observer pattern)
//! into the unified Kit registry as `Arc<dyn DaemonRunner>` under
//! [`DaemonModule`](crate::kit::DaemonModule).
//!
//! # Capability lifecycle
//!
//! Unlike [`QueryCapability`] / [`TraceCapability`], the daemon is a
//! long-running blocking task. [`DaemonCapability`] therefore does **not**
//! hold a `Daemon` instance — it owns only the immutable construction
//! parameters (`db_path`, `debounce_ms`). Each
//! [`DaemonRunner::start`] invocation constructs a fresh [`Daemon`] +
//! [`IndexObserver`] and enters the blocking event loop. This matches the
//! existing `daemon_cmd::run` semantics (one daemon per CLI invocation).
//!
//! # Hot reconfiguration
//!
//! The debounce window (`debounce_ms`) can be hot-reloaded at runtime via
//! [`DaemonCapability::update_debounce_ms`] without restarting the daemon.
//! The capability holds the config behind an `Arc<RwLock<…>>` so each
//! [`DaemonRunner::start`] invocation reads the latest value.
//!
//! # Dependency note
//!
//! Conceptually the Daemon depends on `StorageModule` + `IndexerModule` (it
//! triggers incremental indexing via [`IndexFacade`]). The concrete
//! [`DaemonCapability`] is self-contained, however: it constructs its own
//! [`IndexFacade`] from the supplied `db_path`. Therefore
//! `dependencies = &[]` at the type level; the bootstrap
//! enforces build ordering (Storage → ... → Indexer → Daemon). This mirrors
//! the [`QueryModule`](crate::query::module::QueryModule) and
//! [`TraceModule`](crate::trace::module::TraceModule) design — see
//!
//! [`QueryCapability`]: crate::query::module::QueryCapability
//! [`TraceCapability`]: crate::trace::module::TraceCapability
//! [`Daemon`]: super::Daemon
//! [`IndexObserver`]: super::IndexObserver
//! [`IndexFacade`]: crate::index::IndexFacade
//! [`DaemonRunner::start`]: super::capability::DaemonRunner::start
use std::any::TypeId;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use crate::kit::{AsyncAutoBuilder, AsyncKit, ModuleMeta};

use super::capability::DaemonRunner;
use super::{Daemon, DaemonError, IndexObserver, DEFAULT_DEBOUNCE_MS};
use crate::index::IndexFacade;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for [`DaemonModule`].
///
/// Stored in Kit via `AsyncKit::set_config` and read in
/// [`AsyncAutoBuilder::build`]. The Daemon needs the database path (for
/// [`IndexFacade`]) and the debounce window in milliseconds
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Filesystem path to the LadybugDB database directory.
    pub db_path: PathBuf,
    /// Debounce window in milliseconds. Defaults to
    /// [`DEFAULT_DEBOUNCE_MS`] (2000ms) when not specified.
    pub debounce_ms: u64,
    /// Per-batch debug diagnostics in the event loop (CLI `--verbose`).
    pub verbose_events: bool,
    /// 增量索引完成后输出变更影响告警（CLI `--notify-impact`，默认关闭）。
    pub impact_notify: bool,
    /// 可选 webhook：影响告警同时 POST 到该 URL（`hub` feature，默认 None）。
    #[cfg(feature = "hub")]
    pub notify_webhook: Option<String>,
}

impl DaemonConfig {
    /// Creates a config with the given `db_path` and the default debounce
    /// window ([`DEFAULT_DEBOUNCE_MS`]).
    #[must_use]
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            verbose_events: false,
            impact_notify: false,
            #[cfg(feature = "hub")]
            notify_webhook: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Module (ModuleMeta + AsyncAutoBuilder)
// ---------------------------------------------------------------------------

/// trait-kit module tag for the Daemon subsystem.
///
/// Zero-sized marker — construction logic lives in
/// [`DaemonModule::build_cap`] (called from the [`AsyncAutoBuilder`] impl).
/// Register the capability in Kit via:
///
/// ```ignore
/// use codenexus::kit::{AsyncKit, DaemonModule};
/// use codenexus::daemon::DaemonConfig;
/// use std::path::PathBuf;
///
/// let mut kit = AsyncKit::new();
/// kit.set_config(DaemonConfig::new(PathBuf::from(":memory:")));
/// kit.register::<DaemonModule>()?;
/// let kit = kit.build().await?;
/// let daemon = kit.require::<DaemonModule>()?;
/// ```
pub struct DaemonModule;

impl ModuleMeta for DaemonModule {
    const NAME: &'static str = "daemon";
    fn dependencies() -> &'static [(&'static str, TypeId)] {
        &[]
    }
}

impl AsyncAutoBuilder for DaemonModule {
    type Capability = Arc<dyn DaemonRunner>;
    type Error = DaemonError;

    fn build<'a>(
        kit: &'a AsyncKit,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Capability, Self::Error>> + Send + 'a>> {
        Box::pin(async move {
            let config = kit
                .config::<DaemonConfig>()
                .map_err(|e| DaemonError::Io(std::io::Error::other(e.to_string())))?;
            Self::build_cap(&config)
        })
    }
}

impl DaemonModule {
    /// Constructs a DaemonCapability from the given config.
    ///
    /// Shared between [`AsyncAutoBuilder::build`] and tests so that
    /// capability-level tests can run without an async runtime.
    pub(crate) fn build_cap(config: &DaemonConfig) -> Result<Arc<dyn DaemonRunner>, DaemonError> {
        Ok(Arc::new(DaemonCapability {
            db_path: config.db_path.clone(),
            config: Arc::new(RwLock::new(config.clone())),
        }))
    }
}

// ---------------------------------------------------------------------------
// Concrete dyn DaemonRunner implementation
// ---------------------------------------------------------------------------

/// Concrete implementation of [`dyn DaemonRunner`] that constructs a fresh
/// [`Daemon`] + [`IndexObserver`] on every [`DaemonRunner::start`] call.
///
/// The capability holds `db_path` (immutable, `Send + Sync`) and the daemon
/// config behind an `Arc<RwLock<…>>` so that [`DaemonCapability::update_debounce_ms`]
/// can hot-reload the debounce window without restarting. Each `start`
/// invocation:
///
/// 1. Opens a fresh [`IndexFacade`] from `db_path` (lazy — does not touch
///    the database until indexing).
/// 2. Reads the current `debounce_ms` from the shared config.
/// 3. Constructs a [`Daemon`] with the configured `debounce_ms`.
/// 4. Registers an [`IndexObserver`] that triggers incremental indexing on
///    code-file changes.
/// 5. Enters the blocking event loop ([`Daemon::run`]).
///
/// This matches the existing `daemon_cmd::run` semantics (one daemon per
/// CLI invocation).
struct DaemonCapability {
    /// Database path passed to [`IndexFacade::new`].
    db_path: PathBuf,
    /// Shared daemon config (hot-reloadable via [`DaemonCapability::update_debounce_ms`]).
    config: Arc<RwLock<DaemonConfig>>,
}

impl DaemonRunner for DaemonCapability {
    fn start(&self, watch_path: &Path, project_name: &str) -> Result<(), DaemonError> {
        // Read the current debounce_ms from the shared config (hot-reloadable).
        let (debounce_ms, verbose_events, impact_notify, notify_webhook) = self
            .config
            .read()
            .map(|c| {
                (
                    c.debounce_ms,
                    c.verbose_events,
                    c.impact_notify,
                    #[cfg(feature = "hub")]
                    c.notify_webhook.clone(),
                )
            })
            .unwrap_or((
                DEFAULT_DEBOUNCE_MS,
                false,
                false,
                #[cfg(feature = "hub")]
                None,
            ));

        // Construct the IndexFacade (lazy — opens DB on first index call).
        let facade = IndexFacade::new(&self.db_path)
            .map_err(|e| std::io::Error::other(format!("IndexFacade::new: {e}")))?;

        // Construct the daemon with the current debounce window.
        let mut daemon = Daemon::new(watch_path, project_name, debounce_ms, &self.db_path);
        daemon.set_verbose(verbose_events);

        // Register the IndexObserver (Observer pattern) — triggers
        // incremental indexing on code-file changes.
        let mut observer =
            IndexObserver::new(facade, project_name.to_string(), watch_path.to_path_buf());
        // 影响告警（默认关闭）：增量索引成功后对变更文件输出受影响符号通知。
        if impact_notify {
            let notify_observer =
                crate::daemon::impact_observer::ImpactNotifyObserver::new(self.db_path.clone());
            #[cfg(feature = "hub")]
            let notify_observer = notify_observer.with_webhook(notify_webhook);
            observer.add_completion_observer(Box::new(notify_observer));
        }
        daemon.add_observer(Box::new(observer));

        // Enter the blocking event loop. Returns when the daemon stops
        // (user interrupt, watcher error, or channel disconnect).
        daemon.run()
    }

    fn update_debounce_ms(&self, new_ms: u64) {
        if let Ok(mut cfg) = self.config.write() {
            cfg.debounce_ms = new_ms;
        }
    }

    fn update_impact_notify(&self, enabled: bool) {
        if let Ok(mut cfg) = self.config.write() {
            cfg.impact_notify = enabled;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{AsyncKit, DaemonModule};

    #[test]
    fn build_returns_send_sync_capability() {
        let cap = DaemonModule::build_cap(&DaemonConfig::new(PathBuf::from(":memory:")))
            .expect("DaemonModule::build_cap");
        // If this compiles, DaemonCapability is Send + Sync (the dyn
        // DaemonRunner bound requires it). The Arc<dyn DaemonRunner> is also
        // Send + Sync.
        fn _assert_send_sync<T: Send + Sync>(_: &T) {}
        _assert_send_sync(&cap);
    }

    /// `start` with a nonexistent watch path must return `DaemonError::Notify`
    /// immediately (non-blocking failure path — the watcher cannot start).
    ///
    /// This is the only `start` code path that is safe to exercise in a unit
    /// test: all other paths enter the blocking event loop. End-to-end
    /// coverage lives in the `kit_bootstrap` integration test.
    #[test]
    fn capability_start_nonexistent_watch_path_returns_error() {
        let cap = DaemonModule::build_cap(&DaemonConfig::new(PathBuf::from(":memory:")))
            .expect("DaemonModule::build_cap");
        let result = cap.start(Path::new("/nonexistent/path/xyz/abc"), "demo");
        assert!(
            result.is_err(),
            "nonexistent watch path should fail immediately, got {result:?}"
        );
        // The error variant is DaemonError::Notify (from debouncer.watch).
        let err = result.err().unwrap();
        assert!(
            matches!(err, DaemonError::Notify(_)),
            "expected DaemonError::Notify, got {err:?}"
        );
    }

    /// Verify the full AsyncKit registration flow works end-to-end.
    #[tokio::test]
    async fn kit_registration_flow() {
        let mut kit = AsyncKit::new();
        kit.set_config(DaemonConfig::new(PathBuf::from(":memory:")));
        kit.register::<DaemonModule>()
            .expect("register::<DaemonModule>");
        let kit = kit.build().await.expect("build");

        assert!(kit.contains::<DaemonModule>(), "DaemonModule missing");

        let _required = kit
            .require::<DaemonModule>()
            .expect("require::<DaemonModule>");
    }

    /// `DaemonConfig::new` seeds the default debounce window.
    #[test]
    fn daemon_config_new_uses_default_debounce() {
        let cfg = DaemonConfig::new(PathBuf::from("/tmp/db.lbug"));
        assert_eq!(cfg.db_path, PathBuf::from("/tmp/db.lbug"));
        assert_eq!(cfg.debounce_ms, DEFAULT_DEBOUNCE_MS);
    }

    /// `update_debounce_ms` hot-reloads the debounce window in the shared
    /// config; subsequent `start` calls would use the new value.
    #[test]
    fn update_debounce_ms_changes_shared_config() {
        // Directly construct DaemonCapability (test is in the same module).
        let config = DaemonConfig::new(PathBuf::from(":memory:"));
        let cap = DaemonCapability {
            db_path: config.db_path.clone(),
            config: Arc::new(RwLock::new(config)),
        };

        // Default debounce should be DEFAULT_DEBOUNCE_MS.
        {
            let cfg = cap.config.read().unwrap();
            assert_eq!(cfg.debounce_ms, DEFAULT_DEBOUNCE_MS);
        }

        // Hot-reload to a new value.
        cap.update_debounce_ms(500);
        {
            let cfg = cap.config.read().unwrap();
            assert_eq!(cfg.debounce_ms, 500);
        }
    }
}
