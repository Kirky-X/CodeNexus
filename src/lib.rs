// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CodeNexus: A queryable code knowledge graph tool.
//!
//! Builds a queryable code knowledge graph from source repositories using
//! LadybugDB ([`lbug`]) for storage and [`tree_sitter`] for multi-language
//! parsing. This crate exposes the public API used by the CLI binary and
//! downstream embedders.
//!
//! See the module-level documentation for each subsystem:
//! - [`model`]: domain entities (nodes, edges, graph).
//! - [`discover`]: file discovery honoring ignore rules.
//! - [`parse`]: tree-sitter based multi-language extraction.
//! - [`resolve`]: symbol resolution and data-flow analysis.
//! - [`storage`]: LadybugDB persistence layer.
//! - [`index`]: indexing pipeline facade.
//! - [`embed`]: optional vector embedding (behind the `embed` feature).
//! - [`daemon`]: file-watching daemon.
//! - [`trace`]: call/data-flow tracing engine.
//! - [`cli`]: command-line interface.
//! - [`query`]: query engine.

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
)))]
compile_error!(
    "CodeNexus requires at least one `lang-*` feature enabled. \
     Use `--features lang-rust` (or lang-c/lang-fortran/lang-python/lang-typescript/lang-go/lang-java/lang-cpp), \
     or a preset like `--features minimal`/`core`/`full`."
);

pub mod cli;
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
pub mod storage;
pub mod trace;

#[cfg(feature = "embed")]
pub mod embed;

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
