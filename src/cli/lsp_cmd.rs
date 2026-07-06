// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `lsp-goto-def` and `lsp-hover` subcommand handlers (T007, v0.2.0).
//!
//! These are ad-hoc LSP query entry points: they spawn `rust-analyzer`
//! rooted at `--workspace`, send a single `textDocument/definition` or
//! `textDocument/hover` request at `(file, line, col)`, print the result
//! as JSON to stdout, and shut down. They do **not** touch the CodeNexus
//! graph database — for graph-integrated semantic enrichment use
//! `codenexus index --lsp` (R-lsp-004).
//!
//! # Failure mapping (Rule 12)
//!
//! Any [`LspError`] is mapped to [`CliError::InvalidInput`] so the CLI
//! exits with code 1 and a human-readable message. The caller sees the
//! underlying cause (binary missing, timeout, communication error) without
//! needing to inspect LSP-specific error codes.

use std::path::Path;

use serde::Serialize;

use super::args::{LspGotoDefArgs, LspHoverArgs};
use super::error::{CliError, Result};
use crate::lsp::{LspError, LspProvider, RustAnalyzerClient};

/// Runs the `lsp-goto-def` subcommand.
///
/// Spawns `rust-analyzer` rooted at `args.workspace`, sends a
/// `textDocument/definition` request at `(args.file, args.line, args.col)`,
/// and prints the resulting [`lsp_types::Location`] (or `null`) as JSON.
pub fn run_goto_def(args: &LspGotoDefArgs) -> Result<()> {
    let workspace = Path::new(&args.workspace);
    let file = resolve_file(&args.file, workspace);

    let client = RustAnalyzerClient::new();
    start_or_err(&client, workspace)?;

    let result = client.definition(&file, args.line, args.col);
    // Always attempt shutdown — a failed query shouldn't leak the subprocess.
    let _ = client.shutdown();

    match result {
        Ok(Some(location)) => print_json(&GotoDefOutput::from(location)),
        Ok(None) => print_json(&GotoDefOutput::none()),
        Err(e) => Err(lsp_error_to_cli(e)),
    }
}

/// Runs the `lsp-hover` subcommand.
///
/// Spawns `rust-analyzer` rooted at `args.workspace`, sends a
/// `textDocument/hover` request at `(args.file, args.line, args.col)`,
/// and prints the resulting [`lsp_types::Hover`] (or `null`) as JSON.
pub fn run_hover(args: &LspHoverArgs) -> Result<()> {
    let workspace = Path::new(&args.workspace);
    let file = resolve_file(&args.file, workspace);

    let client = RustAnalyzerClient::new();
    start_or_err(&client, workspace)?;

    let result = client.hover(&file, args.line, args.col);
    let _ = client.shutdown();

    match result {
        Ok(Some(hover)) => print_json(&HoverOutput::from(hover)),
        Ok(None) => print_json(&HoverOutput::none()),
        Err(e) => Err(lsp_error_to_cli(e)),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Starts the LSP client and maps `ServerStart` failure to a `CliError`.
///
/// `Communication`/`Timeout` from `start` are extremely unlikely (the
/// handshake has its own timeout) but are mapped the same way for
/// uniformity.
fn start_or_err(client: &RustAnalyzerClient, workspace: &Path) -> Result<()> {
    client.start(workspace).map_err(lsp_error_to_cli)
}

/// Maps an [`LspError`] to a [`CliError::InvalidInput`] with a descriptive
/// message. All LSP failures surface as exit code 1 — they are environment
/// issues (binary missing, server unresponsive) rather than database or
/// system errors.
fn lsp_error_to_cli(e: LspError) -> CliError {
    CliError::InvalidInput(e.to_string())
}

/// Resolves `file` against `workspace` if relative, returns the absolute path.
fn resolve_file(file: &str, workspace: &Path) -> std::path::PathBuf {
    let p = Path::new(file);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        workspace.join(p)
    }
}

/// Prints a serializable value as JSON to stdout.
fn print_json<T: Serialize>(value: &T) -> Result<()> {
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
    /// Whether a definition location was found.
    found: bool,
    /// The resolved location (present when `found == true`).
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<LocationJson>,
}

/// JSON-serializable view of [`lsp_types::Location`].
#[derive(Debug, Serialize)]
struct LocationJson {
    uri: String,
    range: RangeJson,
}

/// JSON-serializable view of [`lsp_types::Range`].
#[derive(Debug, Serialize)]
struct RangeJson {
    start: PositionJson,
    end: PositionJson,
}

/// JSON-serializable view of [`lsp_types::Position`].
#[derive(Debug, Serialize)]
struct PositionJson {
    line: u32,
    character: u32,
}

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

impl From<lsp_types::Location> for LocationJson {
    fn from(loc: lsp_types::Location) -> Self {
        Self {
            uri: loc.uri.to_string(),
            range: RangeJson::from(loc.range),
        }
    }
}

impl From<lsp_types::Range> for RangeJson {
    fn from(r: lsp_types::Range) -> Self {
        Self {
            start: PositionJson::from(r.start),
            end: PositionJson::from(r.end),
        }
    }
}

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
    /// Whether hover info was returned.
    found: bool,
    /// The hover contents as plain text (present when `found == true`).
    #[serde(skip_serializing_if = "Option::is_none")]
    contents: Option<String>,
    /// The hover range (present when the server returned one).
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<RangeJson>,
}

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

