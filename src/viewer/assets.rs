// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! 前端资源：编译期嵌入 `graph-viewer/dist` 的运行时资产；缺失时呈回退页。

use rust_embed::Embed;

use crate::i18n::tr;

/// 编译期嵌入的前端构建产物。
///
/// 白名单只含运行时资产（`index.html`、`assets/**`、图标 png），
/// 排除 e2e fixture（`*.lbug`）；`graph-viewer/dist` 不存在时
/// `allow_missing` 使嵌入集合为空，由 [`fallback_page`] 承担回退。
#[derive(Embed)]
#[folder = "graph-viewer/dist"]
#[include = "index.html"]
#[include = "assets/**"]
#[include = "*.png"]
#[exclude = "*.lbug"]
#[allow_missing = true]
struct ViewerAssets;

/// 按扩展名解析 content-type（嵌入资产与静态服务共用同一路径）。
#[must_use]
pub(super) fn content_type_for(rel: &str) -> String {
    mime_guess::from_path(rel)
        .first_or_octet_stream()
        .essence_str()
        .to_string()
}

/// 查询嵌入资产，命中时返回（内容, content-type）。
#[must_use]
pub(super) fn get_asset(rel: &str) -> Option<(Vec<u8>, String)> {
    let file = ViewerAssets::get(rel)?;
    Some((file.data.into_owned(), content_type_for(rel)))
}

/// dist 缺失时的回退页（含构建指引）。
#[must_use]
pub(super) fn fallback_page() -> String {
    // FTL 文案为编译期静态资源；仍做最小 HTML 转义，防未来文案/翻译引入标记。
    let message = tr("graph-viewer-dist-missing")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let title = tr("graph-viewer-fallback-title")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let heading = tr("graph-viewer-fallback-heading")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title}</title></head>\
         <body style=\"font-family: system-ui, sans-serif; margin: 3rem auto; max-width: 40rem; line-height: 1.6;\">\
         <h1>{heading}</h1><p>{message}</p></body></html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lbug_fixtures_are_never_embedded() {
        assert!(ViewerAssets::get("test_e2e.lbug").is_none());
        assert!(ViewerAssets::get("e2e_test.lbug").is_none());
        assert!(!ViewerAssets::iter().any(|p| p.ends_with(".lbug")));
    }

    #[test]
    fn embedded_set_matches_dist_presence() {
        if ViewerAssets::get("index.html").is_some() {
            assert!(ViewerAssets::iter().any(|p| p.starts_with("assets/")));
            let entry = get_asset("index.html");
            assert!(entry.is_some_and(|(bytes, ct)| !bytes.is_empty() && ct == "text/html"));
        } else {
            assert_eq!(ViewerAssets::iter().count(), 0);
            assert!(fallback_page().contains("npm run build"));
        }
    }

    /// The fallback page's title and heading come from the catalog (not from
    /// a hardcoded literal), so both must render through `init()`.
    #[test]
    #[serial_test::serial(i18n_catalog)]
    fn fallback_page_title_and_heading_come_from_catalog() {
        crate::i18n::init();
        let page = fallback_page();
        assert!(
            page.contains(&tr("graph-viewer-fallback-title")),
            "title must be the catalog entry: {page}"
        );
        assert!(
            page.contains(&tr("graph-viewer-fallback-heading")),
            "heading must be the catalog entry: {page}"
        );
        assert!(
            page.contains(&tr("graph-viewer-dist-missing")),
            "body guidance must stay translated: {page}"
        );
    }

    #[test]
    fn content_types_follow_extension() {
        assert_eq!(content_type_for("index.html"), "text/html");
        assert_eq!(content_type_for("assets/app.css"), "text/css");
        assert!(content_type_for("assets/app.js").contains("javascript"));
        assert_eq!(content_type_for("assets/engine.wasm"), "application/wasm");
        assert_eq!(content_type_for("assets/font.woff2"), "font/woff2");
        assert_eq!(content_type_for("CodeNexus-128.png"), "image/png");
    }
}
