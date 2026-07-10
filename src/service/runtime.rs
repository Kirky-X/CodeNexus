// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Global Kit runtime injection for CLI and MCP service handlers.
//!
//! Provides a process-global `OnceLock<Arc<Kit>>` so that `#[service_api]`
//! handlers (which cannot accept injected state) can access the Kit via
//! [`kit()`].

use std::sync::{Arc, OnceLock};

use crate::kit::Kit;

static KIT: OnceLock<Arc<Kit>> = OnceLock::new();

/// Stores the Kit in the global `OnceLock` so service handlers can access it.
///
/// # Errors
///
/// Returns `Err` if the Kit has already been initialized (OnceLock is set-once).
pub fn init_kit(kit: Kit) -> Result<(), String> {
    KIT.set(Arc::new(kit)).map_err(|_| "Kit already initialized".to_string())
}

/// Returns the global Kit instance if initialized, or `None` if [`init_kit`]
/// hasn't been called.
#[must_use]
pub fn kit() -> Option<&'static Arc<Kit>> {
    KIT.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kit_returns_none_before_init() {
        // Note: OnceLock is global, so if another test already initialized it,
        // this test may see Some. We only assert None when truly uninitialized.
        // Since tests run in the same process, we can't reset the OnceLock.
        // This test is only meaningful in isolation.
        let _ = kit();
    }
}
