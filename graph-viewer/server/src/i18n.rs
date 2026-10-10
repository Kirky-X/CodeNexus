// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Message-level i18n for the graph server (axum binary).
//!
//! Standalone mirror of the main crate's `codenexus::i18n` — the server
//! crate is deliberately lightweight (no dependency on the `codenexus`
//! library) but embeds the *same* shared `locales/{en,zh}/messages.ftl`
//! files, which remain the single source of truth for message text.
//!
//! The embedded FTL entries are registered into the sdforge i18n registry
//! by [`init()`] (called from `main`); [`t`]/[`tr`] forward to
//! `sdforge::i18n::t` (host registry → built-in en catalog → key itself,
//! never panics). Before `init()` the registry holds none of this crate's
//! keys, so `t`/`tr` serve bare keys.
//!
//! Detection chain (forced order): `CODENEXUS_LANG` → `LC_ALL` →
//! `LC_MESSAGES` → `LANG` → `sys-locale` → `"en"`; domain is exactly
//! `{en, zh}`. `init()` injects the detected locale via
//! `sdforge::i18n::set_locale`, so sdforge's `SDFORGE_LANG` auto-detection
//! never overrides it.

use std::sync::OnceLock;

/// Compile-time embedded copies of the shared `locales/` directory
/// (registered manually — the sync-guard test fails if a file is missed).
const EMBEDDED_LOCALES: &[(&str, &[(&str, &str)])] = &[
    (
        "en",
        &[(
            "messages.ftl",
            include_str!("../../../locales/en/messages.ftl"),
        )],
    ),
    (
        "zh",
        &[(
            "messages.ftl",
            include_str!("../../../locales/zh/messages.ftl"),
        )],
    ),
];

static GLOBAL_LOCALE: OnceLock<&'static str> = OnceLock::new();

/// Terminal fallback: English.
fn en() -> &'static str {
    "en"
}

/// Parse `key = value` pairs from an FTL document, in file order.
///
/// The catalog convention is single-line patterns, so the value is simply
/// the trimmed remainder of the line (placeholders keep their `{ $name }`
/// text — sdforge's `format_template` syntax matches this catalog's).
fn ftl_entries(ftl: &str) -> impl Iterator<Item = (&str, &str)> {
    ftl.lines().filter_map(|line| {
        let (head, tail) = line.split_once('=')?;
        let key = head.trim();
        let is_key = !key.is_empty()
            && !key.starts_with('#')
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        is_key.then_some((key, tail.trim()))
    })
}

/// Warm the detection chain eagerly (called from `main`), register the
/// embedded FTL catalogs into the sdforge i18n registry, and pin the active
/// locale to the detected one.
///
/// Idempotent — re-registering the same `(locale, key)` pairs overwrites
/// them with identical values.
pub fn init() {
    let _ = GLOBAL_LOCALE.get_or_init(detect_locale);
    for (locale, files) in EMBEDDED_LOCALES {
        for (_, ftl) in *files {
            for (key, value) in ftl_entries(ftl) {
                sdforge::i18n::register_translation(locale, key, value);
            }
        }
    }
    sdforge::i18n::set_locale(current_locale());
}

/// Detect the locale using the forced chain (see module docs).
fn detect_locale() -> &'static str {
    let env = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
    resolve_inputs(
        env("CODENEXUS_LANG").as_deref(),
        env("LC_ALL").as_deref(),
        env("LC_MESSAGES").as_deref(),
        env("LANG").as_deref(),
        sys_locale::get_locale().as_deref(),
    )
}

/// Normalize a raw locale string to one of the two supported languages.
/// `zh-*`/`en-*` variants collapse to bare `zh`/`en`; `C`/`POSIX` and
/// unsupported languages yield `None` so the detection chain keeps walking.
///
/// Shape check (ASCII-alphanumeric subtags separated by `-`) mirrors what
/// `unic-langid`'s parser rejected, keeping the chain's domain exactly
/// `{en, zh}` now that the direct `unic-langid` dependency is gone.
fn normalize(raw: &str) -> Option<&'static str> {
    let s = raw.split('@').next()?.split('.').next()?.replace('_', "-");
    if s.is_empty() || matches!(s.as_str(), "C" | "POSIX") {
        return None;
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    match s.split('-').next()?.to_ascii_lowercase().as_str() {
        "zh" => Some("zh"),
        "en" => Some(en()),
        _ => None,
    }
}

