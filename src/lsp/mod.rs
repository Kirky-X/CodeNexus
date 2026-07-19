// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! LSP semantic type resolution.
//!
//! Integrates external Language Server Protocol servers (e.g. `rust-analyzer`
//! for Rust) to provide IDE-grade semantic queries — Go-to-Definition,
//! Type-Definition, Hover, References — over a JSON-RPC subprocess channel.
//! The resolved semantic types are mapped back onto CodeNexus
//! [`crate::model::NodeLabel`] variants via [`map_lsp_symbol_kind`].
//!
//! # Scope
//!
//! Seven languages are wired up: Rust (`RustAnalyzerClient`), Python
//! (`PyrightClient`), C/C++ (`ClangdClient`), Go (`GoplsClient`),
//! TypeScript/JavaScript (`TypeScriptLanguageClient`), Fortran
//! (`FortlsClient`), and Java (`JdtlsClient`).
//!
//! `textDocument/references` (C9) is implemented for
//! `RustAnalyzerClient` / `PyrightClient` / `ClangdClient` only; the
//! remaining clients inherit the trait default which returns
//! [`LspError::NotImplemented`]. Results are cached for 5 minutes keyed
//! by `(uri, line, column)` — see [`references_cache::ReferencesCache`].
//!
//! # Feature gating
//!
//! The entire module compiles only when the `lsp` cargo feature is enabled:
//!
//! ```toml
//! lsp = ["dep:lsp-types", "dep:lsp-server", "dep:crossbeam-channel", "dep:url"]
//! ```

pub mod clangd;
pub mod client;
pub mod extract;
pub mod fortls;
pub mod gopls;
pub mod jdtls;
pub mod pyright;
pub(crate) mod references_cache;
pub(crate) mod session;
pub mod types;
pub mod typescript_ls;

pub use clangd::ClangdClient;
pub use client::RustAnalyzerClient;
pub use extract::extract_hover_text;
pub use fortls::FortlsClient;
pub use gopls::GoplsClient;
pub use jdtls::JdtlsClient;
pub use pyright::PyrightClient;
pub use references_cache::{CacheKey, Clock, MockClock, ReferencesCache, SystemClock};
pub use types::map_lsp_symbol_kind;
pub use typescript_ls::TypeScriptLanguageClient;

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

    /// The LSP method is not implemented for this client. Used as the
    /// default return value for [`LspProvider::references`] on clients
    /// that have not yet wired up the method (e.g. `GoplsClient`,
    /// `JdtlsClient`, `FortlsClient`, `TypeScriptLanguageClient` for
    /// `textDocument/references` — see C9 spec R-lsp-002).
    ///
    /// The string identifies the client + method (e.g.
    /// `"gopls: textDocument/references not implemented"`) so callers
    /// can surface a precise hint in diagnostics.
    #[error("LSP method not implemented: {0}")]
    NotImplemented(String),
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

    /// `textDocument/references` — resolve all reference sites of the symbol
    /// at `(file, line, col)`. Returns `Ok(Vec::new())` when the server
    /// reports no references (e.g. unused symbol, built-in primitive).
    ///
    /// # C9 scope (R-lsp-002)
    ///
    /// Only `RustAnalyzerClient` / `PyrightClient` / `ClangdClient`
    /// override this default. All other clients return
    /// [`LspError::NotImplemented`]; the default message embeds the
    /// client name so callers can route to a fallback (e.g. tree-sitter
    /// CALLS edge traversal) without re-querying.
    ///
    /// # Caching (R-lsp-004)
    ///
    /// Implementations MUST consult their [`ReferencesCache`] before
    /// dispatching the LSP request, and store the result on miss. Cache
    /// key is `(file_uri, line, col)`; TTL is 5 minutes; capacity is
    /// 1000 entries (LRU). File changes arriving via
    /// `textDocument/didChange` must invalidate the corresponding URI.
    ///
    /// # Line/column convention
    ///
    /// Same as [`definition`](LspProvider::definition): 0-based, matching
    /// the LSP `Position.line` / `Position.character` convention.
    fn references(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<lsp_types::Location>, LspError> {
        let _ = (file, line, col);
        Err(LspError::NotImplemented(format!(
            "{}: textDocument/references not implemented",
            std::any::type_name::<Self>()
        )))
    }

    /// Send `shutdown` + `exit` to the server and reap the subprocess.
    ///
    /// Must be safe to call on a client that was never successfully
    /// [`start`](LspProvider::start)ed (returns `Ok(())` without panicking)
    /// so that drop paths and CLI error branches can call it unconditionally.
    fn shutdown(&self) -> Result<(), LspError>;
}
