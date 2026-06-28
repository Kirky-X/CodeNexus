// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Error types for the indexing pipeline (PRD §4.1.6).
//!
//! Maps each failure mode of the indexing pipeline to a distinct
//! [`IndexError`] variant so the CLI can produce the correct exit code:
//!
//! | Variant            | Exit code | Trigger                                  |
//! |--------------------|-----------|------------------------------------------|
//! | `PathNotFound`     | 1         | The supplied path does not exist.        |
//! | `DatabaseLocked`   | 2         | DB locked after 3 retries.               |
//! | `Storage`          | 2/4       | Underlying storage error (corrupt/lock). |
//! | `DatabaseCorrupt`  | 4         | LadybugDB files are corrupt.             |
//! | `Discover`         | 1         | File discovery failed.                   |
//! | `Parse`            | —         | Parse failure (skip file, continue).     |
//! | `Io`               | 3         | IO error (memory/disk).                  |

use thiserror::Error;

/// A specialized [`Result`](std::result::Result) for index operations.
pub type Result<T> = std::result::Result<T, IndexError>;

/// Errors that can occur during the indexing pipeline.
#[derive(Debug, Error)]
pub enum IndexError {
    /// The supplied path does not exist (PRD §4.1.6, exit code 1).
    #[error("path not found: {0}")]
    PathNotFound(String),

    /// The database was locked after 3 retries (PRD §4.1.6, exit code 2).
    #[error("database locked after 3 retries")]
    DatabaseLocked,

    /// The database is corrupt (PRD §4.1.6, exit code 4).
    #[error("database corrupt: {0}")]
    DatabaseCorrupt(String),

    /// A storage-layer error (wrapped from [`crate::storage::StorageError`]).
    ///
    /// Note: the `From<StorageError>` impl is manual (not `#[from]`) so that
    /// `StorageError::Corrupt(msg)` is mapped to
    /// [`IndexError::DatabaseCorrupt`] (exit code 4) rather than
    /// `IndexError::Storage` (exit code 2).
    #[error("storage error: {0}")]
    Storage(crate::storage::error::StorageError),

    /// A discover-layer error (wrapped from [`crate::discover::DiscoverError`]).
    #[error("discover error: {0}")]
    Discover(#[from] crate::discover::DiscoverError),

    /// A parse error. Per PRD §4.1.6, parse failures skip the file and
    /// continue indexing rather than aborting the pipeline.
    #[error("parse error: {0}")]
    Parse(String),

    /// An I/O error (file system, disk, memory).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl IndexError {
    /// Returns the process exit code the CLI should use for this error,
    /// following PRD §4.1.6.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            IndexError::PathNotFound(_) => 1,
            IndexError::DatabaseLocked => 2,
            IndexError::DatabaseCorrupt(_) => 4,
            // Storage errors default to exit code 2 (treat as DB lock/IO); the
            // CLI can refine this by inspecting the underlying StorageError.
            IndexError::Storage(_) => 2,
            IndexError::Discover(_) => 1,
            IndexError::Parse(_) => 0,
            IndexError::Io(_) => 3,
        }
    }
}

/// Converts a [`StorageError`] into an [`IndexError`].
///
/// Maps [`StorageError::Corrupt`] to [`IndexError::DatabaseCorrupt`] (exit
/// code 4) so the CLI produces the correct exit code for corrupt databases.
/// All other variants are wrapped as [`IndexError::Storage`] (exit code 2).
impl From<crate::storage::error::StorageError> for IndexError {
    fn from(e: crate::storage::error::StorageError) -> Self {
        match e {
            crate::storage::error::StorageError::Corrupt(msg) => {
                IndexError::DatabaseCorrupt(msg)
            }
            other => IndexError::Storage(other),
        }
    }
}

