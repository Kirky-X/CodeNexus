// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Fluent message catalog: `locales/{en,zh}/messages.ftl` embedded at
//! compile time and registered into the sdforge i18n registry by [`init()`].
//!
//! The FTL assets remain the single source of truth (same `key = value`
//! single-line patterns, same `{ $name }` placeholders — which is exactly
//! sdforge's host-template syntax). Bundle construction is no longer
//! hand-rolled: [`t`]/[`tr`] forward to `sdforge::i18n::t`, which resolves
//! host registrations → built-in catalog → the key itself (never panics).
//!
//! Applications must call [`init()`] once at startup (the CLI `main` does;
//! see [`crate::i18n::init`]). Before `init()` the sdforge registry holds
//! none of this crate's keys, so `t`/`tr` serve bare keys.

use super::locale::current_locale;

/// Compile-time embedded copies of the `locales/` directory (inklog
/// `EMBEDDED_LOCALES` pattern). Each entry maps a locale directory name to
/// its `.ftl` files, embedded via `include_str!` so deployed binaries
/// translate without the build machine's `locales/` directory. When adding
/// a new `.ftl` file or locale, register it here manually —
/// [`embedded_locales_cover_locales_dir`] fails if a file on disk is
/// missing from this table.
const EMBEDDED_LOCALES: &[(&str, &[(&str, &str)])] = &[
    (
        "en",
        &[(
            "messages.ftl",
            include_str!("../../locales/en/messages.ftl"),
        )],
    ),
    (
        "zh",
        &[(
            "messages.ftl",
            include_str!("../../locales/zh/messages.ftl"),
        )],
    ),
];

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

/// Register the embedded FTL catalogs into the sdforge i18n registry and
/// pin the active locale to the detected one.
///
/// Must be called once from the application entry point before serving
/// any messages: before `init()` the registry holds none of this crate's
/// keys, so [`t`] falls back to bare keys (no panic, no translation).
/// Idempotent — re-registering the same `(locale, key)` pairs overwrites
/// them with identical values.
///
/// Keeps the `CODENEXUS_LANG` contract: the active locale is injected via
/// `sdforge::i18n::set_locale` from this crate's own detection chain (see
/// [`super::locale`]), so sdforge's `SDFORGE_LANG` auto-detection never
/// overrides it.
pub fn init() {
    super::locale::init();
    for (locale, files) in EMBEDDED_LOCALES {
        for (_, ftl) in *files {
            for (key, value) in ftl_entries(ftl) {
                sdforge::i18n::register_translation(locale, key, value);
            }
        }
    }
    sdforge::i18n::set_locale(&current_locale().to_string());
}