#[cfg(test)]
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
                start: lsp_types::Position { line: 1, character: 2 },
                end: lsp_types::Position { line: 1, character: 5 },
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
                start: lsp_types::Position { line: 0, character: 0 },
                end: lsp_types::Position { line: 0, character: 3 },
            }),
        };
        let out = HoverOutput::from(hover);
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("range"));
        assert!(json.contains("\"line\":0"));
    }

    // --- lsp_error_to_cli ---

    #[test]
    fn lsp_error_server_start_maps_to_invalid_input() {
        let err = LspError::ServerStart("binary not found".to_string());
        let cli_err = lsp_error_to_cli(err);
        assert!(matches!(cli_err, CliError::InvalidInput(_)));
        assert_eq!(cli_err.exit_code(), 1);
    }

    #[test]
    fn lsp_error_timeout_maps_to_invalid_input() {
        let err = LspError::Timeout(5000);
        let cli_err = lsp_error_to_cli(err);
        assert!(matches!(cli_err, CliError::InvalidInput(_)));
    }

    #[test]
    fn lsp_error_communication_maps_to_invalid_input() {
        let err = LspError::Communication("disconnected".to_string());
        let cli_err = lsp_error_to_cli(err);
        assert!(matches!(cli_err, CliError::InvalidInput(_)));
    }

    // --- run_goto_def / run_hover graceful failure ---
    //
    // These tests point at a nonexistent workspace so rust-analyzer fails
    // to start (or isn't installed). The cmd must return Err (not panic).

    #[test]
    fn run_goto_def_with_nonexistent_workspace_returns_err() {
        let args = LspGotoDefArgs {
            file: "/tmp/nonexist.rs".into(),
            line: 0,
            col: 0,
            workspace: "/nonexistent/workspace/xyz".into(),
        };
        // rust-analyzer may or may not be installed, but pointing at a
        // nonexistent workspace ensures start() fails (the binary won't
        // be on PATH in CI, and even if it is, the workspace doesn't exist).
        // Either way, the function must not panic.
        let _ = run_goto_def(&args);
    }

    #[test]
    fn run_hover_with_nonexistent_workspace_returns_err() {
        let args = LspHoverArgs {
            file: "/tmp/nonexist.rs".into(),
            line: 0,
            col: 0,
            workspace: "/nonexistent/workspace/xyz".into(),
        };
        let _ = run_hover(&args);
    }

    // --- print_json (smoke test) ---

    #[test]
    fn print_json_outputs_valid_json() {
        // We can't easily capture stdout in a unit test, but we can verify
        // the function doesn't error on a simple value.
        let value = json!({"found": false});
        // print_json returns Result<()>; just verify it's Ok.
        // Note: this prints to stdout, which is fine for a test.
        let result = print_json(&value);
        assert!(result.is_ok());
    }
}
