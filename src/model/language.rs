// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Language enum representing the source languages supported by CodeNexus
//! (DDD §7.3). Variants are feature-gated by `lang-*` Cargo features; the
//! default `full` preset enables 21 languages.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The source languages supported by CodeNexus (DDD §7.3).
///
/// Each variant is gated by a `lang-*` Cargo feature (unified-architecture
/// Phase 1). The set of available variants therefore depends on the enabled
/// features; use [`Language::all()`] to enumerate the variants compiled into
/// the current build rather than assuming a fixed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    #[cfg(feature = "lang-c")]
    C,
    #[cfg(feature = "lang-rust")]
    Rust,
    #[cfg(feature = "lang-fortran")]
    Fortran,
    #[cfg(feature = "lang-python")]
    Python,
    #[cfg(feature = "lang-typescript")]
    TypeScript,
    #[cfg(feature = "lang-go")]
    Go,
    #[cfg(feature = "lang-java")]
    Java,
    #[cfg(feature = "lang-cpp")]
    Cpp,
    #[cfg(feature = "lang-javascript")]
    JavaScript,
    #[cfg(feature = "lang-ruby")]
    Ruby,
    #[cfg(feature = "lang-haskell")]
    Haskell,
    #[cfg(feature = "lang-ocaml")]
    OCaml,
    #[cfg(feature = "lang-scala")]
    Scala,
    #[cfg(feature = "lang-php")]
    Php,
    #[cfg(feature = "lang-csharp")]
    CSharp,
    #[cfg(feature = "lang-bash")]
    Bash,
    #[cfg(feature = "lang-html")]
    Html,
    #[cfg(feature = "lang-css")]
    Css,
    #[cfg(feature = "lang-json")]
    Json,
    #[cfg(feature = "lang-regex")]
    Regex,
    #[cfg(feature = "lang-verilog")]
    Verilog,
}