/// Translate `key` for the current locale, formatting `{ $var }`
/// placeholders with `args`.
///
/// Forwarded to `sdforge::i18n::t`. Fallback chain: current language →
/// `en` → the key itself. Never panics.
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

    /// Extract message keys from an FTL document (single-line patterns only,
    /// which is all this catalog uses).
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

    /// Guard: EN and ZH catalogs must expose exactly the same key set, so a
    /// missing translation can never silently fall through to another
    /// language's text or a bare key.
    #[test]
    fn test_key_parity() {
        let en = ftl_keys(embedded_ftl("en"));
        let zh = ftl_keys(embedded_ftl("zh"));
        assert!(!en.is_empty(), "EN catalog must not be empty");
        assert_eq!(en, zh, "EN/ZH key sets must be identical");
    }

    /// Guard: every `.ftl` file under `locales/` must be registered in
    /// EMBEDDED_LOCALES, otherwise a freshly added file silently ships
    /// untranslated in production (inklog test_embedded_locales_cover_locales_dir).
    #[test]
    fn embedded_locales_cover_locales_dir() {
        let locales_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("locales");
        for entry in std::fs::read_dir(&locales_dir).expect("locales dir exists") {
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

    // Registry lookups via `init()` + sdforge mutate the process-global
    // registry, so they run serialized (dbnexus catalog.rs pattern).

    #[test]
    #[serial_test::serial(i18n_catalog)]
    fn test_en_bundle_lookups() {
        init();
        assert_eq!(
            sdforge::i18n::translate_for("en", "impact-notice", &[]),
            "Change impact alert".to_string()
        );
        assert_eq!(
            sdforge::i18n::translate_for(
                "en",
                "project-not-found",
                &[("name", "demo".to_string())]
            ),
            "Project 'demo' not found".to_string()
        );
        assert_eq!(
            sdforge::i18n::translate_for("en", "node-not-found", &[("name", "main".to_string())]),
            "Node 'main' not found".to_string()
        );
    }

    #[test]
    #[serial_test::serial(i18n_catalog)]
    fn test_zh_bundle_lookups() {
        init();
        assert_eq!(
            sdforge::i18n::translate_for("zh", "impact-notice", &[]),
            "变更影响告警".to_string()
        );
        assert_eq!(
            sdforge::i18n::translate_for(
                "zh",
                "project-not-found",
                &[("name", "demo".to_string())]
            ),
            "项目 'demo' 未找到".to_string()
        );
        assert_eq!(
            sdforge::i18n::translate_for("zh", "index-incremental-triggered", &[]),
            "触发增量索引".to_string()
        );
    }

    /// Guard: missing keys resolve to the key itself without panicking,
    /// including before any explicit init() and for both languages.
    #[test]
    #[serial_test::serial(i18n_catalog)]
    fn test_missing_key_returns_key_without_panic() {
        assert_eq!(t("nonexistent-key", &[]), "nonexistent-key");
        assert_eq!(tr("another-missing-key"), "another-missing-key");
    }

    // --- sdforge bridge behavior (selfdev-lib-adoption-r1) ---

    /// `init()` must register every embedded FTL entry into the sdforge
    /// registry, so each `(locale, key)` resolves verbatim (host registrations
    /// win over built-in catalogs and carry `{ $name }` placeholders).
    #[test]
    #[serial_test::serial(i18n_catalog)]
    fn init_registers_all_embedded_entries_in_sdforge() {
        init();
        for (locale, files) in EMBEDDED_LOCALES {
            for (_, ftl) in *files {
                for (key, value) in ftl_entries(ftl) {
                    assert_eq!(
                        sdforge::i18n::translate_for(locale, key, &[]),
                        value,
                        "sdforge registry must serve embedded ({locale}, {key}) verbatim"
                    );
                }
            }
        }
    }

    /// `init()` must inject the detected locale into sdforge via
    /// `set_locale` (keeping the CODENEXUS_LANG contract), and `t` must
    /// translate through the registry for the active locale.
    #[test]
    #[serial_test::serial(i18n_catalog)]
    fn init_wires_detected_locale_and_translates_via_sdforge() {
        // The override stands in for CODENEXUS_LANG=zh — the same path
        // detect_locale feeds, without mutating process env.
        crate::i18n::set_locale("zh").expect("zh supported");
        init();
        assert_eq!(
            sdforge::i18n::get_locale(),
            "zh",
            "init() must call sdforge set_locale with the detected locale"
        );
        assert_eq!(
            t("project-not-found", &[("name", "demo".to_string())]),
            "项目 'demo' 未找到",
            "zh must translate through the sdforge registry"
        );
        assert_eq!(tr("impact-notice"), "变更影响告警");
        // Restore a neutral locale for other tests sharing the process-level
        // registry.
        crate::i18n::set_locale("en").expect("en supported");
    }

    /// Explicit unsupported locale falls back to the en catalog (the old
    /// `format_from_bundle("ar", …)` case).
    #[test]
    #[serial_test::serial(i18n_catalog)]
    fn explicit_unsupported_locale_falls_back_to_en() {
        init();
        assert_eq!(
            sdforge::i18n::translate_for(
                "ar",
                "project-not-found",
                &[("name", "demo".to_string())]
            ),
            "Project 'demo' not found"
        );
        assert_eq!(
            sdforge::i18n::translate_for("en", "impact-notice", &[]),
            "Change impact alert"
        );
    }

    /// Missing keys resolve to the key itself before and after `init()`.
    #[test]
    #[serial_test::serial(i18n_catalog)]
    fn missing_key_returns_bare_key_before_and_after_init() {
        assert_eq!(t("nonexistent-key", &[]), "nonexistent-key");
        assert_eq!(tr("another-missing-key"), "another-missing-key");
        init();
        assert_eq!(t("nonexistent-key", &[]), "nonexistent-key");
        assert_eq!(tr("another-missing-key"), "another-missing-key");
    }
}
