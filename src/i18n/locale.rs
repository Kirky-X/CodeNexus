// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Locale detection and global locale storage for user-facing messages.
//!
//! Detection chain (forced order — change `unify-rust-i18n`
//! reference-pattern §2; the resolved domain is exactly `{en, zh}`):
//!
//! 1. `CODENEXUS_LANG` — project override variable
//! 2. `LC_ALL` → `LC_MESSAGES` → `LANG` — POSIX environment chain
//! 3. [`sys_locale::get_locale`] — system detection
//! 4. `"en"` — terminal fallback
//!
//! Unrelated: [`crate::model::i18n`] is Unicode text processing (case
//! folding / NFC normalization), not message localization.

use std::str::FromStr;
use std::sync::{OnceLock, RwLock};

use unic_langid::LanguageIdentifier;

/// Global default locale, resolved once on first access (lazy init —
/// [`t`](super::t) works before an explicit [`init()`] call).
static GLOBAL_LOCALE: OnceLock<LanguageIdentifier> = OnceLock::new();

/// Override locale set via [`set_locale()`].
static OVERRIDE_LOCALE: RwLock<Option<LanguageIdentifier>> = RwLock::new(None);

/// Terminal fallback: English.
fn en() -> LanguageIdentifier {
    LanguageIdentifier::from_str("en").expect("'en' is a valid language identifier")
}

/// Normalize a raw locale string to one of the two supported languages.
///
/// Strips `@modifier` and `.codeset` suffixes, maps `zh-*` variants
/// (zh-TW/HK/SG/Hans) to `zh` and `en-*` to `en`; anything else — including
/// `C`/`POSIX` and unsupported languages — yields `None` so the detection
/// chain keeps walking.
fn normalize(raw: &str) -> Option<LanguageIdentifier> {
    let s = raw.split('@').next()?.split('.').next()?.replace('_', "-");
    if matches!(s.as_str(), "" | "C" | "POSIX") {
        return None;
    }
    let id = LanguageIdentifier::from_str(&s).ok()?;
    match id.language.as_str() {
        "zh" => Some(LanguageIdentifier::from_str("zh").expect("'zh' is a valid language id")),
        "en" => Some(en()),
        _ => None,
    }
}

/// Pure detection-chain resolver over explicit inputs.
///
/// Env reads are injected so tests exercise the chain without mutating
/// process globals (which would race parallel tests — reference-pattern §5).
fn resolve_inputs(
    codenexus_lang: Option<&str>,
    lc_all: Option<&str>,
    lc_messages: Option<&str>,
    lang: Option<&str>,
    sys: Option<&str>,
) -> LanguageIdentifier {
    let chain = [codenexus_lang, lc_all, lc_messages, lang];
    for raw in chain.into_iter().flatten() {
        if let Some(id) = normalize(raw) {
            return id;
        }
    }
    if let Some(id) = sys.and_then(normalize) {
        return id;
    }
    en()
}

/// Detect the locale using the full forced chain.
fn detect_locale() -> LanguageIdentifier {
    let env = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
    resolve_inputs(
        env("CODENEXUS_LANG").as_deref(),
        env("LC_ALL").as_deref(),
        env("LC_MESSAGES").as_deref(),
        env("LANG").as_deref(),
        sys_locale::get_locale().as_deref(),
    )
}

/// Warm the detection chain eagerly (entry points).
///
/// Optional: [`t`](super::t) lazily initializes on first use, so calling
/// this only guarantees detection happens once, up front.
pub fn init() {
    let _ = GLOBAL_LOCALE.get_or_init(detect_locale);
}

/// Get the current locale.
///
/// Resolution order: [`set_locale()`] override, then the global default
/// (detected once on first access).
pub fn current_locale() -> LanguageIdentifier {
    if let Ok(guard) = OVERRIDE_LOCALE.read() {
        if let Some(locale) = guard.as_ref() {
            return locale.clone();
        }
    }
    GLOBAL_LOCALE.get_or_init(detect_locale).clone()
}

