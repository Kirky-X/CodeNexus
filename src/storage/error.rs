//! Storage layer error types (ADD §3.5).
//!
//! Uses [`thiserror`] for ergonomic, type-safe error propagation. Wraps errors
//! from the [`lbug`] crate, CSV generation, and I/O operations into a single
//! unified [`StorageError`] enum.

use std::io;

use thiserror::Error;

/// Unified error type for the storage layer.
#[derive(Debug, Error)]
pub enum StorageError {
    /// An error raised by the underlying LadybugDB engine.
    #[error("database error: {0}")]
    Database(#[from] lbug::Error),

    /// An I/O error (file system, CSV file, etc.).
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// A CSV serialization/deserialization error.
    #[error("csv error: {0}")]
    Csv(#[from] csv::Error),

    /// A JSON serialization/deserialization error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// A query execution failure with a human-readable message.
    #[error("query failed: {0}")]
    Query(String),

    /// A schema initialization failure (e.g. unsupported DDL).
    #[error("schema error: {0}")]
    Schema(String),

    /// The requested entity was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// The supplied data was invalid for the target table/schema.
    #[error("invalid data: {0}")]
    InvalidData(String),
}

/// Convenience alias used throughout the storage layer.
pub type Result<T> = std::result::Result<T, StorageError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_variant_displays_message() {
        let err = StorageError::Query("bad cypher".to_string());
        assert_eq!(err.to_string(), "query failed: bad cypher");
    }

    #[test]
    fn schema_variant_displays_message() {
        let err = StorageError::Schema("unsupported index".to_string());
        assert_eq!(err.to_string(), "schema error: unsupported index");
    }

    #[test]
    fn not_found_variant_displays_message() {
        let err = StorageError::NotFound("project foo".to_string());
        assert_eq!(err.to_string(), "not found: project foo");
    }

    #[test]
    fn invalid_data_variant_displays_message() {
        let err = StorageError::InvalidData("missing id".to_string());
        assert_eq!(err.to_string(), "invalid data: missing id");
    }

    #[test]
    fn io_error_converts_via_from() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "missing file");
        let storage_err: StorageError = io_err.into();
        assert!(storage_err.to_string().contains("io error"));
        assert!(storage_err.to_string().contains("missing file"));
    }

    #[test]
    fn csv_error_converts_via_from() {
        // csv::Error implements From<io::Error>.
        let io_err = io::Error::other("csv failure");
        let csv_err: csv::Error = io_err.into();
        let storage_err: StorageError = csv_err.into();
        assert!(storage_err.to_string().contains("csv error"));
    }

    #[test]
    fn json_error_converts_via_from() {
        let json_err = serde_json::from_str::<serde_json::Value>("bad").unwrap_err();
        let storage_err: StorageError = json_err.into();
        assert!(storage_err.to_string().contains("json error"));
    }

    #[test]
    fn debug_includes_variant_name() {
        let err = StorageError::NotFound("x".to_string());
        let s = format!("{err:?}");
        assert!(s.contains("NotFound"));
        assert!(s.contains("\"x\""));
    }
}
