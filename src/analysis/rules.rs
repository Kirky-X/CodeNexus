// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};

use crate::storage::capability::Storage;

/// Severity of a rule violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSeverity {
    Warning,
    Error,
}

impl RuleSeverity {
    /// Canonical lowercase name used in output JSON and `--fail_on`.
    pub fn as_str(self) -> &'static str {
        match self {
            RuleSeverity::Warning => "warning",
            RuleSeverity::Error => "error",
        }
    }
}

/// What a rule asserts about its result rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Expectation {
    /// The query must return zero rows (each row is a violation instance).
    ForbidMatches,
    /// The query must return at least `min` rows.
    RequireMin { min: u32 },
}

impl Expectation {
    /// Human-readable summary used in violation messages.
    pub fn describe(&self) -> String {
        match self {
            Expectation::ForbidMatches => "forbids any match".to_string(),
            Expectation::RequireMin { min } => format!("requires at least {min} match(es)"),
        }
    }
}

/// One user-authored architecture rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub severity: RuleSeverity,
    /// Read-only Cypher (same subset as the `query` command).
    pub cypher: String,
    #[serde(default)]
    pub message: String,
    pub expectation: Expectation,
}

/// Top-level rules-file shape: `{"rules": [...]}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RulesFile {
    pub rules: Vec<RuleDef>,
}

/// Parses and validates a rules file's raw JSON text.
///
/// # Errors
///
/// [`CodeNexusError::InvalidInput`] when the JSON is malformed or a rule is
/// missing required fields (the serde message names the offending field).
pub fn parse_rules(raw: &str) -> Result<Vec<RuleDef>, crate::service::error::CodeNexusError> {
    let file: RulesFile = serde_json::from_str(raw).map_err(|e| {
        crate::service::error::CodeNexusError::InvalidInput(format!("invalid rules file: {e}"))
    })?;
    Ok(file.rules)
}

/// A failed rule.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuleViolation {
    pub name: String,
    pub severity: RuleSeverity,
    pub message: String,
    /// Whether the expectation was actually violated (execution errors
    /// always violate — fail-closed).
    pub violated: bool,
    /// Row count observed when the expectation was evaluated (absent when
    /// the rule itself errored).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_count: Option<usize>,
    /// First rows returned by the rule query (capped at 20).
    pub sample_rows: Vec<Vec<serde_json::Value>>,
    /// Execution failure reason (rule query error → error-severity).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

const SAMPLE_ROW_CAP: usize = 20;

/// Evaluates rule packs against the indexed graph.
pub struct ArchitectureLinter<'a> {
    storage: &'a dyn Storage,
}

impl<'a> ArchitectureLinter<'a> {
    pub fn new(storage: &'a dyn Storage) -> Self {
        Self { storage }
    }

    /// Runs every rule; never aborts early.
    pub fn lint(&self, rules: &[RuleDef]) -> Vec<RuleViolation> {
        rules.iter().map(|rule| self.evaluate(rule)).collect()
    }

