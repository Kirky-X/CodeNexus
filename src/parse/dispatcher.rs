// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Language extractor dispatcher (Factory pattern, ADR-003).
//!
//! Maps a [`Language`] to the corresponding [`Extractor`] implementation,
//! enabling language-agnostic extraction in the indexing pipeline.

use crate::model::Language;

#[cfg(feature = "lang-bash")]
use super::bash::BashExtractor;
#[cfg(feature = "lang-c")]
use super::c::CExtractor;
#[cfg(feature = "lang-cpp")]
use super::cpp::CppExtractor;
#[cfg(feature = "lang-csharp")]
use super::csharp::CSharpExtractor;
#[cfg(feature = "lang-css")]
use super::css::CssExtractor;
use super::extractor::Extractor;
#[cfg(feature = "lang-fortran")]
use super::fortran::FortranExtractor;
#[cfg(feature = "lang-go")]
use super::go::GoExtractor;
#[cfg(feature = "lang-haskell")]
use super::haskell::HaskellExtractor;
#[cfg(feature = "lang-html")]
use super::html::HtmlExtractor;
#[cfg(feature = "lang-java")]
use super::java::JavaExtractor;
#[cfg(feature = "lang-javascript")]
use super::javascript::JavaScriptExtractor;
#[cfg(feature = "lang-json")]
use super::json::JsonExtractor;
#[cfg(feature = "lang-ocaml")]
use super::ocaml::OCamlExtractor;
#[cfg(feature = "lang-php")]
use super::php::PhpExtractor;
#[cfg(feature = "lang-python")]
use super::python::PythonExtractor;
#[cfg(feature = "lang-regex")]
use super::regex::RegexExtractor;
#[cfg(feature = "lang-ruby")]
use super::ruby::RubyExtractor;
#[cfg(feature = "lang-rust")]
use super::rust_extractor::RustExtractor;
#[cfg(feature = "lang-scala")]
use super::scala::ScalaExtractor;
#[cfg(feature = "lang-typescript")]
use super::typescript::TypeScriptExtractor;
#[cfg(feature = "lang-verilog")]
use super::verilog::VerilogExtractor;

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
        #[cfg(feature = "lang-cpp")]
        Language::Cpp => Box::new(CppExtractor::new()),
        #[cfg(feature = "lang-javascript")]
        Language::JavaScript => Box::new(JavaScriptExtractor::new()),
        #[cfg(feature = "lang-ruby")]
        Language::Ruby => Box::new(RubyExtractor::new()),
        #[cfg(feature = "lang-haskell")]
        Language::Haskell => Box::new(HaskellExtractor::new()),
        #[cfg(feature = "lang-ocaml")]
        Language::OCaml => Box::new(OCamlExtractor::new()),
        #[cfg(feature = "lang-scala")]
        Language::Scala => Box::new(ScalaExtractor::new()),
        #[cfg(feature = "lang-php")]
        Language::Php => Box::new(PhpExtractor::new()),
        #[cfg(feature = "lang-csharp")]
        Language::CSharp => Box::new(CSharpExtractor::new()),
        #[cfg(feature = "lang-bash")]
        Language::Bash => Box::new(BashExtractor::new()),
        #[cfg(feature = "lang-html")]
        Language::Html => Box::new(HtmlExtractor::new()),
        #[cfg(feature = "lang-css")]
        Language::Css => Box::new(CssExtractor::new()),
        #[cfg(feature = "lang-json")]
        Language::Json => Box::new(JsonExtractor::new()),
        #[cfg(feature = "lang-regex")]
        Language::Regex => Box::new(RegexExtractor::new()),
        #[cfg(feature = "lang-verilog")]
        Language::Verilog => Box::new(VerilogExtractor::new()),
        #[allow(unreachable_patterns)]
        _ => panic!(
            "no tree-sitter extractor compiled for language '{language}'; \
             enable the corresponding lang-* Cargo feature"
        ),
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

    #[cfg(feature = "lang-cpp")]
    #[test]
    fn get_extractor_returns_cpp_for_cpp() {
        let ext = get_extractor(Language::Cpp);
        assert_eq!(ext.language(), Language::Cpp);
    }

    #[cfg(feature = "lang-javascript")]
    #[test]
    fn get_extractor_returns_javascript_for_javascript() {
        let ext = get_extractor(Language::JavaScript);
        assert_eq!(ext.language(), Language::JavaScript);
    }

    #[cfg(feature = "lang-ruby")]
    #[test]
    fn get_extractor_returns_ruby_for_ruby() {
        let ext = get_extractor(Language::Ruby);
        assert_eq!(ext.language(), Language::Ruby);
    }

    #[cfg(feature = "lang-haskell")]
    #[test]
    fn get_extractor_returns_haskell_for_haskell() {
        let ext = get_extractor(Language::Haskell);
        assert_eq!(ext.language(), Language::Haskell);
    }

    #[cfg(feature = "lang-ocaml")]
    #[test]
    fn get_extractor_returns_ocaml_for_ocaml() {
        let ext = get_extractor(Language::OCaml);
        assert_eq!(ext.language(), Language::OCaml);
    }

    #[cfg(feature = "lang-scala")]
    #[test]
    fn get_extractor_returns_scala_for_scala() {
        let ext = get_extractor(Language::Scala);
        assert_eq!(ext.language(), Language::Scala);
    }

    #[cfg(feature = "lang-php")]
    #[test]
    fn get_extractor_returns_php_for_php() {
        let ext = get_extractor(Language::Php);
        assert_eq!(ext.language(), Language::Php);
    }

    #[cfg(feature = "lang-csharp")]
    #[test]
    fn get_extractor_returns_csharp_for_csharp() {
        let ext = get_extractor(Language::CSharp);
        assert_eq!(ext.language(), Language::CSharp);
    }

    #[cfg(feature = "lang-bash")]
    #[test]
    fn get_extractor_returns_bash_for_bash() {
        let ext = get_extractor(Language::Bash);
        assert_eq!(ext.language(), Language::Bash);
    }

    #[cfg(feature = "lang-html")]
    #[test]
    fn get_extractor_returns_html_for_html() {
        let ext = get_extractor(Language::Html);
        assert_eq!(ext.language(), Language::Html);
    }

    #[cfg(feature = "lang-css")]
    #[test]
    fn get_extractor_returns_css_for_css() {
        let ext = get_extractor(Language::Css);
        assert_eq!(ext.language(), Language::Css);
    }

    #[cfg(feature = "lang-json")]
    #[test]
    fn get_extractor_returns_json_for_json() {
        let ext = get_extractor(Language::Json);
        assert_eq!(ext.language(), Language::Json);
    }

    #[cfg(feature = "lang-regex")]
    #[test]
    fn get_extractor_returns_regex_for_regex() {
        let ext = get_extractor(Language::Regex);
        assert_eq!(ext.language(), Language::Regex);
    }

    #[cfg(feature = "lang-verilog")]
    #[test]
    fn get_extractor_returns_verilog_for_verilog() {
        let ext = get_extractor(Language::Verilog);
        assert_eq!(ext.language(), Language::Verilog);
    }

    #[test]
    fn get_extractor_returns_correct_language_for_all_variants() {
        for lang in Language::compiled() {
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
        let all_langs = Language::compiled();
        let extractors: Vec<Box<dyn Extractor>> =
            all_langs.iter().map(|&lang| get_extractor(lang)).collect();
        assert_eq!(extractors.len(), all_langs.len());
        for (i, lang) in all_langs.iter().enumerate() {
            assert_eq!(extractors[i].language(), *lang);
        }
    }
}
