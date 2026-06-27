// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Parse capability traits (T6/unified-architecture Phase 2, Task 2.3).
//!
//! Defines [`ParserRegistry`] and [`ExtractorRegistry`], the capability trait
//! objects stored in [`Kit`](crate::kit::Kit) under [`ParserKey`] and
//! [`ExtractorKey`] respectively.
//!
//! # Design note
//!
//! [`ParserPool`](super::ParserPool) is `!Sync` (it uses `RefCell` for
//! thread-local caching, ADR-010), so it cannot itself be the capability.
//! Instead, [`ParserRegistry`] exposes the stateless factory interface
//! ([`ParserFactory::create_parser`](super::ParserFactory::create_parser));
//! the thread-local pool stays as a per-thread cache layered on top.
//!
//! [`ParserKey`]: crate::kit::ParserKey
//! [`ExtractorKey`]: crate::kit::ExtractorKey

use tree_sitter::Parser;

use crate::model::Language;

use super::error::ParseError;
use super::extractor::Extractor;

/// Capability trait for the Parser subsystem (tree-sitter parser factory).
///
/// Stored in [`Kit`](crate::kit::Kit) as `Arc<dyn ParserRegistry>` under
/// [`ParserKey`](crate::kit::ParserKey). The concrete impl (Task 2.5) wraps
/// [`ParserFactory`](super::ParserFactory); the thread-local
/// [`ParserPool`](super::ParserPool) remains a per-thread cache on top.
pub trait ParserRegistry: Send + Sync {
    /// Creates and configures a tree-sitter [`Parser`] for `lang`.
    fn create_parser(&self, lang: Language) -> std::result::Result<Parser, ParseError>;

    /// Returns the languages available in this build (feature-gated).
    fn supported_languages(&self) -> Vec<Language>;
}

/// Capability trait for the Extractor registry (per-language dispatch).
///
/// Stored in [`Kit`](crate::kit::Kit) as `Arc<dyn ExtractorRegistry>` under
/// [`ExtractorKey`](crate::kit::ExtractorKey). The concrete impl (Task 2.6)
/// wraps [`get_extractor`](super::get_extractor). Requires `ParserKey`.
pub trait ExtractorRegistry: Send + Sync {
    /// Returns a boxed [`Extractor`] for the given [`Language`].
    fn get_extractor(&self, language: Language) -> Box<dyn Extractor>;

    /// Returns the languages for which an extractor is available.
    fn supported_languages(&self) -> Vec<Language>;
}

/// Compile-time assertions: both traits are object-safe and `Send + Sync`.
#[cfg(test)]
const _: () = {
    fn _assert_object_safe(_: &dyn ParserRegistry, _: &dyn ExtractorRegistry) {}
    fn _assert_send_sync<T: Send + Sync + ?Sized>() {}
    fn _check() {
        _assert_send_sync::<dyn ParserRegistry>();
        _assert_send_sync::<dyn ExtractorRegistry>();
        let _ = _assert_object_safe;
    }
};