impl Language {
    /// Returns all variants compiled into this build, in declaration order.
    ///
    /// The length varies with the enabled `lang-*` features (1–5 variants),
    /// so callers must not assume a fixed length. Returns a `Vec<Language>`
    /// rather than a fixed-size array for this reason.
    #[must_use]
    pub fn all() -> Vec<Language> {
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
            #[cfg(feature = "lang-c")]
            Language::C => &["c", "h"],
            #[cfg(feature = "lang-rust")]
            Language::Rust => &["rs"],
            #[cfg(feature = "lang-fortran")]
            Language::Fortran => &["f90", "f", "f95"],
            #[cfg(feature = "lang-python")]
            Language::Python => &["py"],
            #[cfg(feature = "lang-typescript")]
            Language::TypeScript => &["ts", "tsx"],
            #[cfg(feature = "lang-go")]
            Language::Go => &["go"],
            #[cfg(feature = "lang-java")]
            Language::Java => &["java"],
            #[cfg(feature = "lang-cpp")]
            Language::Cpp => &["cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx", "h++"],
            #[cfg(feature = "lang-javascript")]
            Language::JavaScript => &["js", "jsx", "mjs", "cjs"],
            #[cfg(feature = "lang-ruby")]
            Language::Ruby => &["rb"],
            #[cfg(feature = "lang-haskell")]
            Language::Haskell => &["hs"],
            #[cfg(feature = "lang-ocaml")]
            Language::OCaml => &["ml"],
            #[cfg(feature = "lang-scala")]
            Language::Scala => &["scala", "sbt"],
            #[cfg(feature = "lang-php")]
            Language::Php => &["php"],
            #[cfg(feature = "lang-csharp")]
            Language::CSharp => &["cs", "csx"],
            #[cfg(feature = "lang-bash")]
            Language::Bash => &["sh", "bash"],
            #[cfg(feature = "lang-html")]
            Language::Html => &["html", "htm"],
            #[cfg(feature = "lang-css")]
            Language::Css => &["css", "scss"],
            #[cfg(feature = "lang-json")]
            Language::Json => &["json", "jsonc"],
            #[cfg(feature = "lang-regex")]
            Language::Regex => &["regex"],
            #[cfg(feature = "lang-verilog")]
            Language::Verilog => &["v", "sv"],
        }
    }

    /// Maps a file extension (without the leading dot) to a language.
    ///
    /// Returns `None` for unsupported extensions (or extensions whose language
    /// is not compiled into this build).
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<Language> {
        match ext.to_lowercase().as_str() {
            #[cfg(feature = "lang-c")]
            "c" | "h" => Some(Language::C),
            #[cfg(feature = "lang-rust")]
            "rs" => Some(Language::Rust),
            #[cfg(feature = "lang-fortran")]
            "f90" | "f" | "f95" => Some(Language::Fortran),
            #[cfg(feature = "lang-python")]
            "py" => Some(Language::Python),
            #[cfg(feature = "lang-typescript")]
            "ts" | "tsx" => Some(Language::TypeScript),
            #[cfg(feature = "lang-go")]
            "go" => Some(Language::Go),
            #[cfg(feature = "lang-java")]
            "java" => Some(Language::Java),
            #[cfg(feature = "lang-cpp")]
            "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" => Some(Language::Cpp),
            #[cfg(feature = "lang-javascript")]
            "js" | "jsx" | "mjs" | "cjs" => Some(Language::JavaScript),
            #[cfg(feature = "lang-ruby")]
            "rb" => Some(Language::Ruby),
            #[cfg(feature = "lang-haskell")]
            "hs" => Some(Language::Haskell),
            #[cfg(feature = "lang-ocaml")]
            "ml" => Some(Language::OCaml),
            #[cfg(feature = "lang-scala")]
            "scala" | "sbt" => Some(Language::Scala),
            #[cfg(feature = "lang-php")]
            "php" => Some(Language::Php),
            #[cfg(feature = "lang-csharp")]
            "cs" | "csx" => Some(Language::CSharp),
            #[cfg(feature = "lang-bash")]
            "sh" | "bash" => Some(Language::Bash),
            #[cfg(feature = "lang-html")]
            "html" | "htm" => Some(Language::Html),
            #[cfg(feature = "lang-css")]
            "css" | "scss" => Some(Language::Css),
            #[cfg(feature = "lang-json")]
            "json" | "jsonc" => Some(Language::Json),
            #[cfg(feature = "lang-regex")]
            "regex" => Some(Language::Regex),
            #[cfg(feature = "lang-verilog")]
            "v" | "sv" => Some(Language::Verilog),
            _ => None,
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "lang-c")]
            Language::C => f.write_str("c"),
            #[cfg(feature = "lang-rust")]
            Language::Rust => f.write_str("rust"),
            #[cfg(feature = "lang-fortran")]
            Language::Fortran => f.write_str("fortran"),
            #[cfg(feature = "lang-python")]
            Language::Python => f.write_str("python"),
            #[cfg(feature = "lang-typescript")]
            Language::TypeScript => f.write_str("typescript"),
            #[cfg(feature = "lang-go")]
            Language::Go => f.write_str("go"),
            #[cfg(feature = "lang-java")]
            Language::Java => f.write_str("java"),
            #[cfg(feature = "lang-cpp")]
            Language::Cpp => f.write_str("cpp"),
            #[cfg(feature = "lang-javascript")]
            Language::JavaScript => f.write_str("javascript"),
            #[cfg(feature = "lang-ruby")]
            Language::Ruby => f.write_str("ruby"),
            #[cfg(feature = "lang-haskell")]
            Language::Haskell => f.write_str("haskell"),
            #[cfg(feature = "lang-ocaml")]
            Language::OCaml => f.write_str("ocaml"),
            #[cfg(feature = "lang-scala")]
            Language::Scala => f.write_str("scala"),
            #[cfg(feature = "lang-php")]
            Language::Php => f.write_str("php"),
            #[cfg(feature = "lang-csharp")]
            Language::CSharp => f.write_str("csharp"),
            #[cfg(feature = "lang-bash")]
            Language::Bash => f.write_str("bash"),
            #[cfg(feature = "lang-html")]
            Language::Html => f.write_str("html"),
            #[cfg(feature = "lang-css")]
            Language::Css => f.write_str("css"),
            #[cfg(feature = "lang-json")]
            Language::Json => f.write_str("json"),
            #[cfg(feature = "lang-regex")]
            Language::Regex => f.write_str("regex"),
            #[cfg(feature = "lang-verilog")]
            Language::Verilog => f.write_str("verilog"),
        }
    }
}

