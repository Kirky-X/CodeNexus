// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! LSP semantic type resolution.
//!
//! Integrates external Language Server Protocol servers (e.g. `rust-analyzer`
//! for Rust) to provide IDE-grade semantic queries — Go-to-Definition,
//! Type-Definition, Hover — over a JSON-RPC subprocess channel. The resolved
//! semantic types are mapped back onto CodeNexus [`crate::model::NodeLabel`]
//! variants via [`map_lsp_symbol_kind`].
//!
//! # Scope (v0.2.0)
//!
//! Only the Rust language is wired up (`RustAnalyzerClient`). Other languages
//! (Python/pyright, Go/gopls, Java/jdtls, C++/clangd) are deferred to
//! v0.3.0+. `textDocument/references` is intentionally out of scope — the
//! existing `trace --type calls` traversal already covers callers.
//!
//! # Feature gating
//!
//! The entire module compiles only when the `lsp` cargo feature is enabled:
//!
//! ```toml
//! lsp = ["dep:lsp-types", "dep:lsp-server"]
//! ```

pub mod client;
pub mod extract;
pub mod types;

pub use client::RustAnalyzerClient;
pub use extract::extract_hover_text;
pub use types::map_lsp_symbol_kind;

use std::path::Path;

/// LSP integration errors (Rule 12: failures must be explicit).
///
/// Every variant carries enough context to diagnose the failure without
/// needing to grep server logs.
#[derive(Debug, thiserror::Error)]
pub enum LspError {
    /// The external LSP server binary could not be started — either it is
    /// missing from `PATH`, not executable, or the spawn call itself failed.
    /// The string is a human-readable cause (typically the io::Error message).
    #[error("failed to start LSP server: {0}")]
    ServerStart(String),

    /// A JSON-RPC request/response round-trip failed — channel disconnect,
    /// deserialization mismatch, or a server-returned `ResponseError`. The
    /// string carries the protocol-level detail.
    #[error("LSP communication error: {0}")]
    Communication(String),

    /// The server did not respond within the [`REQUEST_TIMEOUT_MS`] window.
    /// Distinct from `Communication` so callers can apply retry/backoff
    /// policies specifically for transient overload.
    #[error("LSP request timed out after {0} ms")]
    Timeout(u64),
}

/// JSON-RPC round-trip timeout (specmark spec.md §Constraints: 5 seconds).
///
/// Exposed as a `pub const` so tests and downstream callers can reference the
/// exact threshold rather than hard-coding a magic number (Rule 5:
/// deterministic thresholds must be explicit).
pub const REQUEST_TIMEOUT_MS: u64 = 5_000;

/// Uniform abstraction over LSP server backends.
///
/// `RustAnalyzerClient` is the v0.2.0 reference implementation; the trait
/// exists so the indexing pipeline (R-lsp-004) and CLI commands can depend
/// on a stable shape that v0.3.0+ can extend with `GoplsClient`,
/// `PyrightClient`, etc. without touching call sites.
///
/// # Line/column convention
///
/// All `line`/`col` parameters are **0-based** to match the LSP spec
/// (`Position.line` and `Position.character` are zero-indexed). Callers
/// converting from 1-based editor coordinates must subtract one before
/// invoking these methods.
pub trait LspProvider: Send + Sync {
    /// Spawn the underlying LSP server and complete the `initialize`/
    /// `initialized` handshake rooted at `workspace`.
    ///
    /// Returns [`LspError::ServerStart`] if the binary cannot be spawned.
    /// Calling `start` twice without an intervening [`shutdown`] is
    /// implementation-defined — `RustAnalyzerClient` returns `Ok(())`
    /// (idempotent no-op) to keep the failure-tolerant indexing path simple.
    ///
    /// [`shutdown`]: LspProvider::shutdown
    fn start(&self, workspace: &Path) -> Result<(), LspError>;

    /// `textDocument/definition` — resolve the definition site of the symbol
    /// at `(file, line, col)`. Returns `Ok(None)` when the server reports
    /// no definition (e.g. keyword, built-in primitive).
    fn definition(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Option<lsp_types::Location>, LspError>;

    /// `textDocument/typeDefinition` — resolve the **type** definition site
    /// of the symbol at `(file, line, col)`. Distinct from `definition`:
    /// for `let x: Foo = ...`, `definition` jumps to `x`'s declaration
    /// while `type_definition` jumps to `Foo`'s definition.
    fn type_definition(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Option<lsp_types::Location>, LspError>;

    /// `textDocument/hover` — fetch hover info (type signature, docstring)
    /// for the symbol at `(file, line, col)`. Returns `Ok(None)` when the
    /// server has nothing to show.
    fn hover(&self, file: &Path, line: u32, col: u32)
        -> Result<Option<lsp_types::Hover>, LspError>;

    /// Send `shutdown` + `exit` to the server and reap the subprocess.
    ///
    /// Must be safe to call on a client that was never successfully
    /// [`start`](LspProvider::start)ed (returns `Ok(())` without panicking)
    /// so that drop paths and CLI error branches can call it unconditionally.
    fn shutdown(&self) -> Result<(), LspError>;
}
