// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Error conversion helpers for service handlers.

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;

/// Wraps an error as an `ApiError::Internal` with a unique `error_id`.
#[cfg(any(feature = "cli", feature = "mcp"))]
pub fn wrap_error<E: std::error::Error + Send + Sync + 'static>(
    message: impl Into<String>,
    source: E,
) -> ApiError {
    use std::sync::atomic::{AtomicU64, Ordering};
    static ERROR_COUNTER: AtomicU64 = AtomicU64::new(0);
    let error_id = format!("err-{:016x}", ERROR_COUNTER.fetch_add(1, Ordering::Relaxed));
    ApiError::internal_with_source(message, error_id, source)
}

/// Returns an `ApiError::Internal` for "Kit not initialized".
#[cfg(any(feature = "cli", feature = "mcp"))]
pub fn kit_not_initialized() -> ApiError {
    ApiError::internal_error("Kit not initialized", "kit_not_initialized")
}
