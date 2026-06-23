//! Error types for the trace subsystem.
//!
//! Uses [`thiserror`] for ergonomic, type-safe error propagation during
//! call-graph / data-flow tracing and impact analysis.

use thiserror::Error;

/// A specialized [`Result`](std::result::Result) for trace operations.
pub type Result<T> = std::result::Result<T, TraceError>;

/// Errors that can occur during tracing.
#[derive(Debug, Error)]
pub enum TraceError {
    /// The requested symbol could not be located in the graph.
    #[error("symbol not found: {0}")]
    SymbolNotFound(String),

    /// The requested symbol matched multiple nodes, making the choice ambiguous.
    #[error("ambiguous symbol '{0}': {1} candidates found")]
    AmbiguousSymbol(String, usize),

    /// The requested depth was zero or otherwise invalid.
    #[error("invalid depth: {0} (must be >= 1)")]
    InvalidDepth(usize),

    /// The start node id was not present in the graph.
    #[error("start node not in graph: {0}")]
    StartNodeMissing(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_not_found_display() {
        let err = TraceError::SymbolNotFound("foo".to_string());
        assert_eq!(err.to_string(), "symbol not found: foo");
    }

    #[test]
    fn ambiguous_symbol_display() {
        let err = TraceError::AmbiguousSymbol("bar".to_string(), 3);
        let msg = err.to_string();
        assert!(msg.contains("bar"), "message should contain name: {msg}");
        assert!(msg.contains("3"), "message should contain count: {msg}");
    }

    #[test]
    fn invalid_depth_display() {
        let err = TraceError::InvalidDepth(0);
        assert_eq!(err.to_string(), "invalid depth: 0 (must be >= 1)");
    }

    #[test]
    fn start_node_missing_display() {
        let err = TraceError::StartNodeMissing("node-123".to_string());
        assert_eq!(err.to_string(), "start node not in graph: node-123");
    }

    #[test]
    fn result_alias_compiles() {
        let ok: Result<i32> = Ok(42);
        assert!(ok.is_ok());
        let err: Result<i32> = Err(TraceError::SymbolNotFound("x".to_string()));
        assert!(err.is_err());
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TraceError>();
    }

    #[test]
    fn debug_includes_variant_name() {
        let err = TraceError::SymbolNotFound("x".to_string());
        let s = format!("{err:?}");
        assert!(s.contains("SymbolNotFound"));
        assert!(s.contains("\"x\""));
    }
}
