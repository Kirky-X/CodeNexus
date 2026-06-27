// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! DaemonRunner capability trait (T6/unified-architecture Phase 2, Task 2.11).
//!
//! Defines [`DaemonRunner`], the capability trait object stored in
//! [`Kit`](crate::kit::Kit) under [`DaemonKey`](crate::kit::DaemonKey) when
//! the `daemon` feature is enabled. The concrete impl (Task 2.11) wraps the
//! existing [`Daemon`] + [`IndexObserver`] (Observer pattern) so that the
//! unified Kit can hand a pre-configured daemon handle to `daemon_cmd::run`
//! instead of having the CLI construct subsystems ad-hoc.
//!
//! # Blocking semantics
//!
//! [`DaemonRunner::start`] is **blocking** — it enters the daemon event loop
//! and returns only when the daemon stops (user interrupt, watcher error, or
//! channel disconnect). This mirrors the existing [`Daemon::run`] semantics.
//! Callers that need non-blocking behavior should spawn a thread.
//!
//! [`Daemon`]: super::Daemon
//! [`IndexObserver`]: super::IndexObserver
//! [`Daemon::run`]: super::Daemon::run

use std::path::Path;

use super::DaemonError;

/// Capability trait for the Daemon subsystem (file-watcher + incremental
/// indexing).
///
/// Stored in [`Kit`](crate::kit::Kit) as `Arc<dyn DaemonRunner>` under
/// [`DaemonKey`](crate::kit::DaemonKey) when the `daemon` feature is enabled.
/// Conceptually requires `StorageKey` + `IndexerKey`; the concrete impl
/// (Task 2.11) is self-contained — it opens its own [`IndexFacade`] from the
/// supplied `db_path` and constructs a fresh [`Daemon`] per `start` call.
/// Therefore `Requirements = NoRequirements` at the type level; the bootstrap
/// (Task 2.13) enforces build ordering (Storage → ... → Indexer → Daemon).
///
/// [`IndexFacade`]: crate::index::IndexFacade
/// [`Daemon`]: super::Daemon
pub trait DaemonRunner: Send + Sync {
    /// Starts the file-watching daemon over `watch_path`, triggering
    /// incremental indexing on code-file changes.
    ///
    /// # Blocking
    ///
    /// This method blocks until the daemon stops. See
    /// [Blocking semantics](self#blocking-semantics).
    ///
    /// # Errors
    ///
    /// Returns [`DaemonError::Notify`] if the watcher cannot be created or
    /// started (e.g. `watch_path` does not exist).
    fn start(&self, watch_path: &Path, project_name: &str) -> Result<(), DaemonError>;
}

/// Compile-time assertion that `DaemonRunner` is object-safe and `Send + Sync`.
#[cfg(test)]
const _: () = {
    fn _assert_object_safe(_: &dyn DaemonRunner) {}
    fn _assert_send_sync<T: Send + Sync + ?Sized>() {}
    fn _check() {
        _assert_send_sync::<dyn DaemonRunner>();
        let _ = _assert_object_safe;
    }
};
