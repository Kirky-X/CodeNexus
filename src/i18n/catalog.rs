// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Fluent message catalog: `locales/{en,zh}/messages.ftl` embedded at
//! compile time into concurrent (Send+Sync) Fluent bundles, cached in
//! `OnceLock`s (dbnexus pattern — non-concurrent `FluentBundle` holds a
//! `RefCell` and cannot live in a `static`).

use std::sync::OnceLock;

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

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

/// Cached concurrent Fluent bundles (thread-safe, built once on first use).
static EN_BUNDLE: OnceLock<FluentBundle<FluentResource>> = OnceLock::new();
static ZH_BUNDLE: OnceLock<FluentBundle<FluentResource>> = OnceLock::new();

/// Translate `key` for the current locale, formatting `{ $var }`
/// placeholders with `args`.
///
/// Fallback chain: current language → `en` → the key itself. Never panics.
pub fn t(key: &str, args: &[(&str, String)]) -> String {
    let locale = current_locale();
    format_from_bundle(locale.language.as_str(), key, args)
        .or_else(|| format_from_bundle("en", key, args))
        .unwrap_or_else(|| key.to_string())
}

/// Convenience: [`t`] with no dynamic arguments.
pub fn tr(key: &str) -> String {
    t(key, &[])
}

/// Format a message from the catalog for the given language.
///
/// Unknown languages resolve to the `en` bundle; missing keys return `None`
/// (the caller falls back to the key itself).
fn format_from_bundle(lang: &str, key: &str, args: &[(&str, String)]) -> Option<String> {
    let bundle = match lang {
        "zh" => ZH_BUNDLE.get_or_init(build_zh_bundle),
        _ => EN_BUNDLE.get_or_init(build_en_bundle),
    };
    let msg = bundle.get_message(key)?;
    let pattern = msg.value()?;
    let mut fluent_args = FluentArgs::new();
    for (name, value) in args {
        fluent_args.set(*name, FluentValue::from(value.clone()));
    }
    let mut errors = vec![];
    Some(
        bundle
            .format_pattern(pattern, Some(&fluent_args), &mut errors)
            .to_string(),
    )
}

fn build_bundle(langid: LanguageIdentifier, ftl: &str) -> FluentBundle<FluentResource> {
    let resource = FluentResource::try_new(ftl.to_string()).unwrap_or_else(|e| e.0);
    let mut bundle = FluentBundle::new_concurrent(vec![langid]);
    bundle.set_use_isolating(false);
    bundle
        .add_resource(resource)
        .expect("FTL resources should add without conflict");
    bundle
}

fn build_en_bundle() -> FluentBundle<FluentResource> {
    build_bundle(
        "en".parse().expect("'en' is a valid language identifier"),
        EN_FTL,
    )
}

fn build_zh_bundle() -> FluentBundle<FluentResource> {
    build_bundle(
        "zh".parse().expect("'zh' is a valid language identifier"),
        ZH_FTL,
    )
}

const EN_FTL: &str = EMBEDDED_LOCALES[0].1[0].1;
const ZH_FTL: &str = EMBEDDED_LOCALES[1].1[0].1;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Extract message keys from an FTL document (single-line patterns only,
    /// which is all this catalog uses).
    fn ftl_keys(ftl: &str) -> BTreeSet<String> {
        ftl.lines()
            .filter_map(|line| line.split_once('='))
            .filter_map(|(head, _)| {
                let key = head.trim();
                let is_key = !key.is_empty()
                    && !key.starts_with('#')
                    && key
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
                is_key.then(|| key.to_string())
            })
            .collect()
    }

    /// Guard: EN and ZH catalogs must expose exactly the same key set, so a
    /// missing translation can never silently fall through to another
    /// language's text or a bare key.
    #[test]
    fn test_key_parity() {
        let en = ftl_keys(EN_FTL);
        let zh = ftl_keys(ZH_FTL);
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

    // Bundle lookups target a fixed language (no global locale state) — safe
    // under parallel test execution (dbnexus catalog.rs pattern).

    #[test]
    fn test_en_bundle_lookups() {
        assert_eq!(
            format_from_bundle("en", "impact-notice", &[]),
            Some("Change impact alert".to_string())
        );
        assert_eq!(
            format_from_bundle("en", "project-not-found", &[("name", "demo".to_string())]),
            Some("Project 'demo' not found".to_string())
        );
        assert_eq!(
            format_from_bundle("en", "node-not-found", &[("name", "main".to_string())]),
            Some("Node 'main' not found".to_string())
        );
    }

    #[test]
    fn test_zh_bundle_lookups() {
        assert_eq!(
            format_from_bundle("zh", "impact-notice", &[]),
            Some("变更影响告警".to_string())
        );
        assert_eq!(
            format_from_bundle("zh", "project-not-found", &[("name", "demo".to_string())]),
            Some("项目 'demo' 未找到".to_string())
        );
        assert_eq!(
            format_from_bundle("zh", "index-incremental-triggered", &[]),
            Some("触发增量索引".to_string())
        );
    }

    #[test]
    fn test_unknown_lang_falls_back_to_en_bundle() {
        assert_eq!(
            format_from_bundle("ar", "impact-notice", &[]),
            Some("Change impact alert".to_string())
        );
    }

    /// Guard: missing keys resolve to the key itself without panicking,
    /// including before any explicit init() and for both languages.
    #[test]
    fn test_missing_key_returns_key_without_panic() {
        assert_eq!(t("nonexistent-key", &[]), "nonexistent-key");
        assert_eq!(tr("another-missing-key"), "another-missing-key");
        assert_eq!(format_from_bundle("en", "nonexistent-key", &[]), None);
        assert_eq!(format_from_bundle("zh", "nonexistent-key", &[]), None);
    }
}
