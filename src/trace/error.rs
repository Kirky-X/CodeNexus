// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Error types for the trace subsystem.
//!
//! Defines [`TraceError`] and a specialized [`Result`] alias for trace
//! operations (call-graph / data-flow tracing and impact analysis).
//!
//! `TraceError` implements a hand-written [`fmt::Display`] (rather than
//! `thiserror`'s derive) so that [`TraceError::AmbiguousSymbol`] can render a
//! numbered candidate FQN list for CLI disambiguation (P1-1, GitNexus UX).

use std::fmt;

use crate::storage::StorageError;

/// A specialized [`Result`](std::result::Result) for trace operations.
pub type Result<T> = std::result::Result<T, TraceError>;

/// Errors that can occur during tracing.
#[derive(Debug)]
pub enum TraceError {
    /// The requested symbol could not be located in the graph.
    SymbolNotFound(String),

    /// The requested symbol matched multiple nodes, making the choice
    /// ambiguous.
    ///
    /// `candidates` carries the fully-qualified names of every matching node
    /// so the CLI can surface them to the user for disambiguation (P1-1,
    /// GitNexus UX). Order is preserved from the underlying graph iteration.
    AmbiguousSymbol {
        /// The user-supplied symbol string that matched more than one node.
        symbol: String,
        /// Fully-qualified names of the matching candidate nodes.
        candidates: Vec<String>,
    },

    /// The requested depth was zero or otherwise invalid.
    InvalidDepth(usize),

    /// The start node id was not present in the graph.
    StartNodeMissing(String),

    /// A storage-layer error while loading the subgraph for tracing.
    ///
    /// Added in T6/Phase-2 Task 2.10 so that [`TraceCapability`](super::module::TraceCapability)
    /// can propagate database failures from [`load_graph_for_symbol`](super::graph_loader::load_graph_for_symbol)
    /// without depending on the CLI error type.
    Storage(StorageError),
}

impl fmt::Display for TraceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SymbolNotFound(s) => write!(f, "symbol not found: {s}"),
            Self::AmbiguousSymbol { symbol, candidates } => {
                write!(
                    f,
                    "ambiguous symbol '{symbol}': {} candidates found:",
                    candidates.len()
                )?;
                for (i, qn) in candidates.iter().enumerate() {
                    write!(f, "\n  [{}] {qn}", i + 1)?;
                }
                Ok(())
            }
            Self::InvalidDepth(d) => write!(f, "invalid depth: {d} (must be >= 1)"),
            Self::StartNodeMissing(id) => write!(f, "start node not in graph: {id}"),
            Self::Storage(e) => write!(f, "storage error: {e}"),
        }
    }
}

impl std::error::Error for TraceError {}

impl From<StorageError> for TraceError {
    /// Ergonomic conversion so `?` works in code that loads graphs via
    /// [`load_graph_for_symbol`](super::graph_loader::load_graph_for_symbol).
    fn from(e: StorageError) -> Self {
        Self::Storage(e)
    }
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
    fn ambiguous_symbol_display_lists_candidates() {
        let err = TraceError::AmbiguousSymbol {
            symbol: "bar".to_string(),
            candidates: vec!["proj.a.rs.bar".to_string(), "proj.b.rs.bar".to_string()],
        };
        let msg = err.to_string();
        assert!(msg.contains("bar"), "message should contain name: {msg}");
        assert!(msg.contains("2"), "message should contain count: {msg}");
        assert!(
            msg.contains("proj.a.rs.bar"),
            "message should list first candidate FQN: {msg}"
        );
        assert!(
            msg.contains("proj.b.rs.bar"),
            "message should list second candidate FQN: {msg}"
        );
        assert!(
            msg.contains("[1]"),
            "message should number candidates starting at 1: {msg}"
        );
    }

    #[test]
    fn ambiguous_symbol_display_empty_candidates() {
        // Defensive: even with zero candidates (shouldn't happen in practice),
        // Display must not panic.
        let err = TraceError::AmbiguousSymbol {
            symbol: "x".to_string(),
            candidates: Vec::new(),
        };
        let msg = err.to_string();
        assert!(msg.contains("0 candidates found"), "msg: {msg}");
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
    fn storage_error_display_wraps_inner() {
        let inner = StorageError::Query("boom".to_string());
        let err = TraceError::Storage(inner);
        let msg = err.to_string();
        assert!(
            msg.contains("storage error"),
            "should be prefixed with 'storage error': {msg}"
        );
        assert!(msg.contains("boom"), "should include inner message: {msg}");
    }

    #[test]
    fn from_storage_error_produces_storage_variant() {
        let inner = StorageError::Query("x".to_string());
        let err: TraceError = inner.into();
        assert!(matches!(err, TraceError::Storage(_)), "got {err:?}");
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

    #[test]
    fn debug_ambiguous_symbol_includes_candidates() {
        let err = TraceError::AmbiguousSymbol {
            symbol: "foo".to_string(),
            candidates: vec!["proj.a.foo".to_string()],
        };
        let s = format!("{err:?}");
        assert!(s.contains("AmbiguousSymbol"), "debug: {s}");
        assert!(s.contains("proj.a.foo"), "debug should include FQN: {s}");
    }

    #[test]
    fn std_error_trait_object_works() {
        // Verify std::error::Error is implemented (can be boxed as trait object).
        let err: Box<dyn std::error::Error> = Box::new(TraceError::SymbolNotFound("x".to_string()));
        assert_eq!(err.to_string(), "symbol not found: x");
    }
}
