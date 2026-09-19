// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

use serde::Serialize;

use crate::kit::StorageModule;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::kit::{AsyncKit, AsyncReady};
use crate::service::detect_changes::{run_detect_changes, AffectedSymbolOutput};
use crate::service::error::CodeNexusError;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_error};
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::project::resolve_project_id;
#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::runtime::kit;

#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::forge;
#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;

/// Risk severity ordering used by the gate: `low` < `medium` < `high`.
fn severity_of(risk: &str) -> u8 {
    match risk {
        "medium" => 1,
        "high" => 2,
        _ => 0,
    }
}

/// Parses `--fail_on`: `none` (never fail), or the minimum risk level that
/// triggers a failure (`low` < `medium` < `high`). Empty string = `high`.
fn parse_fail_on(raw: &str) -> Result<Option<u8>, CodeNexusError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" => Ok(Some(2)),
        "none" => Ok(None),
        "low" => Ok(Some(0)),
        "medium" => Ok(Some(1)),
        "high" => Ok(Some(2)),
        other => Err(CodeNexusError::InvalidInput(format!(
            "invalid --fail_on value: {other} (expected none|low|medium|high)"
        ))),
    }
}

/// Aggregated risk counts over the affected-symbol set.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RiskCounts {
    pub low: usize,
    pub medium: usize,
    pub high: usize,
}

/// Pure gate evaluation — shared by the wrapper and unit tests.
#[derive(Debug, Clone, PartialEq)]
struct GateEvaluation {
    risk_counts: RiskCounts,
    verdict: &'static str,
    failed_symbols: Vec<AffectedSymbolOutput>,
}

/// Classifies the affected-symbol set against the `fail_on` threshold.
///
/// `fail_on = None` never fails. Otherwise the gate fails when any symbol's
/// severity ≥ threshold. Failed symbols are ordered by incoming edge count
/// (descending), then qualified name.
fn evaluate_gate(affected: &[AffectedSymbolOutput], fail_on: Option<u8>) -> GateEvaluation {
    let mut risk_counts = RiskCounts::default();
    for sym in affected {
        match severity_of(&sym.risk_level) {
            0 => risk_counts.low += 1,
            1 => risk_counts.medium += 1,
            _ => risk_counts.high += 1,
        }
    }
    let mut failed_symbols: Vec<AffectedSymbolOutput> = match fail_on {
        None => Vec::new(),
        Some(threshold) => affected
            .iter()
            .filter(|s| severity_of(&s.risk_level) >= threshold)
            .cloned()
            .collect(),
    };
    failed_symbols.sort_by(|a, b| {
        b.incoming_edge_count
            .cmp(&a.incoming_edge_count)
            .then_with(|| a.qualified_name.cmp(&b.qualified_name))
    });
    let verdict = if failed_symbols.is_empty() {
        "pass"
    } else {
        "fail"
    };
    GateEvaluation {
        risk_counts,
        verdict,
        failed_symbols,
    }
}

/// JSON-serializable CI gate output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CiGateOutput {
    /// The repository root that was diffed.
    pub path: String,
    /// The diff mode used (`unstaged` / `staged` / `head`).
    pub base_mode: String,
    /// The resolved fail-on threshold (`none`/`low`/`medium`/`high`).
    pub fail_on: String,
    pub files_changed: usize,
    pub affected_count: usize,
    pub risk_counts: RiskCounts,
    /// `pass` or `fail`.
    pub verdict: String,
    /// Symbols at or above the threshold (empty when passing).
    pub failed_symbols: Vec<AffectedSymbolOutput>,
    /// PR-comment-ready Markdown report.
    pub markdown: String,
}

