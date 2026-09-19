// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Symbol resolution and data-flow analysis.
//!
//! Generates fully-qualified names (ADD §7.1), maintains scope chains and
//! symbol tables, and resolves call/data-flow/FFI edges (ADR-011).
//!
//! # Modules
//!
//! - [`error`]: [`ResolveError`] and [`Result`](error::Result) alias.
//! - [`fqn`]: [`FqnGenerator`] for ADD §7.1 FQN generation.
//! - [`includes_graph`]: [`IncludesGraph`] for C++ `#include` tracking and
//!   scope-aware cross-file call resolution.
//! - [`scope`]: [`Scope`] and [`ScopeChain`] for nested scope resolution.
//! - [`symbol_table`]: [`SymbolEntry`], [`FileSymbolTable`],
//!   [`ProjectSymbolTable`] for symbol indexing.
//! - [`calls`]: [`CallResolver`] for resolving CALLS edges (ADR-011).
//! - [`dataflow`]: [`DataFlowResolver`] for resolving DataFlows edges
//! - [`cross_lang`]: [`FfiResolver`] for resolving FfiCalls edges across
//!   languages (ADD §7.4).
//! - [`orchestrator`]: top-level orchestration functions
//!   ([`build_symbol_table`], [`resolve_all`]).
pub mod calls;
pub mod capability;
pub mod module;
// Cross-language FFI resolution is only meaningful when both C and Rust are
// compiled in (Rust extern "C" -> C definitions). Gate the entire module so
// leaner builds (e.g. `--features minimal`) don't reference unavailable
// `Language::C` / `Language::Rust` variants.
#[cfg(all(feature = "lang-c", feature = "lang-rust"))]
pub mod cross_lang;
pub mod dataflow;
pub mod error;
pub mod external_deps;
pub mod fqn;
pub mod imports;
pub mod includes_graph;
pub mod mro;
pub mod orchestrator;
pub mod scope;
pub mod symbol_table;
pub mod type_resolver;

pub use calls::CallResolver;
#[cfg(all(feature = "lang-c", feature = "lang-rust"))]
pub use cross_lang::{FfiResolver, MatchStrategy};
pub use dataflow::DataFlowResolver;
pub use error::{ResolveError, Result};
pub use fqn::FqnGenerator;
pub use imports::ImportResolver;
pub use includes_graph::{resolve_include, IncludesGraph};
pub use module::ResolverModule;
pub use mro::{mro_for, MroResolver, MroStrategy};
pub use orchestrator::{build_symbol_table, resolve_all};
pub use orchestrator::{enrich_symbol_table_from_rows, ResolveEnrichment};
pub use scope::{Scope, ScopeChain, ScopeContext, ScopeResolver, ScopeResolverRegistry};
pub use symbol_table::{FileSymbolTable, ProjectSymbolTable, SymbolEntry};
pub use type_resolver::TypeResolver;
