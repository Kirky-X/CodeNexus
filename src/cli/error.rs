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

use crate::daemon::DaemonError;
use crate::index::IndexError;
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
}

impl CliError {
    /// Returns the process exit code the CLI should use for this error,
    /// following PRD §4.1.6.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
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
            CliError::Daemon(_) => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
