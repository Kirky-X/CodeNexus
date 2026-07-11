// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

/// Extracts the first non-empty line from an LSP [`Hover`] response as the
/// `semantic_type` string (R-lsp-004). Truncates to 200 chars to keep the
/// property lean.
///
/// Shared by the service-layer LSP enhancement (`service::index::enhance_with_lsp`) and
/// the mock-testable pipeline unit (`index::pipeline::enhance_with_lsp`) so the
/// extraction contract is defined once (Rule 8: no duplicate implementations).
pub fn extract_hover_text(hover: &lsp_types::Hover) -> Option<String> {
    use lsp_types::{HoverContents, MarkedString};

    let raw = match &hover.contents {
        HoverContents::Scalar(MarkedString::String(s)) => s.clone(),
        HoverContents::Scalar(MarkedString::LanguageString(ls)) => ls.value.clone(),
        HoverContents::Array(vec) => vec
            .iter()
            .map(|ms| match ms {
                MarkedString::String(s) => s.clone(),
                MarkedString::LanguageString(ls) => ls.value.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        HoverContents::Markup(mc) => mc.value.clone(),
    };

    let first_line = raw.lines().find(|l| !l.trim().is_empty())?;
    let truncated = if first_line.len() > 200 {
        &first_line[..200]
    } else {
        first_line
    };
    if truncated.is_empty() {
        None
    } else {
        Some(truncated.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::extract_hover_text;

    #[test]
    fn extract_hover_text_from_markup_content() {
        use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "fn add(a: i32, b: i32) -> i32\n\nAdds two numbers.".to_string(),
            }),
            range: None,
        };
        let text = extract_hover_text(&hover).expect("should extract text");
        assert_eq!(text, "fn add(a: i32, b: i32) -> i32");
    }

    #[test]
    fn extract_hover_text_from_scalar_string() {
        use lsp_types::{Hover, HoverContents, MarkedString};
        let hover = Hover {
            contents: HoverContents::Scalar(MarkedString::String(
                "struct Foo\n\nA struct.".to_string(),
            )),
            range: None,
        };
        let text = extract_hover_text(&hover).expect("should extract text");
        assert_eq!(text, "struct Foo");
    }

    #[test]
    fn extract_hover_text_from_language_string() {
        use lsp_types::{Hover, HoverContents, LanguageString, MarkedString};
        let hover = Hover {
            contents: HoverContents::Scalar(MarkedString::LanguageString(LanguageString {
                language: "rust".to_string(),
                value: "fn main()".to_string(),
            })),
            range: None,
        };
        let text = extract_hover_text(&hover).expect("should extract text");
        assert_eq!(text, "fn main()");
    }

    #[test]
    fn extract_hover_text_from_array_joins_lines() {
        use lsp_types::{Hover, HoverContents, MarkedString};
        let hover = Hover {
            contents: HoverContents::Array(vec![
                MarkedString::String("fn foo()".to_string()),
                MarkedString::String("fn bar()".to_string()),
            ]),
            range: None,
        };
        let text = extract_hover_text(&hover).expect("should extract text");
        assert_eq!(text, "fn foo()");
    }

    #[test]
    fn extract_hover_text_skips_empty_lines() {
        use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "\n\n\nfn real_signature()\n".to_string(),
            }),
            range: None,
        };
        let text = extract_hover_text(&hover).expect("should extract text");
        assert_eq!(text, "fn real_signature()");
    }

    #[test]
    fn extract_hover_text_returns_none_for_empty() {
        use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "".to_string(),
            }),
            range: None,
        };
        assert_eq!(extract_hover_text(&hover), None);
    }

    #[test]
    fn extract_hover_text_truncates_long_lines() {
        use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};
        let long_line = "x".repeat(300);
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: long_line,
            }),
            range: None,
        };
        let text = extract_hover_text(&hover).expect("should extract text");
        assert_eq!(text.len(), 200);
    }
}
