// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Error types for the resolve subsystem.
//!
//! Uses [`thiserror`] for ergonomic, type-safe error propagation during
//! symbol resolution, FQN generation, and scope/symbol-table operations.

use thiserror::Error;

/// A specialized [`Result`](std::result::Result) for resolve operations.
pub type Result<T> = std::result::Result<T, ResolveError>;

/// Errors that can occur during symbol resolution.
#[derive(Debug, Error)]
pub enum ResolveError {
    /// A symbol could not be found in the symbol table or scope chain.
    #[error("symbol not found: {0}")]
    SymbolNotFound(String),

    /// A symbol name resolved to multiple candidates, making the choice ambiguous.
    #[error("ambiguous symbol '{0}': {1} candidates found")]
    AmbiguousSymbol(String, usize),

    /// An FQN was malformed or could not be generated.
    #[error("invalid FQN: {0}")]
    InvalidFqn(String),

    /// A scope-chain operation failed (e.g. popping an empty chain).
    #[error("scope error: {0}")]
    Scope(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_not_found_display() {
        let err = ResolveError::SymbolNotFound("foo".to_string());
        assert_eq!(err.to_string(), "symbol not found: foo");
    }

    #[test]
    fn ambiguous_symbol_display() {
        let err = ResolveError::AmbiguousSymbol("bar".to_string(), 3);
        let msg = err.to_string();
        assert!(msg.contains("bar"), "message should contain name: {msg}");
        assert!(msg.contains("3"), "message should contain count: {msg}");
    }

    #[test]
    fn invalid_fqn_display() {
        let err = ResolveError::InvalidFqn("bad..qn".to_string());
        assert_eq!(err.to_string(), "invalid FQN: bad..qn");
    }

    #[test]
    fn scope_error_display() {
        let err = ResolveError::Scope("empty chain".to_string());
        assert_eq!(err.to_string(), "scope error: empty chain");
    }

    #[test]
    fn result_alias_compiles() {
        let ok: Result<i32> = Ok(42);
        assert!(ok.is_ok());
        let err: Result<i32> = Err(ResolveError::SymbolNotFound("x".to_string()));
        assert!(err.is_err());
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ResolveError>();
    }

    #[test]
    fn debug_includes_variant_name() {
        let err = ResolveError::SymbolNotFound("x".to_string());
        let s = format!("{err:?}");
        assert!(s.contains("SymbolNotFound"));
        assert!(s.contains("\"x\""));
    }
}
