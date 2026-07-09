// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! MCP server module (v0.3.0, T009) — sdforge-based MCP protocol exposure.
//!
//! Replaces the hand-written JSON-RPC in `src/cli/mcp_cmd.rs` with sdforge's
//! declarative `#[service_api]` macro + rmcp stdio transport.
//!
//! # Architecture
//!
//! 1. [`init_kit()`] stores the Kit in a global `OnceLock<Arc<Kit>>`
//! 2. Tool handlers (defined via `#[service_api]`) access the Kit via [`kit()`]
//! 3. [`serve()`] builds the sdforge MCP server from registered tools and
//!    serves it over stdio using a tokio runtime
//! 4. [`run()`] is the entry point that calls `init_kit` then `serve`
//!
//! # Why OnceLock instead of passing Kit to handlers
//!
//! sdforge's `#[service_api]` macro generates standalone async functions with
//! no mechanism to inject runtime state. A process-global `OnceLock` is the
//! simplest way to make the Kit available to these functions without wrapping
//! every handler in a closure or struct.

use std::sync::{Arc, OnceLock};

use codenexus::cli::args::McpArgs;
use codenexus::cli::error::{CliError, Result};
use codenexus::kit::Kit;

/// Global Kit instance injected into MCP tool handlers.
///
/// Set once by [`init_kit()`], accessed by tool handlers via [`kit()`].
static KIT: OnceLock<Arc<Kit>> = OnceLock::new();

/// Returns the Kit instance if initialized, or `None` if [`run`] hasn't been
/// called.
///
/// Tool handlers use this to access the query/trace/storage capabilities:
///
/// ```no_run
/// let kit = codenexus::mcp::kit().expect("Kit not initialized");
/// let query = kit.require::<codenexus::kit::QueryKey>()?;
/// ```
#[must_use]
pub fn kit() -> Option<&'static Arc<Kit>> {
    KIT.get()
}

/// Stores the Kit in the global `OnceLock` so tool handlers can access it.
///
/// This is separated from [`serve()`] so it can be tested independently
/// (the serve loop blocks on stdin, which is not testable in unit tests).
///
/// # Errors
///
/// Returns [`CliError::InvalidInput`] if the Kit has already been initialized
/// (the `OnceLock` is set-once).
pub fn init_kit(kit: Kit) -> Result<()> {
    KIT.set(Arc::new(kit)).map_err(|_| {
        CliError::InvalidInput("MCP server already initialized".to_string())
    })
}

/// Starts the sdforge MCP server over stdio.
///
/// Builds the MCP server from registered tools (via `inventory`) and serves
/// it over stdio using rmcp's `ServiceExt::serve` with a tokio runtime.
///
/// # Errors
///
/// Returns [`CliError::Io`] if the tokio runtime fails to create, or
/// [`CliError::InvalidInput`] if the MCP server fails to start or serve.
fn serve() -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        use rmcp::ServiceExt;
        let server = sdforge::mcp::build();
        let transport = rmcp::transport::stdio();
        let service = server
            .serve(transport)
            .await
            .map_err(|e| CliError::InvalidInput(format!("MCP serve error: {e}")))?;
        service
            .waiting()
            .await
            .map_err(|e| CliError::InvalidInput(format!("MCP service error: {e}")))?;
        Ok(())
    })
}

/// Entry point for the `mcp` CLI subcommand.
///
/// Stores the Kit in the global `OnceLock` via [`init_kit()`], then starts
/// the sdforge MCP server over stdio via [`serve()`].
///
/// # Arguments
///
/// * `kit` - The fully-wired Kit with all capabilities registered.
/// * `_args` - MCP subcommand arguments (currently unused — the server
///   speaks stdio only).
///
/// # Errors
///
/// See [`init_kit()`] and [`serve()`] for error conditions.
pub fn run(kit: Kit, _args: &McpArgs) -> Result<()> {
    init_kit(kit)?;
    serve()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenexus::kit::{build_kit, KitBootstrapConfig};

    /// Verifies that `init_kit` stores the Kit in the global `OnceLock`.
    ///
    /// Note: This test sets the process-global `KIT` OnceLock. Since `OnceLock`
    /// is set-once, this test can only pass once per process. Cargo runs each
    /// test in a separate process, so this is safe.
    #[test]
    fn mcp_run_initializes_kit() {
        // Build a minimal Kit with an in-memory database.
        let tmp = tempfile::NamedTempFile::new().expect("create temp db file");
        let config = KitBootstrapConfig::new(tmp.path().to_path_buf());
        let built = build_kit(&config).expect("build_kit should succeed");

        // Before init, kit() should return None (unless another test already
        // set it in this process — which shouldn't happen since cargo uses
        // separate processes per test).
        // Note: We don't assert kit().is_none() before init because if another
        // test in the same binary already called init_kit, it would be Some.

        init_kit(built).expect("init_kit should succeed");

        // After init, kit() must return Some.
        assert!(kit().is_some(), "kit() should return Some after init_kit");
    }
}