/// Renders the PR-comment Markdown for a gate result.
fn render_markdown(output: &CiGateOutput) -> String {
    let mut md = String::new();
    md.push_str("## CodeNexus Architecture Gate\n\n");
    if output.verdict == "pass" {
        md.push_str(&format!(
            "✅ **PASS** — {} changed file(s), {} affected symbol(s) \
             (low: {}, medium: {}, high: {}), fail_on ≥ `{}`.\n",
            output.files_changed,
            output.affected_count,
            output.risk_counts.low,
            output.risk_counts.medium,
            output.risk_counts.high,
            output.fail_on,
        ));
        return md;
    }
    md.push_str(&format!(
        "❌ **FAIL** — {} affected symbol(s) at or above `{}` risk \
         (low: {}, medium: {}, high: {}).\n\n",
        output.failed_symbols.len(),
        output.fail_on,
        output.risk_counts.low,
        output.risk_counts.medium,
        output.risk_counts.high,
    ));
    md.push_str("| Symbol | File | Risk | Incoming edges |\n");
    md.push_str("|---|---|---|---|\n");
    for sym in &output.failed_symbols {
        md.push_str(&format!(
            "| `{}` | `{}` | {} | {} |\n",
            sym.qualified_name, sym.file_path, sym.risk_level, sym.incoming_edge_count
        ));
    }
    md.push_str(
        "\n_Review the blast radius (`codenexus impact --symbol <name>`) before merging._\n",
    );
    md
}

/// Core logic — runs the gate against an injected Kit (testable core).
#[cfg(any(feature = "cli", feature = "mcp"))]
pub(crate) fn run_ci_gate(
    kit: &AsyncKit<AsyncReady>,
    path: &str,
    base_mode: &str,
    fail_on: &str,
    project: &str,
) -> Result<CiGateOutput, CodeNexusError> {
    let threshold = parse_fail_on(fail_on)?;
    if !project.trim().is_empty() {
        let storage = kit.require::<StorageModule>()?;
        resolve_project_id(&*storage, project)?;
    }
    let detect = run_detect_changes(kit, path, base_mode)?;
    let evaluation = evaluate_gate(&detect.affected, threshold);
    let output = CiGateOutput {
        path: path.to_string(),
        base_mode: base_mode.to_string(),
        fail_on: match threshold {
            None => "none".to_string(),
            Some(0) => "low".to_string(),
            Some(1) => "medium".to_string(),
            _ => "high".to_string(),
        },
        files_changed: detect.files_changed,
        affected_count: detect.affected.len(),
        risk_counts: evaluation.risk_counts,
        verdict: evaluation.verdict.to_string(),
        failed_symbols: evaluation.failed_symbols,
        markdown: String::new(),
    };
    Ok(CiGateOutput {
        markdown: render_markdown(&output),
        ..output
    })
}

