// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CLI error types and error conversion helpers for service handlers.
//!
//! [`CliError`] wraps subsystem errors and surfaces a uniform
//! [`exit_code`](CliError::exit_code) for `main.rs`.
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

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;

/// A specialized [`Result`](std::result::Result) for CLI operations.
pub type Result<T> = std::result::Result<T, CliError>;

/// Errors that can occur during CLI command execution.
///
/// Each variant maps to a specific process exit code via [`exit_code`](Self::exit_code).
#[derive(Debug, Error)]
pub enum CliError {
    #[error("{0}")]
    Index(#[from] IndexError),

    #[error("{0}")]
    Query(#[from] QueryError),

    #[error("{0}")]
    Trace(#[from] TraceError),

    #[error("{0}")]
    Storage(#[from] StorageError),

    #[cfg(feature = "daemon")]
    #[error("{0}")]
    Daemon(#[from] DaemonError),

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

impl CliError {
    /// Creates a "Kit not initialized" error.
    ///
    /// Single source for this message across all core functions.
    #[must_use]
    pub fn kit_not_initialized() -> Self {
        CliError::Internal("Kit not initialized".to_string())
    }

    /// Returns the process exit code for this error.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        let code = match self {
            CliError::Index(e) => e.exit_code(),
            CliError::InvalidInput(_) => 2,
            CliError::ProjectNotFound(_) => 2,
            CliError::NotFound(_) => 4,
            CliError::Internal(_) => 1,
            CliError::Io(_) => 1,
            CliError::Json(_) => 1,
            CliError::Query(_) => 2,
            CliError::Trace(_) => 2,
            CliError::Storage(_) => 2,
            #[cfg(feature = "daemon")]
            CliError::Daemon(_) => 1,
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

/// Converts an [`ApiError`] (from sdforge) into a [`CliError`].
///
/// Mapping:
/// - `InvalidInput` → `CliError::InvalidInput` (exit 2)
/// - `Internal` → `CliError::Internal` (exit 1), error_id embedded in message
/// - `NotFound` → `CliError::NotFound` (exit 4)
/// - All other variants → `CliError::Internal` (exit 1)
#[cfg(any(feature = "cli", feature = "mcp"))]
impl From<ApiError> for CliError {
    fn from(e: ApiError) -> Self {
        use sdforge::prelude::ApiError;
        match e {
            ApiError::InvalidInput { message, .. } => CliError::InvalidInput(message),
            ApiError::Internal {
                message, error_id, ..
            } => CliError::Internal(format!("[error_id: {error_id}] {message}")),
            ApiError::NotFound { resource, .. } => CliError::NotFound(resource),
            other => CliError::Internal(format!("Unexpected error: {other}")),
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

/// Wraps an error as an `ApiError::Internal` with a unique `error_id`.
///
/// The `error_id` includes the process boot timestamp to remain unique
/// across restarts.
#[cfg(any(feature = "cli", feature = "mcp"))]
pub fn wrap_error<E: std::error::Error + Send + Sync + 'static>(
    message: impl Into<String>,
    source: E,
) -> ApiError {
    use std::sync::atomic::{AtomicU64, Ordering};
    static ERROR_COUNTER: AtomicU64 = AtomicU64::new(0);
    let epoch = boot_epoch();
    let seq = ERROR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let error_id = format!("err-{epoch:012x}-{seq:04x}");
    ApiError::internal_with_source(message, error_id, source)
}

/// Returns an `ApiError::Internal` for "Kit not initialized".
#[cfg(any(feature = "cli", feature = "mcp"))]
pub fn kit_not_initialized() -> ApiError {
    ApiError::internal_error("Kit not initialized", "kit_not_initialized")
}

/// Converts a [`CliError`] into an [`ApiError`] at the service boundary.
///
/// - `InvalidInput` → `ApiError::InvalidInput`
/// - `NotFound` / `Trace(SymbolNotFound)` → `ApiError::NotFound`
/// - All other variants → `ApiError::Internal` with `tag` as error_id,
///   preserving the original `CliError` as the error source
#[cfg(any(feature = "cli", feature = "mcp"))]
pub fn to_api_error(e: CliError, tag: &str) -> ApiError {
    match e {
        CliError::InvalidInput(msg) => ApiError::InvalidInput {
            message: msg,
            field: None,
            value: None,
        },
        CliError::NotFound(resource) => ApiError::NotFound {
            resource,
            resource_id: None,
        },
        CliError::Trace(TraceError::SymbolNotFound(s)) => ApiError::NotFound {
            resource: "symbol".to_string(),
            resource_id: Some(s),
        },
        other => {
            let message = format!("{other}");
            ApiError::internal_with_source(message, tag.to_string(), other)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_input_displays_message() {
        let err = CliError::InvalidInput("bad trace type".to_string());
        let msg = err.to_string();
        assert!(msg.contains("invalid input"), "got: {msg}");
        assert!(msg.contains("bad trace type"), "got: {msg}");
    }

    #[test]
    fn exit_code_invalid_input_is_2() {
        let err = CliError::InvalidInput("x".to_string());
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn exit_code_not_found_is_4() {
        let err = CliError::NotFound("symbol foo".to_string());
        assert_eq!(err.exit_code(), 4);
    }

    #[test]
    fn exit_code_internal_is_1() {
        let err = CliError::Internal("unexpected failure".to_string());
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn exit_code_index_database_corrupt_is_4() {
        let err: CliError = IndexError::DatabaseCorrupt("x".to_string()).into();
        assert_eq!(err.exit_code(), 4);
    }

    #[test]
    fn exit_code_kit_build_failed_with_storage_corrupt_is_4() {
        let kit_err = KitError::BuildFailed {
            module: "storage",
            source: Box::new(StorageError::Corrupt("invalid LadybugDB header".to_string())),
        };
        let err: CliError = kit_err.into();
        assert_eq!(err.exit_code(), 4);
    }

    #[test]
    fn exit_code_kit_missing_capability_is_1() {
        let kit_err = KitError::MissingCapability { key: "storage" };
        let err: CliError = kit_err.into();
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn from_api_error_invalid_input_maps_to_cli_invalid_input() {
        let api_err = ApiError::invalid_input("bad cypher", Some("query".to_string()), None);
        let cli_err: CliError = api_err.into();
        assert!(matches!(cli_err, CliError::InvalidInput(_)));
        assert_eq!(cli_err.exit_code(), 2);
    }

    #[test]
    fn from_api_error_internal_maps_to_cli_internal_with_error_id() {
        let api_err = ApiError::internal_error("boom", "err-deadbeef");
        let cli_err: CliError = api_err.into();
        assert_eq!(cli_err.exit_code(), 1);
        match cli_err {
            CliError::Internal(msg) => {
                assert!(msg.contains("error_id"), "should contain error_id, got: {msg}");
                assert!(msg.contains("err-deadbeef"), "should contain error_id value");
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn from_api_error_not_found_maps_to_cli_not_found() {
        let api_err = ApiError::not_found("symbol", Some("foo".to_string()));
        let cli_err: CliError = api_err.into();
        assert!(matches!(cli_err, CliError::NotFound(_)));
        assert_eq!(cli_err.exit_code(), 4);
    }

    #[test]
    fn to_api_error_invalid_input() {
        let err = CliError::InvalidInput("bad".to_string());
        let api_err = to_api_error(err, "test");
        assert!(matches!(api_err, ApiError::InvalidInput { .. }));
    }

    #[test]
    fn to_api_error_trace_symbol_not_found() {
        let err = CliError::Trace(TraceError::SymbolNotFound("foo".to_string()));
        let api_err = to_api_error(err, "test");
        assert!(matches!(api_err, ApiError::NotFound { .. }));
    }

    #[test]
    fn to_api_error_not_found_variant() {
        let err = CliError::NotFound("project demo".to_string());
        let api_err = to_api_error(err, "test");
        match api_err {
            ApiError::NotFound { resource, resource_id } => {
                assert_eq!(resource, "project demo");
                assert!(resource_id.is_none());
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn to_api_error_other_maps_to_internal_with_tag() {
        let err = CliError::Internal("boom".to_string());
        let api_err = to_api_error(err, "my_service");
        match api_err {
            ApiError::Internal { error_id, .. } => {
                assert_eq!(error_id, "my_service");
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn to_api_error_other_preserves_source() {
        let err = CliError::Internal("boom".to_string());
        let api_err = to_api_error(err, "svc");
        let source = std::error::Error::source(&api_err);
        assert!(
            source.is_some(),
            "source chain should be preserved for debugging"
        );
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CliError>();
    }

    #[test]
    fn kit_not_initialized_constructor_returns_internal_with_exit_code_1() {
        let err = CliError::kit_not_initialized();
        assert!(matches!(err, CliError::Internal(_)));
        assert_eq!(err.exit_code(), 1);
        assert!(err.to_string().contains("Kit not initialized"));
    }
}
