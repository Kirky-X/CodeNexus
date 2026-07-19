// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CodeNexus: A queryable code knowledge graph tool.
//!
//! Builds a queryable code knowledge graph from source repositories using
//! LadybugDB ([`lbug`]) for storage and [`tree_sitter`] for multi-language
//! parsing. This crate exposes the public API used by the CLI binary and
//! downstream embedders.

// In non-full feature subsets (minimal/core/single-language), some code paths
// (cli wrappers, analysis functions, language-specific resolvers) are compiled
// but not exercised. Allow dead_code/unused_imports/unused_mut in those
// configurations; the `full` feature enforces strict checks.
#![cfg_attr(not(feature = "full"), allow(dead_code, unused_imports, unused_mut))]

// Compile-time assertion: at least one language feature must be enabled
// (unified-architecture Phase 1). Without any `lang-*` feature the crate has
// no tree-sitter grammars and cannot parse anything; fail fast with a clear
// message instead of emitting downstream "variant not found" errors.
#[cfg(not(any(
    feature = "lang-c",
    feature = "lang-rust",
    feature = "lang-fortran",
    feature = "lang-python",
    feature = "lang-typescript",
    feature = "lang-go",
    feature = "lang-java",
    feature = "lang-cpp",
    feature = "lang-javascript",
    feature = "lang-ruby",
    feature = "lang-haskell",
    feature = "lang-ocaml",
    feature = "lang-scala",
    feature = "lang-php",
    feature = "lang-csharp",
    feature = "lang-bash",
    feature = "lang-html",
    feature = "lang-css",
    feature = "lang-json",
    feature = "lang-regex",
    feature = "lang-verilog",
    feature = "lsp",
)))]
compile_error!(
    "CodeNexus requires at least one `lang-*` or `lsp` feature enabled. \
     Use `--features lang-rust` (or any lang-* variant), `--features lsp`, \
     or a preset like `--features minimal`/`core`/`full`."
);

#[cfg(feature = "cache")]
pub mod cache;
#[cfg(feature = "daemon")]
pub mod daemon;
pub mod discover;
pub mod index;
pub mod ir;
pub mod kit;
pub mod model;
pub mod parse;
pub mod query;
pub mod resolve;
pub mod service;
pub mod storage;
pub mod trace;

pub use service::error::CodeNexusError;

/// Re-export of [`sdforge`] so binary targets (e.g. `codenexus-verify`) can
/// reuse `sdforge::clap` for CLI parsing without taking a direct `clap`
/// dependency. Only available when the `cli` feature is enabled.
#[cfg(feature = "cli")]
pub use sdforge;

#[cfg(feature = "analysis")]
pub mod analysis;

#[cfg(feature = "embed")]
pub mod embed;

#[cfg(feature = "lsp")]
pub mod lsp;

/// Test log capture utilities backed by inklog's `LoggerSubscriber`.
/// Available only in test builds; used by unit tests across the crate.
#[cfg(test)]
mod test_log_capture;

/// Returns the crate version, primarily for use by the CLI `--version` flag.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }
}
