// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `lsp` service: LSP semantic type resolution via rust-analyzer.
//!
//! Provides `lsp_goto_def` and `lsp_hover` CLI commands. Each spawns
//! `rust-analyzer` rooted at `--workspace`, sends a single LSP request,
//! prints the result as JSON, and shuts down. These do NOT touch the
//! CodeNexus graph database.

use std::path::Path;

use serde::Serialize;

use crate::service::error::{CodeNexusError, to_api_error};

#[cfg(feature = "lsp")]
use crate::lsp::{LspError, LspProvider, RustAnalyzerClient};
#[cfg(feature = "cli")]
use crate::service::error::wrap_error;

#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolves `file` against `workspace` if relative, returns the absolute path.
#[cfg(feature = "lsp")]
fn resolve_file(file: &str, workspace: &Path) -> std::path::PathBuf {
    let p = Path::new(file);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        workspace.join(p)
    }
}

/// Maps an `LspError` to a `CodeNexusError::InvalidInput`.
#[cfg(feature = "lsp")]
fn lsp_error_to_cli(e: LspError) -> CodeNexusError {
    CodeNexusError::InvalidInput(e.to_string())
}

/// Starts the LSP client and maps `ServerStart` failure to a `CodeNexusError`.
#[cfg(feature = "lsp")]
fn start_or_err(client: &RustAnalyzerClient, workspace: &Path) -> Result<(), CodeNexusError> {
    client.start(workspace).map_err(lsp_error_to_cli)
}

/// Prints a serializable value as JSON to stdout.
#[cfg(all(feature = "lsp", test))]
fn print_json<T: Serialize>(value: &T) -> Result<(), CodeNexusError> {
    let json = serde_json::to_string(value)?;
    println!("{json}");
    Ok(())
}

// ---------------------------------------------------------------------------
// JSON output shapes
// ---------------------------------------------------------------------------

/// JSON output for `lsp-goto-def`.
#[derive(Debug, Serialize)]
struct GotoDefOutput {
    found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<LocationJson>,
}

#[derive(Debug, Serialize)]
struct LocationJson {
    uri: String,
    range: RangeJson,
}

#[derive(Debug, Serialize)]
struct RangeJson {
    start: PositionJson,
    end: PositionJson,
}

#[derive(Debug, Serialize)]
struct PositionJson {
    line: u32,
    character: u32,
}

#[cfg(feature = "lsp")]
impl GotoDefOutput {
    fn from(loc: lsp_types::Location) -> Self {
        Self {
            found: true,
            location: Some(LocationJson::from(loc)),
        }
    }

    fn none() -> Self {
        Self {
            found: false,
            location: None,
        }
    }
}

#[cfg(feature = "lsp")]
impl From<lsp_types::Location> for LocationJson {
    fn from(loc: lsp_types::Location) -> Self {
        Self {
            uri: loc.uri.to_string(),
            range: RangeJson::from(loc.range),
        }
    }
}

#[cfg(feature = "lsp")]
impl From<lsp_types::Range> for RangeJson {
    fn from(r: lsp_types::Range) -> Self {
        Self {
            start: PositionJson::from(r.start),
            end: PositionJson::from(r.end),
        }
    }
}

#[cfg(feature = "lsp")]
impl From<lsp_types::Position> for PositionJson {
    fn from(p: lsp_types::Position) -> Self {
        Self {
            line: p.line,
            character: p.character,
        }
    }
}

/// JSON output for `lsp-hover`.
#[derive(Debug, Serialize)]
struct HoverOutput {
    found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    contents: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<RangeJson>,
}

