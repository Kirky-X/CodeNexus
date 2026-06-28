// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CLI error types with PRD §4.1.6 exit-code mapping.
//!
//! [`CliError`] wraps the underlying subsystem errors ([`IndexError`],
//! [`QueryError`], [`TraceError`], [`StorageError`]) and surfaces a uniform
//! [`exit_code`](CliError::exit_code) so `main.rs` can `std::process::exit`
//! with the correct status.
//!
//! # Exit codes (PRD §4.1.6)
//!
//! | Code | Meaning                | Source                              |
//! |------|------------------------|-------------------------------------|
//! | 0    | success                | —                                   |
//! | 1    | input error            | path not found, bad args            |
//! | 2    | database locked / IO   | storage error, query error          |
//! | 3    | system error           | IO error (memory/disk)              |
//! | 4    | database corrupt       | corrupt database                    |

use thiserror::Error;
use tracing::error;

#[cfg(feature = "daemon")]
use crate::daemon::DaemonError;
use crate::index::IndexError;
use crate::kit::KitError;
use crate::query::QueryError;
use crate::storage::StorageError;
use crate::trace::TraceError;

/// A specialized [`Result`](std::result::Result) for CLI operations.
pub type Result<T> = std::result::Result<T, CliError>;

/// Errors that can occur during CLI command execution.
///
/// Each variant maps to a specific process exit code via [`exit_code`](Self::exit_code).
#[derive(Debug, Error)]
pub enum CliError {
    /// An indexing pipeline error (PRD §4.1.6).
    #[error("{0}")]
    Index(#[from] IndexError),

    /// A query engine error.
    #[error("{0}")]
    Query(#[from] QueryError),

    /// A trace engine error.
    #[error("{0}")]
    Trace(#[from] TraceError),

    /// A storage-layer error.
    #[error("{0}")]
    Storage(#[from] StorageError),

    /// A daemon-mode error (file watcher / IO).
    #[cfg(feature = "daemon")]
    #[error("{0}")]
    Daemon(#[from] DaemonError),

    /// An I/O error (file system, disk, memory).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A JSON serialization/deserialization error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Invalid user input (bad flag value, unknown trace type, etc.).
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// A project was not found in the database.
    #[error("project not found: {0}")]
    ProjectNotFound(String),

    /// A Kit registry error (missing capability, build failure, etc.).
    ///
    /// Maps to exit code 3 (system error) — Kit failures are programming /
    /// bootstrap bugs, not user input or database issues.
    #[error("kit error: {0}")]
    Kit(#[from] KitError),
}

impl CliError {
    /// Returns the process exit code the CLI should use for this error,
    /// following PRD §4.1.6.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        let code = match self {
            // Index errors carry their own exit-code mapping.
            CliError::Index(e) => e.exit_code(),
            // Input validation failures → exit 1.
            CliError::InvalidInput(_) => 1,
            // Missing project → input error → exit 1.
            CliError::ProjectNotFound(_) => 1,
            // IO errors (disk/memory) → exit 3.
            CliError::Io(_) => 3,
            // JSON serialization is a system error → exit 3.
            CliError::Json(_) => 3,
            // Query/Trace/Storage errors are database-side → exit 2 by default.
            CliError::Query(_) => 2,
            CliError::Trace(_) => 2,
            CliError::Storage(_) => 2,
            // Daemon errors (notify watcher / IO) are system errors → exit 3.
            #[cfg(feature = "daemon")]
            CliError::Daemon(_) => 3,
            // Kit errors (missing capability, build failure) → exit 3 by
            // default, but if the underlying source is a database-corruption
            // error (StorageError::Corrupt or IndexError::DatabaseCorrupt),
            // surface exit 4 so `codenexus index` on a corrupt DB returns the
            // PRD §4.1.6 corrupt-database exit code even when build_kit
            // fails before the index command runs.
            CliError::Kit(e) => kit_exit_code(e),
        };
        error!(
            event = "error",
            error_type = ?self,
            exit_code = code,
            "CLI error occurred"
        );
        code
    }
}

/// Resolves the exit code for a [`CliError::Kit`] by walking the error
/// source chain.
///
/// `build_kit` runs before any CLI command (Task 2.14); when the database is
/// corrupt, `StorageModuleBuilder::build` fails and the error is wrapped as
/// `KitError::BuildFailed { source: Box<StorageError::Corrupt> }`. The
/// default `Kit → exit 3` mapping would mask the corruption, so this helper
/// traverses `Error::source()` looking for:
///
/// - [`IndexError::DatabaseCorrupt`] → exit 4
/// - [`StorageError::Corrupt`] → exit 4
///
/// If neither is found, falls back to exit 3 (system error).
fn kit_exit_code(e: &KitError) -> i32 {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = current {
        if let Some(index_err) = err.downcast_ref::<IndexError>() {
            if matches!(index_err, IndexError::DatabaseCorrupt(_)) {
                return 4;
            }
        }
        if let Some(storage_err) = err.downcast_ref::<StorageError>() {
            if matches!(storage_err, StorageError::Corrupt(_)) {
                return 4;
            }
        }
        current = err.source();
    }
    3
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    // --- Display messages ---

    #[test]
    fn invalid_input_displays_message() {
        let err = CliError::InvalidInput("bad trace type".to_string());
        let msg = err.to_string();
        assert!(msg.contains("invalid input"), "got: {msg}");
        assert!(msg.contains("bad trace type"), "got: {msg}");
    }

    #[test]
    fn project_not_found_displays_message() {
        let err = CliError::ProjectNotFound("demo".to_string());
        let msg = err.to_string();
        assert!(msg.contains("project not found"), "got: {msg}");
        assert!(msg.contains("demo"), "got: {msg}");
    }

    #[test]
    fn io_error_displays_message() {
        let err = CliError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing",
        ));
        let msg = err.to_string();
        assert!(msg.contains("io error"), "got: {msg}");
        assert!(msg.contains("missing"), "got: {msg}");
    }

    #[test]
    fn json_error_displays_message() {
        let json_err = serde_json::from_str::<serde_json::Value>("bad").unwrap_err();
        let err: CliError = json_err.into();
        let msg = err.to_string();
        assert!(msg.contains("json error"), "got: {msg}");
    }

    #[test]
    fn index_error_wraps_display() {
        let err: CliError = IndexError::PathNotFound("/missing".to_string()).into();
        let msg = err.to_string();
        assert!(msg.contains("path not found"), "got: {msg}");
        assert!(msg.contains("/missing"), "got: {msg}");
    }

    #[test]
    fn query_error_wraps_display() {
        let err: CliError = QueryError::InvalidQuery("empty".to_string()).into();
        let msg = err.to_string();
        assert!(msg.contains("invalid query"), "got: {msg}");
    }

    #[test]
    fn trace_error_wraps_display() {
        let err: CliError = TraceError::SymbolNotFound("foo".to_string()).into();
        let msg = err.to_string();
        assert!(msg.contains("symbol not found"), "got: {msg}");
    }

    #[test]
    fn storage_error_wraps_display() {
        let err: CliError = StorageError::Query("bad cypher".to_string()).into();
        let msg = err.to_string();
        assert!(msg.contains("query failed"), "got: {msg}");
    }

    #[test]
    #[cfg(feature = "daemon")]
    fn daemon_error_wraps_display() {
        let err: CliError = DaemonError::Io(std::io::Error::other("watcher down")).into();
        let msg = err.to_string();
        assert!(msg.contains("io error"), "got: {msg}");
        assert!(msg.contains("watcher down"), "got: {msg}");
    }

    // --- exit_code mapping (PRD §4.1.6) ---

    #[test]
    fn exit_code_index_path_not_found_is_1() {
        let err: CliError = IndexError::PathNotFound("/x".to_string()).into();
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn exit_code_index_database_locked_is_2() {
        let err: CliError = IndexError::DatabaseLocked.into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_index_database_corrupt_is_4() {
        let err: CliError = IndexError::DatabaseCorrupt("x".to_string()).into();
        assert_eq!(err.exit_code(), 4);
    }

    #[test]
    fn exit_code_index_io_is_3() {
        let err: CliError = IndexError::Io(std::io::Error::other("x")).into();
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn exit_code_invalid_input_is_1() {
        let err = CliError::InvalidInput("x".to_string());
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn exit_code_project_not_found_is_1() {
        let err = CliError::ProjectNotFound("x".to_string());
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn exit_code_io_is_3() {
        let err = CliError::Io(std::io::Error::other("x"));
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn exit_code_json_is_3() {
        let json_err = serde_json::from_str::<serde_json::Value>("bad").unwrap_err();
        let err: CliError = json_err.into();
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn exit_code_query_is_2() {
        let err: CliError = QueryError::Query("x".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_trace_is_2() {
        let err: CliError = TraceError::SymbolNotFound("x".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_storage_is_2() {
        let err: CliError = StorageError::Query("x".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    #[cfg(feature = "daemon")]
    fn exit_code_daemon_is_3() {
        let err: CliError = DaemonError::Io(std::io::Error::other("x")).into();
        assert_eq!(err.exit_code(), 3, "daemon error → exit 3 (system error)");
    }

    // --- From conversions ---

    #[test]
    fn from_index_error() {
        let err: CliError = IndexError::PathNotFound("/x".to_string()).into();
        assert!(matches!(err, CliError::Index(_)));
    }

    #[test]
    fn from_query_error() {
        let err: CliError = QueryError::Query("x".to_string()).into();
        assert!(matches!(err, CliError::Query(_)));
    }

    #[test]
    fn from_trace_error() {
        let err: CliError = TraceError::InvalidDepth(0).into();
        assert!(matches!(err, CliError::Trace(_)));
    }

    #[test]
    fn from_storage_error() {
        let err: CliError = StorageError::Query("x".to_string()).into();
        assert!(matches!(err, CliError::Storage(_)));
    }

    #[test]
    #[cfg(feature = "daemon")]
    fn from_daemon_error() {
        let err: CliError = DaemonError::Io(std::io::Error::other("x")).into();
        assert!(matches!(err, CliError::Daemon(_)));
    }

    #[test]
    fn from_io_error() {
        let err: CliError = std::io::Error::other("x").into();
        assert!(matches!(err, CliError::Io(_)));
    }

    #[test]
    fn from_json_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("bad").unwrap_err();
        let err: CliError = json_err.into();
        assert!(matches!(err, CliError::Json(_)));
    }

    // --- Debug / Send + Sync ---

    #[test]
    fn debug_includes_variant() {
        let err = CliError::InvalidInput("x".to_string());
        let s = format!("{err:?}");
        assert!(s.contains("InvalidInput"), "got: {s}");
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CliError>();
    }

    #[test]
    fn result_alias_compiles() {
        let ok: Result<i32> = Ok(42);
        assert!(ok.is_ok());
        let err: Result<i32> = Err(CliError::InvalidInput("x".to_string()));
        assert!(err.is_err());
    }

    // --- LOG-004: error event emission ---

    /// A `MakeWriter` that buffers emitted events into a shared `Vec<u8>` so a
    /// test can assert on what the subscriber actually wrote.
    struct CapturingMakeWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl MakeWriter for CapturingMakeWriter {
        type Writer = CapturingWriter;

        fn make_writer(&self) -> Self::Writer {
            CapturingWriter {
                buf: self.buf.clone(),
            }
        }
    }

    struct CapturingWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for CapturingWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.buf.lock().unwrap().write_all(bytes)?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Runs `f` inside a scoped tracing subscriber that captures all event
    /// output into a string, returning that string.
    fn capture_tracing<R>(f: impl FnOnce() -> R) -> String {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::FmtSubscriber::builder()
            .with_target(false)
            .with_writer(CapturingMakeWriter { buf: buf.clone() })
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = buf.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn log_004_error_event_emitted_on_exit_code() {
        let err = CliError::InvalidInput("bad input".to_string());

        let captured = capture_tracing(|| {
            let code = err.exit_code();
            assert_eq!(code, 1);
        });

        assert!(
            captured.contains("error"),
            "LOG-004: error event should be emitted, got: {captured:?}"
        );
        assert!(
            captured.contains("exit_code"),
            "error event should carry exit_code field"
        );
        assert!(
            captured.contains("InvalidInput"),
            "error event should carry the error type via Debug"
        );
    }

    #[test]
    fn log_004_error_event_carries_correct_exit_code() {
        let err: CliError = IndexError::PathNotFound("/missing".to_string()).into();

        let captured = capture_tracing(|| {
            let _ = err.exit_code();
        });

        // The error event should mention exit_code=1 (path not found → exit 1).
        assert!(
            captured.contains("exit_code") && captured.contains("1"),
            "LOG-004: error event should carry the correct exit code, got: {captured:?}"
        );
    }

    // --- Kit source-chain downcast (corruption surfacing) ---

    /// `KitError::BuildFailed` wrapping `StorageError::Corrupt` must surface
    /// exit code 4 (database corrupt), not the default 3 (system error).
    /// This mirrors the real `build_kit` failure path when the LadybugDB
    /// files are corrupt: `StorageModuleBuilder::build` →
    /// `StorageConnection::open` → `StorageError::Corrupt` →
    /// `KitError::BuildFailed { source: Box<StorageError::Corrupt> }`.
    #[test]
    fn exit_code_kit_build_failed_with_storage_corrupt_is_4() {
        let kit_err = KitError::BuildFailed {
            module: "storage",
            source: Box::new(StorageError::Corrupt("invalid LadybugDB header".to_string())),
        };
        let err: CliError = kit_err.into();
        assert_eq!(
            err.exit_code(),
            4,
            "Kit(BuildFailed{{StorageError::Corrupt}}) → exit 4 (database corrupt)"
        );
    }

    /// `KitError::BuildFailed` wrapping `IndexError::DatabaseCorrupt` must
    /// also surface exit code 4. This covers the path where an intermediate
    /// layer has already converted `StorageError::Corrupt` into
    /// `IndexError::DatabaseCorrupt` via the manual `From` impl.
    #[test]
    fn exit_code_kit_build_failed_with_index_database_corrupt_is_4() {
        let kit_err = KitError::BuildFailed {
            module: "indexer",
            source: Box::new(IndexError::DatabaseCorrupt("schema mismatch".to_string())),
        };
        let err: CliError = kit_err.into();
        assert_eq!(
            err.exit_code(),
            4,
            "Kit(BuildFailed{{IndexError::DatabaseCorrupt}}) → exit 4 (database corrupt)"
        );
    }

    /// `KitError::MissingCapability` (no source chain) falls back to the
    /// default Kit exit code 3 (system error).
    #[test]
    fn exit_code_kit_missing_capability_is_3() {
        let kit_err = KitError::MissingCapability { key: "storage" };
        let err: CliError = kit_err.into();
        assert_eq!(
            err.exit_code(),
            3,
            "Kit(MissingCapability) → exit 3 (default system error)"
        );
    }

    /// `KitError::BuildFailed` wrapping a non-corruption error (e.g. plain
    /// `std::io::Error`) must fall back to exit code 3, not 4.
    #[test]
    fn exit_code_kit_build_failed_with_other_error_is_3() {
        let kit_err = KitError::BuildFailed {
            module: "parser",
            source: Box::new(std::io::Error::other("config missing")),
        };
        let err: CliError = kit_err.into();
        assert_eq!(
            err.exit_code(),
            3,
            "Kit(BuildFailed{{io::Error}}) → exit 3 (default system error)"
        );
    }
}
