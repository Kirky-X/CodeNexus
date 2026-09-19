// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

use serde::Serialize;

use crate::analysis::rules::{parse_rules, ArchitectureLinter, RuleSeverity, RuleViolation};
use crate::diagnostics::Diagnostic;
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
use crate::service::error::CodeNexusError;
#[cfg(feature = "cli")]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_error};
use crate::service::project::resolve_project_id;
#[cfg(feature = "cli")]
use crate::service::runtime::kit;

#[cfg(feature = "cli")]
use sdforge::forge;
#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;

/// Parses `--fail_on`: `error` (default, fail only on error-severity
/// violations), `warning` (fail on any violated rule), `never`.
fn parse_fail_on(raw: &str) -> Result<Option<RuleSeverity>, CodeNexusError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" => Ok(Some(RuleSeverity::Error)),
        "error" => Ok(Some(RuleSeverity::Error)),
        "warning" => Ok(Some(RuleSeverity::Warning)),
        "never" => Ok(None),
        other => Err(CodeNexusError::InvalidInput(format!(
            "invalid --fail_on value: {other} (expected error|warning|never)"
        ))),
    }
}

/// JSON-serializable lint output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LintOutput {
    pub project: String,
    pub rules_file: String,
    pub rules_evaluated: usize,
    /// Rules whose expectation was violated (execution errors included).
    pub violations: Vec<RuleViolation>,
    /// `pass` or `fail`.
    pub verdict: String,
    /// Digest of the rules file (BLAKE3) for traceability.
    pub diagnostics: Vec<Diagnostic>,
}

/// Core logic — evaluates the rule pack against an injected Kit.
pub fn run_lint(
    kit: &AsyncKit<AsyncReady>,
    project: &str,
    rules_path: &str,
    fail_on: &str,
) -> Result<LintOutput, CodeNexusError> {
    let threshold = parse_fail_on(fail_on)?;
    let raw = std::fs::read_to_string(rules_path).map_err(|e| {
        CodeNexusError::InvalidInput(format!("cannot read rules file {rules_path}: {e}"))
    })?;
    let rules = parse_rules(&raw)?;

    if !project.trim().is_empty() {
        let storage = kit.require::<StorageModule>()?;
        resolve_project_id(&*storage, project)?;
    }
    let storage = kit.require::<StorageModule>()?;
    let linter = ArchitectureLinter::new(&*storage);
    let evaluated = linter.lint(&rules);
    let violations: Vec<RuleViolation> = evaluated.iter().filter(|v| v.violated).cloned().collect();

    let digest = blake3::hash(raw.as_bytes());
    let diagnostics = vec![Diagnostic {
        code: "lint/rules-file".to_string(),
        severity: crate::diagnostics::Severity::Warning,
        subject: rules_path.to_string(),
        message: "rules file digest".to_string(),
        evidence: serde_json::json!({
            "algorithm": "blake3",
            "hash": digest.to_string(),
            "bytes": raw.len() as u64,
            "rules": rules.len(),
        }),
        supported_fixes: Vec::new(),
    }];

    let fail = threshold.is_some_and(|t| {
        violations
            .iter()
            .any(|v| severity_rank(v.severity) >= severity_rank(t))
    });
    Ok(LintOutput {
        project: project.to_string(),
        rules_file: rules_path.to_string(),
        rules_evaluated: evaluated.len(),
        violations,
        verdict: if fail { "fail" } else { "pass" }.to_string(),
        diagnostics,
    })
}

fn severity_rank(severity: RuleSeverity) -> u8 {
    match severity {
        RuleSeverity::Warning => 1,
        RuleSeverity::Error => 2,
    }
}

