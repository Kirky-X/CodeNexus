// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit module for the Daemon subsystem (T6/unified-architecture
//! Phase 2, Task 2.11; v0.3.3 AsyncKit migration).
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
//! # Hot reconfiguration (future work)
//!
//! The spec mentions `DaemonConfig` via `ConfigHandle` for hot-reloading
//! `debounce_ms` without rebuilding the watcher. This is **not implemented**
//! in Task 2.11 — the current [`Daemon`] takes `debounce_ms` as a
//! construction-time constant. Hot reload would require refactoring
//! [`Daemon`] to read from a shared `ConfigHandle<DaemonConfig>` on each
//! debouncer tick. Tracked as future work; out of scope for the
//! unified-registry migration.
//!
//! # Dependency note
//!
//! Conceptually the Daemon depends on `StorageModule` + `IndexerModule` (it
//! triggers incremental indexing via [`IndexFacade`]). The concrete
//! [`DaemonCapability`] is self-contained, however: it constructs its own
//! [`IndexFacade`] from the supplied `db_path`. Therefore
//! `dependencies = &[]` at the type level; the bootstrap (Task 2.13)
//! enforces build ordering (Storage → ... → Indexer → Daemon). This mirrors
//! the [`QueryModule`](crate::query::module::QueryModule) and
//! [`TraceModule`](crate::trace::module::TraceModule) design — see
//! `design.md` D1 for the rationale.
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
use std::sync::Arc;

use crate::kit::{AsyncAutoBuilder, AsyncKit, ModuleMeta};

use super::capability::DaemonRunner;
use super::{Daemon, DaemonError, IndexObserver, DEFAULT_DEBOUNCE_MS};
use crate::index::IndexFacade;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for [`DaemonModule`] (Task 2.11).
///
/// Stored in Kit via `AsyncKit::set_config` and read in
/// [`AsyncAutoBuilder::build`]. The Daemon needs the database path (for
/// [`IndexFacade`]) and the debounce window in milliseconds
/// (BR-DAEMON-001/004).
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Filesystem path to the LadybugDB database directory.
    pub db_path: PathBuf,
    /// Debounce window in milliseconds (BR-DAEMON-001/004). Defaults to
    /// [`DEFAULT_DEBOUNCE_MS`] (2000ms) when not specified.
    pub debounce_ms: u64,
}

impl DaemonConfig {
    /// Creates a config with the given `db_path` and the default debounce
    /// window ([`DEFAULT_DEBOUNCE_MS`]).
    #[must_use]
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            debounce_ms: DEFAULT_DEBOUNCE_MS,
        }
    }
}

// ---------------------------------------------------------------------------
// Module (ModuleMeta + AsyncAutoBuilder)
// ---------------------------------------------------------------------------

/// trait-kit module tag for the Daemon subsystem (Task 2.11).
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
                .map_err(|e| {
                    DaemonError::Io(std::io::Error::other(e.to_string()))
                })?;
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
            debounce_ms: config.debounce_ms,
        }))
    }
}

// ---------------------------------------------------------------------------
// Concrete dyn DaemonRunner implementation
// ---------------------------------------------------------------------------

/// Concrete implementation of [`dyn DaemonRunner`] that constructs a fresh
/// [`Daemon`] + [`IndexObserver`] on every [`DaemonRunner::start`] call.
///
/// The capability owns only `db_path` and `debounce_ms` (both immutable,
/// `Send + Sync`). Each `start` invocation:
///
/// 1. Opens a fresh [`IndexFacade`] from `db_path` (lazy — does not touch
///    the database until indexing).
/// 2. Constructs a [`Daemon`] with the configured `debounce_ms`.
/// 3. Registers an [`IndexObserver`] that triggers incremental indexing on
///    code-file changes.
/// 4. Enters the blocking event loop ([`Daemon::run`]).
///
/// This matches the existing `daemon_cmd::run` semantics (one daemon per
/// CLI invocation).
struct DaemonCapability {
    /// Database path passed to [`IndexFacade::new`].
    db_path: PathBuf,
    /// Debounce window in milliseconds (BR-DAEMON-001/004).
    debounce_ms: u64,
}

impl DaemonRunner for DaemonCapability {
    fn start(&self, watch_path: &Path, project_name: &str) -> Result<(), DaemonError> {
        // Construct the IndexFacade (lazy — opens DB on first index call).
        let facade = IndexFacade::new(&self.db_path)
            .map_err(|e| std::io::Error::other(format!("IndexFacade::new: {e}")))?;

        // Construct the daemon with the configured debounce window.
        let mut daemon = Daemon::new(watch_path, project_name, self.debounce_ms, &self.db_path);

        // Register the IndexObserver (Observer pattern) — triggers
        // incremental indexing on code-file changes (BR-DAEMON-003).
        let observer =
            IndexObserver::new(facade, project_name.to_string(), watch_path.to_path_buf());
        daemon.add_observer(Box::new(observer));

        // Enter the blocking event loop. Returns when the daemon stops
        // (user interrupt, watcher error, or channel disconnect).
        daemon.run()
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
    /// coverage lives in the `kit_bootstrap` integration test (Task 1.7).
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
}
