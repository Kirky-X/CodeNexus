// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Language enum representing the source languages supported by CodeNexus
//! (DDD §7.3).
//!
//! All 21 variants are always compiled in — the enum is a model concept
//! independent of which tree-sitter parsers are enabled. The `lang-*` Cargo
//! features gate only the parser implementations (in `src/parse/`), not the
//! `Language` type itself. This allows LSP-only builds (no tree-sitter) to
//! still identify languages by extension and store language metadata on nodes.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The source languages supported by CodeNexus (DDD §7.3).
///
/// All variants are always available. Tree-sitter parser support is gated by
/// `lang-*` Cargo features at the parser dispatch layer, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    C,
    Rust,
    Fortran,
    Python,
    TypeScript,
    Go,
    Java,
    Cpp,
    JavaScript,
    Ruby,
    Haskell,
    OCaml,
    Scala,
    Php,
    CSharp,
    Bash,
    Html,
    Css,
    Json,
    Regex,
    Verilog,
}

impl Language {
    /// Returns all variants, in declaration order.
    #[must_use]
    pub fn all() -> Vec<Language> {
        vec![
            Language::C,
            Language::Rust,
            Language::Fortran,
            Language::Python,
            Language::TypeScript,
            Language::Go,
            Language::Java,
            Language::Cpp,
            Language::JavaScript,
            Language::Ruby,
            Language::Haskell,
            Language::OCaml,
            Language::Scala,
            Language::Php,
            Language::CSharp,
            Language::Bash,
            Language::Html,
            Language::Css,
            Language::Json,
            Language::Regex,
            Language::Verilog,
        ]
    }

    /// Returns only variants whose tree-sitter parser is compiled in
    /// (i.e., whose `lang-*` Cargo feature is enabled).
    ///
    /// In an LSP-only build (no `lang-*` features), this returns an empty Vec.
    #[must_use]
    pub fn compiled() -> Vec<Language> {
        vec![
            #[cfg(feature = "lang-c")]
            Language::C,
            #[cfg(feature = "lang-rust")]
            Language::Rust,
            #[cfg(feature = "lang-fortran")]
            Language::Fortran,
            #[cfg(feature = "lang-python")]
            Language::Python,
            #[cfg(feature = "lang-typescript")]
            Language::TypeScript,
            #[cfg(feature = "lang-go")]
            Language::Go,
            #[cfg(feature = "lang-java")]
            Language::Java,
            #[cfg(feature = "lang-cpp")]
            Language::Cpp,
            #[cfg(feature = "lang-javascript")]
            Language::JavaScript,
            #[cfg(feature = "lang-ruby")]
            Language::Ruby,
            #[cfg(feature = "lang-haskell")]
            Language::Haskell,
            #[cfg(feature = "lang-ocaml")]
            Language::OCaml,
            #[cfg(feature = "lang-scala")]
            Language::Scala,
            #[cfg(feature = "lang-php")]
            Language::Php,
            #[cfg(feature = "lang-csharp")]
            Language::CSharp,
            #[cfg(feature = "lang-bash")]
            Language::Bash,
            #[cfg(feature = "lang-html")]
            Language::Html,
            #[cfg(feature = "lang-css")]
            Language::Css,
            #[cfg(feature = "lang-json")]
            Language::Json,
            #[cfg(feature = "lang-regex")]
            Language::Regex,
            #[cfg(feature = "lang-verilog")]
            Language::Verilog,
        ]
    }

