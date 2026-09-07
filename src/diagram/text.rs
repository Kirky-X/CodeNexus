// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CJK-aware text measurement for deterministic diagram layout.
//!
//! Ports archify's `textUnits` (renderers/shared/utils.mjs) so label boxes
//! are sized from measured advance width, not `chars().count()`. Wide glyphs
//! (CJK, Hangul, emoji) take two units; variation selectors re-present the
//! base character and carry no width of their own.

/// Code points with East Asian Wide/Fullwidth advance (two units) per UAX #11,
/// tracking Unicode 17.0. Includes the BMP emoji-presentation symbols that
/// render at the same square advance as supplementary-plane emoji, and Hangul
/// Jamo Extended-A up to its last assigned jamo (U+A97C) — unassigned code
/// points default to Neutral, not Wide. Ranges are spelled out because Rust
/// has no East_Asian_Width property escape either.
const FULLWIDTH_RANGES: &[(u32, u32)] = &[
    (0x1100, 0x115F),
    (0x231A, 0x231B),
    (0x2329, 0x232A),
    (0x23E9, 0x23EC),
    (0x23F0, 0x23F0),
    (0x23F3, 0x23F3),
    (0x25FD, 0x25FE),
    (0x2614, 0x2615),
    (0x2630, 0x2637),
    (0x2648, 0x2653),
    (0x267F, 0x267F),
    (0x268A, 0x268F),
    (0x2693, 0x2693),
    (0x26A1, 0x26A1),
    (0x26AA, 0x26AB),
    (0x26BD, 0x26BE),
    (0x26C4, 0x26C5),
    (0x26CE, 0x26CE),
    (0x26D4, 0x26D4),
    (0x26EA, 0x26EA),
    (0x26F2, 0x26F3),
    (0x26F5, 0x26F5),
    (0x26FA, 0x26FA),
    (0x26FD, 0x26FD),
    (0x2705, 0x2705),
    (0x270A, 0x270B),
    (0x2728, 0x2728),
    (0x274C, 0x274C),
    (0x274E, 0x274E),
    (0x2753, 0x2755),
    (0x2757, 0x2757),
    (0x2795, 0x2797),
    (0x27B0, 0x27B0),
    (0x27BF, 0x27BF),
    (0x2B1B, 0x2B1C),
    (0x2B50, 0x2B50),
    (0x2B55, 0x2B55),
    (0x2E80, 0xA4CF),
    (0xA960, 0xA97C),
    (0xAC00, 0xD7A3),
    (0xF900, 0xFAFF),
    (0xFE10, 0xFE19),
    (0xFE30, 0xFE6F),
    (0xFF01, 0xFF60),
    (0xFFE0, 0xFFE6),
    (0x16FE0, 0x18DFF),
    (0x1AFF0, 0x1AFFF),
    (0x1B000, 0x1B2FF),
    (0x1F000, 0x1FAFF),
    (0x20000, 0x3FFFD),
];

const VARIATION_SELECTOR_FIRST: u32 = 0xFE00;
const VARIATION_SELECTOR_LAST: u32 = 0xFE0F;
const VARIATION_SELECTOR_TEXT: u32 = 0xFE0E;
const VARIATION_SELECTOR_EMOJI: u32 = 0xFE0F;