    fn evaluate(&self, rule: &RuleDef) -> RuleViolation {
        let rows = match self.storage.query(&rule.cypher) {
            Ok(rows) => rows,
            Err(err) => {
                return RuleViolation {
                    name: rule.name.clone(),
                    severity: RuleSeverity::Error,
                    violated: true,
                    message: if rule.message.is_empty() {
                        format!("rule '{}' failed to execute", rule.name)
                    } else {
                        rule.message.clone()
                    },
                    match_count: None,
                    sample_rows: Vec::new(),
                    error: Some(err.to_string()),
                };
            }
        };
        let match_count = rows.len();
        let violated = match rule.expectation {
            Expectation::ForbidMatches => match_count > 0,
            Expectation::RequireMin { min } => (match_count as u32) < min,
        };
        RuleViolation {
            name: rule.name.clone(),
            severity: rule.severity,
            violated,
            message: if rule.message.is_empty() {
                format!("rule '{}' {}", rule.name, rule.expectation.describe())
            } else {
                rule.message.clone()
            },
            match_count: Some(match_count),
            sample_rows: rows.into_iter().take(SAMPLE_ROW_CAP).collect(),
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
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
        let kit = build_kit_for_db(&dir.path().join("rules_db"));
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

    fn forbid_rule() -> RuleDef {
        RuleDef {
            name: "ui-not-import-db".to_string(),
            description: "UI layer must not depend on the DB layer".to_string(),
            severity: RuleSeverity::Error,
            // The Cypher subset queries edges node-style via CodeRelation
            // (relationship patterns `(a)-[r:T]->(b)` are not supported —
            // same idiom as dead_code's edge loader).
            cypher: "MATCH (e:CodeRelation) WHERE e.type = 'IMPORTS' RETURN e.source AS src, e.target AS dst;".to_string(),
            message: "UI imports DB directly".to_string(),
            expectation: Expectation::ForbidMatches,
        }
    }

    #[test]
    fn forbid_matches_flags_existing_edge() {
        let (_dir, kit) = seeded_kit();
        let storage = kit.require::<StorageModule>().unwrap();
        let linter = ArchitectureLinter::new(&*storage);
        let violations = linter.lint(&[forbid_rule()]);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].violated);
        assert_eq!(violations[0].match_count, Some(1));
        assert_eq!(violations[0].sample_rows.len(), 1);
        assert_eq!(violations[0].severity, RuleSeverity::Error);
    }

    #[test]
    fn require_min_flags_below_threshold_and_passes_at_boundary() {
        let (_dir, kit) = seeded_kit();
        let storage = kit.require::<StorageModule>().unwrap();
        let linter = ArchitectureLinter::new(&*storage);
        let rule = RuleDef {
            expectation: Expectation::RequireMin { min: 2 },
            severity: RuleSeverity::Warning,
            cypher: "MATCH (f:File) RETURN f.id;".to_string(),
            message: "need at least two modules".to_string(),
            ..forbid_rule()
        };
        let violations = linter.lint(std::slice::from_ref(&rule));
        // Only 2 files < min 2? No: require_min min=2 needs ≥2 rows; 2 rows exist.
        assert!(!violations[0].violated, "2 files satisfy min=2");

        let rule3 = RuleDef {
            expectation: Expectation::RequireMin { min: 3 },
            ..rule.clone()
        };
        let violations = linter.lint(&[rule3]);
        assert!(violations[0].violated, "2 files do not satisfy min=3");
    }

    #[test]
    fn bad_cypher_degrades_to_error_violation_without_aborting() {
        let (_dir, kit) = seeded_kit();
        let storage = kit.require::<StorageModule>().unwrap();
        let linter = ArchitectureLinter::new(&*storage);
        let bad = RuleDef {
            name: "broken".to_string(),
            cypher: "THIS IS NOT CYPHER;".to_string(),
            message: String::new(),
            ..forbid_rule()
        };
        let violations = linter.lint(&[bad, forbid_rule()]);
        assert_eq!(violations.len(), 2, "all rules evaluated");
        assert!(violations[0].violated);
        assert_eq!(violations[0].severity, RuleSeverity::Error);
        assert!(violations[0].error.is_some());
        assert!(violations[1].violated);
    }

    #[test]
    fn parse_rules_rejects_missing_fields_with_field_context() {
        let err = parse_rules("{\"rules\": [{\"name\": \"x\"}]}").expect_err("missing fields");
        let msg = err.to_string();
        assert!(msg.contains("invalid rules file"), "{msg}");
        let ok = parse_rules(
            "{\"rules\": [{\"name\": \"r\", \"severity\": \"error\", \"cypher\": \"MATCH (n) RETURN n;\", \"expectation\": {\"type\": \"forbid_matches\"}}]}",
        )
        .expect("valid rules");
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].expectation, Expectation::ForbidMatches);
        let min = parse_rules(
            "{\"rules\": [{\"name\": \"r\", \"severity\": \"warning\", \"cypher\": \"MATCH (n) RETURN n;\", \"expectation\": {\"type\": \"require_min\", \"min\": 2}}]}",
        )
        .expect("valid require_min");
        assert_eq!(min[0].expectation, Expectation::RequireMin { min: 2 });
    }
}
