// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Factory for creating tree-sitter parsers per language (Factory pattern, ADR-003).
//!
//! [`ParserFactory`] maps each CodeNexus [`Language`] to its tree-sitter grammar
//! and creates configured [`Parser`] instances.

use tree_sitter::{Language as TsLanguage, Parser};

use crate::model::Language;

use super::error::{ParseError, Result};

/// Factory that creates tree-sitter [`Parser`]s and [`TsLanguage`]s for the
/// five CodeNexus source languages (Factory pattern, ADR-003).
///
/// Language mapping:
/// - [`Language::C`] → `tree_sitter_c::LANGUAGE`
/// - [`Language::Rust`] → `tree_sitter_rust::LANGUAGE`
/// - [`Language::Fortran`] → `tree_sitter_fortran::LANGUAGE`
/// - [`Language::Python`] → `tree_sitter_python::LANGUAGE`
/// - [`Language::TypeScript`] → `tree_sitter_typescript::LANGUAGE_TYPESCRIPT`
pub struct ParserFactory;

impl ParserFactory {
    /// Returns the tree-sitter [`TsLanguage`] for the given CodeNexus [`Language`].
    ///
    /// Each grammar crate exposes a `LANGUAGE` constant (a
    /// `tree_sitter_language::LanguageFn`) that is converted to a
    /// [`TsLanguage`] via `.into()` (i.e. [`TsLanguage::new`]).
    pub fn create_language(lang: Language) -> Result<TsLanguage> {
        let ts_lang = match lang {
            #[cfg(feature = "lang-c")]
            Language::C => tree_sitter_c::LANGUAGE.into(),
            #[cfg(feature = "lang-rust")]
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
            #[cfg(feature = "lang-fortran")]
            Language::Fortran => tree_sitter_fortran::LANGUAGE.into(),
            #[cfg(feature = "lang-python")]
            Language::Python => tree_sitter_python::LANGUAGE.into(),
            #[cfg(feature = "lang-typescript")]
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            #[cfg(feature = "lang-go")]
            Language::Go => tree_sitter_go::LANGUAGE.into(),
            #[cfg(feature = "lang-java")]
            Language::Java => tree_sitter_java::LANGUAGE.into(),
            #[cfg(feature = "lang-cpp")]
            Language::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        };
        Ok(ts_lang)
    }