fn is_fullwidth(code_point: u32) -> bool {
    FULLWIDTH_RANGES
        .binary_search_by(|&(lo, hi)| {
            if code_point < lo {
                std::cmp::Ordering::Greater
            } else if code_point > hi {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Measures `text` in advance-width units: one per narrow code point, two per
/// East Asian Wide/Fullwidth or emoji code point.
///
/// A variation selector (U+FE00–U+FE0F) carries no advance of its own — it
/// re-presents the preceding character. A base followed by VS16 (emoji
/// presentation) is measured at the square two-unit advance from the
/// selector; VS15 (text presentation) measures narrow. Over-measuring pads a
/// box, under-measuring spills the label out of it, so malformed sequences
/// (selector after a non-emoji base) err wide on purpose.
#[must_use]
pub fn text_units(text: &str) -> f64 {
    let chars: Vec<char> = text.chars().collect();
    let mut units: u64 = 0;
    for (i, &ch) in chars.iter().enumerate() {
        let code_point = u32::from(ch);
        if (VARIATION_SELECTOR_FIRST..=VARIATION_SELECTOR_LAST).contains(&code_point) {
            continue;
        }
        let next = chars.get(i + 1).map_or(0, |&c| u32::from(c));
        if next == VARIATION_SELECTOR_EMOJI {
            units += 2;
        } else if next == VARIATION_SELECTOR_TEXT {
            units += 1;
        } else if is_fullwidth(code_point) {
            units += 2;
        } else {
            units += 1;
        }
    }
    f64::from(u32::try_from(units).unwrap_or(u32::MAX))
}

/// The ellipsis appended by [`fit_label`]. U+2026 measures one unit.
const ELLIPSIS: char = '…';

/// Truncates `text` to at most `max_units` advance units, appending an
/// ellipsis when content was dropped. Text that already fits is returned
/// unchanged (same chars, same order).
#[must_use]
pub fn fit_label(text: &str, max_units: f64) -> String {
    let budget = max_units.max(1.0);
    if text_units(text) <= budget {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 1.0_f64; // reserve one unit for the ellipsis
    for ch in text.chars() {
        let ch_units = text_units(&ch.to_string());
        if used + ch_units > budget {
            break;
        }
        out.push(ch);
        used += ch_units;
    }
    out.push(ELLIPSIS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_measures_one_unit_per_char() {
        assert_eq!(text_units(""), 0.0);
        assert_eq!(text_units("A"), 1.0);
        assert_eq!(text_units("main.rs"), 7.0);
    }

    #[test]
    fn cjk_measures_two_units_per_char() {
        assert_eq!(text_units("中"), 2.0);
        assert_eq!(text_units("中A"), 3.0, "CJK 2 + latin 1");
        assert_eq!(text_units("中文标签"), 8.0);
    }

    #[test]
    fn emoji_and_wide_symbols_measure_two_units() {
        assert_eq!(text_units("\u{2B50}"), 2.0, "star is Wide since Unicode 16");
        assert_eq!(text_units("\u{1F600}"), 2.0, "supplementary emoji");
        assert_eq!(text_units("\u{AC00}"), 2.0, "Hangul syllable");
    }

    #[test]
    fn variation_selectors_represent_their_base() {
        assert_eq!(
            text_units("\u{2B50}\u{FE0F}"),
            2.0,
            "VS16 keeps star square, not 3"
        );
        assert_eq!(
            text_units("\u{2708}\u{FE0F}"),
            2.0,
            "narrow base widened by VS16"
        );
        assert_eq!(text_units("\u{2708}"), 1.0, "same base without selector");
        assert_eq!(
            text_units("\u{4E2D}\u{FE0E}"),
            1.0,
            "VS15 asks text presentation"
        );
        assert_eq!(text_units("\u{FE0F}"), 0.0, "lone selector carries nothing");
    }

    #[test]
    fn fit_label_keeps_fitting_text() {
        assert_eq!(fit_label("abc", 5.0), "abc");
        assert_eq!(fit_label("中文", 4.0), "中文");
    }

    #[test]
    fn fit_label_truncates_with_ellipsis() {
        let fitted = fit_label("abcdefgh", 5.0);
        assert!(fitted.ends_with('…'));
        assert!(text_units(&fitted) <= 5.0, "{fitted} exceeds budget");
        assert!(
            fitted.starts_with("abcd"),
            "greedy prefix kept, got {fitted}"
        );
    }

    #[test]
    fn fit_label_handles_cjk_budgets() {
        let fitted = fit_label("中文标签很长", 5.0);
        assert!(text_units(&fitted) <= 5.0, "{fitted} exceeds budget");
        assert!(fitted.contains('中'));
    }
}
