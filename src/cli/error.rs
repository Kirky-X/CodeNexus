// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CLI error types with exit-code mapping for the unified sdforge CLI.
//!
//! [`CliError`] wraps the underlying subsystem errors ([`IndexError`],
//! [`QueryError`], [`TraceError`], [`StorageError`]) and surfaces a uniform
//! [`exit_code`](CliError::exit_code) so `main.rs` can `std::process::exit`
//! with the correct status.
//!
//! # Exit codes (v0.3.2 unified CLI)
//!
//! | Code | Meaning                | Variants                              |
//! |------|------------------------|---------------------------------------|
//! | 0    | success                | —                                     |
//! | 1    | internal/system error  | Internal, Io, Kit, Json, Daemon       |
//! | 2    | client error           | InvalidInput, ProjectNotFound, Query, Trace, Storage |
//! | 3    | (reserved)             | —                                     |
//! | 4    | not found / corrupt    | NotFound, Index(corrupt), Kit(corrupt)|

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

    /// A resource was not found (symbol, project, file, etc.).
    #[error("not found: {0}")]
    NotFound(String),

    /// An internal/system error (unexpected failure, not user-caused).
    #[error("internal error: {0}")]
    Internal(String),

    /// A project was not found in the database.
    #[error("project not found: {0}")]
    ProjectNotFound(String),

    /// A Kit registry error (missing capability, build failure, etc.).
    ///
    /// Maps to exit code 1 (system error), or 4 if the underlying cause is
    /// database corruption. Kit failures are programming / bootstrap bugs,
    /// not user input or database issues.
    #[error("kit error: {0}")]
    Kit(#[from] KitError),
}

impl CliError {
    /// Returns the process exit code the CLI should use for this error.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        let code = match self {
            // Index errors carry their own exit-code mapping.
            CliError::Index(e) => e.exit_code(),
            // Input validation failures → exit 2.
            CliError::InvalidInput(_) => 2,
            // Missing project → input error → exit 2.
            CliError::ProjectNotFound(_) => 2,
            // Resource not found → exit 4.
            CliError::NotFound(_) => 4,
            // Internal/system errors → exit 1.
            CliError::Internal(_) => 1,
            CliError::Io(_) => 1,
            CliError::Json(_) => 1,
            // Query/Trace/Storage errors are database-side → exit 2.
            CliError::Query(_) => 2,
            CliError::Trace(_) => 2,
            CliError::Storage(_) => 2,
            // Daemon errors (notify watcher / IO) are system errors → exit 1.
            #[cfg(feature = "daemon")]
            CliError::Daemon(_) => 1,
            // Kit errors → exit 1 (system error), or 4 if corrupt.
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
/// `build_kit` runs before any CLI command; when the database is corrupt,
/// `StorageModuleBuilder::build` fails and the error is wrapped as
/// `KitError::BuildFailed { source: Box<StorageError::Corrupt> }`. This
/// helper traverses `Error::source()` looking for:
///
/// - [`IndexError::DatabaseCorrupt`] → exit 4
/// - [`StorageError::Corrupt`] → exit 4
///
/// Falls back to exit 1 (system error) if neither is found.
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
    1
}

