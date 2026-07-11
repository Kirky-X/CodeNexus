// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Global Kit runtime injection for CLI and MCP service handlers.
//!
//! Provides a process-global `Mutex<Option<Arc<AsyncKit<AsyncReady>>>>` so that
//! `#[service_api]` handlers (which cannot accept injected state) can access
//! the Kit via [`kit()`].
//!
//! # Why AsyncKit<AsyncReady> and not Kit
//!
//! trait-kit 0.2.4's synchronous `Kit` uses `RefCell` internally and is
//! therefore `!Send + !Sync`. A `static Mutex<Option<Arc<Kit>>>` requires
//! `Send + Sync`, so we store `AsyncKit<AsyncReady>` instead — it is backed
//! by `Arc<RwLock<...>>` and implements `Send + Sync`. See `kit/mod.rs` and
//! `design.md` D5 for the rationale.

use std::sync::{Arc, Mutex};

use crate::kit::{AsyncKit, AsyncReady};

static KIT: Mutex<Option<Arc<AsyncKit<AsyncReady>>>> = Mutex::new(None);

/// Stores the Kit in the global `Mutex` so service handlers can access it.
///
/// # Errors
///
/// Returns `Err` if the Kit has already been initialized.
pub fn init_kit(kit: AsyncKit<AsyncReady>) -> Result<(), String> {
    let mut guard = KIT.lock().map_err(|_| "Kit lock poisoned".to_string())?;
    if guard.is_some() {
        return Err("Kit already initialized".to_string());
    }
    *guard = Some(Arc::new(kit));
    Ok(())
}

/// Returns the global Kit instance if initialized, or `None` if [`init_kit`]
/// hasn't been called.
///
/// If the internal `Mutex` is poisoned (a thread panicked while holding the
/// lock), logs an error before returning `None` so the failure mode is
/// distinguishable from "not yet initialized".
#[must_use]
pub fn kit() -> Option<Arc<AsyncKit<AsyncReady>>> {
    KIT.lock()
        .map_err(|e| tracing::error!("Kit mutex poisoned: {e}"))
        .ok()
        .and_then(|guard| guard.clone())
}

/// Resets the global Kit. Test-only — allows tests to avoid polluting each
/// other via the process-global `Mutex`.
#[cfg(test)]
pub fn reset_kit_for_testing() {
    if let Ok(mut guard) = KIT.lock() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn init_kit_second_call_returns_error() {
        reset_kit_for_testing();
        // Build empty AsyncKit<Ready> instances — build() on an empty kit
        // succeeds (no modules to fail). We only need the type, not real
        // capabilities, to test the init_kit mutex behavior.
        let kit1 = AsyncKit::new().build().await.expect("build kit1");
        assert!(init_kit(kit1).is_ok(), "first init_kit should succeed");
        let kit2 = AsyncKit::new().build().await.expect("build kit2");
        let result = init_kit(kit2);
        assert!(result.is_err(), "second init_kit call must return Err");
        assert!(
            result.unwrap_err().contains("already initialized"),
            "error message should mention 'already initialized'"
        );
        reset_kit_for_testing();
    }
}
