// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! tree-sitter based multi-language parsing.
//!
//! Provides the [`Extractor`] adapter trait, a [`ParserFactory`] (Factory
//! pattern), a thread-local [`ParserPool`] (ADR-010), the shared
//! extraction result types used by the language-specific extractors
//! implemented in Task 6, a [`get_extractor`] dispatcher, and
//! [`parallel_parse`] for rayon-based file-level parallel parsing (Task 7,
//! ADR-010).
//!
//! # Modules
//!
//! - [`error`]: [`ParseError`] and [`Result`](error::Result) alias.
//! - [`parser_factory`]: [`ParserFactory`] maps [`Language`] to tree-sitter
//!   grammars and creates [`Parser`](tree_sitter::Parser) instances.
//! - [`parser_pool`]: [`ParserPool`] caches parsers per language per thread
//!   (ADR-010), with a thread-local instance via [`with_thread_pool`].
//! - [`extractor`]: [`Extractor`] trait (Adapter pattern), [`ExtractResult`],
//!   and intermediate record types ([`ImportInfo`], [`CallInfo`],
//!   [`AssignInfo`], [`ExternInfo`], [`ReadInfo`], [`WriteInfo`]).
//! - [`c`], [`rust_extractor`], [`fortran`], [`python`], [`typescript`]:
//!   language-specific [`Extractor`] implementations. Each is gated by its
//!   `lang-*` Cargo feature (unified-architecture Phase 1); only the
//!   languages compiled into the current build are available.
//! - [`dispatcher`]: [`get_extractor`] dispatches by [`Language`].
//! - [`parallel`]: [`parallel_parse`] parses batches of files in parallel with
//!   rayon (ADR-010), collecting failures without aborting the batch.

#[cfg(feature = "lang-c")]
pub mod c;
pub mod capability;
#[cfg(feature = "lang-cpp")]
pub mod cpp;
pub mod dispatcher;
pub mod error;
pub mod extractor;
#[cfg(feature = "lang-fortran")]
pub mod fortran;
#[cfg(feature = "lang-go")]
pub mod go;
pub mod helpers;
#[cfg(feature = "lang-java")]
pub mod java;
pub mod module;
pub mod parallel;
pub mod parser_factory;
pub mod parser_pool;
#[cfg(feature = "lang-python")]
pub mod python;
#[cfg(feature = "lang-rust")]
pub mod rust_extractor;
#[cfg(feature = "lang-typescript")]
pub mod typescript;

#[cfg(feature = "lang-c")]
pub use c::CExtractor;
#[cfg(feature = "lang-cpp")]
pub use cpp::CppExtractor;
pub use dispatcher::get_extractor;
pub use error::{ParseError, Result};
pub use extractor::{
    extract_file, extract_from_source, AssignInfo, CallInfo, ExternInfo, ExtractResult, Extractor,
    ImportInfo, ReadInfo, WriteInfo,
};
#[cfg(feature = "lang-fortran")]
pub use fortran::FortranExtractor;
#[cfg(feature = "lang-go")]
pub use go::GoExtractor;
pub use helpers::dedupe_qn;
#[cfg(feature = "lang-java")]
pub use java::JavaExtractor;
pub use module::{
    ExtractorRegistryModule, ExtractorRegistryModuleBuilder, ParserFactoryModule,
    ParserFactoryModuleBuilder,
};
pub use parallel::{
    parallel_parse, parallel_parse_ram_first, parse_single, ParallelParseResult, RamFirstSources,
};
pub use parser_factory::ParserFactory;
pub use parser_pool::{with_thread_pool, ParserGuard, ParserPool};
#[cfg(feature = "lang-python")]
pub use python::PythonExtractor;
#[cfg(feature = "lang-rust")]
pub use rust_extractor::RustExtractor;
#[cfg(feature = "lang-typescript")]
pub use typescript::TypeScriptExtractor;