/// Converts a [`PhaseError`] into an [`IndexError`], preserving the original
/// [`IndexError`] variant when possible (Rule 12: fail loud).
///
/// Phases box their [`IndexError`] into [`PhaseError::ExecutionFailed`]; this
/// impl downcasts it back so the CLI produces the correct exit code.
/// Non-`IndexError` failures (infrastructure errors like cycles, missing deps)
/// fall back to [`IndexError::Storage`].
impl From<super::pipeline_dag::PhaseError> for IndexError {
    fn from(e: super::pipeline_dag::PhaseError) -> Self {
        use super::pipeline_dag::PhaseError;
        match e {
            PhaseError::ExecutionFailed { inner, .. } => match inner.downcast::<IndexError>() {
                Ok(boxed) => *boxed,
                Err(other) => {
                    IndexError::Storage(crate::storage::StorageError::Query(other.to_string()))
                }
            },
            other => IndexError::Storage(crate::storage::StorageError::Query(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::DiscoverError;
    use crate::storage::StorageError;

    // --- Display messages ---

    #[test]
    fn path_not_found_displays_message() {
        let err = IndexError::PathNotFound("/missing/path".to_string());
        let msg = err.to_string();
        assert!(msg.contains("path not found"), "got: {msg}");
        assert!(msg.contains("/missing/path"), "got: {msg}");
    }

    #[test]
    fn database_locked_displays_message() {
        let err = IndexError::DatabaseLocked;
        let msg = err.to_string();
        assert!(msg.contains("database locked"), "got: {msg}");
        assert!(msg.contains("3 retries"), "got: {msg}");
    }

    #[test]
    fn database_corrupt_displays_message() {
        let err = IndexError::DatabaseCorrupt("schema mismatch".to_string());
        let msg = err.to_string();
        assert!(msg.contains("database corrupt"), "got: {msg}");
        assert!(msg.contains("schema mismatch"), "got: {msg}");
    }

    #[test]
    fn parse_displays_message() {
        let err = IndexError::Parse("syntax error in foo.rs".to_string());
        let msg = err.to_string();
        assert!(msg.contains("parse error"), "got: {msg}");
        assert!(msg.contains("syntax error in foo.rs"), "got: {msg}");
    }

    #[test]
    fn io_displays_message() {
        let err = IndexError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"));
        let msg = err.to_string();
        assert!(msg.contains("io error"), "got: {msg}");
        assert!(msg.contains("missing"), "got: {msg}");
    }

    // --- From conversions ---

    #[test]
    fn from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: IndexError = io_err.into();
        assert!(matches!(err, IndexError::Io(_)));
    }

    #[test]
    fn from_storage_error() {
        let storage_err = StorageError::Query("bad cypher".to_string());
        let err: IndexError = storage_err.into();
        assert!(matches!(err, IndexError::Storage(_)));
        let msg = err.to_string();
        assert!(msg.contains("storage error"), "got: {msg}");
        assert!(msg.contains("bad cypher"), "got: {msg}");
    }

    #[test]
    fn from_discover_error() {
        let discover_err = DiscoverError::from(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        ));
        let err: IndexError = discover_err.into();
        assert!(matches!(err, IndexError::Discover(_)));
        let msg = err.to_string();
        assert!(msg.contains("discover error"), "got: {msg}");
    }

    // --- exit_code mapping (PRD §4.1.6) ---

    #[test]
    fn exit_code_path_not_found_is_1() {
        assert_eq!(IndexError::PathNotFound("/x".to_string()).exit_code(), 1);
    }

    #[test]
    fn exit_code_database_locked_is_2() {
        assert_eq!(IndexError::DatabaseLocked.exit_code(), 2);
    }

    #[test]
    fn exit_code_database_corrupt_is_4() {
        assert_eq!(IndexError::DatabaseCorrupt("x".to_string()).exit_code(), 4);
    }

    #[test]
    fn exit_code_storage_is_2() {
        let err: IndexError = StorageError::Query("x".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_discover_is_1() {
        let err: IndexError = DiscoverError::from(std::io::Error::other("x")).into();
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn exit_code_parse_is_0() {
        // Parse errors are non-fatal (skip file, continue) → exit code 0.
        assert_eq!(IndexError::Parse("x".to_string()).exit_code(), 0);
    }

    #[test]
    fn exit_code_io_is_3() {
        let err: IndexError = std::io::Error::other("x").into();
        assert_eq!(err.exit_code(), 3);
    }

    // --- Debug ---

    #[test]
    fn debug_includes_variant_name() {
        let err = IndexError::PathNotFound("/x".to_string());
        let s = format!("{err:?}");
        assert!(s.contains("PathNotFound"), "got: {s}");
        assert!(s.contains("/x"), "got: {s}");
    }

    // --- Result alias ---

    #[test]
    fn result_alias_compiles() {
        let ok: Result<i32> = Ok(42);
        assert!(ok.is_ok());
        let err: Result<i32> = Err(IndexError::Parse("x".to_string()));
        assert!(err.is_err());
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<IndexError>();
    }
}
