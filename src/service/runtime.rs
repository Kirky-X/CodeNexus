// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Global Kit runtime injection for CLI and MCP service handlers.
//!
//! Provides a process-global `Mutex<Option<Arc<Kit>>>` so that `#[service_api]`
//! handlers (which cannot accept injected state) can access the Kit via
//! [`kit()`].

use std::sync::{Arc, Mutex};

use crate::kit::Kit;

static KIT: Mutex<Option<Arc<Kit>>> = Mutex::new(None);

/// Stores the Kit in the global `Mutex` so service handlers can access it.
///
/// # Errors
///
/// Returns `Err` if the Kit has already been initialized.
pub fn init_kit(kit: Kit) -> Result<(), String> {
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
pub fn kit() -> Option<Arc<Kit>> {
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

    #[test]
    fn init_kit_second_call_returns_error() {
        reset_kit_for_testing();
        use crate::kit::Kit;
        let kit = Kit::new();
        assert!(init_kit(kit).is_ok(), "first init_kit should succeed");
        let kit2 = Kit::new();
        let result = init_kit(kit2);
        assert!(result.is_err(), "second init_kit call must return Err");
        assert!(
            result.unwrap_err().contains("already initialized"),
            "error message should mention 'already initialized'"
        );
        reset_kit_for_testing();
    }
}
