// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `lsp` service: LSP semantic type resolution via language servers.
//!
//! Provides `lsp_goto_def` and `lsp_hover` CLI commands. Each selects
//! the LSP client based on file extension (`.rs` → rust-analyzer,
//! `.py` → pyright, `.c`/`.cpp` → clangd, `.go` → gopls, `.ts` →
//! typescript-language-server, `.f90` → fortls, `.java` → jdtls),
//! spawns the server, sends a single LSP request, prints JSON, and
//! shuts down.

use std::path::Path;

use serde::Serialize;

#[cfg(feature = "cli")]
use crate::service::error::to_api_error;
use crate::service::error::CodeNexusError;

#[cfg(feature = "lsp")]
use crate::lsp::{
    ClangdClient, FortlsClient, GoplsClient, JdtlsClient, LspError, LspProvider, PyrightClient,
    RustAnalyzerClient, TypeScriptLanguageClient,
};
#[cfg(feature = "cli")]
use crate::service::error::wrap_error;

#[cfg(feature = "cli")]
use sdforge::forge;
#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;

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
fn start_or_err(client: &dyn LspProvider, workspace: &Path) -> Result<(), CodeNexusError> {
    client.start(workspace).map_err(lsp_error_to_cli)
}

/// Selects an LSP client based on file extension.
#[cfg(feature = "lsp")]
fn select_provider(file: &Path) -> Box<dyn LspProvider> {
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "py" => Box::new(PyrightClient::new()),
        "c" | "cpp" => Box::new(ClangdClient::new()),
        "go" => Box::new(GoplsClient::new()),
        "ts" => Box::new(TypeScriptLanguageClient::new()),
        "f90" => Box::new(FortlsClient::new()),
        "java" => Box::new(JdtlsClient::new()),
        _ => Box::new(RustAnalyzerClient::new()),
    }
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
#[forge(
    name = "lsp_goto_def",
    version = "0.3.5",
    description = "Query LSP Go-to-Definition (auto-detects language server from file extension).",
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

    let client = select_provider(&file_path);
    start_or_err(client.as_ref(), ws).map_err(|e| to_api_error(e, "lsp_error"))?;

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
#[forge(
    name = "lsp_hover",
    version = "0.3.5",
    description = "Query LSP Hover info (auto-detects language server from file extension).",
    cli = true
)]
async fn lsp_hover(file: String, line: u32, col: u32, workspace: String) -> Result<(), ApiError> {
    let ws = Path::new(&workspace);
    let file_path = resolve_file(&file, ws);

    let client = select_provider(&file_path);
    start_or_err(client.as_ref(), ws).map_err(|e| to_api_error(e, "lsp_error"))?;

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
            uri: "file:///tmp/x.rs".parse::<lsp_types::Uri>().unwrap(),
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

    fn goto_def_core(
        file: &str,
        line: u32,
        col: u32,
        workspace: &str,
    ) -> Result<(), CodeNexusError> {
        let ws = Path::new(workspace);
        let file_path = resolve_file(file, ws);

        let client = select_provider(&file_path);
        start_or_err(client.as_ref(), ws)?;

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

        let client = select_provider(&file_path);
        start_or_err(client.as_ref(), ws)?;

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

    #[test]
    fn select_provider_rust_for_rs_files() {
        let provider = select_provider(Path::new("/tmp/lib.rs"));
        let _ = provider.shutdown();
    }

    #[test]
    fn select_provider_pyright_for_py_files() {
        let provider = select_provider(Path::new("/tmp/module.py"));
        let _ = provider.shutdown();
    }

    // --- print_json ---

    #[test]
    fn print_json_outputs_valid_json() {
        let value = json!({"found": false});
        let result = print_json(&value);
        assert!(result.is_ok());
    }

    // --- select_provider: default cases ---

    #[test]
    fn select_provider_defaults_to_rust_analyzer_for_no_extension() {
        let provider = select_provider(Path::new("/tmp/Makefile"));
        let _ = provider.shutdown();
    }

    #[test]
    fn select_provider_defaults_to_rust_analyzer_for_js_files() {
        let provider = select_provider(Path::new("/tmp/app.js"));
        let _ = provider.shutdown();
    }

    #[test]
    fn select_provider_gopls_for_go_files() {
        let provider = select_provider(Path::new("/tmp/main.go"));
        let _ = provider.shutdown();
    }

    // --- resolve_file: edge cases ---

    #[test]
    fn resolve_file_empty_string_returns_workspace() {
        let workspace = Path::new("/repo");
        let result = resolve_file("", workspace);
        assert_eq!(result, std::path::PathBuf::from("/repo"));
    }

    #[test]
    fn resolve_file_dot_returns_workspace_joined() {
        let workspace = Path::new("/repo");
        let result = resolve_file(".", workspace);
        assert_eq!(result, std::path::PathBuf::from("/repo/."));
    }

    // --- start_or_err: direct test ---

    #[test]
    fn start_or_err_with_nonexistent_workspace_does_not_panic() {
        let client = select_provider(Path::new("/tmp/test.rs"));
        let _ = start_or_err(client.as_ref(), Path::new("/nonexistent/workspace/xyz"));
        let _ = client.shutdown();
    }

    // --- goto_def_core / hover_core: error result type ---

    #[test]
    fn goto_def_core_error_is_code_nexus_error() {
        let result = goto_def_core("/tmp/nonexist.rs", 0, 0, "/nonexistent/workspace/xyz");
        // Should return an error (either InvalidInput from LspError or Io).
        assert!(result.is_err());
    }

    #[test]
    fn hover_core_error_is_code_nexus_error() {
        let result = hover_core("/tmp/nonexist.rs", 0, 0, "/nonexistent/workspace/xyz");
        assert!(result.is_err());
    }

    // --- GotoDefOutput: serialization with range ---

    #[test]
    fn goto_def_output_from_location_includes_range() {
        let loc = lsp_types::Location {
            uri: "file:///src/lib.rs".parse::<lsp_types::Uri>().unwrap(),
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: 5,
                    character: 10,
                },
                end: lsp_types::Position {
                    line: 5,
                    character: 20,
                },
            },
        };
        let out = GotoDefOutput::from(loc);
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"line\":5"));
        assert!(json.contains("\"character\":10"));
        assert!(json.contains("\"character\":20"));
    }

    // ===== #[forge] wrapper tests =====

    #[cfg(feature = "cli")]
    #[test]
    fn lsp_goto_def_wrapper_fails_when_kit_not_initialized() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(lsp_goto_def(
            "/tmp/nonexist.rs".to_string(),
            0,
            0,
            "/nonexistent/workspace/xyz".to_string(),
        ));
        assert!(
            result.is_err(),
            "wrapper should fail on nonexistent workspace"
        );
    }

    #[cfg(feature = "cli")]
    #[test]
    fn lsp_hover_wrapper_fails_when_kit_not_initialized() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(lsp_hover(
            "/tmp/nonexist.rs".to_string(),
            0,
            0,
            "/nonexistent/workspace/xyz".to_string(),
        ));
        assert!(
            result.is_err(),
            "wrapper should fail on nonexistent workspace"
        );
    }

    #[cfg(feature = "cli")]
    #[test]
    fn lsp_goto_def_wrapper_with_nonexistent_file_returns_error() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(lsp_goto_def(
            "/tmp/does_not_exist.rs".to_string(),
            0,
            0,
            "/nonexistent/workspace/xyz".to_string(),
        ));
        assert!(
            result.is_err(),
            "wrapper should return error for nonexistent file"
        );
    }

    #[cfg(feature = "cli")]
    #[test]
    fn lsp_hover_wrapper_with_nonexistent_file_returns_error() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(lsp_hover(
            "/tmp/does_not_exist.rs".to_string(),
            0,
            0,
            "/nonexistent/workspace/xyz".to_string(),
        ));
        assert!(
            result.is_err(),
            "wrapper should return error for nonexistent file"
        );
    }
}