impl FromStr for Language {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            #[cfg(feature = "lang-c")]
            "c" => Ok(Language::C),
            #[cfg(feature = "lang-rust")]
            "rust" => Ok(Language::Rust),
            #[cfg(feature = "lang-fortran")]
            "fortran" => Ok(Language::Fortran),
            #[cfg(feature = "lang-python")]
            "python" => Ok(Language::Python),
            #[cfg(feature = "lang-typescript")]
            "typescript" => Ok(Language::TypeScript),
            #[cfg(feature = "lang-go")]
            "go" => Ok(Language::Go),
            #[cfg(feature = "lang-java")]
            "java" => Ok(Language::Java),
            #[cfg(feature = "lang-cpp")]
            "cpp" => Ok(Language::Cpp),
            #[cfg(feature = "lang-javascript")]
            "javascript" | "js" => Ok(Language::JavaScript),
            #[cfg(feature = "lang-ruby")]
            "ruby" | "rb" => Ok(Language::Ruby),
            #[cfg(feature = "lang-haskell")]
            "haskell" | "hs" => Ok(Language::Haskell),
            #[cfg(feature = "lang-ocaml")]
            "ocaml" | "ml" => Ok(Language::OCaml),
            #[cfg(feature = "lang-scala")]
            "scala" => Ok(Language::Scala),
            #[cfg(feature = "lang-php")]
            "php" => Ok(Language::Php),
            #[cfg(feature = "lang-csharp")]
            "csharp" | "c#" | "cs" => Ok(Language::CSharp),
            #[cfg(feature = "lang-bash")]
            "bash" | "sh" => Ok(Language::Bash),
            #[cfg(feature = "lang-html")]
            "html" => Ok(Language::Html),
            #[cfg(feature = "lang-css")]
            "css" => Ok(Language::Css),
            #[cfg(feature = "lang-json")]
            "json" => Ok(Language::Json),
            #[cfg(feature = "lang-regex")]
            "regex" => Ok(Language::Regex),
            #[cfg(feature = "lang-verilog")]
            "verilog" | "v" => Ok(Language::Verilog),
            other => Err(format!("unknown Language: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: the number of variants depends on the enabled `lang-*` features,
    // so we only assert non-emptiness (at least one language is compiled in).
    #[test]
    fn has_at_least_one_variant() {
        assert!(!Language::all().is_empty());
    }

    #[cfg(all(
        feature = "lang-c",
        feature = "lang-rust",
        feature = "lang-fortran",
        feature = "lang-python",
        feature = "lang-typescript"
    ))]
    #[test]
    fn display_outputs_lowercase() {
        assert_eq!(Language::C.to_string(), "c");
        assert_eq!(Language::Rust.to_string(), "rust");
        assert_eq!(Language::Fortran.to_string(), "fortran");
        assert_eq!(Language::Python.to_string(), "python");
        assert_eq!(Language::TypeScript.to_string(), "typescript");
    }

    #[cfg(all(
        feature = "lang-c",
        feature = "lang-rust",
        feature = "lang-fortran",
        feature = "lang-python",
        feature = "lang-typescript"
    ))]
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

    #[cfg(all(
        feature = "lang-c",
        feature = "lang-rust",
        feature = "lang-fortran",
        feature = "lang-python",
        feature = "lang-typescript"
    ))]
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

    #[cfg(all(
        feature = "lang-rust",
        feature = "lang-fortran",
        feature = "lang-typescript"
    ))]
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
        #[cfg(not(feature = "lang-java"))]
        assert!("java".parse::<Language>().is_err());
        #[cfg(not(feature = "lang-go"))]
        assert!("go".parse::<Language>().is_err());
        assert!("".parse::<Language>().is_err());
        assert!("c++".parse::<Language>().is_err());
    }

    #[test]
    fn from_str_error_message_contains_input() {
        // "java" is a valid language when lang-java is enabled, so use a
        // guaranteed-unknown string for the error-message check.
        let err = "cobol".parse::<Language>().unwrap_err();
        assert!(err.contains("cobol"));
    }

    #[cfg(all(
        feature = "lang-c",
        feature = "lang-rust",
        feature = "lang-fortran",
        feature = "lang-python",
        feature = "lang-typescript"
    ))]
    #[test]
    fn extensions_returns_supported_extensions() {
        assert_eq!(Language::C.extensions(), &["c", "h"]);
        assert_eq!(Language::Rust.extensions(), &["rs"]);
        assert_eq!(Language::Fortran.extensions(), &["f90", "f", "f95"]);
        assert_eq!(Language::Python.extensions(), &["py"]);
        assert_eq!(Language::TypeScript.extensions(), &["ts", "tsx"]);
    }

    #[cfg(feature = "lang-c")]
    #[test]
    fn from_extension_maps_c_extensions() {
        assert_eq!(Language::from_extension("c"), Some(Language::C));
        assert_eq!(Language::from_extension("h"), Some(Language::C));
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn from_extension_maps_rust_extension() {
        assert_eq!(Language::from_extension("rs"), Some(Language::Rust));
    }

    #[cfg(feature = "lang-fortran")]
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

    #[cfg(all(
        feature = "lang-c",
        feature = "lang-rust",
        feature = "lang-python",
        feature = "lang-typescript"
    ))]
    #[test]
    fn from_extension_is_case_insensitive() {
        assert_eq!(Language::from_extension("RS"), Some(Language::Rust));
        assert_eq!(Language::from_extension("Py"), Some(Language::Python));
        assert_eq!(Language::from_extension("TS"), Some(Language::TypeScript));
        assert_eq!(Language::from_extension("TSX"), Some(Language::TypeScript));
        assert_eq!(Language::from_extension("C"), Some(Language::C));
        assert_eq!(Language::from_extension("H"), Some(Language::C));
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn from_extension_maps_python_extension() {
        assert_eq!(Language::from_extension("py"), Some(Language::Python));
    }

    #[cfg(feature = "lang-typescript")]
    #[test]
    fn from_extension_maps_typescript_extensions() {
        assert_eq!(Language::from_extension("ts"), Some(Language::TypeScript));
        assert_eq!(Language::from_extension("tsx"), Some(Language::TypeScript));
    }

    #[cfg(feature = "lang-go")]
    #[test]
    fn from_extension_maps_go_extension() {
        assert_eq!(Language::from_extension("go"), Some(Language::Go));
        assert_eq!(Language::from_extension("GO"), Some(Language::Go));
    }

    #[cfg(feature = "lang-java")]
    #[test]
    fn from_extension_maps_java_extension() {
        assert_eq!(Language::from_extension("java"), Some(Language::Java));
        assert_eq!(Language::from_extension("JAVA"), Some(Language::Java));
    }

    #[cfg(feature = "lang-cpp")]
    #[test]
    fn from_extension_maps_cpp_extensions() {
        assert_eq!(Language::from_extension("cpp"), Some(Language::Cpp));
        assert_eq!(Language::from_extension("cc"), Some(Language::Cpp));
        assert_eq!(Language::from_extension("hpp"), Some(Language::Cpp));
        assert_eq!(Language::from_extension("CPP"), Some(Language::Cpp));
    }

    #[test]
    fn from_extension_returns_none_for_unknown() {
        #[cfg(not(feature = "lang-java"))]
        assert_eq!(Language::from_extension("java"), None);
        #[cfg(not(feature = "lang-go"))]
        assert_eq!(Language::from_extension("go"), None);
        #[cfg(not(feature = "lang-cpp"))]
        assert_eq!(Language::from_extension("cpp"), None);
        assert_eq!(Language::from_extension(""), None);
        assert_eq!(Language::from_extension(".rs"), None);
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

    #[cfg(feature = "lang-rust")]
    #[test]
    fn is_copy() {
        let lang = Language::Rust;
        let copied = lang;
        assert_eq!(lang, copied);
    }
}
