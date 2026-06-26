// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Language enum representing the 5 supported source languages (DDD §7.3).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The 5 source languages supported by CodeNexus (DDD §7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    C,
    Rust,
    Fortran,
    Python,
    TypeScript,
}

impl Language {
    /// Returns all variants in declaration order.
    #[must_use]
    pub const fn all() -> [Language; 5] {
        [
            Language::C,
            Language::Rust,
            Language::Fortran,
            Language::Python,
            Language::TypeScript,
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
            other => Err(format!("unknown Language: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_five_variants() {
        assert_eq!(Language::all().len(), 5);
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
        assert!("java".parse::<Language>().is_err());
        assert!("go".parse::<Language>().is_err());
        assert!("".parse::<Language>().is_err());
        assert!("c++".parse::<Language>().is_err());
    }

    #[test]
    fn from_str_error_message_contains_input() {
        let err = "java".parse::<Language>().unwrap_err();
        assert!(err.contains("java"));
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
    fn from_extension_returns_none_for_unknown() {
        assert_eq!(Language::from_extension("java"), None);
        assert_eq!(Language::from_extension("go"), None);
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

    #[test]
    fn is_copy() {
        let lang = Language::Rust;
        let copied = lang;
        assert_eq!(lang, copied);
    }
}
