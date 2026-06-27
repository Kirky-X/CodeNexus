// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit module for the Parser subsystem (T6/unified-architecture
//! Phase 2, Task 2.5).
//!
//! Implements [`Module`] / [`ModuleBuilder`] for [`ParserFactoryModule`],
//! wiring the existing [`ParserFactory`] into the unified Kit registry as
//! `Arc<dyn ParserRegistry>` under
//! [`ParserKey`](crate::kit::ParserKey).
//!
//! # Design note
//!
//! [`ParserPool`] is `!Sync` (it uses `RefCell` for thread-local caching,
//! ADR-010), so it cannot itself be the capability. Instead,
//! [`ParserRegistryCapability`] exposes the **stateless** factory interface
//! ([`ParserFactory::create_parser`]); the thread-local [`ParserPool`]
//! remains a per-thread cache layered on top (see
//! [`capability.rs`](super::capability)).
//!
//! [`Module`]: crate::kit::Module
//! [`ModuleBuilder`]: crate::kit::ModuleBuilder
//! [`ParserPool`]: super::ParserPool
//! [`ParserFactory`]: super::ParserFactory

use std::sync::Arc;

use tree_sitter::Parser;

use crate::kit::{Module, ModuleBuilder, NoConfig, NoRequirements};
use crate::model::Language;

use super::capability::{ExtractorRegistry, ParserRegistry};
use super::dispatcher::get_extractor;
use super::error::ParseError;
use super::extractor::Extractor;
use super::parser_factory::ParserFactory;

// ---------------------------------------------------------------------------
// Module + Builder
// ---------------------------------------------------------------------------

/// trait-kit module tag for the Parser subsystem (Task 2.5).
///
/// Zero-sized marker — construction logic lives in
/// [`ParserFactoryModuleBuilder::build`]. Register in Kit via:
///
/// ```ignore
/// use codenexus::kit::{IntoKitModuleBuilder, Kit, ParserKey};
/// use codenexus::parse::ParserFactoryModuleBuilder;
///
/// let kit = Kit::new();
/// let parser = ParserFactoryModuleBuilder::new()
///     .kit(&kit)
///     .provide::<ParserKey>()?;
/// ```
pub struct ParserFactoryModule;

/// Builder for [`ParserFactoryModule`] (Task 2.5).
///
/// No configuration is required — the parser factory is stateless.
pub struct ParserFactoryModuleBuilder;

impl ParserFactoryModuleBuilder {
    /// Creates a new builder.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for ParserFactoryModuleBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for ParserFactoryModule {
    type Config = NoConfig;
    type Requirements = NoRequirements;
    type Capability = Arc<dyn ParserRegistry>;
    type Error = ParseError;
    type Builder = ParserFactoryModuleBuilder;
    const NAME: &'static str = "parser";
}

impl ModuleBuilder<ParserFactoryModule> for ParserFactoryModuleBuilder {
    fn build(self) -> Result<Arc<dyn ParserRegistry>, ParseError> {
        Ok(Arc::new(ParserRegistryCapability))
    }
}

// ---------------------------------------------------------------------------
// Concrete dyn ParserRegistry implementation
// ---------------------------------------------------------------------------

/// Concrete implementation of [`dyn ParserRegistry`] delegating to the
/// stateless [`ParserFactory`].
///
/// This is zero-sized — every call to [`create_parser`](ParserRegistry::create_parser)
/// creates a fresh [`Parser`] via [`ParserFactory::create_parser`]. The
/// thread-local [`ParserPool`](super::ParserPool) can be layered on top by
/// callers who want per-thread caching.
struct ParserRegistryCapability;

impl ParserRegistry for ParserRegistryCapability {
    fn create_parser(&self, lang: Language) -> Result<Parser, ParseError> {
        ParserFactory::create_parser(lang)
    }

    fn supported_languages(&self) -> Vec<Language> {
        Language::all()
    }
}

// ===========================================================================
// ExtractorRegistryModule (Task 2.6)
// ===========================================================================
//
// Wraps `dispatcher::get_extractor` as `Arc<dyn ExtractorRegistry>` under
// `ExtractorKey`. Conceptually depends on `ParserKey` (extractors operate on
// trees produced by parsers), but the concrete impl is a stateless factory
// that does not need the parser at build time, so `Requirements = NoRequirements`.
// The bootstrap enforces ordering by building `ParserKey` before `ExtractorKey`.

