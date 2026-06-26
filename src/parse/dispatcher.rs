// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Language extractor dispatcher (Factory pattern, ADR-003).
//!
//! Maps a [`Language`] to the corresponding [`Extractor`] implementation,
//! enabling language-agnostic extraction in the indexing pipeline.

use crate::model::Language;

use super::c::CExtractor;
use super::extractor::Extractor;
use super::fortran::FortranExtractor;
use super::python::PythonExtractor;
use super::rust_extractor::RustExtractor;
use super::typescript::TypeScriptExtractor;

/// Returns a boxed [`Extractor`] for the given [`Language`].
///
/// This is the central dispatch point used by [`extract_file`](super::extract_file)
/// and the indexing pipeline to obtain the appropriate language-specific
/// extractor without the caller needing to know which concrete type to use.
#[must_use]
pub fn get_extractor(language: Language) -> Box<dyn Extractor> {
    match language {
        Language::C => Box::new(CExtractor::new()),
        Language::Rust => Box::new(RustExtractor::new()),
        Language::Fortran => Box::new(FortranExtractor::new()),
        Language::Python => Box::new(PythonExtractor::new()),
        Language::TypeScript => Box::new(TypeScriptExtractor::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_extractor_returns_c_for_c() {
        let ext = get_extractor(Language::C);
        assert_eq!(ext.language(), Language::C);
    }

    #[test]
    fn get_extractor_returns_rust_for_rust() {
        let ext = get_extractor(Language::Rust);
        assert_eq!(ext.language(), Language::Rust);
    }

    #[test]
    fn get_extractor_returns_fortran_for_fortran() {
        let ext = get_extractor(Language::Fortran);
        assert_eq!(ext.language(), Language::Fortran);
    }

    #[test]
    fn get_extractor_returns_python_for_python() {
        let ext = get_extractor(Language::Python);
        assert_eq!(ext.language(), Language::Python);
    }

    #[test]
    fn get_extractor_returns_typescript_for_typescript() {
        let ext = get_extractor(Language::TypeScript);
        assert_eq!(ext.language(), Language::TypeScript);
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

    #[test]
    fn get_extractor_can_extract_simple_source() {
        let ext = get_extractor(Language::Rust);
        let result = ext.extract("fn main() {}", "test.rs", "proj").unwrap();
        assert_eq!(result.language, Language::Rust);
        assert!(!result.nodes.is_empty(), "should extract at least one node");
    }

    #[test]
    fn get_extractor_returns_send_sync_trait_object() {
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        let ext = get_extractor(Language::C);
        assert_send_sync(&ext);
    }

    #[test]
    fn get_extractor_can_be_used_in_collection() {
        let extractors: Vec<Box<dyn Extractor>> = Language::all()
            .iter()
            .map(|&lang| get_extractor(lang))
            .collect();
        assert_eq!(extractors.len(), 5);
        for (i, lang) in Language::all().iter().enumerate() {
            assert_eq!(extractors[i].language(), *lang);
        }
    }
}
