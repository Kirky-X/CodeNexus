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
//!   [`AssignInfo`], [`ExternInfo`]).
//! - [`c`], [`rust_extractor`], [`fortran`], [`python`], [`typescript`]:
//!   language-specific [`Extractor`] implementations.
//! - [`dispatcher`]: [`get_extractor`] dispatches by [`Language`].
//! - [`parallel`]: [`parallel_parse`] parses batches of files in parallel with
//!   rayon (ADR-010), collecting failures without aborting the batch.

pub mod c;
pub mod dispatcher;
pub mod error;
pub mod extractor;
pub mod fortran;
pub mod parallel;
pub mod parser_factory;
pub mod parser_pool;
pub mod python;
pub mod rust_extractor;
pub mod typescript;

pub use c::CExtractor;
pub use dispatcher::get_extractor;
pub use error::{ParseError, Result};
pub use extractor::{
    extract_file, AssignInfo, CallInfo, ExternInfo, ExtractResult, Extractor, ImportInfo,
};
pub use fortran::FortranExtractor;
pub use parallel::{parallel_parse, parse_single, ParallelParseResult};
pub use parser_factory::ParserFactory;
pub use parser_pool::{with_thread_pool, ParserGuard, ParserPool};
pub use python::PythonExtractor;
pub use rust_extractor::RustExtractor;
pub use typescript::TypeScriptExtractor;