/// CLI wrapper — prints the gate receipt to stdout, exits 2 on a failed gate.
#[cfg(feature = "cli")]
#[forge(
    name = "ci",
    version = "0.4.0",
    description = "Architecture gate for CI: map git changes to affected symbols with risk classification and fail (exit 2) when any symbol meets the --fail_on threshold. Params: path (required); base_mode — unstaged|staged|head (default head); fail_on — none|low|medium|high (default high); project — validate project exists (empty = skip).",
    cli = true
)]
async fn ci(
    path: String,
    base_mode: String,
    fail_on: String,
    project: String,
) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output = run_ci_gate(&kit, &path, &base_mode, &fail_on, &project)
        .map_err(|e| to_api_error(e, "ci_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    if output.verdict == "fail" {
        return Err(ApiError::InvalidInput {
            message: format!(
                "ci gate failed: {} symbol(s) at or above '{}' risk",
                output.failed_symbols.len(),
                output.fail_on
            ),
            field: Some("fail_on".to_string()),
            value: None,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(name: &str, file: &str, risk: &str, incoming: usize) -> AffectedSymbolOutput {
        AffectedSymbolOutput {
            name: name.to_string(),
            label: "Function".to_string(),
            qualified_name: format!("demo.{name}"),
            file_path: file.to_string(),
            start_line: 1,
            end_line: 9,
            incoming_edge_count: incoming,
            risk_level: risk.to_string(),
        }
    }

    // --- parse_fail_on ---

    #[test]
    fn fail_on_parses_known_levels() {
        assert_eq!(parse_fail_on("high").unwrap(), Some(2));
        assert_eq!(parse_fail_on("medium").unwrap(), Some(1));
        assert_eq!(parse_fail_on("low").unwrap(), Some(0));
        assert_eq!(parse_fail_on("none").unwrap(), None);
        assert_eq!(parse_fail_on("").unwrap(), Some(2));
        assert_eq!(parse_fail_on(" HIGH ").unwrap(), Some(2));
    }

    #[test]
    fn fail_on_rejects_garbage() {
        let err = parse_fail_on("bogus").expect_err("garbage must fail");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
        assert!(err.to_string().contains("invalid --fail_on value"));
    }

    // --- evaluate_gate ---

    #[test]
    fn no_changes_is_pass() {
        let evaluation = evaluate_gate(&[], Some(2));
        assert_eq!(evaluation.verdict, "pass");
        assert!(evaluation.failed_symbols.is_empty());
        assert_eq!(evaluation.risk_counts, RiskCounts::default());
    }

    #[test]
    fn high_risk_fails_at_high_threshold() {
        let affected = vec![
            sym("hub", "src/hub.rs", "high", 7),
            sym("leaf", "src/leaf.rs", "low", 0),
        ];
        let evaluation = evaluate_gate(&affected, Some(2));
        assert_eq!(evaluation.verdict, "fail");
        assert_eq!(evaluation.failed_symbols.len(), 1);
        assert_eq!(evaluation.failed_symbols[0].name, "hub");
        assert_eq!(evaluation.risk_counts.high, 1);
        assert_eq!(evaluation.risk_counts.low, 1);
    }

    #[test]
    fn fail_on_none_never_fails() {
        let affected = vec![sym("hub", "src/hub.rs", "high", 12)];
        let evaluation = evaluate_gate(&affected, None);
        assert_eq!(evaluation.verdict, "pass");
        assert!(evaluation.failed_symbols.is_empty());
    }

    #[test]
    fn medium_threshold_catches_high_and_medium() {
        let affected = vec![
            sym("hub", "src/hub.rs", "high", 9),
            sym("mid", "src/mid.rs", "medium", 2),
            sym("leaf", "src/leaf.rs", "low", 0),
        ];
        let evaluation = evaluate_gate(&affected, Some(1));
        assert_eq!(evaluation.verdict, "fail");
        assert_eq!(evaluation.failed_symbols.len(), 2);
        // Sorted by incoming edges descending.
        assert_eq!(evaluation.failed_symbols[0].name, "hub");
        assert_eq!(evaluation.failed_symbols[1].name, "mid");
    }

    // --- markdown ---

    #[test]
    fn markdown_contains_report_table_on_fail() {
        let affected = vec![sym("hub", "src/hub.rs", "high", 7)];
        let evaluation = evaluate_gate(&affected, Some(2));
        let output = CiGateOutput {
            path: ".".to_string(),
            base_mode: "head".to_string(),
            fail_on: "high".to_string(),
            files_changed: 1,
            affected_count: 1,
            risk_counts: evaluation.risk_counts.clone(),
            verdict: evaluation.verdict.to_string(),
            failed_symbols: evaluation.failed_symbols.clone(),
            markdown: String::new(),
        };
        let md = render_markdown(&output);
        assert!(md.contains("## CodeNexus Architecture Gate"));
        assert!(md.contains("❌ **FAIL**"));
        assert!(md.contains("| `demo.hub` | `src/hub.rs` | high | 7 |"));
    }

    #[test]
    fn markdown_pass_report_has_no_table() {
        let evaluation = evaluate_gate(&[], Some(2));
        let output = CiGateOutput {
            path: ".".to_string(),
            base_mode: "head".to_string(),
            fail_on: "high".to_string(),
            files_changed: 0,
            affected_count: 0,
            risk_counts: evaluation.risk_counts,
            verdict: evaluation.verdict.to_string(),
            failed_symbols: evaluation.failed_symbols,
            markdown: String::new(),
        };
        let md = render_markdown(&output);
        assert!(md.contains("✅ **PASS**"));
        assert!(!md.contains('|'));
    }
}
