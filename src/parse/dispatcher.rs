// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Language extractor dispatcher (Factory pattern, ADR-003).
//!
//! Maps a [`Language`] to the corresponding [`Extractor`] implementation,
//! enabling language-agnostic extraction in the indexing pipeline.

use crate::model::Language;

use super::extractor::Extractor;
#[cfg(feature = "lang-c")]
use super::c::CExtractor;
#[cfg(feature = "lang-fortran")]
use super::fortran::FortranExtractor;
#[cfg(feature = "lang-go")]
use super::go::GoExtractor;
#[cfg(feature = "lang-java")]
use super::java::JavaExtractor;
#[cfg(feature = "lang-python")]
use super::python::PythonExtractor;
#[cfg(feature = "lang-rust")]
use super::rust_extractor::RustExtractor;
#[cfg(feature = "lang-typescript")]
use super::typescript::TypeScriptExtractor;

/// Returns a boxed [`Extractor`] for the given [`Language`].
///
/// This is the central dispatch point used by [`extract_file`](super::extract_file)
/// and the indexing pipeline to obtain the appropriate language-specific
/// extractor without the caller needing to know which concrete type to use.
#[must_use]
pub fn get_extractor(language: Language) -> Box<dyn Extractor> {
    match language {
        #[cfg(feature = "lang-c")]
        Language::C => Box::new(CExtractor::new()),
        #[cfg(feature = "lang-rust")]
        Language::Rust => Box::new(RustExtractor::new()),
        #[cfg(feature = "lang-fortran")]
        Language::Fortran => Box::new(FortranExtractor::new()),
        #[cfg(feature = "lang-python")]
        Language::Python => Box::new(PythonExtractor::new()),
        #[cfg(feature = "lang-typescript")]
        Language::TypeScript => Box::new(TypeScriptExtractor::new()),
        #[cfg(feature = "lang-go")]
        Language::Go => Box::new(GoExtractor::new()),
        #[cfg(feature = "lang-java")]
        Language::Java => Box::new(JavaExtractor::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "lang-c")]
    #[test]
    fn get_extractor_returns_c_for_c() {
        let ext = get_extractor(Language::C);
        assert_eq!(ext.language(), Language::C);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn get_extractor_returns_rust_for_rust() {
        let ext = get_extractor(Language::Rust);
        assert_eq!(ext.language(), Language::Rust);
    }

    #[cfg(feature = "lang-fortran")]
    #[test]
    fn get_extractor_returns_fortran_for_fortran() {
        let ext = get_extractor(Language::Fortran);
        assert_eq!(ext.language(), Language::Fortran);
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn get_extractor_returns_python_for_python() {
        let ext = get_extractor(Language::Python);
        assert_eq!(ext.language(), Language::Python);
    }

    #[cfg(feature = "lang-typescript")]
    #[test]
    fn get_extractor_returns_typescript_for_typescript() {
        let ext = get_extractor(Language::TypeScript);
        assert_eq!(ext.language(), Language::TypeScript);
    }

    #[cfg(feature = "lang-go")]
    #[test]
    fn get_extractor_returns_go_for_go() {
        let ext = get_extractor(Language::Go);
        assert_eq!(ext.language(), Language::Go);
    }

    #[cfg(feature = "lang-java")]
    #[test]
    fn get_extractor_returns_java_for_java() {
        let ext = get_extractor(Language::Java);
        assert_eq!(ext.language(), Language::Java);
    }

    #[test]
    fn get_extractor_returns_correct_language_for_all_variants() {
        for lang in Language::all() {
            let ext = get_extractor(lang);
            assert_eq!(
                ext.language(),
                lang,
                "dispatcher should return extractor for {lang}"
            );
        }
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn get_extractor_can_extract_simple_source() {
        let ext = get_extractor(Language::Rust);
        let result = ext.extract("fn main() {}", "test.rs", "proj").unwrap();
        assert_eq!(result.language, Language::Rust);
        assert!(!result.nodes.is_empty(), "should extract at least one node");
    }

    #[cfg(feature = "lang-c")]
    #[test]
    fn get_extractor_returns_send_sync_trait_object() {
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        let ext = get_extractor(Language::C);
        assert_send_sync(&ext);
    }

    #[test]
    fn get_extractor_can_be_used_in_collection() {
        let all_langs = Language::all();
        let extractors: Vec<Box<dyn Extractor>> = all_langs
            .iter()
            .map(|&lang| get_extractor(lang))
            .collect();
        assert_eq!(extractors.len(), all_langs.len());
        for (i, lang) in all_langs.iter().enumerate() {
            assert_eq!(extractors[i].language(), *lang);
        }
    }
}
