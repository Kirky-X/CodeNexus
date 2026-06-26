// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Error types for the parse subsystem.
//!
//! Defines [`ParseError`] and a specialized [`Result`] alias used throughout
//! the parsing and extraction pipeline.

use thiserror::Error;

/// A specialized [`Result`](std::result::Result) for parse operations.
pub type Result<T> = std::result::Result<T, ParseError>;

/// Errors that can occur during parsing or symbol extraction.
#[derive(Debug, Error)]
pub enum ParseError {
    /// Failed to assign a tree-sitter language to a parser.
    #[error("failed to set language {language} on parser: {source}")]
    LanguageSet {
        /// The CodeNexus language name.
        language: String,
        /// The underlying tree-sitter language error.
        #[source]
        source: tree_sitter::LanguageError,
    },

    /// The parser returned no syntax tree (e.g. no language was set).
    #[error("failed to parse file {file_path}: parser returned no tree")]
    ParseFailed {
        /// The file path that failed to parse.
        file_path: String,
    },

    /// Failed to read a source file from disk.
    #[error("failed to read file {file_path}: {source}")]
    Io {
        /// The file path that could not be read.
        file_path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The language is not supported by the parser subsystem.
    #[error("unsupported language: {0}")]
    UnsupportedLanguage(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_set_display_contains_language_and_source() {
        let err = ParseError::LanguageSet {
            language: "rust".to_string(),
            source: tree_sitter::LanguageError::Version(99),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("rust"),
            "message should contain language: {msg}"
        );
        assert!(
            msg.contains("parser"),
            "message should mention parser: {msg}"
        );
    }

    #[test]
    fn parse_failed_display_contains_file_path() {
        let err = ParseError::ParseFailed {
            file_path: "/src/main.rs".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("/src/main.rs"),
            "message should contain file path: {msg}"
        );
    }

    #[test]
    fn io_display_contains_file_path_and_source() {
        let err = ParseError::Io {
            file_path: "/missing.rs".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("/missing.rs"),
            "message should contain file path: {msg}"
        );
        assert!(
            msg.contains("file not found"),
            "message should contain source error: {msg}"
        );
    }

    #[test]
    fn unsupported_language_display_contains_input() {
        let err = ParseError::UnsupportedLanguage("java".to_string());
        let msg = err.to_string();
        assert!(
            msg.contains("java"),
            "message should contain language name: {msg}"
        );
    }

    #[test]
    fn io_error_preserves_not_found_kind() {
        let err = ParseError::Io {
            file_path: "/nope.rs".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
        };
        match err {
            ParseError::Io { source, .. } => {
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn language_set_preserves_version() {
        let err = ParseError::LanguageSet {
            language: "c".to_string(),
            source: tree_sitter::LanguageError::Version(42),
        };
        match err {
            ParseError::LanguageSet { source, .. } => match source {
                tree_sitter::LanguageError::Version(v) => assert_eq!(v, 42),
            },
            other => panic!("expected LanguageSet, got {other:?}"),
        }
    }

    #[test]
    fn result_alias_compiles() {
        let ok: Result<i32> = Ok(42);
        assert!(ok.is_ok());

        let err: Result<i32> = Err(ParseError::UnsupportedLanguage("x".to_string()));
        assert!(err.is_err());
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ParseError>();
    }
}
