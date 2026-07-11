// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! trait-kit module for the Parser subsystem (T6/unified-architecture
//! Phase 2, Task 2.5; v0.3.3 AsyncKit migration).
//!
//! Implements [`ModuleMeta`] + [`AsyncAutoBuilder`] for
//! [`ParserFactoryModule`] and [`ExtractorRegistryModule`], wiring the
//! existing [`ParserFactory`] and [`get_extractor`] dispatcher into the
//! unified Kit registry as `Arc<dyn ParserRegistry>` and
//! `Arc<dyn ExtractorRegistry>` respectively.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tree_sitter::Parser;

use crate::kit::{AsyncAutoBuilder, AsyncKit, ModuleMeta};
use crate::model::Language;

use super::capability::{ExtractorRegistry, ParserRegistry};
use super::dispatcher::get_extractor;
use super::error::ParseError;
use super::extractor::Extractor;
use super::parser_factory::ParserFactory;

// ===========================================================================
// ParserFactoryModule
// ===========================================================================

/// trait-kit module tag for the Parser subsystem (Task 2.5).
///
/// Zero-sized marker — construction logic lives in the [`AsyncAutoBuilder`]
/// impl. Register in Kit via:
///
/// ```ignore
/// use codenexus::kit::{AsyncKit, ParserFactoryModule};
///
/// let mut kit = AsyncKit::new();
/// kit.register::<ParserFactoryModule>()?;
/// let kit = kit.build().await?;
/// let parser = kit.require::<ParserFactoryModule>()?;
/// ```
pub struct ParserFactoryModule;

impl ModuleMeta for ParserFactoryModule {
    const NAME: &'static str = "parser";
    fn dependencies() -> &'static [(&'static str, TypeId)] {
        &[]
    }
}

impl AsyncAutoBuilder for ParserFactoryModule {
    type Capability = Arc<dyn ParserRegistry>;
    type Error = ParseError;

    fn build<'a>(
        _kit: &'a AsyncKit,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Capability, Self::Error>> + Send + 'a>> {
        Box::pin(async move { Self::build_cap() })
    }
}

impl ParserFactoryModule {
    /// Constructs a ParserRegistryCapability.
    ///
    /// Shared between [`AsyncAutoBuilder::build`] and tests.
    pub(crate) fn build_cap() -> Result<Arc<dyn ParserRegistry>, ParseError> {
        Ok(Arc::new(ParserRegistryCapability))
    }
}

/// Concrete implementation of [`dyn ParserRegistry`] delegating to the
/// stateless [`ParserFactory`].
///
/// This is zero-sized — every call to [`create_parser`](ParserRegistry::create_parser)
/// creates a fresh [`Parser`] via [`ParserFactory::create_parser`].
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

/// trait-kit module tag for the Extractor registry (Task 2.6).
pub struct ExtractorRegistryModule;

impl ModuleMeta for ExtractorRegistryModule {
    const NAME: &'static str = "extractor";
    fn dependencies() -> &'static [(&'static str, TypeId)] {
        &[]
    }
}

impl AsyncAutoBuilder for ExtractorRegistryModule {
    type Capability = Arc<dyn ExtractorRegistry>;
    type Error = ParseError;

    fn build<'a>(
        _kit: &'a AsyncKit,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Capability, Self::Error>> + Send + 'a>> {
        Box::pin(async move { Self::build_cap() })
    }
}

impl ExtractorRegistryModule {
    /// Constructs an ExtractorRegistryCapability.
    ///
    /// Shared between [`AsyncAutoBuilder::build`] and tests.
    pub(crate) fn build_cap() -> Result<Arc<dyn ExtractorRegistry>, ParseError> {
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
    use crate::kit::{AsyncKit, ExtractorRegistryModule, ParserFactoryModule};

    #[test]
    fn build_returns_capability() {
        let cap = ParserFactoryModule::build_cap().expect("build_cap");
        let langs = cap.supported_languages();
        assert!(
            !langs.is_empty(),
            "at least one language must be compiled in"
        );
    }

    #[test]
    fn capability_creates_parser_for_compiled_languages() {
        let cap = ParserFactoryModule::build_cap().expect("build_cap");
        for lang in cap.supported_languages() {
            let parser = cap.create_parser(lang);
            assert!(parser.is_ok(), "create_parser failed for {lang}");
        }
    }

    /// Verify the full AsyncKit registration flow works end-to-end.
    #[tokio::test]
    async fn kit_registration_flow() {
        let mut kit = AsyncKit::new();
        kit.register::<ParserFactoryModule>()
            .expect("register::<ParserFactoryModule>");
        let kit = kit.build().await.expect("build");

        assert!(kit.contains::<ParserFactoryModule>());

        let required = kit
            .require::<ParserFactoryModule>()
            .expect("require::<ParserFactoryModule>");
        assert!(!required.supported_languages().is_empty());
    }

    // --- ExtractorRegistryModule tests (Task 2.6) ---

    #[test]
    fn extractor_build_returns_capability() {
        let cap = ExtractorRegistryModule::build_cap().expect("build_cap");
        let langs = cap.supported_languages();
        assert!(
            !langs.is_empty(),
            "at least one language must be compiled in"
        );
    }

    #[test]
    fn extractor_capability_returns_extractor_for_compiled_languages() {
        let cap = ExtractorRegistryModule::build_cap().expect("build_cap");
        for lang in cap.supported_languages() {
            let ext = cap.get_extractor(lang);
            assert_eq!(
                ext.language(),
                lang,
                "extractor should report its language as {lang}"
            );
        }
    }

    /// Verify the Extractor registry registers end-to-end.
    #[tokio::test]
    async fn extractor_kit_registration_flow() {
        let mut kit = AsyncKit::new();
        kit.register::<ExtractorRegistryModule>()
            .expect("register::<ExtractorRegistryModule>");
        let kit = kit.build().await.expect("build");

        assert!(kit.contains::<ExtractorRegistryModule>());

        let required = kit
            .require::<ExtractorRegistryModule>()
            .expect("require::<ExtractorRegistryModule>");
        assert!(!required.supported_languages().is_empty());
    }
}