    /// Creates a new [`Parser`] configured for the given language.
    ///
    /// The parser is ready to call `.parse()` on source text immediately.
    pub fn create_parser(lang: Language) -> Result<Parser> {
        let ts_lang = Self::create_language(lang)?;
        let mut parser = Parser::new();
        parser
            .set_language(&ts_lang)
            .map_err(|source| ParseError::LanguageSet {
                language: lang.to_string(),
                source,
            })?;
        Ok(parser)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "lang-c")]
    #[test]
    fn create_language_c_succeeds() {
        let lang = ParserFactory::create_language(Language::C);
        assert!(lang.is_ok(), "C language should be supported");
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn create_language_rust_succeeds() {
        let lang = ParserFactory::create_language(Language::Rust);
        assert!(lang.is_ok(), "Rust language should be supported");
    }

    #[cfg(feature = "lang-fortran")]
    #[test]
    fn create_language_fortran_succeeds() {
        let lang = ParserFactory::create_language(Language::Fortran);
        assert!(lang.is_ok(), "Fortran language should be supported");
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn create_language_python_succeeds() {
        let lang = ParserFactory::create_language(Language::Python);
        assert!(lang.is_ok(), "Python language should be supported");
    }

    #[cfg(feature = "lang-typescript")]
    #[test]
    fn create_language_typescript_succeeds() {
        let lang = ParserFactory::create_language(Language::TypeScript);
        assert!(lang.is_ok(), "TypeScript language should be supported");
    }

    #[cfg(feature = "lang-go")]
    #[test]
    fn create_language_go_succeeds() {
        let lang = ParserFactory::create_language(Language::Go);
        assert!(lang.is_ok(), "Go language should be supported");
    }

    #[cfg(feature = "lang-java")]
    #[test]
    fn create_language_java_succeeds() {
        let lang = ParserFactory::create_language(Language::Java);
        assert!(lang.is_ok(), "Java language should be supported");
    }

    #[cfg(feature = "lang-cpp")]
    #[test]
    fn create_language_cpp_succeeds() {
        let lang = ParserFactory::create_language(Language::Cpp);
        assert!(lang.is_ok(), "C++ language should be supported");
    }

    #[test]
    fn create_language_all_variants_succeed() {
        for lang in Language::all() {
            let result = ParserFactory::create_language(lang);
            assert!(result.is_ok(), "create_language should succeed for {lang}");
        }
    }

    #[cfg(feature = "lang-c")]
    #[test]
    fn create_parser_c_succeeds() {
        let parser = ParserFactory::create_parser(Language::C);
        assert!(parser.is_ok(), "C parser should be creatable");
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn create_parser_rust_succeeds() {
        let parser = ParserFactory::create_parser(Language::Rust);
        assert!(parser.is_ok(), "Rust parser should be creatable");
    }

    #[cfg(feature = "lang-fortran")]
    #[test]
    fn create_parser_fortran_succeeds() {
        let parser = ParserFactory::create_parser(Language::Fortran);
        assert!(parser.is_ok(), "Fortran parser should be creatable");
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn create_parser_python_succeeds() {
        let parser = ParserFactory::create_parser(Language::Python);
        assert!(parser.is_ok(), "Python parser should be creatable");
    }

    #[cfg(feature = "lang-typescript")]
    #[test]
    fn create_parser_typescript_succeeds() {
        let parser = ParserFactory::create_parser(Language::TypeScript);
        assert!(parser.is_ok(), "TypeScript parser should be creatable");
    }

    #[cfg(feature = "lang-go")]
    #[test]
    fn create_parser_go_succeeds() {
        let parser = ParserFactory::create_parser(Language::Go);
        assert!(parser.is_ok(), "Go parser should be creatable");
    }

    #[cfg(feature = "lang-java")]
    #[test]
    fn create_parser_java_succeeds() {
        let parser = ParserFactory::create_parser(Language::Java);
        assert!(parser.is_ok(), "Java parser should be creatable");
    }

    #[cfg(feature = "lang-cpp")]
    #[test]
    fn create_parser_cpp_succeeds() {
        let parser = ParserFactory::create_parser(Language::Cpp);
        assert!(parser.is_ok(), "C++ parser should be creatable");
    }

    #[test]
    fn create_parser_all_languages_succeed() {
        for lang in Language::all() {
            let result = ParserFactory::create_parser(lang);
            assert!(result.is_ok(), "create_parser should succeed for {lang}");
        }
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn create_parser_returns_usable_parser() {
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse("fn main() {}", None);
        assert!(tree.is_some(), "parser should produce a tree");
    }

    // --- Parsing smoke tests for each language ---

    #[cfg(feature = "lang-rust")]
    #[test]
    fn parse_simple_rust_file() {
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser
            .parse("fn main() {}", None)
            .expect("Rust parse failed");
        assert!(
            !tree.root_node().has_error(),
            "Rust source should parse without errors"
        );
    }

    #[cfg(feature = "lang-c")]
    #[test]
    fn parse_simple_c_file() {
        let mut parser = ParserFactory::create_parser(Language::C).unwrap();
        let tree = parser
            .parse("int main() { return 0; }", None)
            .expect("C parse failed");
        assert!(
            !tree.root_node().has_error(),
            "C source should parse without errors"
        );
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn parse_simple_python_file() {
        let mut parser = ParserFactory::create_parser(Language::Python).unwrap();
        let tree = parser
            .parse("def foo(): pass", None)
            .expect("Python parse failed");
        assert!(
            !tree.root_node().has_error(),
            "Python source should parse without errors"
        );
    }

    #[cfg(feature = "lang-fortran")]
    #[test]
    fn parse_simple_fortran_file() {
        let mut parser = ParserFactory::create_parser(Language::Fortran).unwrap();
        let tree = parser
            .parse("subroutine foo()\nend subroutine", None)
            .expect("Fortran parse failed");
        assert!(
            !tree.root_node().has_error(),
            "Fortran source should parse without errors"
        );
    }

    #[cfg(feature = "lang-typescript")]
    #[test]
    fn parse_simple_typescript_file() {
        let mut parser = ParserFactory::create_parser(Language::TypeScript).unwrap();
        let tree = parser
            .parse("function foo(): void {}", None)
            .expect("TypeScript parse failed");
        assert!(
            !tree.root_node().has_error(),
            "TypeScript source should parse without errors"
        );
    }

    #[cfg(feature = "lang-go")]
    #[test]
    fn parse_simple_go_file() {
        let mut parser = ParserFactory::create_parser(Language::Go).unwrap();
        let tree = parser
            .parse("package main\nfunc foo() {}\n", None)
            .expect("Go parse failed");
        assert!(
            !tree.root_node().has_error(),
            "Go source should parse without errors"
        );
    }

    #[cfg(feature = "lang-java")]
    #[test]
    fn parse_simple_java_file() {
        let mut parser = ParserFactory::create_parser(Language::Java).unwrap();
        let tree = parser
            .parse("class Foo { void bar() {} }\n", None)
            .expect("Java parse failed");
        assert!(
            !tree.root_node().has_error(),
            "Java source should parse without errors"
        );
    }

    #[cfg(feature = "lang-cpp")]
    #[test]
    fn parse_simple_cpp_file() {
        let mut parser = ParserFactory::create_parser(Language::Cpp).unwrap();
        let tree = parser
            .parse("int add(int a, int b) { return a + b; }\n", None)
            .expect("C++ parse failed");
        assert!(
            !tree.root_node().has_error(),
            "C++ source should parse without errors"
        );
    }

    // --- Invalid / unsupported language handling ---

    #[test]
    fn unsupported_language_error_is_constructible_and_displayable() {
        // Language is a closed enum, so all variants are supported. However,
        // the UnsupportedLanguage error variant exists for API completeness
        // and future language additions. Verify it can be constructed and
        // displayed correctly.
        let err = ParseError::UnsupportedLanguage("java".to_string());
        let msg = err.to_string();
        assert!(
            msg.contains("java"),
            "error message should contain the language name: {msg}"
        );
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn create_language_returns_distinct_objects() {
        // Each call to create_language should produce a valid, independent
        // tree-sitter Language (internally ref-counted).
        let lang1 = ParserFactory::create_language(Language::Rust).unwrap();
        let lang2 = ParserFactory::create_language(Language::Rust).unwrap();
        // Both should have the same name.
        assert_eq!(lang1.name(), lang2.name());
    }

    #[cfg(feature = "lang-c")]
    #[test]
    fn create_parser_produces_independent_parsers() {
        let mut p1 = ParserFactory::create_parser(Language::C).unwrap();
        let mut p2 = ParserFactory::create_parser(Language::C).unwrap();
        let t1 = p1.parse("int a;", None).unwrap();
        let t2 = p2.parse("int b;", None).unwrap();
        // Different source → different root kind is not guaranteed, but both
        // should parse without error.
        assert!(!t1.root_node().has_error());
        assert!(!t2.root_node().has_error());
    }
}