    /// Returns the file extensions (without the leading dot) for this language.
    #[must_use]
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Language::C => &["c", "h"],
            Language::Rust => &["rs"],
            Language::Fortran => &["f90", "f", "f95"],
            Language::Python => &["py"],
            Language::TypeScript => &["ts", "tsx"],
            Language::Go => &["go"],
            Language::Java => &["java"],
            Language::Cpp => &["cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx", "h++"],
            Language::JavaScript => &["js", "jsx", "mjs", "cjs"],
            Language::Ruby => &["rb"],
            Language::Haskell => &["hs"],
            Language::OCaml => &["ml"],
            Language::Scala => &["scala", "sbt"],
            Language::Php => &["php"],
            Language::CSharp => &["cs", "csx"],
            Language::Bash => &["sh", "bash"],
            Language::Html => &["html", "htm"],
            Language::Css => &["css", "scss"],
            Language::Json => &["json", "jsonc"],
            Language::Regex => &["regex"],
            Language::Verilog => &["v", "sv"],
        }
    }

    /// Maps a file extension (without the leading dot) to a language.
    ///
    /// Returns `None` for unsupported extensions.
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<Language> {
        match ext.to_lowercase().as_str() {
            "c" | "h" => Some(Language::C),
            "rs" => Some(Language::Rust),
            "f90" | "f" | "f95" => Some(Language::Fortran),
            "py" => Some(Language::Python),
            "ts" | "tsx" => Some(Language::TypeScript),
            "go" => Some(Language::Go),
            "java" => Some(Language::Java),
            "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" => Some(Language::Cpp),
            "js" | "jsx" | "mjs" | "cjs" => Some(Language::JavaScript),
            "rb" => Some(Language::Ruby),
            "hs" => Some(Language::Haskell),
            "ml" => Some(Language::OCaml),
            "scala" | "sbt" => Some(Language::Scala),
            "php" => Some(Language::Php),
            "cs" | "csx" => Some(Language::CSharp),
            "sh" | "bash" => Some(Language::Bash),
            "html" | "htm" => Some(Language::Html),
            "css" | "scss" => Some(Language::Css),
            "json" | "jsonc" => Some(Language::Json),
            "regex" => Some(Language::Regex),
            "v" | "sv" => Some(Language::Verilog),
            _ => None,
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Language::C => f.write_str("c"),
            Language::Rust => f.write_str("rust"),
            Language::Fortran => f.write_str("fortran"),
            Language::Python => f.write_str("python"),
            Language::TypeScript => f.write_str("typescript"),
            Language::Go => f.write_str("go"),
            Language::Java => f.write_str("java"),
            Language::Cpp => f.write_str("cpp"),
            Language::JavaScript => f.write_str("javascript"),
            Language::Ruby => f.write_str("ruby"),
            Language::Haskell => f.write_str("haskell"),
            Language::OCaml => f.write_str("ocaml"),
            Language::Scala => f.write_str("scala"),
            Language::Php => f.write_str("php"),
            Language::CSharp => f.write_str("csharp"),
            Language::Bash => f.write_str("bash"),
            Language::Html => f.write_str("html"),
            Language::Css => f.write_str("css"),
            Language::Json => f.write_str("json"),
            Language::Regex => f.write_str("regex"),
            Language::Verilog => f.write_str("verilog"),
        }
    }
}