/// Pure detection-chain resolver over explicit inputs (parallel-test safe).
fn resolve_inputs(
    codenexus_lang: Option<&str>,
    lc_all: Option<&str>,
    lc_messages: Option<&str>,
    lang: Option<&str>,
    sys: Option<&str>,
) -> &'static str {
    let chain = [codenexus_lang, lc_all, lc_messages, lang];
    for raw in chain.into_iter().flatten() {
        if let Some(locale) = normalize(raw) {
            return locale;
        }
    }
    if let Some(locale) = sys.and_then(normalize) {
        return locale;
    }
    en()
}

/// Get the current locale (the once-detected default).
pub fn current_locale() -> &'static str {
    GLOBAL_LOCALE.get_or_init(detect_locale)
}

/// Translate `key` for the current locale, formatting `{ $var }` with
/// `args`. Fallback: current language → `en` → the key itself. Never panics.
///
/// Forwarded to `sdforge::i18n::t`; translations are available after
/// [`init()`] has registered the embedded catalogs.
pub fn t(key: &str, args: &[(&str, String)]) -> String {
    sdforge::i18n::t(key, args)
}

/// Convenience: [`t`] with no dynamic arguments.
pub fn tr(key: &str) -> String {
    t(key, &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn ftl_keys(ftl: &str) -> BTreeSet<String> {
        ftl_entries(ftl).map(|(key, _)| key.to_string()).collect()
    }

    /// Look up the embedded FTL text of a locale registered in
    /// EMBEDDED_LOCALES.
    fn embedded_ftl(locale: &str) -> &'static str {
        EMBEDDED_LOCALES
            .iter()
            .find(|(l, _)| *l == locale)
            .unwrap_or_else(|| panic!("EMBEDDED_LOCALES missing locale '{locale}'"))
            .1[0]
            .1
    }

    /// Guard: the shared catalog's EN/ZH key sets must stay identical.
    #[test]
    fn test_key_parity() {
        let en = ftl_keys(embedded_ftl("en"));
        let zh = ftl_keys(embedded_ftl("zh"));
        assert!(!en.is_empty(), "EN catalog must not be empty");
        assert_eq!(en, zh, "EN/ZH key sets must be identical");
    }

    /// Guard: the shared `locales/` dir must not outgrow EMBEDDED_LOCALES.
    #[test]
    fn embedded_locales_cover_locales_dir() {
        let locales_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales");
        let locales_dir = locales_dir
            .canonicalize()
            .expect("shared locales dir exists");
        for entry in std::fs::read_dir(&locales_dir).expect("locales dir readable") {
            let path = entry.expect("locale entry").path();
            if !path.is_dir() {
                continue;
            }
            let locale = path
                .file_name()
                .and_then(|n| n.to_str())
                .expect("locale dir name");
            let files = EMBEDDED_LOCALES
                .iter()
                .find(|(l, _)| *l == locale)
                .unwrap_or_else(|| panic!("EMBEDDED_LOCALES missing locale dir '{locale}'"))
                .1;
            for ftl_entry in std::fs::read_dir(&path).expect("locale dir readable") {
                let ftl_path = ftl_entry.expect("ftl entry").path();
                if ftl_path.extension().and_then(|e| e.to_str()) != Some("ftl") {
                    continue;
                }
                let stem = ftl_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .expect("ftl stem");
                assert!(
                    files
                        .iter()
                        .any(|(name, _)| name.strip_suffix(".ftl") == Some(stem)),
                    "locale '{locale}': '{stem}.ftl' exists on disk but is not registered \
                     in EMBEDDED_LOCALES"
                );
            }
        }
    }

    #[test]
    fn test_lookups_and_fallbacks() {
        let _guard = I18N_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        init();
        assert_eq!(
            sdforge::i18n::translate_for(
                "en",
                "project-not-found",
                &[("name", "demo".to_string())]
            ),
            "Project 'demo' not found".to_string()
        );
        assert_eq!(
            sdforge::i18n::translate_for(
                "zh",
                "project-not-found",
                &[("name", "demo".to_string())]
            ),
            "项目 'demo' 未找到".to_string()
        );
        // Unknown language → en catalog; missing key → key itself.
        assert_eq!(
            sdforge::i18n::translate_for("ar", "node-not-found", &[("name", "x".to_string())]),
            "Node 'x' not found".to_string()
        );
        assert_eq!(t("nonexistent-key", &[]), "nonexistent-key");
        assert_eq!(tr("another-missing-key"), "another-missing-key");
    }

    #[test]
    fn test_detection_chain_pure_logic() {
        // Project override beats POSIX chain; zh-TW collapses to zh.
        assert_eq!(
            resolve_inputs(Some("zh_TW"), Some("en_US"), None, None, None),
            "zh"
        );
        // C/POSIX and unsupported languages fall through; terminal is en.
        assert_eq!(
            resolve_inputs(Some("C"), Some("fr_FR"), None, Some("zh_CN"), None),
            "zh"
        );
        assert_eq!(resolve_inputs(None, None, None, None, None), "en");
    }

    // --- sdforge bridge behavior (selfdev-lib-adoption-r1) ---

    /// Serializes tests that call `init()` / mutate the process-global
    /// sdforge registry (no serial_test dev-dependency in this crate).
    static I18N_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// `init()` must register every shared FTL entry into the sdforge
    /// registry, so each `(locale, key)` resolves verbatim.
    #[test]
    fn init_registers_all_shared_entries_in_sdforge() {
        let _guard = I18N_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        init();
        for (locale, files) in EMBEDDED_LOCALES {
            for (_, ftl) in *files {
                for (key, value) in ftl_entries(ftl) {
                    assert_eq!(
                        sdforge::i18n::translate_for(locale, key, &[]),
                        value,
                        "sdforge registry must serve shared ({locale}, {key}) verbatim"
                    );
                }
            }
        }
    }

    /// `init()` wires the detection chain into sdforge via `set_locale`, and
    /// `t`/`tr` translate through the registry.
    #[test]
    fn init_wires_detected_locale_and_translates_via_sdforge() {
        let _guard = I18N_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        init();
        // init() must inject the detected locale into sdforge's registry.
        assert_eq!(
            sdforge::i18n::get_locale(),
            current_locale(),
            "init() must set sdforge's locale from the detection chain"
        );
        // Pin zh explicitly for a deterministic translation assertion (no env
        // mutation); CODENEXUS_LANG wiring is covered by
        // test_detection_chain_pure_logic.
        sdforge::i18n::set_locale("zh");
        assert_eq!(
            t("project-not-found", &[("name", "demo".to_string())]),
            "项目 'demo' 未找到",
            "zh must translate through the sdforge registry"
        );
        assert_eq!(tr("impact-notice"), "变更影响告警");
        // Restore a neutral locale for other tests sharing the registry.
        sdforge::i18n::set_locale("en");
    }

    /// Explicit unsupported locale falls back to the en catalog.
    #[test]
    fn explicit_unsupported_locale_falls_back_to_en() {
        let _guard = I18N_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        init();
        assert_eq!(
            sdforge::i18n::translate_for(
                "ar",
                "project-not-found",
                &[("name", "demo".to_string())]
            ),
            "Project 'demo' not found"
        );
    }

    /// Missing keys resolve to the key itself, before and after `init()`.
    #[test]
    fn missing_keys_return_bare_key() {
        assert_eq!(t("nonexistent-key", &[]), "nonexistent-key");
        assert_eq!(tr("another-missing-key"), "another-missing-key");
    }
}
