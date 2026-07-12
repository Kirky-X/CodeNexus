// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Unified top-level error type for CodeNexus.
//!
//! [`CodeNexusError`] wraps all subsystem errors and surfaces a uniform
//! [`exit_code`](CodeNexusError::exit_code) for `main.rs`.
//!
//! # Exit codes (v0.3.2 unified CLI)
//!
//! | Code | Meaning                | Variants                              |
//! |------|------------------------|---------------------------------------|
//! | 0    | success                | —                                     |
//! | 1    | internal/system error  | Internal, Io, Json, Daemon, Lsp, Discover, Cache, Embed |
//! | 2    | client error           | InvalidInput, ProjectNotFound, Query, Trace, Storage, Resolve, Phase |
//! | 4    | not found / corrupt    | NotFound, Index(corrupt), Kit(corrupt)|

use thiserror::Error;
use tracing::error;

#[cfg(feature = "daemon")]
use crate::daemon::DaemonError;
#[cfg(feature = "cache")]
use crate::cache::CacheError;
#[cfg(feature = "embed")]
use crate::embed::EmbedError;
#[cfg(feature = "lsp")]
use crate::lsp::LspError;
use crate::discover::DiscoverError;
use crate::index::IndexError;
use crate::index::pipeline_dag::PhaseError;
use crate::kit::KitError;
use crate::parse::ParseError;
use crate::query::QueryError;
use crate::resolve::ResolveError;
use crate::storage::StorageError;
use crate::trace::TraceError;

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;

/// A specialized [`Result`](std::result::Result) for CodeNexus operations.
pub type Result<T> = std::result::Result<T, CodeNexusError>;

