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
        match lang {
            #[cfg(feature = "lang-c")]
            Language::C => Ok(tree_sitter_c::LANGUAGE.into()),
            #[cfg(feature = "lang-rust")]
            Language::Rust => Ok(tree_sitter_rust::LANGUAGE.into()),
            #[cfg(feature = "lang-fortran")]
            Language::Fortran => Ok(tree_sitter_fortran::LANGUAGE.into()),
            #[cfg(feature = "lang-python")]
            Language::Python => Ok(tree_sitter_python::LANGUAGE.into()),
            #[cfg(feature = "lang-typescript")]
            Language::TypeScript => Ok(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
            #[cfg(feature = "lang-go")]
            Language::Go => Ok(tree_sitter_go::LANGUAGE.into()),
            #[cfg(feature = "lang-java")]
            Language::Java => Ok(tree_sitter_java::LANGUAGE.into()),
            #[cfg(feature = "lang-cpp")]
            Language::Cpp => Ok(tree_sitter_cpp::LANGUAGE.into()),
            #[cfg(feature = "lang-javascript")]
            Language::JavaScript => Ok(tree_sitter_javascript::LANGUAGE.into()),
            #[cfg(feature = "lang-ruby")]
            Language::Ruby => Ok(tree_sitter_ruby::LANGUAGE.into()),
            #[cfg(feature = "lang-haskell")]
            Language::Haskell => Ok(tree_sitter_haskell::LANGUAGE.into()),
            #[cfg(feature = "lang-ocaml")]
            Language::OCaml => Ok(tree_sitter_ocaml::LANGUAGE_OCAML.into()),
            #[cfg(feature = "lang-scala")]
            Language::Scala => Ok(tree_sitter_scala::LANGUAGE.into()),
            #[cfg(feature = "lang-php")]
            Language::Php => Ok(tree_sitter_php::LANGUAGE_PHP.into()),
            #[cfg(feature = "lang-csharp")]
            Language::CSharp => Ok(tree_sitter_c_sharp::LANGUAGE.into()),
            #[cfg(feature = "lang-bash")]
            Language::Bash => Ok(tree_sitter_bash::LANGUAGE.into()),
            #[cfg(feature = "lang-html")]
            Language::Html => Ok(tree_sitter_html::LANGUAGE.into()),
            #[cfg(feature = "lang-css")]
            Language::Css => Ok(tree_sitter_css::LANGUAGE.into()),
            #[cfg(feature = "lang-json")]
            Language::Json => Ok(tree_sitter_json::LANGUAGE.into()),
            #[cfg(feature = "lang-regex")]
            Language::Regex => Ok(tree_sitter_regex::LANGUAGE.into()),
            #[cfg(feature = "lang-verilog")]
            Language::Verilog => Ok(tree_sitter_verilog::LANGUAGE.into()),
            #[allow(unreachable_patterns)]
            _ => Err(ParseError::UnsupportedLanguage(lang.to_string())),
        }
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
        for lang in Language::compiled() {
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

    #[cfg(feature = "lang-javascript")]
    #[test]
    fn create_language_javascript_succeeds() {
        let lang = ParserFactory::create_language(Language::JavaScript);
        assert!(lang.is_ok(), "JavaScript language should be supported");
    }

    #[cfg(feature = "lang-javascript")]
    #[test]
    fn create_parser_javascript_succeeds() {
        let parser = ParserFactory::create_parser(Language::JavaScript);
        assert!(parser.is_ok(), "JavaScript parser should be creatable");
    }

    #[cfg(feature = "lang-javascript")]
    #[test]
    fn parse_simple_javascript_file() {
        let mut parser = ParserFactory::create_parser(Language::JavaScript).unwrap();
        let tree = parser
            .parse("function foo() { return 1; }\n", None)
            .expect("JavaScript parse failed");
        assert!(
            !tree.root_node().has_error(),
            "JavaScript source should parse without errors"
        );
    }

    #[cfg(feature = "lang-ruby")]
    #[test]
    fn create_language_ruby_succeeds() {
        let lang = ParserFactory::create_language(Language::Ruby);
        assert!(lang.is_ok(), "Ruby language should be supported");
    }

    #[cfg(feature = "lang-ruby")]
    #[test]
    fn create_parser_ruby_succeeds() {
        let parser = ParserFactory::create_parser(Language::Ruby);
        assert!(parser.is_ok(), "Ruby parser should be creatable");
    }

    #[cfg(feature = "lang-ruby")]
    #[test]
    fn parse_simple_ruby_file() {
        let mut parser = ParserFactory::create_parser(Language::Ruby).unwrap();
        let tree = parser
            .parse("def foo\n  1\nend\n", None)
            .expect("Ruby parse failed");
        assert!(
            !tree.root_node().has_error(),
            "Ruby source should parse without errors"
        );
    }

    #[cfg(feature = "lang-haskell")]
    #[test]
    fn create_language_haskell_succeeds() {
        let lang = ParserFactory::create_language(Language::Haskell);
        assert!(lang.is_ok(), "Haskell language should be supported");
    }

    #[cfg(feature = "lang-haskell")]
    #[test]
    fn create_parser_haskell_succeeds() {
        let parser = ParserFactory::create_parser(Language::Haskell);
        assert!(parser.is_ok(), "Haskell parser should be creatable");
    }

    #[cfg(feature = "lang-haskell")]
    #[test]
    fn parse_simple_haskell_file() {
        let mut parser = ParserFactory::create_parser(Language::Haskell).unwrap();
        let tree = parser
            .parse("foo :: Int\nfoo = 1\n", None)
            .expect("Haskell parse failed");
        assert!(
            !tree.root_node().has_error(),
            "Haskell source should parse without errors"
        );
    }

    #[cfg(feature = "lang-ocaml")]
    #[test]
    fn create_language_ocaml_succeeds() {
        let lang = ParserFactory::create_language(Language::OCaml);
        assert!(lang.is_ok(), "OCaml language should be supported");
    }

    #[cfg(feature = "lang-ocaml")]
    #[test]
    fn create_parser_ocaml_succeeds() {
        let parser = ParserFactory::create_parser(Language::OCaml);
        assert!(parser.is_ok(), "OCaml parser should be creatable");
    }

    #[cfg(feature = "lang-ocaml")]
    #[test]
    fn parse_simple_ocaml_file() {
        let mut parser = ParserFactory::create_parser(Language::OCaml).unwrap();
        let tree = parser
            .parse("let foo = 1\n", None)
            .expect("OCaml parse failed");
        assert!(
            !tree.root_node().has_error(),
            "OCaml source should parse without errors"
        );
    }

    #[cfg(feature = "lang-scala")]
    #[test]
    fn create_language_scala_succeeds() {
        let lang = ParserFactory::create_language(Language::Scala);
        assert!(lang.is_ok(), "Scala language should be supported");
    }

    #[cfg(feature = "lang-scala")]
    #[test]
    fn create_parser_scala_succeeds() {
        let parser = ParserFactory::create_parser(Language::Scala);
        assert!(parser.is_ok(), "Scala parser should be creatable");
    }

    #[cfg(feature = "lang-scala")]
    #[test]
    fn parse_simple_scala_file() {
        let mut parser = ParserFactory::create_parser(Language::Scala).unwrap();
        let tree = parser
            .parse("object Foo { def bar(): Int = 1 }\n", None)
            .expect("Scala parse failed");
        assert!(
            !tree.root_node().has_error(),
            "Scala source should parse without errors"
        );
    }

    #[cfg(feature = "lang-php")]
    #[test]
    fn create_language_php_succeeds() {
        let lang = ParserFactory::create_language(Language::Php);
        assert!(lang.is_ok(), "PHP language should be supported");
    }

    #[cfg(feature = "lang-php")]
    #[test]
    fn create_parser_php_succeeds() {
        let parser = ParserFactory::create_parser(Language::Php);
        assert!(parser.is_ok(), "PHP parser should be creatable");
    }

    #[cfg(feature = "lang-php")]
    #[test]
    fn parse_simple_php_file() {
        let mut parser = ParserFactory::create_parser(Language::Php).unwrap();
        let tree = parser
            .parse("<?php\nfunction foo() { return 1; }\n?>\n", None)
            .expect("PHP parse failed");
        assert!(
            !tree.root_node().has_error(),
            "PHP source should parse without errors"
        );
    }

    #[cfg(feature = "lang-csharp")]
    #[test]
    fn create_language_csharp_succeeds() {
        let lang = ParserFactory::create_language(Language::CSharp);
        assert!(lang.is_ok(), "C# language should be supported");
    }

    #[cfg(feature = "lang-csharp")]
    #[test]
    fn create_parser_csharp_succeeds() {
        let parser = ParserFactory::create_parser(Language::CSharp);
        assert!(parser.is_ok(), "C# parser should be creatable");
    }

    #[cfg(feature = "lang-csharp")]
    #[test]
    fn parse_simple_csharp_file() {
        let mut parser = ParserFactory::create_parser(Language::CSharp).unwrap();
        let tree = parser
            .parse("class Foo { void Bar() { } }\n", None)
            .expect("C# parse failed");
        assert!(
            !tree.root_node().has_error(),
            "C# source should parse without errors"
        );
    }

    #[cfg(feature = "lang-bash")]
    #[test]
    fn create_language_bash_succeeds() {
        let lang = ParserFactory::create_language(Language::Bash);
        assert!(lang.is_ok(), "Bash language should be supported");
    }

    #[cfg(feature = "lang-bash")]
    #[test]
    fn create_parser_bash_succeeds() {
        let parser = ParserFactory::create_parser(Language::Bash);
        assert!(parser.is_ok(), "Bash parser should be creatable");
    }

    #[cfg(feature = "lang-bash")]
    #[test]
    fn parse_simple_bash_file() {
        let mut parser = ParserFactory::create_parser(Language::Bash).unwrap();
        let tree = parser
            .parse("foo() { echo hi; }\n", None)
            .expect("Bash parse failed");
        assert!(
            !tree.root_node().has_error(),
            "Bash source should parse without errors"
        );
    }

    #[cfg(feature = "lang-html")]
    #[test]
    fn create_language_html_succeeds() {
        let lang = ParserFactory::create_language(Language::Html);
        assert!(lang.is_ok(), "HTML language should be supported");
    }

    #[cfg(feature = "lang-html")]
    #[test]
    fn create_parser_html_succeeds() {
        let parser = ParserFactory::create_parser(Language::Html);
        assert!(parser.is_ok(), "HTML parser should be creatable");
    }

    #[cfg(feature = "lang-html")]
    #[test]
    fn parse_simple_html_file() {
        let mut parser = ParserFactory::create_parser(Language::Html).unwrap();
        let tree = parser
            .parse("<html><body><p>hi</p></body></html>\n", None)
            .expect("HTML parse failed");
        assert!(
            !tree.root_node().has_error(),
            "HTML source should parse without errors"
        );
    }

    #[cfg(feature = "lang-css")]
    #[test]
    fn create_language_css_succeeds() {
        let lang = ParserFactory::create_language(Language::Css);
        assert!(lang.is_ok(), "CSS language should be supported");
    }

    #[cfg(feature = "lang-css")]
    #[test]
    fn create_parser_css_succeeds() {
        let parser = ParserFactory::create_parser(Language::Css);
        assert!(parser.is_ok(), "CSS parser should be creatable");
    }

    #[cfg(feature = "lang-css")]
    #[test]
    fn parse_simple_css_file() {
        let mut parser = ParserFactory::create_parser(Language::Css).unwrap();
        let tree = parser
            .parse("body { color: red; }\n", None)
            .expect("CSS parse failed");
        assert!(
            !tree.root_node().has_error(),
            "CSS source should parse without errors"
        );
    }

    #[cfg(feature = "lang-json")]
    #[test]
    fn create_language_json_succeeds() {
        let lang = ParserFactory::create_language(Language::Json);
        assert!(lang.is_ok(), "JSON language should be supported");
    }

    #[cfg(feature = "lang-json")]
    #[test]
    fn create_parser_json_succeeds() {
        let parser = ParserFactory::create_parser(Language::Json);
        assert!(parser.is_ok(), "JSON parser should be creatable");
    }

    #[cfg(feature = "lang-json")]
    #[test]
    fn parse_simple_json_file() {
        let mut parser = ParserFactory::create_parser(Language::Json).unwrap();
        let tree = parser
            .parse("{\"key\": \"value\"}\n", None)
            .expect("JSON parse failed");
        assert!(
            !tree.root_node().has_error(),
            "JSON source should parse without errors"
        );
    }

    #[cfg(feature = "lang-regex")]
    #[test]
    fn create_language_regex_succeeds() {
        let lang = ParserFactory::create_language(Language::Regex);
        assert!(lang.is_ok(), "Regex language should be supported");
    }

    #[cfg(feature = "lang-regex")]
    #[test]
    fn create_parser_regex_succeeds() {
        let parser = ParserFactory::create_parser(Language::Regex);
        assert!(parser.is_ok(), "Regex parser should be creatable");
    }

    #[cfg(feature = "lang-regex")]
    #[test]
    fn parse_simple_regex_file() {
        let mut parser = ParserFactory::create_parser(Language::Regex).unwrap();
        let tree = parser
            .parse("abc.*def\n", None)
            .expect("Regex parse failed");
        assert!(
            !tree.root_node().has_error(),
            "Regex source should parse without errors"
        );
    }

    #[cfg(feature = "lang-verilog")]
    #[test]
    fn create_language_verilog_succeeds() {
        let lang = ParserFactory::create_language(Language::Verilog);
        assert!(lang.is_ok(), "Verilog language should be supported");
    }

    #[cfg(feature = "lang-verilog")]
    #[test]
    fn create_parser_verilog_succeeds() {
        let parser = ParserFactory::create_parser(Language::Verilog);
        assert!(parser.is_ok(), "Verilog parser should be creatable");
    }

    #[cfg(feature = "lang-verilog")]
    #[test]
    fn parse_simple_verilog_file() {
        let mut parser = ParserFactory::create_parser(Language::Verilog).unwrap();
        let tree = parser
            .parse("module foo();\nendmodule\n", None)
            .expect("Verilog parse failed");
        assert!(
            !tree.root_node().has_error(),
            "Verilog source should parse without errors"
        );
    }

    #[test]
    fn create_parser_all_languages_succeed() {
        for lang in Language::compiled() {
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