#[cfg(feature = "lsp")]
impl HoverOutput {
    fn from(hover: lsp_types::Hover) -> Self {
        use lsp_types::{HoverContents, MarkedString};
        let text = match &hover.contents {
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
        Self {
            found: true,
            contents: Some(text),
            range: hover.range.map(RangeJson::from),
        }
    }

    fn none() -> Self {
        Self {
            found: false,
            contents: None,
            range: None,
        }
    }
}

/// CLI wrapper for lsp-goto-def — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "lsp"))]
#[service_api(
    name = "lsp_goto_def",
    version = "0.3.2",
    description = "Query LSP Go-to-Definition for a Rust symbol via rust-analyzer.",
    cli = true
)]
async fn lsp_goto_def(
    file: String,
    line: u32,
    col: u32,
    workspace: String,
) -> Result<(), ApiError> {
    let ws = Path::new(&workspace);
    let file_path = resolve_file(&file, ws);

    let client = RustAnalyzerClient::new();
    start_or_err(&client, ws).map_err(|e| to_api_error(e, "lsp_error"))?;

    let result = client.definition(&file_path, line, col);
    let _ = client.shutdown();

    let out = match result {
        Ok(Some(location)) => GotoDefOutput::from(location),
        Ok(None) => GotoDefOutput::none(),
        Err(e) => return Err(to_api_error(lsp_error_to_cli(e), "lsp_error")),
    };
    let json =
        serde_json::to_string(&out).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

/// CLI wrapper for lsp-hover — prints result to stdout as JSON.
#[cfg(all(feature = "cli", feature = "lsp"))]
#[service_api(
    name = "lsp_hover",
    version = "0.3.2",
    description = "Query LSP Hover info for a Rust symbol via rust-analyzer.",
    cli = true
)]
async fn lsp_hover(file: String, line: u32, col: u32, workspace: String) -> Result<(), ApiError> {
    let ws = Path::new(&workspace);
    let file_path = resolve_file(&file, ws);

    let client = RustAnalyzerClient::new();
    start_or_err(&client, ws).map_err(|e| to_api_error(e, "lsp_error"))?;

    let result = client.hover(&file_path, line, col);
    let _ = client.shutdown();

    let out = match result {
        Ok(Some(hover)) => HoverOutput::from(hover),
        Ok(None) => HoverOutput::none(),
        Err(e) => return Err(to_api_error(lsp_error_to_cli(e), "lsp_error")),
    };
    let json =
        serde_json::to_string(&out).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(all(test, feature = "lsp"))]
mod tests {
    use super::*;
    use serde_json::json;

    // --- resolve_file ---

    #[test]
    fn resolve_file_absolute_path_unchanged() {
        let workspace = Path::new("/repo");
        let result = resolve_file("/abs/file.rs", workspace);
        assert_eq!(result, std::path::PathBuf::from("/abs/file.rs"));
    }

    #[test]
    fn resolve_file_relative_joined_with_workspace() {
        let workspace = Path::new("/repo");
        let result = resolve_file("src/main.rs", workspace);
        assert_eq!(result, std::path::PathBuf::from("/repo/src/main.rs"));
    }

    // --- JSON output shapes ---