impl FromStr for Language {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "c" => Ok(Language::C),
            "rust" => Ok(Language::Rust),
            "fortran" => Ok(Language::Fortran),
            "python" => Ok(Language::Python),
            "typescript" => Ok(Language::TypeScript),
            "go" => Ok(Language::Go),
            "java" => Ok(Language::Java),
            "cpp" => Ok(Language::Cpp),
            "javascript" | "js" => Ok(Language::JavaScript),
            "ruby" | "rb" => Ok(Language::Ruby),
            "haskell" | "hs" => Ok(Language::Haskell),
            "ocaml" | "ml" => Ok(Language::OCaml),
            "scala" => Ok(Language::Scala),
            "php" => Ok(Language::Php),
            "csharp" | "c#" | "cs" => Ok(Language::CSharp),
            "bash" | "sh" => Ok(Language::Bash),
            "html" => Ok(Language::Html),
            "css" => Ok(Language::Css),
            "json" => Ok(Language::Json),
            "regex" => Ok(Language::Regex),
            "verilog" | "v" => Ok(Language::Verilog),
            other => Err(format!("unknown Language: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_all_21_variants() {
        assert_eq!(Language::all().len(), 21);
    }

    #[test]
    fn display_outputs_lowercase() {
        assert_eq!(Language::C.to_string(), "c");
        assert_eq!(Language::Rust.to_string(), "rust");
        assert_eq!(Language::Fortran.to_string(), "fortran");
        assert_eq!(Language::Python.to_string(), "python");
        assert_eq!(Language::TypeScript.to_string(), "typescript");
    }

    #[test]
    fn from_str_parses_lowercase() {
        assert_eq!("c".parse::<Language>().unwrap(), Language::C);
        assert_eq!("rust".parse::<Language>().unwrap(), Language::Rust);
        assert_eq!("fortran".parse::<Language>().unwrap(), Language::Fortran);
        assert_eq!("python".parse::<Language>().unwrap(), Language::Python);
        assert_eq!(
            "typescript".parse::<Language>().unwrap(),
            Language::TypeScript
        );
    }

    #[test]
    fn from_str_parses_uppercase() {
        assert_eq!("C".parse::<Language>().unwrap(), Language::C);
        assert_eq!("RUST".parse::<Language>().unwrap(), Language::Rust);
        assert_eq!("FORTRAN".parse::<Language>().unwrap(), Language::Fortran);
        assert_eq!("PYTHON".parse::<Language>().unwrap(), Language::Python);
        assert_eq!(
            "TYPESCRIPT".parse::<Language>().unwrap(),
            Language::TypeScript
        );
    }

    #[test]
    fn from_str_parses_mixed_case() {
        assert_eq!("Rust".parse::<Language>().unwrap(), Language::Rust);
        assert_eq!("Fortran".parse::<Language>().unwrap(), Language::Fortran);
        assert_eq!(
            "TyPeScRiPt".parse::<Language>().unwrap(),
            Language::TypeScript
        );
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!("cobol".parse::<Language>().is_err());
        assert!("".parse::<Language>().is_err());
        assert!("c++".parse::<Language>().is_err());
    }

    #[test]
    fn from_str_error_message_contains_input() {
        let err = "cobol".parse::<Language>().unwrap_err();
        assert!(err.contains("cobol"));
    }

    #[test]
    fn extensions_returns_supported_extensions() {
        assert_eq!(Language::C.extensions(), &["c", "h"]);
        assert_eq!(Language::Rust.extensions(), &["rs"]);
        assert_eq!(Language::Fortran.extensions(), &["f90", "f", "f95"]);
        assert_eq!(Language::Python.extensions(), &["py"]);
        assert_eq!(Language::TypeScript.extensions(), &["ts", "tsx"]);
    }

    #[test]
    fn from_extension_maps_c_extensions() {
        assert_eq!(Language::from_extension("c"), Some(Language::C));
        assert_eq!(Language::from_extension("h"), Some(Language::C));
    }

    #[test]
    fn from_extension_maps_rust_extension() {
        assert_eq!(Language::from_extension("rs"), Some(Language::Rust));
    }

    #[test]
    fn from_extension_maps_fortran_extensions() {
        assert_eq!(Language::from_extension("f90"), Some(Language::Fortran));
        assert_eq!(Language::from_extension("f"), Some(Language::Fortran));
        assert_eq!(Language::from_extension("f95"), Some(Language::Fortran));
        // 大写扩展名（WRF 等科学计算项目使用 .F/.F90/.F95）
        assert_eq!(Language::from_extension("F"), Some(Language::Fortran));
        assert_eq!(Language::from_extension("F90"), Some(Language::Fortran));
        assert_eq!(Language::from_extension("F95"), Some(Language::Fortran));
    }

    #[test]
    fn from_extension_is_case_insensitive() {
        assert_eq!(Language::from_extension("RS"), Some(Language::Rust));
        assert_eq!(Language::from_extension("Py"), Some(Language::Python));
        assert_eq!(Language::from_extension("TS"), Some(Language::TypeScript));
        assert_eq!(Language::from_extension("TSX"), Some(Language::TypeScript));
        assert_eq!(Language::from_extension("C"), Some(Language::C));
        assert_eq!(Language::from_extension("H"), Some(Language::C));
    }

    #[test]
    fn from_extension_maps_python_extension() {
        assert_eq!(Language::from_extension("py"), Some(Language::Python));
    }

    #[test]
    fn from_extension_maps_typescript_extensions() {
        assert_eq!(Language::from_extension("ts"), Some(Language::TypeScript));
        assert_eq!(Language::from_extension("tsx"), Some(Language::TypeScript));
    }

    #[test]
    fn from_extension_maps_go_extension() {
        assert_eq!(Language::from_extension("go"), Some(Language::Go));
        assert_eq!(Language::from_extension("GO"), Some(Language::Go));
    }

    #[test]
    fn from_extension_maps_java_extension() {
        assert_eq!(Language::from_extension("java"), Some(Language::Java));
        assert_eq!(Language::from_extension("JAVA"), Some(Language::Java));
    }

    #[test]
    fn from_extension_maps_cpp_extensions() {
        assert_eq!(Language::from_extension("cpp"), Some(Language::Cpp));
        assert_eq!(Language::from_extension("cc"), Some(Language::Cpp));
        assert_eq!(Language::from_extension("hpp"), Some(Language::Cpp));
        assert_eq!(Language::from_extension("CPP"), Some(Language::Cpp));
    }

    #[test]
    fn from_extension_returns_none_for_unknown() {
        assert_eq!(Language::from_extension(""), None);
        assert_eq!(Language::from_extension(".rs"), None);
        assert_eq!(Language::from_extension("cobol"), None);
    }

    #[test]
    fn from_extension_covers_all_declared_extensions() {
        for lang in Language::all() {
            for ext in lang.extensions() {
                assert_eq!(
                    Language::from_extension(ext),
                    Some(lang),
                    "extension {ext} should map to {lang}"
                );
            }
        }
    }

    #[test]
    fn display_fromstr_roundtrip() {
        for lang in Language::all() {
            let s = lang.to_string();
            let parsed: Language = s.parse().unwrap();
            assert_eq!(lang, parsed);
        }
    }

    #[test]
    fn serde_roundtrip() {
        for lang in Language::all() {
            let json = serde_json::to_string(&lang).unwrap();
            let parsed: Language = serde_json::from_str(&json).unwrap();
            assert_eq!(lang, parsed);
        }
    }

    #[test]
    fn is_copy() {
        let lang = Language::Rust;
        let copied = lang;
        assert_eq!(lang, copied);
    }

    #[test]
    fn compiled_returns_only_enabled_languages() {
        let compiled = Language::compiled();
        for lang in &compiled {
            assert!(
                Language::all().contains(lang),
                "compiled() returned a language not in all(): {lang}"
            );
        }
    }

    #[test]
    fn compiled_is_subset_of_all() {
        let compiled = Language::compiled();
        let all = Language::all();
        assert!(compiled.len() <= all.len());
    }
}