/// CLI wrapper — prints the lint receipt to stdout, exits 2 on a failed gate.
#[cfg(feature = "cli")]
#[forge(
    name = "lint",
    version = "0.4.0",
    description = "Evaluate a custom architecture rule pack (Cypher assertions with forbid_matches/require_min expectations) against the indexed graph. Fails (exit 2) when violated rules meet the --fail_on severity. Params: rules — rules file path (default .codenexus/rules.json); fail_on — error|warning|never (default error); project — validate project exists (empty = skip).",
    cli = true
)]
async fn lint(rules: String, fail_on: String, project: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let output =
        run_lint(&kit, &project, &rules, &fail_on).map_err(|e| to_api_error(e, "lint_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    if output.verdict == "fail" {
        return Err(ApiError::InvalidInput {
            message: format!("lint failed: {} rule(s) violated", output.violations.len()),
            field: Some("fail_on".to_string()),
            value: None,
        });
    }
    Ok(())
}

#[cfg(test)]
#[cfg(feature = "analysis")]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use tempfile::TempDir;

    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    fn seeded_kit() -> (TempDir, AsyncKit<AsyncReady>) {
        let dir = TempDir::new().unwrap();
        let kit = build_kit_for_db(&dir.path().join("lint_db"));
        let storage = kit.require::<StorageModule>().unwrap();
        storage
            .execute("CREATE (:File {id: 'f1', project: 'demo', name: 'ui.rs', filePath: '/src/ui.rs', language: 'rust'});")
            .unwrap();
        storage
            .execute("CREATE (:File {id: 'f2', project: 'demo', name: 'db.rs', filePath: '/src/db.rs', language: 'rust'});")
            .unwrap();
        storage
            .execute("CREATE (:CodeRelation {id: 'e1', source: 'f1', target: 'f2', type: 'IMPORTS', confidence: 0.95, confidenceTier: 'High', reason: '', startLine: 1, project: 'demo'});")
            .unwrap();
        (dir, kit)
    }

    fn write_rules(dir: &TempDir, body: &str) -> String {
        let path = dir.path().join("rules.json");
        std::fs::write(&path, body).unwrap();
        path.to_string_lossy().to_string()
    }

    const VIOLATED_RULE: &str = r#"{"rules": [{"name": "no-imports", "severity": "error", "cypher": "MATCH (e:CodeRelation) WHERE e.type = 'IMPORTS' RETURN e.source;", "expectation": {"type": "forbid_matches"}, "message": "no IMPORTS allowed"}]}"#;

    #[test]
    fn lint_fails_on_violated_error_rule() {
        let (dir, kit) = seeded_kit();
        let rules = write_rules(&dir, VIOLATED_RULE);
        let out = run_lint(&kit, "", &rules, "error").expect("lint runs");
        assert_eq!(out.verdict, "fail");
        assert_eq!(out.rules_evaluated, 1);
        assert_eq!(out.violations.len(), 1);
        assert_eq!(out.violations[0].match_count, Some(1));
        assert!(
            out.diagnostics.iter().any(|d| d.code == "lint/rules-file"),
            "rules file digest attached"
        );
    }

    #[test]
    fn lint_fail_on_never_passes() {
        let (dir, kit) = seeded_kit();
        let rules = write_rules(&dir, VIOLATED_RULE);
        let out = run_lint(&kit, "", &rules, "never").expect("lint runs");
        assert_eq!(out.verdict, "pass");
        assert_eq!(out.violations.len(), 1, "violations still reported");
    }

    #[test]
    fn lint_warning_threshold_catches_warning_severity() {
        let (dir, kit) = seeded_kit();
        let warning_rule = r#"{"rules": [{"name": "w", "severity": "warning", "cypher": "MATCH (e:CodeRelation) WHERE e.type = 'IMPORTS' RETURN e.source;", "expectation": {"type": "forbid_matches"}}]}"#;
        let rules = write_rules(&dir, warning_rule);
        let strict = run_lint(&kit, "", &rules, "warning").expect("lint runs");
        assert_eq!(strict.verdict, "fail");
        let lax = run_lint(&kit, "", &rules, "error").expect("lint runs");
        assert_eq!(
            lax.verdict, "pass",
            "warning violation below error threshold"
        );
    }

    #[test]
    fn lint_missing_rules_file_is_invalid_input() {
        let (_dir, kit) = seeded_kit();
        let err = run_lint(&kit, "", "/nonexistent/rules.json", "error")
            .expect_err("missing file must fail");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
        assert!(err.to_string().contains("cannot read rules file"));
    }

    #[test]
    fn lint_invalid_fail_on_rejected() {
        let (dir, kit) = seeded_kit();
        let rules = write_rules(&dir, VIOLATED_RULE);
        let err = run_lint(&kit, "", &rules, "bogus").expect_err("bogus fail_on");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
    }

    #[test]
    fn parse_fail_on_levels() {
        assert_eq!(parse_fail_on("").unwrap(), Some(RuleSeverity::Error));
        assert_eq!(
            parse_fail_on("warning").unwrap(),
            Some(RuleSeverity::Warning)
        );
        assert_eq!(parse_fail_on("never").unwrap(), None);
        assert!(parse_fail_on("x").is_err());
    }
}
