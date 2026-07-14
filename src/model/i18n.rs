// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Unicode-aware i18n helpers: case folding, NFC normalization, CJK detection.
//!
//! Behind the `i18n` feature these use ICU4X (`icu_casemap` + `icu_normalizer`)
//! with `compiled_data` — no runtime `DataProvider` needed. Without the feature
//! the functions degrade to ASCII-only behavior, preserving existing semantics.

/// Unicode case folding — replaces `to_ascii_lowercase` for non-ASCII text.
///
/// With `i18n`: full Unicode case folding via `icu_casemap::CaseMapper`
/// (e.g. German `ß` → `ss`, Turkish `İ` → `i̇`).
/// Without `i18n`: ASCII-only `to_ascii_lowercase` (current behavior).
#[cfg(feature = "i18n")]
pub fn fold_case(s: &str) -> String {
    use icu_casemap::CaseMapper;
    let cm = CaseMapper::new();
    cm.fold_string(s).into_owned()
}

#[cfg(not(feature = "i18n"))]
pub fn fold_case(s: &str) -> String {
    s.to_ascii_lowercase()
}

/// NFC normalization — canonical composition for stable FQN comparison.
///
/// With `i18n`: `icu_normalizer::ComposingNormalizer` NFC.
/// Without `i18n`: returns input unchanged (no normalization).
#[cfg(feature = "i18n")]
pub fn normalize_nfc(s: &str) -> String {
    use icu_normalizer::ComposingNormalizer;
    let normalizer = ComposingNormalizer::new_nfc();
    normalizer.normalize(s).into_owned()
}

#[cfg(not(feature = "i18n"))]
pub fn normalize_nfc(s: &str) -> String {
    s.to_string()
}

/// CJK character detection — no ICU dependency (manual char-range check).
///
/// Covers: CJK Unified Ideographs, Extension A, Compatibility Ideographs,
/// Hiragana, Katakana. Used by tokenizer to avoid camelCase splits on CJK.
#[must_use]
pub fn is_cjk(ch: char) -> bool {
    matches!(
        ch,
        '\u{4E00}'..='\u{9FFF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{3040}'..='\u{309F}'
            | '\u{30A0}'..='\u{30FF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== fold_case =====

    #[test]
    fn fold_case_ascii_unchanged() {
        assert_eq!(fold_case("Hello"), "hello");
        assert_eq!(fold_case("PARSE"), "parse");
    }

    #[test]
    fn fold_case_german_sharp_s() {
        // ß folds to "ss" under Unicode case folding
        assert_eq!(fold_case("Straße"), "strasse");
    }

    #[test]
    fn fold_case_turkish_i_with_dot() {
        // Turkish İ (U+0130) folds to i + combining dot above (U+0307)
        let result = fold_case("İ");
        assert_eq!(result, "i\u{0307}");
    }

    #[test]
    fn fold_case_empty_string() {
        assert_eq!(fold_case(""), "");
    }

    #[test]
    fn fold_case_already_lowercase() {
        assert_eq!(fold_case("hello"), "hello");
    }

    #[test]
    fn fold_case_cjk_unchanged() {
        // CJK characters have no case — folding is identity
        assert_eq!(fold_case("你好"), "你好");
        assert_eq!(fold_case("こんにちは"), "こんにちは");
    }

    // ===== normalize_nfc =====

    #[test]
    fn normalize_nfc_already_normalized() {
        assert_eq!(normalize_nfc("hello"), "hello");
    }

    #[test]
    fn normalize_nfc_decomposed_to_composed() {
        // NFD: 'e' (U+0065) + combining acute (U+0301)
        // NFC: 'é' (U+00E9)
        let nfd = "e\u{0301}";
        let nfc = normalize_nfc(nfd);
        assert_eq!(nfc, "\u{00E9}");
        assert_ne!(nfd, nfc);
    }

    #[test]
    fn normalize_nfc_empty_string() {
        assert_eq!(normalize_nfc(""), "");
    }

    #[test]
    fn normalize_nfc_ascii_unchanged() {
        assert_eq!(normalize_nfc("parse_file"), "parse_file");
    }

    #[test]
    fn normalize_nfc_cjk_unchanged() {
        assert_eq!(normalize_nfc("你好"), "你好");
    }

    // ===== is_cjk =====

    #[test]
    fn is_cjk_basic_ideograph() {
        assert!(is_cjk('你'));
        assert!(is_cjk('好'));
        assert!(is_cjk('世'));
        assert!(is_cjk('界'));
    }

    #[test]
    fn is_cjk_extension_a() {
        // U+3500 is in CJK Extension A range (U+3400..U+4DBF)
        assert!(is_cjk('\u{3500}'));
        assert!(is_cjk('\u{3400}'));
        assert!(is_cjk('\u{4DBF}'));
    }

    #[test]
    fn is_cjk_hiragana() {
        assert!(is_cjk('あ')); // U+3042
        assert!(is_cjk('い')); // U+3044
    }

    #[test]
    fn is_cjk_ascii_false() {
        assert!(!is_cjk('a'));
        assert!(!is_cjk('Z'));
        assert!(!is_cjk('0'));
        assert!(!is_cjk('_'));
    }
}
