// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Message-level i18n for the graph server (axum binary).
//!
//! Standalone mirror of the main crate's `codenexus::i18n` — the server
//! crate is deliberately lightweight (no dependency on the `codenexus`
//! library) but embeds the *same* shared `locales/{en,zh}/messages.ftl`
//! files, which remain the single source of truth for message text.
//!
//! Detection chain (forced order): `CODENEXUS_LANG` → `LC_ALL` →
//! `LC_MESSAGES` → `LANG` → `sys-locale` → `"en"`; domain is exactly
//! `{en, zh}`. [`t`] never panics; missing keys resolve to the key itself.

use std::str::FromStr;
use std::sync::OnceLock;

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

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

const EN_FTL: &str = EMBEDDED_LOCALES[0].1[0].1;
const ZH_FTL: &str = EMBEDDED_LOCALES[1].1[0].1;

static EN_BUNDLE: OnceLock<FluentBundle<FluentResource>> = OnceLock::new();
static ZH_BUNDLE: OnceLock<FluentBundle<FluentResource>> = OnceLock::new();

static GLOBAL_LOCALE: OnceLock<LanguageIdentifier> = OnceLock::new();

/// Warm the detection chain eagerly (called from `main`); [`t`] would
/// lazily initialize anyway.
pub fn init() {
    let _ = GLOBAL_LOCALE.get_or_init(detect_locale);
}

/// Detect the locale using the forced chain (see module docs).
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

/// Normalize a raw locale string to one of the two supported languages.
/// `zh-*`/`en-*` variants collapse to bare `zh`/`en`; `C`/`POSIX` and
/// unsupported languages yield `None` so the detection chain keeps walking.
fn normalize(raw: &str) -> Option<LanguageIdentifier> {
    let s = raw.split('@').next()?.split('.').next()?.replace('_', "-");
    if matches!(s.as_str(), "" | "C" | "POSIX") {
        return None;
    }
    let id = LanguageIdentifier::from_str(&s).ok()?;
    match id.language.as_str() {
        "zh" => Some(LanguageIdentifier::from_str("zh").expect("'zh' is a valid language id")),
        "en" => Some(LanguageIdentifier::from_str("en").expect("'en' is a valid language id")),
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
    LanguageIdentifier::from_str("en").expect("'en' is a valid language identifier")
}

/// Get the current locale (the once-detected default).
pub fn current_locale() -> LanguageIdentifier {
    GLOBAL_LOCALE.get_or_init(detect_locale).clone()
}

/// Translate `key` for the current locale, formatting `{ $var }` with
/// `args`. Fallback: current language → `en` → the key itself. Never panics.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

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

    /// Guard: the shared catalog's EN/ZH key sets must stay identical.
    #[test]
    fn test_key_parity() {
        let en = ftl_keys(EN_FTL);
        let zh = ftl_keys(ZH_FTL);
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
        assert_eq!(
            format_from_bundle("en", "project-not-found", &[("name", "demo".to_string())]),
            Some("Project 'demo' not found".to_string())
        );
        assert_eq!(
            format_from_bundle("zh", "project-not-found", &[("name", "demo".to_string())]),
            Some("项目 'demo' 未找到".to_string())
        );
        // Unknown language → en bundle; missing key → None → key itself.
        assert_eq!(
            format_from_bundle("ar", "node-not-found", &[("name", "x".to_string())]),
            Some("Node 'x' not found".to_string())
        );
        assert_eq!(t("nonexistent-key", &[]), "nonexistent-key");
        assert_eq!(tr("another-missing-key"), "another-missing-key");
    }

    #[test]
    fn test_detection_chain_pure_logic() {
        let lang = |r: LanguageIdentifier| r.to_string();
        // Project override beats POSIX chain; zh-TW collapses to zh.
        assert_eq!(
            lang(resolve_inputs(
                Some("zh_TW"),
                Some("en_US"),
                None,
                None,
                None
            )),
            "zh"
        );
        // C/POSIX and unsupported languages fall through; terminal is en.
        assert_eq!(
            lang(resolve_inputs(
                Some("C"),
                Some("fr_FR"),
                None,
                Some("zh_CN"),
                None
            )),
            "zh"
        );
        assert_eq!(lang(resolve_inputs(None, None, None, None, None)), "en");
    }
}
