//! Query engine error types (PRD §4.4).
//!
//! Uses [`thiserror`] for ergonomic, type-safe error propagation. Wraps errors
//! from the underlying [`crate::storage::StorageError`] and adds query-specific
//! variants for invalid input and full-text search failures.

use thiserror::Error;

/// A specialized [`Result`](std::result::Result) for query operations.
pub type Result<T> = std::result::Result<T, QueryError>;

/// Errors that can occur during query execution.
#[derive(Debug, Error)]
pub enum QueryError {
    /// A storage-layer error (wrapped from [`crate::storage::StorageError`]).
    #[error("storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),

    /// A Cypher execution failure with a human-readable message.
    #[error("query failed: {0}")]
    Query(String),

    /// The supplied query input was invalid (empty, malformed, etc.).
    #[error("invalid query: {0}")]
    InvalidQuery(String),

    /// A full-text search failure (FTS extension missing or query failed).
    #[error("fulltext search failed: {0}")]
    FullText(String),
}

impl QueryError {
    /// Returns `true` if this error originated from an invalid query input.
    #[must_use]
    pub fn is_invalid_query(&self) -> bool {
        matches!(self, QueryError::InvalidQuery(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StorageError;

    #[test]
    fn storage_variant_displays_message() {
        let err = QueryError::Storage(StorageError::Query("bad cypher".to_string()));
        let msg = err.to_string();
        assert!(msg.contains("storage error"), "got: {msg}");
        assert!(msg.contains("bad cypher"), "got: {msg}");
    }

    #[test]
    fn query_variant_displays_message() {
        let err = QueryError::Query("syntax error".to_string());
        assert_eq!(err.to_string(), "query failed: syntax error");
    }

    #[test]
    fn invalid_query_variant_displays_message() {
        let err = QueryError::InvalidQuery("empty query".to_string());
        assert_eq!(err.to_string(), "invalid query: empty query");
    }

    #[test]
    fn fulltext_variant_displays_message() {
        let err = QueryError::FullText("fts unavailable".to_string());
        assert_eq!(err.to_string(), "fulltext search failed: fts unavailable");
    }

    #[test]
    fn from_storage_error_converts() {
        let storage_err = StorageError::Query("oops".to_string());
        let err: QueryError = storage_err.into();
        assert!(matches!(err, QueryError::Storage(_)));
    }

    #[test]
    fn is_invalid_query_detects_invalid_variant() {
        assert!(QueryError::InvalidQuery("x".to_string()).is_invalid_query());
        assert!(!QueryError::Query("x".to_string()).is_invalid_query());
        assert!(!QueryError::FullText("x".to_string()).is_invalid_query());
    }

    #[test]
    fn debug_includes_variant_name() {
        let err = QueryError::InvalidQuery("x".to_string());
        let s = format!("{err:?}");
        assert!(s.contains("InvalidQuery"));
        assert!(s.contains("\"x\""));
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<QueryError>();
    }

    #[test]
    fn result_alias_compiles() {
        let ok: Result<i32> = Ok(42);
        assert!(ok.is_ok());
        let err: Result<i32> = Err(QueryError::Query("x".to_string()));
        assert!(err.is_err());
    }
}