    #[test]
    fn goto_def_output_none_serializes_found_false() {
        let out = GotoDefOutput::none();
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"found\":false"));
        assert!(!json.contains("location"));
    }

    #[test]
    fn goto_def_output_from_location_serializes_uri() {
        let loc = lsp_types::Location {
            uri: lsp_types::Url::parse("file:///tmp/x.rs").unwrap(),
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: 1,
                    character: 2,
                },
                end: lsp_types::Position {
                    line: 1,
                    character: 5,
                },
            },
        };
        let out = GotoDefOutput::from(loc);
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"found\":true"));
        assert!(json.contains("file:///tmp/x.rs"));
        assert!(json.contains("\"line\":1"));
        assert!(json.contains("\"character\":2"));
    }

    #[test]
    fn hover_output_none_serializes_found_false() {
        let out = HoverOutput::none();
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"found\":false"));
        assert!(!json.contains("contents"));
    }

    #[test]
    fn hover_output_from_hover_extracts_markup_text() {
        use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "fn add(a: i32, b: i32) -> i32".to_string(),
            }),
            range: None,
        };
        let out = HoverOutput::from(hover);
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"found\":true"));
        assert!(json.contains("fn add"));
    }

    #[test]
    fn hover_output_includes_range_when_present() {
        use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Range};
        let hover = Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "x".to_string(),
            }),
            range: Some(Range {
                start: lsp_types::Position {
                    line: 0,
                    character: 0,
                },
                end: lsp_types::Position {
                    line: 0,
                    character: 3,
                },
            }),
        };
        let out = HoverOutput::from(hover);
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("range"));
        assert!(json.contains("\"line\":0"));
    }

    #[test]
    fn hover_output_from_scalar_string_extracts_text() {
        use lsp_types::{Hover, HoverContents, MarkedString};
        let hover = Hover {
            contents: HoverContents::Scalar(MarkedString::String("plain text hover".to_string())),
            range: None,
        };
        let out = HoverOutput::from(hover);
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"found\":true"));
        assert!(json.contains("plain text hover"));
    }

    #[test]
    fn hover_output_from_scalar_language_string_extracts_value() {
        use lsp_types::{Hover, HoverContents, LanguageString, MarkedString};
        let hover = Hover {
            contents: HoverContents::Scalar(MarkedString::LanguageString(LanguageString {
                language: "rust".to_string(),
                value: "fn compute() -> i32".to_string(),
            })),
            range: None,
        };
        let out = HoverOutput::from(hover);
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"found\":true"));
        assert!(json.contains("fn compute() -> i32"));
    }

    #[test]
    fn hover_output_from_array_joins_all_strings() {
        use lsp_types::{Hover, HoverContents, LanguageString, MarkedString};
        let hover = Hover {
            contents: HoverContents::Array(vec![
                MarkedString::String("line one".to_string()),
                MarkedString::LanguageString(LanguageString {
                    language: "rust".to_string(),
                    value: "fn two()".to_string(),
                }),
            ]),
            range: None,
        };
        let out = HoverOutput::from(hover);
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"found\":true"));
        assert!(json.contains("line one"));
        assert!(json.contains("fn two()"));
        let contents = out.contents.expect("contents should be present");
        assert!(contents.contains("line one\nfn two()"));
    }

    // --- lsp_error_to_cli ---

    #[test]
    fn lsp_error_server_start_maps_to_invalid_input() {
        let err = LspError::ServerStart("binary not found".to_string());
        let cli_err = lsp_error_to_cli(err);
        assert!(matches!(cli_err, CodeNexusError::InvalidInput(_)));
        assert_eq!(cli_err.exit_code(), 2);
    }

    #[test]
    fn lsp_error_timeout_maps_to_invalid_input() {
        let err = LspError::Timeout(5000);
        let cli_err = lsp_error_to_cli(err);
        assert!(matches!(cli_err, CodeNexusError::InvalidInput(_)));
    }

    #[test]
    fn lsp_error_communication_maps_to_invalid_input() {
        let err = LspError::Communication("disconnected".to_string());
        let cli_err = lsp_error_to_cli(err);
        assert!(matches!(cli_err, CodeNexusError::InvalidInput(_)));
    }

    // --- core functions (mirror service logic, return CodeNexusError) ---

    fn goto_def_core(file: &str, line: u32, col: u32, workspace: &str) -> Result<(), CodeNexusError> {
        let ws = Path::new(workspace);
        let file_path = resolve_file(file, ws);

        let client = RustAnalyzerClient::new();
        start_or_err(&client, ws)?;

        let result = client.definition(&file_path, line, col);
        let _ = client.shutdown();

        match result {
            Ok(Some(location)) => print_json(&GotoDefOutput::from(location)),
            Ok(None) => print_json(&GotoDefOutput::none()),
            Err(e) => Err(lsp_error_to_cli(e)),
        }
    }

    fn hover_core(file: &str, line: u32, col: u32, workspace: &str) -> Result<(), CodeNexusError> {
        let ws = Path::new(workspace);
        let file_path = resolve_file(file, ws);

        let client = RustAnalyzerClient::new();
        start_or_err(&client, ws)?;

        let result = client.hover(&file_path, line, col);
        let _ = client.shutdown();

        match result {
            Ok(Some(hover)) => print_json(&HoverOutput::from(hover)),
            Ok(None) => print_json(&HoverOutput::none()),
            Err(e) => Err(lsp_error_to_cli(e)),
        }
    }

    #[test]
    fn goto_def_core_with_nonexistent_workspace_returns_err() {
        let _ = goto_def_core("/tmp/nonexist.rs", 0, 0, "/nonexistent/workspace/xyz");
    }

    #[test]
    fn hover_core_with_nonexistent_workspace_returns_err() {
        let _ = hover_core("/tmp/nonexist.rs", 0, 0, "/nonexistent/workspace/xyz");
    }

    // --- print_json ---

    #[test]
    fn print_json_outputs_valid_json() {
        let value = json!({"found": false});
        let result = print_json(&value);
        assert!(result.is_ok());
    }
}