/// Converts an [`ApiError`] (from sdforge) into a [`CliError`].
///
/// Mapping (design.md §3.1):
/// - `InvalidInput` → `CliError::InvalidInput` (exit 2)
/// - `Internal` → `CliError::Internal` (exit 1), prints error_id to stderr
/// - `NotFound` → `CliError::NotFound` (exit 4)
/// - All other variants → `CliError::Internal` (exit 1)
#[cfg(any(feature = "cli", feature = "mcp"))]
impl From<sdforge::prelude::ApiError> for CliError {
    fn from(e: sdforge::prelude::ApiError) -> Self {
        use sdforge::prelude::ApiError;
        match e {
            ApiError::InvalidInput { message, .. } => CliError::InvalidInput(message),
            ApiError::Internal {
                message, error_id, ..
            } => {
                eprintln!("[error_id: {error_id}] {message}");
                CliError::Internal(message)
            }
            ApiError::NotFound { resource, .. } => CliError::NotFound(resource),
            other => CliError::Internal(format!("Unexpected error: {other}")),
        }
    }
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
    fn exit_code_index_io_is_1() {
        let err: CliError = IndexError::Io(std::io::Error::other("x")).into();
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn exit_code_invalid_input_is_2() {
        let err = CliError::InvalidInput("x".to_string());
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_project_not_found_is_2() {
        let err = CliError::ProjectNotFound("x".to_string());
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_internal_is_1() {
        let err = CliError::Internal("unexpected failure".to_string());
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn exit_code_not_found_is_4() {
        let err = CliError::NotFound("symbol foo".to_string());
        assert_eq!(err.exit_code(), 4);
    }

    #[test]
    fn exit_code_io_is_1() {
        let err = CliError::Io(std::io::Error::other("x"));
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn exit_code_json_is_1() {
        let json_err = serde_json::from_str::<serde_json::Value>("bad").unwrap_err();
        let err: CliError = json_err.into();
        assert_eq!(err.exit_code(), 1);
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
    fn exit_code_daemon_is_1() {
        let err: CliError = DaemonError::Io(std::io::Error::other("x")).into();
        assert_eq!(err.exit_code(), 1, "daemon error → exit 1 (system error)");
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

    impl MakeWriter<'_> for CapturingMakeWriter {
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
            assert_eq!(code, 2);
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
    /// default Kit exit code 1 (system error).
    #[test]
    fn exit_code_kit_missing_capability_is_1() {
        let kit_err = KitError::MissingCapability { key: "storage" };
        let err: CliError = kit_err.into();
        assert_eq!(
            err.exit_code(),
            1,
            "Kit(MissingCapability) → exit 1 (default system error)"
        );
    }

    /// `KitError::BuildFailed` wrapping a non-corruption error (e.g. plain
    /// `std::io::Error`) must fall back to exit code 1, not 4.
    #[test]
    fn exit_code_kit_build_failed_with_other_error_is_1() {
        let kit_err = KitError::BuildFailed {
            module: "parser",
            source: Box::new(std::io::Error::other("config missing")),
        };
        let err: CliError = kit_err.into();
        assert_eq!(
            err.exit_code(),
            1,
            "Kit(BuildFailed{{io::Error}}) → exit 1 (default system error)"
        );
    }

    // --- From<ApiError> conversion (design.md §3.1) ---

    #[test]
    fn from_api_error_invalid_input_maps_to_cli_invalid_input() {
        let api_err = sdforge::prelude::ApiError::invalid_input(
            "bad cypher",
            Some("query".to_string()),
            None,
        );
        let cli_err: CliError = api_err.into();
        assert!(matches!(cli_err, CliError::InvalidInput(_)));
        assert_eq!(cli_err.exit_code(), 2);
    }

    #[test]
    fn from_api_error_internal_maps_to_cli_internal() {
        let api_err =
            sdforge::prelude::ApiError::internal_error("boom", "err-deadbeef");
        let cli_err: CliError = api_err.into();
        assert!(matches!(cli_err, CliError::Internal(_)));
        assert_eq!(cli_err.exit_code(), 1);
    }

    #[test]
    fn from_api_error_not_found_maps_to_cli_not_found() {
        let api_err = sdforge::prelude::ApiError::not_found(
            "symbol",
            Some("foo".to_string()),
        );
        let cli_err: CliError = api_err.into();
        assert!(matches!(cli_err, CliError::NotFound(_)));
        assert_eq!(cli_err.exit_code(), 4);
    }

    #[test]
    fn from_api_error_other_variant_maps_to_cli_internal() {
        let api_err = sdforge::prelude::ApiError::authentication_failed("no token");
        let cli_err: CliError = api_err.into();
        assert!(matches!(cli_err, CliError::Internal(_)));
        assert_eq!(cli_err.exit_code(), 1);
    }
}