/// Set a locale override.
///
/// Accepts anything the chain's normalizer understands (`"zh"`, `"en"`,
/// `"zh-CN"`, `"en_US.UTF-8"`, ...). Unsupported languages are rejected so
/// callers can never switch to a third language.
///
/// # Errors
/// Returns an error string for values outside the supported `{en, zh}` set.
pub fn set_locale(locale: &str) -> Result<(), String> {
    let parsed = normalize(locale)
        .ok_or_else(|| format!("unsupported locale '{locale}' (only en/zh are supported)"))?;
    *OVERRIDE_LOCALE
        .write()
        .expect("locale override RwLock poisoned") = Some(parsed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_maps_zh_variants_to_zh() {
        for raw in [
            "zh",
            "zh_CN.UTF-8",
            "zh_TW",
            "zh-HK",
            "zh-Hans-CN",
            "zh_SG@latin",
        ] {
            let id = normalize(raw).unwrap_or_else(|| panic!("{raw} should normalize to zh"));
            assert_eq!(id.language.as_str(), "zh", "{raw} → zh");
            assert_eq!(id.to_string(), "zh", "{raw} must collapse to bare zh");
        }
    }

    #[test]
    fn normalize_maps_en_variants_to_en() {
        for raw in ["en", "en_US.UTF-8", "en_GB", "en-US@posix"] {
            let id = normalize(raw).unwrap_or_else(|| panic!("{raw} should normalize to en"));
            assert_eq!(id.to_string(), "en", "{raw} must collapse to bare en");
        }
    }

    #[test]
    fn normalize_rejects_unsupported_and_degenerate_values() {
        for raw in [
            "fr_FR",
            "de",
            "ja-JP",
            "C",
            "POSIX",
            "",
            "C.UTF-8",
            "not a locale!!",
        ] {
            assert!(normalize(raw).is_none(), "{raw} must not normalize");
        }
    }

    #[test]
    fn chain_codenexus_lang_beats_lc_all() {
        let resolved = resolve_inputs(Some("zh_CN.UTF-8"), Some("en_US.UTF-8"), None, None, None);
        assert_eq!(
            resolved.to_string(),
            "zh",
            "project override wins over LC_ALL"
        );
    }

    #[test]
    fn chain_lc_all_beats_lc_messages_beats_lang() {
        let resolved = resolve_inputs(None, Some("zh_CN"), Some("en_US"), Some("en_US"), None);
        assert_eq!(resolved.to_string(), "zh");
        let resolved = resolve_inputs(None, None, Some("zh_CN"), Some("en_US"), None);
        assert_eq!(resolved.to_string(), "zh");
    }

    #[test]
    fn chain_unsupported_values_fall_through_to_next_link() {
        // C/POSIX and unsupported languages skip to the next chain link.
        let resolved = resolve_inputs(Some("C"), Some("fr_FR"), Some("POSIX"), Some("zh_TW"), None);
        assert_eq!(resolved.to_string(), "zh");
    }

    #[test]
    fn chain_sys_locale_used_only_after_env() {
        let resolved = resolve_inputs(None, None, None, None, Some("zh_CN.UTF-8"));
        assert_eq!(resolved.to_string(), "zh");
        // Env beats sys-locale even when env is the "lower" LANG link.
        let resolved = resolve_inputs(None, None, None, Some("en_US"), Some("zh_CN"));
        assert_eq!(resolved.to_string(), "en");
    }

    #[test]
    fn chain_terminal_fallback_is_en() {
        for resolved in [
            resolve_inputs(None, None, None, None, None),
            resolve_inputs(Some(""), Some(""), Some(""), Some(""), Some("")),
            resolve_inputs(
                Some("fr_FR"),
                Some("de"),
                Some("ja"),
                Some("C"),
                Some("ru_RU"),
            ),
        ] {
            assert_eq!(
                resolved.to_string(),
                "en",
                "every chain must terminate in en"
            );
        }
    }

    #[test]
    fn set_locale_accepts_only_supported_languages() {
        set_locale("zh-CN").expect("zh-CN is supported");
        assert_eq!(current_locale().to_string(), "zh");
        set_locale("en_US.UTF-8").expect("en_US.UTF-8 is supported");
        assert_eq!(current_locale().to_string(), "en");
        assert!(set_locale("fr_FR").is_err(), "third languages are rejected");
        assert!(set_locale("garbage!!").is_err());
    }
}