/// trait-kit module tag for the Extractor registry (Task 2.6).
pub struct ExtractorRegistryModule;

/// Builder for [`ExtractorRegistryModule`] (Task 2.6).
///
/// No configuration is required — the extractor dispatcher is stateless.
pub struct ExtractorRegistryModuleBuilder;

impl ExtractorRegistryModuleBuilder {
    /// Creates a new builder.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for ExtractorRegistryModuleBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for ExtractorRegistryModule {
    type Config = NoConfig;
    type Requirements = NoRequirements;
    type Capability = Arc<dyn ExtractorRegistry>;
    type Error = ParseError;
    type Builder = ExtractorRegistryModuleBuilder;
    const NAME: &'static str = "extractor";
}

impl ModuleBuilder<ExtractorRegistryModule> for ExtractorRegistryModuleBuilder {
    fn build(self) -> Result<Arc<dyn ExtractorRegistry>, ParseError> {
        Ok(Arc::new(ExtractorRegistryCapability))
    }
}

/// Concrete implementation of [`dyn ExtractorRegistry`] delegating to the
/// stateless [`get_extractor`] dispatcher.
struct ExtractorRegistryCapability;

impl ExtractorRegistry for ExtractorRegistryCapability {
    fn get_extractor(&self, language: Language) -> Box<dyn Extractor> {
        get_extractor(language)
    }

    fn supported_languages(&self) -> Vec<Language> {
        Language::all()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{ExtractorKey, ParserKey};

    #[test]
    fn build_returns_capability() {
        let cap = ParserFactoryModuleBuilder::new()
            .build()
            .expect("build");
        // supported_languages should return all compiled-in languages.
        let langs = cap.supported_languages();
        assert!(!langs.is_empty(), "at least one language must be compiled in");
    }

    #[test]
    fn capability_creates_parser_for_compiled_languages() {
        let cap = ParserFactoryModuleBuilder::new()
            .build()
            .expect("build");
        for lang in cap.supported_languages() {
            let parser = cap.create_parser(lang);
            assert!(parser.is_ok(), "create_parser failed for {lang}");
        }
    }

    /// Verify the full Kit registration flow works end-to-end.
    #[test]
    fn kit_registration_flow() {
        use crate::kit::{IntoKitModuleBuilder, Kit};

        let kit = Kit::new();
        let parser = ParserFactoryModuleBuilder::new()
            .kit(&kit)
            .provide::<ParserKey>()
            .expect("provide::<ParserKey>");

        assert!(kit.contains::<ParserKey>());

        let required = kit
            .require::<ParserKey>()
            .expect("require::<ParserKey>");
        assert!(Arc::ptr_eq(&parser, &required));
    }

    // --- ExtractorRegistryModule tests (Task 2.6) ---

    #[test]
    fn extractor_build_returns_capability() {
        let cap = ExtractorRegistryModuleBuilder::new()
            .build()
            .expect("build");
        let langs = cap.supported_languages();
        assert!(
            !langs.is_empty(),
            "at least one language must be compiled in"
        );
    }

    #[test]
    fn extractor_capability_returns_extractor_for_compiled_languages() {
        let cap = ExtractorRegistryModuleBuilder::new()
            .build()
            .expect("build");
        for lang in cap.supported_languages() {
            let ext = cap.get_extractor(lang);
            assert_eq!(
                ext.language(),
                lang,
                "extractor should report its language as {lang}"
            );
        }
    }

    /// Verify the Extractor registry registers under `ExtractorKey` end-to-end.
    #[test]
    fn extractor_kit_registration_flow() {
        use crate::kit::{IntoKitModuleBuilder, Kit};

        let kit = Kit::new();
        let extractor = ExtractorRegistryModuleBuilder::new()
            .kit(&kit)
            .provide::<ExtractorKey>()
            .expect("provide::<ExtractorKey>");

        assert!(kit.contains::<ExtractorKey>());

        let required = kit
            .require::<ExtractorKey>()
            .expect("require::<ExtractorKey>");
        assert!(Arc::ptr_eq(&extractor, &required));
    }
}