/// Unified top-level error type for all CodeNexus operations.
///
/// Each variant maps to a specific process exit code via [`exit_code`](Self::exit_code).
#[derive(Debug, Error)]
pub enum CodeNexusError {
    #[error("{0}")]
    Index(#[from] IndexError),

    #[error("{0}")]
    Query(#[from] QueryError),

    #[error("{0}")]
    Trace(#[from] TraceError),

    #[error("{0}")]
    Storage(#[from] StorageError),

    #[error("{0}")]
    Parse(#[from] ParseError),

    #[error("{0}")]
    Discover(#[from] DiscoverError),

    #[error("{0}")]
    Resolve(#[from] ResolveError),

    #[error("{0}")]
    Phase(#[from] PhaseError),

    #[cfg(feature = "daemon")]
    #[error("{0}")]
    Daemon(#[from] DaemonError),

    #[cfg(feature = "cache")]
    #[error("{0}")]
    Cache(#[from] CacheError),

    #[cfg(feature = "embed")]
    #[error("{0}")]
    Embed(#[from] EmbedError),

    #[cfg(feature = "lsp")]
    #[error("{0}")]
    Lsp(#[from] LspError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("project not found: {0}")]
    ProjectNotFound(String),

    #[error("kit error: {0}")]
    Kit(#[from] KitError),
}

impl CodeNexusError {
    /// Creates a "Kit not initialized" error.
    #[must_use]
    pub fn kit_not_initialized() -> Self {
        CodeNexusError::Internal("Kit not initialized".to_string())
    }

    /// Returns the process exit code for this error.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        let code = match self {
            CodeNexusError::Index(e) => e.exit_code(),
            CodeNexusError::InvalidInput(_) => 2,
            CodeNexusError::ProjectNotFound(_) => 2,
            CodeNexusError::NotFound(_) => 4,
            CodeNexusError::Internal(_) => 1,
            CodeNexusError::Io(_) => 1,
            CodeNexusError::Json(_) => 1,
            CodeNexusError::Query(_) => 2,
            CodeNexusError::Trace(_) => 2,
            CodeNexusError::Storage(_) => 2,
            CodeNexusError::Parse(_) => 0,
            CodeNexusError::Discover(_) => 1,
            CodeNexusError::Resolve(_) => 2,
            CodeNexusError::Phase(_) => 2,
            #[cfg(feature = "daemon")]
            CodeNexusError::Daemon(_) => 1,
            #[cfg(feature = "cache")]
            CodeNexusError::Cache(_) => 1,
            #[cfg(feature = "embed")]
            CodeNexusError::Embed(_) => 1,
            #[cfg(feature = "lsp")]
            CodeNexusError::Lsp(_) => 1,
            CodeNexusError::Kit(e) => kit_exit_code(e),
        };
        error!(
            event = "error",
            error_type = ?self,
            exit_code = code,
            "CodeNexus error occurred"
        );
        code
    }
}

/// Resolves the exit code for a [`CodeNexusError::Kit`] by walking the error
/// source chain for corruption signals.
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

/// Converts an [`ApiError`] (from sdforge) into a [`CodeNexusError`].
#[cfg(any(feature = "cli", feature = "mcp"))]
impl From<ApiError> for CodeNexusError {
    fn from(e: ApiError) -> Self {
        use sdforge::prelude::ApiError;
        match e {
            ApiError::InvalidInput { message, .. } => CodeNexusError::InvalidInput(message),
            ApiError::Internal {
                message, error_id, ..
            } => CodeNexusError::Internal(format!("[error_id: {error_id}] {message}")),
            ApiError::NotFound { resource, .. } => CodeNexusError::NotFound(resource),
            other => CodeNexusError::Internal(format!("Unexpected error: {other}")),
        }
    }
}

// --- Service-layer error helpers ---

/// Returns the process boot timestamp (unix seconds) for error_id uniqueness
/// across restarts.
fn boot_epoch() -> u64 {
    use std::sync::OnceLock;
    use std::time::{SystemTime, UNIX_EPOCH};
    static BOOT: OnceLock<u64> = OnceLock::new();
    *BOOT.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    })
}

/// Returns a unique `error_id` string including the process boot timestamp
/// so it remains unique across restarts.
#[cfg(any(feature = "cli", feature = "mcp"))]
fn next_error_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static ERROR_COUNTER: AtomicU64 = AtomicU64::new(0);
    let epoch = boot_epoch();
    let seq = ERROR_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("err-{epoch:012x}-{seq:04x}")
}

/// Wraps an error as an `ApiError::Internal` with a unique `error_id` and
/// preserves the source chain via `ApiError::internal_with_source`.
///
/// The caller must ensure `E: Send + Sync` — required by sdforge's
/// `ApiError::internal_with_source`. For `KitError` (which is `Send` but
/// not `Sync` due to trait-kit 0.2.4's `Box<dyn StdError + Send + 'static>`),
/// use [`wrap_kit_error`] instead.
#[cfg(any(feature = "cli", feature = "mcp"))]
pub fn wrap_error<E: std::error::Error + Send + Sync + 'static>(
    message: impl Into<String>,
    source: E,
) -> ApiError {
    ApiError::internal_with_source(message, next_error_id(), source)
}

/// Wraps a [`KitError`] as an `ApiError::Internal` with a unique `error_id`.
///
/// Needed because trait-kit 0.2.4's `KitError` uses
/// `Box<dyn StdError + Send + 'static>` (no `Sync` bound), making it
/// `Send + !Sync`. `ApiError::internal_with_source` requires `Send + Sync`,
/// so we convert the `KitError` to a string and use `ApiError::internal_error`.
#[cfg(any(feature = "cli", feature = "mcp"))]
pub fn wrap_kit_error(message: impl Into<String>, source: KitError) -> ApiError {
    let message = message.into();
    let full_msg = format!("{message}: {source}");
    ApiError::internal_error(full_msg, next_error_id())
}

/// Returns an `ApiError::Internal` for "Kit not initialized".
#[cfg(any(feature = "cli", feature = "mcp"))]
pub fn kit_not_initialized() -> ApiError {
    ApiError::internal_error("Kit not initialized", "kit_not_initialized")
}

/// Converts a [`CodeNexusError`] into an [`ApiError`] at the service boundary.
///
/// - `InvalidInput` → `ApiError::InvalidInput`
/// - `NotFound` / `Trace(SymbolNotFound)` → `ApiError::NotFound`
/// - All other variants → `ApiError::Internal` with `tag` as error_id and
///   the error's string representation in the message.
///
/// # Why not `internal_with_source`
///
/// `CodeNexusError::Kit(#[from] KitError)` makes `CodeNexusError: !Sync` (trait-kit 0.2.4's
/// `KitError` uses `Box<dyn StdError + Send + 'static>` without `Sync`).
/// `ApiError::internal_with_source` requires `Send + Sync`, so we cannot pass
/// `CodeNexusError` as a source. We convert to a string-based `ApiError::internal_error`
/// instead, preserving the message but losing the source chain.
#[cfg(any(feature = "cli", feature = "mcp"))]
pub fn to_api_error(e: CodeNexusError, tag: &str) -> ApiError {
    match e {
        CodeNexusError::InvalidInput(msg) => ApiError::InvalidInput {
            message: msg,
            field: None,
            value: None,
        },
        CodeNexusError::NotFound(resource) => ApiError::NotFound {
            resource,
            resource_id: None,
        },
        CodeNexusError::Trace(TraceError::SymbolNotFound(s)) => ApiError::NotFound {
            resource: "symbol".to_string(),
            resource_id: Some(s),
        },
        other => {
            let message = format!("{other}");
            ApiError::internal_error(message, tag)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_input_displays_message() {
        let err = CodeNexusError::InvalidInput("bad trace type".to_string());
        let msg = err.to_string();
        assert!(msg.contains("invalid input"), "got: {msg}");
        assert!(msg.contains("bad trace type"), "got: {msg}");
    }

    #[test]
    fn exit_code_invalid_input_is_2() {
        let err = CodeNexusError::InvalidInput("x".to_string());
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_not_found_is_4() {
        let err = CodeNexusError::NotFound("symbol foo".to_string());
        assert_eq!(err.exit_code(), 4);
    }

    #[test]
    fn exit_code_internal_is_1() {
        let err = CodeNexusError::Internal("unexpected failure".to_string());
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn exit_code_index_database_corrupt_is_4() {
        let err: CodeNexusError = IndexError::DatabaseCorrupt("x".to_string()).into();
        assert_eq!(err.exit_code(), 4);
    }

    #[test]
    fn exit_code_kit_build_failed_with_storage_corrupt_is_4() {
        let kit_err = KitError::BuildFailed {
            context: "storage",
            source: Box::new(StorageError::Corrupt("invalid LadybugDB header".to_string())),
        };
        let err: CodeNexusError = kit_err.into();
        assert_eq!(err.exit_code(), 4);
    }

    #[test]
    fn exit_code_kit_missing_capability_is_1() {
        let kit_err = KitError::MissingCapability { key: "storage" };
        let err: CodeNexusError = kit_err.into();
        assert_eq!(err.exit_code(), 1);
    }

    #[cfg(any(feature = "cli", feature = "mcp"))]
    #[test]
    fn from_api_error_invalid_input_maps_to_cli_invalid_input() {
        let api_err = ApiError::invalid_input("bad cypher", Some("query".to_string()), None);
        let cli_err: CodeNexusError = api_err.into();
        assert!(matches!(cli_err, CodeNexusError::InvalidInput(_)));
        assert_eq!(cli_err.exit_code(), 2);
    }

    #[cfg(any(feature = "cli", feature = "mcp"))]
    #[test]
    fn from_api_error_internal_maps_to_cli_internal_with_error_id() {
        let api_err = ApiError::internal_error("boom", "err-deadbeef");
        let cli_err: CodeNexusError = api_err.into();
        assert_eq!(cli_err.exit_code(), 1);
        match cli_err {
            CodeNexusError::Internal(msg) => {
                assert!(msg.contains("error_id"), "should contain error_id, got: {msg}");
                assert!(msg.contains("err-deadbeef"), "should contain error_id value");
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[cfg(any(feature = "cli", feature = "mcp"))]
    #[test]
    fn from_api_error_not_found_maps_to_cli_not_found() {
        let api_err = ApiError::not_found("symbol", Some("foo".to_string()));
        let cli_err: CodeNexusError = api_err.into();
        assert!(matches!(cli_err, CodeNexusError::NotFound(_)));
        assert_eq!(cli_err.exit_code(), 4);
    }

    #[cfg(any(feature = "cli", feature = "mcp"))]
    #[test]
    fn to_api_error_invalid_input() {
        let err = CodeNexusError::InvalidInput("bad".to_string());
        let api_err = to_api_error(err, "test");
        assert!(matches!(api_err, ApiError::InvalidInput { .. }));
    }

    #[cfg(any(feature = "cli", feature = "mcp"))]
    #[test]
    fn to_api_error_trace_symbol_not_found() {
        let err = CodeNexusError::Trace(TraceError::SymbolNotFound("foo".to_string()));
        let api_err = to_api_error(err, "test");
        assert!(matches!(api_err, ApiError::NotFound { .. }));
    }

    #[cfg(any(feature = "cli", feature = "mcp"))]
    #[test]
    fn to_api_error_not_found_variant() {
        let err = CodeNexusError::NotFound("project demo".to_string());
        let api_err = to_api_error(err, "test");
        match api_err {
            ApiError::NotFound { resource, resource_id } => {
                assert_eq!(resource, "project demo");
                assert!(resource_id.is_none());
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[cfg(any(feature = "cli", feature = "mcp"))]
    #[test]
    fn to_api_error_other_maps_to_internal_with_tag() {
        let err = CodeNexusError::Internal("boom".to_string());
        let api_err = to_api_error(err, "my_service");
        match api_err {
            ApiError::Internal { error_id, .. } => {
                assert_eq!(error_id, "my_service");
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[cfg(any(feature = "cli", feature = "mcp"))]
    #[test]
    fn to_api_error_other_preserves_message() {
        let err = CodeNexusError::Internal("boom".to_string());
        let api_err = to_api_error(err, "svc");
        match api_err {
            ApiError::Internal { message, error_id, .. } => {
                assert_eq!(error_id, "svc");
                assert!(
                    message.contains("boom"),
                    "message should be preserved, got: {message}"
                );
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn error_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<CodeNexusError>();
    }

    #[test]
    fn kit_not_initialized_constructor_returns_internal_with_exit_code_1() {
        let err = CodeNexusError::kit_not_initialized();
        assert!(matches!(err, CodeNexusError::Internal(_)));
        assert_eq!(err.exit_code(), 1);
        assert!(err.to_string().contains("Kit not initialized"));
    }

    // --- exit_code for Index variants (delegates to IndexError::exit_code) ---

    #[test]
    fn exit_code_index_path_not_found_is_1() {
        let err: CodeNexusError = IndexError::PathNotFound("/x".to_string()).into();
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn exit_code_index_database_locked_is_2() {
        let err: CodeNexusError = IndexError::DatabaseLocked.into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_index_storage_is_2() {
        let err: CodeNexusError =
            IndexError::Storage(crate::storage::StorageError::Query("x".to_string())).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_index_discover_is_1() {
        let err: CodeNexusError =
            IndexError::Discover(crate::discover::DiscoverError::from(std::io::Error::other(
                "x",
            )))
            .into();
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn exit_code_index_parse_is_0() {
        let err: CodeNexusError = IndexError::Parse("x".to_string()).into();
        assert_eq!(err.exit_code(), 0);
    }

    #[test]
    fn exit_code_index_io_is_1() {
        let err: CodeNexusError = IndexError::Io(std::io::Error::other("x")).into();
        assert_eq!(err.exit_code(), 1);
    }

    // --- exit_code for Query (2) ---

    #[test]
    fn exit_code_query_is_2() {
        let err: CodeNexusError = QueryError::Query("syntax error".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_query_storage_is_2() {
        let err: CodeNexusError =
            QueryError::Storage(crate::storage::StorageError::Query("x".to_string())).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_query_invalid_query_is_2() {
        let err: CodeNexusError = QueryError::InvalidQuery("empty".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_query_fulltext_is_2() {
        let err: CodeNexusError = QueryError::FullText("fts unavailable".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    // --- exit_code for Trace (2) ---

    #[test]
    fn exit_code_trace_symbol_not_found_is_2() {
        let err: CodeNexusError = TraceError::SymbolNotFound("foo".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_trace_ambiguous_is_2() {
        let err: CodeNexusError = TraceError::AmbiguousSymbol {
            symbol: "bar".to_string(),
            candidates: vec!["a.bar".to_string()],
        }
        .into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_trace_invalid_depth_is_2() {
        let err: CodeNexusError = TraceError::InvalidDepth(0).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_trace_start_node_missing_is_2() {
        let err: CodeNexusError = TraceError::StartNodeMissing("node-1".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_trace_storage_is_2() {
        let err: CodeNexusError =
            TraceError::Storage(crate::storage::StorageError::Query("x".to_string())).into();
        assert_eq!(err.exit_code(), 2);
    }

    // --- exit_code for Storage (2) ---

    #[test]
    fn exit_code_storage_query_is_2() {
        let err: CodeNexusError = crate::storage::StorageError::Query("bad cypher".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_storage_not_found_is_2() {
        let err: CodeNexusError = crate::storage::StorageError::NotFound("x".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_storage_corrupt_is_2() {
        // StorageError::Corrupt wraps as CodeNexusError::Storage (exit 2),
        // NOT DatabaseCorrupt (exit 4) — that mapping is IndexError-only.
        let err: CodeNexusError = crate::storage::StorageError::Corrupt("x".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_storage_schema_is_2() {
        let err: CodeNexusError = crate::storage::StorageError::Schema("x".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_storage_io_is_2() {
        let err: CodeNexusError =
            crate::storage::StorageError::Io(std::io::Error::other("x")).into();
        assert_eq!(err.exit_code(), 2);
    }

    // --- exit_code for Parse (0) ---

    #[test]
    fn exit_code_parse_unsupported_language_is_0() {
        let err: CodeNexusError = ParseError::UnsupportedLanguage("java".to_string()).into();
        assert_eq!(err.exit_code(), 0);
    }

    #[test]
    fn exit_code_parse_parse_failed_is_0() {
        let err: CodeNexusError = ParseError::ParseFailed {
            file_path: "/x.rs".to_string(),
        }
        .into();
        assert_eq!(err.exit_code(), 0);
    }

    // --- exit_code for Discover (1) ---

    #[test]
    fn exit_code_discover_io_is_1() {
        let err: CodeNexusError =
            crate::discover::DiscoverError::from(std::io::Error::other("x")).into();
        assert_eq!(err.exit_code(), 1);
    }

    // --- exit_code for Resolve (2) ---

    #[test]
    fn exit_code_resolve_symbol_not_found_is_2() {
        let err: CodeNexusError = ResolveError::SymbolNotFound("foo".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_resolve_ambiguous_is_2() {
        let err: CodeNexusError = ResolveError::AmbiguousSymbol("bar".to_string(), 3).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_resolve_invalid_fqn_is_2() {
        let err: CodeNexusError = ResolveError::InvalidFqn("bad..qn".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_resolve_scope_is_2() {
        let err: CodeNexusError = ResolveError::Scope("empty chain".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    // --- exit_code for Phase (2) ---

    #[test]
    fn exit_code_phase_cycle_is_2() {
        let err: CodeNexusError = PhaseError::Cycle("a, b".to_string()).into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_phase_missing_dependency_is_2() {
        let err: CodeNexusError = PhaseError::MissingDependency {
            phase: "resolve",
            dep: "scan",
        }
        .into();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_phase_duplicate_is_2() {
        let err: CodeNexusError = PhaseError::DuplicatePhase("scan").into();
        assert_eq!(err.exit_code(), 2);
    }

    // --- exit_code for Io (1) ---

    #[test]
    fn exit_code_io_is_1() {
        let err: CodeNexusError = std::io::Error::other("disk full").into();
        assert_eq!(err.exit_code(), 1);
    }

    // --- exit_code for Json (1) ---

    #[test]
    fn exit_code_json_is_1() {
        let json_err = serde_json::from_str::<serde_json::Value>("bad").unwrap_err();
        let err: CodeNexusError = json_err.into();
        assert_eq!(err.exit_code(), 1);
    }

    // --- exit_code for ProjectNotFound (2) ---

    #[test]
    fn exit_code_project_not_found_is_2() {
        let err = CodeNexusError::ProjectNotFound("demo".to_string());
        assert_eq!(err.exit_code(), 2);
    }

    // --- exit_code for Daemon (1) ---

    #[cfg(feature = "daemon")]
    #[test]
    fn exit_code_daemon_notify_is_1() {
        let err: CodeNexusError = DaemonError::Notify(
            notify_debouncer_full::notify::Error::path_not_found(),
        )
        .into();
        assert_eq!(err.exit_code(), 1);
    }

    #[cfg(feature = "daemon")]
    #[test]
    fn exit_code_daemon_io_is_1() {
        let err: CodeNexusError = DaemonError::Io(std::io::Error::other("x")).into();
        assert_eq!(err.exit_code(), 1);
    }

    // --- exit_code for Cache (1) ---

    #[cfg(feature = "cache")]
    #[test]
    fn exit_code_cache_config_is_1() {
        let err: CodeNexusError = CacheError::Config("missing".to_string()).into();
        assert_eq!(err.exit_code(), 1);
    }

    #[cfg(feature = "cache")]
    #[test]
    fn exit_code_cache_build_failed_is_1() {
        let err: CodeNexusError = CacheError::BuildFailed("capacity 0".to_string()).into();
        assert_eq!(err.exit_code(), 1);
    }

    // --- exit_code for Embed (1) ---

    #[cfg(feature = "embed")]
    #[test]
    fn exit_code_embed_missing_api_key_is_1() {
        let err: CodeNexusError = EmbedError::MissingApiKey.into();
        assert_eq!(err.exit_code(), 1);
    }

    #[cfg(feature = "embed")]
    #[test]
    fn exit_code_embed_unavailable_is_1() {
        let err: CodeNexusError = EmbedError::Unavailable("connection refused".to_string()).into();
        assert_eq!(err.exit_code(), 1);
    }

    // --- exit_code for Lsp (1) ---

    #[cfg(feature = "lsp")]
    #[test]
    fn exit_code_lsp_server_start_is_1() {
        let err: CodeNexusError =
            crate::lsp::LspError::ServerStart("binary not found".to_string()).into();
        assert_eq!(err.exit_code(), 1);
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn exit_code_lsp_communication_is_1() {
        let err: CodeNexusError =
            crate::lsp::LspError::Communication("channel closed".to_string()).into();
        assert_eq!(err.exit_code(), 1);
    }

    #[cfg(feature = "lsp")]
    #[test]
    fn exit_code_lsp_timeout_is_1() {
        let err: CodeNexusError = crate::lsp::LspError::Timeout(5000).into();
        assert_eq!(err.exit_code(), 1);
    }

    // --- kit_exit_code: IndexError::DatabaseCorrupt in source chain ---

    #[test]
    fn exit_code_kit_build_failed_with_index_database_corrupt_is_4() {
        let kit_err = KitError::BuildFailed {
            context: "index",
            source: Box::new(IndexError::DatabaseCorrupt("schema mismatch".to_string())),
        };
        let err: CodeNexusError = kit_err.into();
        assert_eq!(err.exit_code(), 4);
    }

    // --- kit_exit_code: non-corrupt source falls through to 1 ---

    #[test]
    fn exit_code_kit_build_failed_with_non_corrupt_is_1() {
        let kit_err = KitError::BuildFailed {
            context: "query",
            source: Box::new(QueryError::Query("bad cypher".to_string())),
        };
        let err: CodeNexusError = kit_err.into();
        assert_eq!(err.exit_code(), 1);
    }

    // --- wrap_error / wrap_kit_error (cli/mcp feature) ---

    #[cfg(any(feature = "cli", feature = "mcp"))]
    #[test]
    fn wrap_error_creates_internal_with_source() {
        let api_err = wrap_error("boom", std::io::Error::other("inner"));
        match api_err {
            ApiError::Internal { message, .. } => {
                assert!(message.contains("boom"), "got: {message}");
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[cfg(any(feature = "cli", feature = "mcp"))]
    #[test]
    fn wrap_kit_error_creates_internal_with_message() {
        let kit_err = KitError::MissingCapability { key: "storage" };
        let api_err = wrap_kit_error("build failed", kit_err);
        match api_err {
            ApiError::Internal { message, .. } => {
                assert!(message.contains("build failed"), "got: {message}");
                assert!(message.contains("storage"), "got: {message}");
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[cfg(any(feature = "cli", feature = "mcp"))]
    #[test]
    fn kit_not_initialized_api_error_has_fixed_id() {
        let api_err = kit_not_initialized();
        match api_err {
            ApiError::Internal { message, error_id, .. } => {
                assert_eq!(error_id, "kit_not_initialized");
                assert!(message.contains("Kit not initialized"));
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[cfg(any(feature = "cli", feature = "mcp"))]
    #[test]
    fn from_api_error_other_variant_maps_to_internal() {
        // ApiError variants other than InvalidInput/Internal/NotFound map to
        // CodeNexusError::Internal("Unexpected error: ...").
        let api_err = ApiError::AuthenticationFailed {
            reason: "token expired".to_string(),
        };
        let cli_err: CodeNexusError = api_err.into();
        match cli_err {
            CodeNexusError::Internal(msg) => {
                assert!(msg.contains("Unexpected error"), "got: {msg}");
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }
}
