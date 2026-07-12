// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Data quality validation (DQ-002/004/005/006).
//!
//! Provides [`QualityChecker`] to run post-indexing data quality checks
//! and report violations via [`QualityReport`].
//!
//! # Rules
//!
//! | Rule    | Description                                                              |
//! |---------|--------------------------------------------------------------------------|
//! | DQ-002  | FQN uniqueness — no two nodes share the same `qualifiedName` per project.|
//! | DQ-004  | Edge integrity — every `CodeRelation` source/target resolves to a node. |
//! | DQ-005  | Project isolation — per-project node counts sum to the table total.     |
//! | DQ-006  | Hash integrity — every `File` node has a non-empty `hash`.              |

use super::capability::Storage;
use super::error::Result;
use super::schema::escape_identifier;
use crate::model::NodeLabel;

/// A single data quality violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityViolation {
    /// The DQ rule that was violated (e.g. "DQ-002").
    pub rule: &'static str,
    /// Human-readable description of the violation.
    pub message: String,
    /// The project id where the violation was found (if applicable).
    pub project: Option<String>,
}

/// The result of running all DQ checks.
#[derive(Debug, Clone, Default)]
pub struct QualityReport {
    /// All violations found, grouped by rule.
    pub violations: Vec<QualityViolation>,
}

impl QualityReport {
    /// Returns `true` if no violations were found.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    /// Returns the number of violations for a specific DQ rule.
    #[must_use]
    pub fn count_for_rule(&self, rule: &str) -> usize {
        self.violations.iter().filter(|v| v.rule == rule).count()
    }
}

/// Runs data quality checks against a [`Storage`] capability.
///
/// Owned by `index_cmd::run` (T6/Phase-2 Task 2.14) — accepts `&dyn Storage`
/// so it can resolve the storage capability from a [`Kit`](crate::kit::Kit)
/// instead of constructing a [`Repository`](super::Repository) ad-hoc.
pub struct QualityChecker<'a> {
    storage: &'a dyn Storage,
}

impl<'a> QualityChecker<'a> {
    /// Creates a new `QualityChecker` backed by the given storage capability.
    #[must_use]
    pub fn new(storage: &'a dyn Storage) -> Self {
        Self { storage }
    }

    /// Runs all DQ checks and returns a consolidated report.
    ///
    /// Individual check failures are non-fatal: a check that errors (e.g. a
    /// table missing a column) is skipped so the remaining checks can still
    /// run. Only successfully-computed violations are included in the report.
    pub fn run_all(&self) -> Result<QualityReport> {
        let mut report = QualityReport::default();
        if let Ok(v) = self.check_fqn_uniqueness() {
            report.violations.extend(v);
        }
        if let Ok(v) = self.check_edge_integrity() {
            report.violations.extend(v);
        }
        if let Ok(v) = self.check_project_isolation() {
            report.violations.extend(v);
        }
        if let Ok(v) = self.check_hash_integrity() {
            report.violations.extend(v);
        }
        Ok(report)
    }

    /// DQ-002: Checks that no two nodes share the same `qualifiedName`
    /// within the same project.
    ///
    /// Iterates every node table that has `qualifiedName` and `project`
    /// columns (tables lacking either are silently skipped via query failure),
    /// collects `(project, qualified_name, id)` triples, and emits one
    /// violation per duplicate `(project, qualified_name)` pair.
    pub fn check_fqn_uniqueness(&self) -> Result<Vec<QualityViolation>> {
        // (project, qualified_name) -> Vec<node_id>
        // Single-line for coverage: tarpaulin attribute continuation
        let mut seen: std::collections::HashMap<(String, String), Vec<String>> =
            std::collections::HashMap::new();

        for label in NodeLabel::all() {
            // Project has no qualifiedName/project columns; skip explicitly
            // to avoid a guaranteed-to-fail query.
            if label == NodeLabel::Project {
                continue;
            }
            let table = escape_identifier(label.table_name());
            let cypher = format!(
                "MATCH (n:{table}) \
                 WHERE n.qualifiedName IS NOT NULL AND n.project IS NOT NULL \
                 RETURN n.project AS project, n.qualifiedName AS qn, n.id AS id;"
            );
            // Tables without qualifiedName/project columns error here; treat
            // as non-fatal and continue with the next label.
            // Single-line for coverage: tarpaulin attribute continuation
            let Ok(rows) = self.storage.query(&cypher) else {
                continue;
            };
            for row in rows {
                let project = row
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let qn = row
                    .get(1)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let id = row
                    .get(2)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !project.is_empty() && !qn.is_empty() {
                    seen.entry((project, qn)).or_default().push(id);
                }
            }
        }

        let mut violations: Vec<QualityViolation> = seen
            .into_iter()
            .filter(|(_, ids)| ids.len() > 1)
            .map(|((project, qn), ids)| QualityViolation {
                rule: "DQ-002",
                // Single-line for coverage: tarpaulin attribute continuation
                message: format!(
                    "Duplicate FQN '{}' in project '{}' ({} nodes: {})",
                    qn,
                    project,
                    ids.len(),
                    ids.join(", ")
                ),
                project: Some(project),
            })
            .collect();
        // Sort for deterministic output (HashMap iteration order is random).
        violations.sort_by(|a, b| a.message.cmp(&b.message));
        Ok(violations)
    }

    /// DQ-004: Checks that every edge in `CodeRelation` has a source and
    /// target that exist as a node id in some node table.
    ///
    /// Collects all node ids across every node table, then verifies each
    /// edge's `source` and `target` against that set. Emits one violation per
    /// orphan endpoint (so an edge with both endpoints missing yields two
    /// violations).
    pub fn check_edge_integrity(&self) -> Result<Vec<QualityViolation>> {
        // Collect every node id across all node tables.
        // Single-line for coverage: tarpaulin attribute continuation
        let mut all_node_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for label in NodeLabel::all() {
            let table = escape_identifier(label.table_name());
            let cypher = format!("MATCH (n:{table}) RETURN n.id AS id;");
            // Single-line for coverage: tarpaulin attribute continuation
            let Ok(rows) = self.storage.query(&cypher) else {
                continue;
            };
            for row in rows {
                if let Some(id) = row.first().and_then(|v| v.as_str()) {
                    all_node_ids.insert(id.to_string());
                }
            }
        }

        // Query every CodeRelation edge.
        // Single-line for coverage: tarpaulin attribute continuation
        let cypher = "MATCH (r:CodeRelation) RETURN r.source AS source, r.target AS target, r.project AS project;";
        let rows = self.storage.query(cypher)?;

        let mut violations = Vec::new();
        for row in &rows {
            let source = row.first().and_then(|v| v.as_str()).unwrap_or("");
            let target = row.get(1).and_then(|v| v.as_str()).unwrap_or("");
            // Single-line for coverage: tarpaulin attribute continuation
            let project = row
                .get(2)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !all_node_ids.contains(source) {
                violations.push(QualityViolation {
                    rule: "DQ-004",
                    message: format!("Orphan edge: source '{}' does not exist", source),
                    project: Some(project.clone()),
                });
            }
            if !all_node_ids.contains(target) {
                violations.push(QualityViolation {
                    rule: "DQ-004",
                    message: format!("Orphan edge: target '{}' does not exist", target),
                    project: Some(project),
                });
            }
        }
        violations.sort_by(|a, b| a.message.cmp(&b.message));
        Ok(violations)
    }

    /// DQ-005: Checks that project isolation is maintained.
    ///
    /// For each non-Project node table that has a `project` column, compares
    /// the total row count against the sum of per-project counts (over all
    /// projects returned by [`Repository::list_projects`]). A mismatch
    /// indicates either a node with an unknown `project` value or a node
    /// whose `project` column leaked across projects.
    pub fn check_project_isolation(&self) -> Result<Vec<QualityViolation>> {
        let projects = self.storage.list_projects()?;
        let project_ids: Vec<String> = projects.into_iter().map(|p| p.id).collect();

        let mut violations = Vec::new();
        for label in NodeLabel::all() {
            if label == NodeLabel::Project {
                continue;
            }
            let table = escape_identifier(label.table_name());

            // Sum counts per known project.
            let mut per_project_count: i64 = 0;
            for pid in &project_ids {
                let escaped = escape_cypher(pid);
                let cypher = format!(
                    "MATCH (n:{table}) WHERE n.project = '{escaped}' RETURN count(n) AS cnt;"
                );
                // Single-line for coverage: tarpaulin attribute continuation
                let Ok(rows) = self.storage.query(&cypher) else {
                    continue;
                };
                // Single-line for coverage: tarpaulin attribute continuation
                if let Some(cnt) = rows
                    .first()
                    .and_then(|r| r.first())
                    .and_then(|v| v.as_i64())
                {
                    per_project_count += cnt;
                }
            }

            // Total count for the table.
            let cypher = format!("MATCH (n:{table}) RETURN count(n) AS cnt;");
            // Single-line for coverage: tarpaulin attribute continuation
            let Ok(rows) = self.storage.query(&cypher) else {
                continue;
            };
            // Single-line for coverage: tarpaulin attribute continuation
            if let Some(total) = rows
                .first()
                .and_then(|r| r.first())
                .and_then(|v| v.as_i64())
            {
                if total != per_project_count {
                    violations.push(QualityViolation {
                        rule: "DQ-005",
                        // Single-line for coverage: tarpaulin attribute continuation
                        message: format!(
                            "Project isolation violation in {}: total {} vs per-project sum {}",
                            label.table_name(),
                            total,
                            per_project_count
                        ),
                        project: None,
                    });
                }
            }
        }
        Ok(violations)
    }

    /// DQ-006: Checks that every `File` node has a non-empty `hash`.
    ///
    /// A `File` node whose `hash` is null or an empty string is reported as a
    /// violation.
    pub fn check_hash_integrity(&self) -> Result<Vec<QualityViolation>> {
        let cypher = "MATCH (f:File) RETURN f.id AS id, f.project AS project, f.hash AS hash;";
        let rows = self.storage.query(cypher)?;
        let violations: Vec<QualityViolation> = rows
            .iter()
            .filter(|row| {
                let hash = row.get(2).and_then(|v| v.as_str()).unwrap_or("");
                hash.is_empty()
            })
            .map(|row| {
                let id = row.first().and_then(|v| v.as_str()).unwrap_or("");
                // Single-line for coverage: tarpaulin attribute continuation
                let project = row
                    .get(1)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                QualityViolation {
                    rule: "DQ-006",
                    message: format!("File node '{}' has empty hash", id),
                    project: Some(project),
                }
            })
            .collect();
        Ok(violations)
    }
}

/// Escapes a string for safe interpolation into a Cypher single-quoted string
/// literal. LadybugDB uses backslash escaping (see `Cypher.g4` `EscapedChar`):
/// `\` → `\\` and `'` → `\'`.
///
/// This mirrors the private `escape_cypher_string` in `repository.rs`; it is
/// duplicated here because LadybugDB does not support parameterized queries
/// and the original helper is not exported.
fn escape_cypher(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EdgeType, Language};
    use std::sync::Arc;

    /// Builds a fresh in-memory `Arc<dyn Storage>` capability with the schema
    /// initialized. Mirrors the trait-kit bootstrap path so `QualityChecker`
    /// tests exercise the same `&dyn Storage` interface as `index_cmd::run`.
    fn fresh_storage() -> Arc<dyn Storage> {
        crate::storage::StorageModule::build_cap(&crate::storage::StorageConfig::in_memory())
            .expect("StorageModule::build_cap")
    }

    /// Builds a sample Project node.
    fn sample_project(id: &str, name: &str) -> crate::model::Node {
        crate::model::Node::builder(NodeLabel::Project, name, name)
            .id(id)
            .language(Language::Rust)
            .properties(serde_json::json!({
                "rootPath": "/repo/".to_string() + name,
                "fileCount": 10,
                "indexedAt": 1_700_000_000,
            }))
            .build()
    }

    /// Builds a sample File node with the given hash.
    fn sample_file(id: &str, project: &str, path: &str, hash: &str) -> crate::model::Node {
        crate::model::Node::builder(NodeLabel::File, path, path)
            .id(id)
            .project(project)
            .file_path(path)
            .language(Language::Rust)
            .properties(serde_json::json!({"hash": hash, "lineCount": 100}))
            .build()
    }

    /// Builds a sample Function node.
    fn sample_function(id: &str, project: &str, name: &str, qn: &str) -> crate::model::Node {
        crate::model::Node::builder(NodeLabel::Function, name, qn)
            .id(id)
            .project(project)
            .file_path("/src/main.rs")
            .start_line(1)
            .end_line(10)
            .signature("fn main()")
            .build()
    }

    // --- DQ-002: FQN uniqueness ---

    #[test]
    fn test_dq002_detects_duplicate_fqn() {
        let storage = fresh_storage();
        // Two Function nodes with the same qualifiedName in the same project.
        let nodes = vec![
            sample_function("f1", "demo", "main", "demo.main"),
            sample_function("f2", "demo", "other", "demo.main"),
        ];
        storage
            .save_nodes(&nodes, NodeLabel::Function)
            .expect("save_nodes");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_fqn_uniqueness()
            .expect("check_fqn_uniqueness");
        assert_eq!(
            violations.len(),
            1,
            "expected exactly one DQ-002 violation, got {violations:?}"
        );
        assert_eq!(violations[0].rule, "DQ-002");
        assert!(violations[0].message.contains("demo.main"));
        assert!(violations[0].message.contains("demo"));
        assert_eq!(violations[0].project.as_deref(), Some("demo"));
    }

    #[test]
    fn test_dq002_clean_when_unique() {
        let storage = fresh_storage();
        let nodes = vec![
            sample_function("f1", "demo", "main", "demo.main"),
            sample_function("f2", "demo", "helper", "demo.helper"),
        ];
        storage
            .save_nodes(&nodes, NodeLabel::Function)
            .expect("save_nodes");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_fqn_uniqueness()
            .expect("check_fqn_uniqueness");
        assert!(
            violations.is_empty(),
            "expected no DQ-002 violations, got {violations:?}"
        );
    }

    #[test]
    fn test_dq002_same_fqn_in_different_projects_is_not_duplicate() {
        // Same FQN across different projects is allowed (project isolation).
        let storage = fresh_storage();
        let nodes = vec![
            sample_function("f1", "alpha", "main", "alpha.main"),
            sample_function("f2", "beta", "main", "alpha.main"),
        ];
        storage
            .save_nodes(&nodes, NodeLabel::Function)
            .expect("save_nodes");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_fqn_uniqueness()
            .expect("check_fqn_uniqueness");
        assert!(violations.is_empty(), "got {violations:?}");
    }

    // --- DQ-004: Edge integrity ---

    #[test]
    fn test_dq004_detects_orphan_edge() {
        let storage = fresh_storage();
        // Only f1 exists as a node; f2 is a dangling reference.
        storage
            .save_nodes(
                &[sample_function("f1", "demo", "main", "demo.main")],
                NodeLabel::Function,
            )
            .expect("save_nodes");
        storage
            .save_edges(&[
                crate::model::Edge::builder("f1", "f2_missing", EdgeType::Calls, "demo").build(),
            ])
            .expect("save_edges");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_edge_integrity()
            .expect("check_edge_integrity");
        assert_eq!(
            violations.len(),
            1,
            "expected one DQ-004 violation for orphan target, got {violations:?}"
        );
        assert_eq!(violations[0].rule, "DQ-004");
        assert!(violations[0].message.contains("f2_missing"));
    }

    #[test]
    fn test_dq004_clean_when_all_edges_valid() {
        let storage = fresh_storage();
        storage
            .save_nodes(
                &[
                    sample_function("f1", "demo", "main", "demo.main"),
                    sample_function("f2", "demo", "helper", "demo.helper"),
                ],
                NodeLabel::Function,
            )
            .expect("save_nodes");
        storage
            .save_edges(&[crate::model::Edge::builder("f1", "f2", EdgeType::Calls, "demo").build()])
            .expect("save_edges");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_edge_integrity()
            .expect("check_edge_integrity");
        assert!(
            violations.is_empty(),
            "expected no DQ-004 violations, got {violations:?}"
        );
    }

    #[test]
    fn test_dq004_detects_orphan_source_and_target_independently() {
        // Edge with BOTH endpoints missing → two violations (one per endpoint).
        let storage = fresh_storage();
        storage
            .save_edges(&[crate::model::Edge::builder(
                "missing_src",
                "missing_tgt",
                EdgeType::Calls,
                "demo",
            )
            .build()])
            .expect("save_edges");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_edge_integrity()
            .expect("check_edge_integrity");
        assert_eq!(
            violations.len(),
            2,
            "expected two DQ-004 violations (source + target), got {violations:?}"
        );
        assert!(violations.iter().all(|v| v.rule == "DQ-004"));
        let messages: Vec<&str> = violations.iter().map(|v| v.message.as_str()).collect();
        assert!(
            messages
                .iter()
                .any(|m| m.contains("missing_src") && m.contains("source")),
            "should report orphan source: {messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|m| m.contains("missing_tgt") && m.contains("target")),
            "should report orphan target: {messages:?}"
        );
    }

    // --- DQ-005: Project isolation ---

    #[test]
    fn test_dq005_clean_when_projects_isolated() {
        let storage = fresh_storage();
        storage
            .save_project(&sample_project("alpha", "alpha"))
            .expect("save_project");
        storage
            .save_project(&sample_project("beta", "beta"))
            .expect("save_project");
        storage
            .save_nodes(
                &[sample_function("a1", "alpha", "main", "alpha.main")],
                NodeLabel::Function,
            )
            .expect("save_nodes alpha");
        storage
            .save_nodes(
                &[sample_function("b1", "beta", "main", "beta.main")],
                NodeLabel::Function,
            )
            .expect("save_nodes beta");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_project_isolation()
            .expect("check_project_isolation");
        assert!(
            violations.is_empty(),
            "expected no DQ-005 violations for isolated projects, got {violations:?}"
        );
    }

    #[test]
    fn test_dq005_detects_isolation_violation_for_unknown_project() {
        // A Function node whose `project` value is not in the Project table
        // → total count exceeds per-project sum → DQ-005 violation.
        let storage = fresh_storage();
        storage
            .save_project(&sample_project("alpha", "alpha"))
            .expect("save_project");
        // Function in known project "alpha".
        storage
            .save_nodes(
                &[sample_function("a1", "alpha", "main", "alpha.main")],
                NodeLabel::Function,
            )
            .expect("save_nodes alpha");
        // Function in unknown project "ghost" (no matching Project node).
        storage
            .save_nodes(
                &[sample_function("g1", "ghost", "main", "ghost.main")],
                NodeLabel::Function,
            )
            .expect("save_nodes ghost");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_project_isolation()
            .expect("check_project_isolation");
        assert_eq!(
            violations.len(),
            1,
            "expected exactly one DQ-005 violation, got {violations:?}"
        );
        assert_eq!(violations[0].rule, "DQ-005");
        assert!(
            violations[0].message.contains("Function"),
            "violation should mention the table: {}",
            violations[0].message
        );
        assert!(
            violations[0].message.contains("total 2"),
            "total should be 2 (alpha + ghost): {}",
            violations[0].message
        );
        assert!(
            violations[0].message.contains("per-project sum 1"),
            "per-project sum should be 1 (only alpha): {}",
            violations[0].message
        );
    }

    #[test]
    fn test_run_all_includes_dq005_violations() {
        let storage = fresh_storage();
        storage
            .save_project(&sample_project("alpha", "alpha"))
            .expect("save_project");
        storage
            .save_nodes(
                &[sample_function("g1", "ghost", "main", "ghost.main")],
                NodeLabel::Function,
            )
            .expect("save_nodes ghost");

        let checker = QualityChecker::new(&*storage);
        let report = checker.run_all().expect("run_all");
        assert!(
            report.count_for_rule("DQ-005") >= 1,
            "run_all should aggregate DQ-005 violations, got {:?}",
            report.violations
        );
    }

    // --- DQ-006: Hash integrity ---

    #[test]
    fn test_dq006_detects_empty_hash() {
        let storage = fresh_storage();
        // One File with a valid hash, one with an empty hash.
        storage
            .save_nodes(
                &[
                    sample_file("f1", "demo", "/a.rs", "sha256:abc"),
                    sample_file("f2", "demo", "/b.rs", ""),
                ],
                NodeLabel::File,
            )
            .expect("save_nodes");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_hash_integrity()
            .expect("check_hash_integrity");
        assert_eq!(
            violations.len(),
            1,
            "expected one DQ-006 violation, got {violations:?}"
        );
        assert_eq!(violations[0].rule, "DQ-006");
        assert!(violations[0].message.contains("f2"));
        assert_eq!(violations[0].project.as_deref(), Some("demo"));
    }

    #[test]
    fn test_dq006_clean_when_all_hashes_present() {
        let storage = fresh_storage();
        storage
            .save_nodes(
                &[
                    sample_file("f1", "demo", "/a.rs", "sha256:abc"),
                    sample_file("f2", "demo", "/b.rs", "sha256:def"),
                ],
                NodeLabel::File,
            )
            .expect("save_nodes");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_hash_integrity()
            .expect("check_hash_integrity");
        assert!(
            violations.is_empty(),
            "expected no DQ-006 violations, got {violations:?}"
        );
    }

    // --- QualityReport ---

    #[test]
    fn test_quality_report_is_clean() {
        let report = QualityReport::default();
        assert!(report.is_clean());

        let report = QualityReport {
            violations: vec![QualityViolation {
                rule: "DQ-002",
                message: "dup".into(),
                project: None,
            }],
        };
        assert!(!report.is_clean());
    }

    #[test]
    fn test_quality_report_count_for_rule() {
        let report = QualityReport {
            violations: vec![
                QualityViolation {
                    rule: "DQ-002",
                    message: "dup1".into(),
                    project: None,
                },
                QualityViolation {
                    rule: "DQ-002",
                    message: "dup2".into(),
                    project: None,
                },
                QualityViolation {
                    rule: "DQ-006",
                    message: "empty hash".into(),
                    project: None,
                },
            ],
        };
        assert_eq!(report.count_for_rule("DQ-002"), 2);
        assert_eq!(report.count_for_rule("DQ-006"), 1);
        assert_eq!(report.count_for_rule("DQ-004"), 0);
    }

    // --- run_all integration ---

    #[test]
    fn test_run_all_clean_on_fresh_repo() {
        let storage = fresh_storage();
        let checker = QualityChecker::new(&*storage);
        let report = checker.run_all().expect("run_all");
        assert!(report.is_clean(), "fresh repo should have no violations");
    }

    #[test]
    fn test_run_all_aggregates_violations_from_all_checks() {
        let storage = fresh_storage();
        // DQ-002 violation: duplicate FQN.
        storage
            .save_nodes(
                &[
                    sample_function("f1", "demo", "main", "demo.main"),
                    sample_function("f2", "demo", "other", "demo.main"),
                ],
                NodeLabel::Function,
            )
            .expect("save_nodes");
        // DQ-006 violation: empty hash.
        storage
            .save_nodes(
                &[sample_file("file_1", "demo", "/a.rs", "")],
                NodeLabel::File,
            )
            .expect("save_nodes file");

        let checker = QualityChecker::new(&*storage);
        let report = checker.run_all().expect("run_all");
        assert!(!report.is_clean());
        assert!(report.count_for_rule("DQ-002") >= 1);
        assert!(report.count_for_rule("DQ-006") >= 1);
    }

    // --- escape_cypher helper ---

    #[test]
    fn escape_cypher_escapes_backslash_and_single_quote() {
        assert_eq!(escape_cypher("plain"), "plain");
        assert_eq!(escape_cypher("it's"), "it\\'s");
        assert_eq!(escape_cypher("path\\to"), "path\\\\to");
        assert_eq!(escape_cypher(""), "");
    }

    // --- Additional coverage tests ---

    #[test]
    fn test_dq002_multiple_duplicate_groups_generate_multiple_violations() {
        // Exercises the DQ-002 violation construction (lines 150, 153-155)
        // with multiple duplicate groups across different FQNs.
        let storage = fresh_storage();
        storage
            .save_nodes(
                &[
                    sample_function("f1", "demo", "main", "demo.main"),
                    sample_function("f2", "demo", "alt", "demo.main"),
                    sample_function("f3", "demo", "helper", "demo.helper"),
                    sample_function("f4", "demo", "extra", "demo.helper"),
                ],
                NodeLabel::Function,
            )
            .expect("save_nodes");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_fqn_uniqueness()
            .expect("check_fqn_uniqueness");
        assert_eq!(
            violations.len(),
            2,
            "expected two DQ-002 violations for two duplicate groups, got {violations:?}"
        );
        for v in &violations {
            assert_eq!(v.rule, "DQ-002");
            assert!(v.project.as_deref() == Some("demo"));
            assert!(
                v.message.contains("demo.main") || v.message.contains("demo.helper"),
                "message should mention a duplicate FQN: {}",
                v.message
            );
            assert!(
                v.message.contains("2 nodes"),
                "message should report 2 duplicate nodes: {}",
                v.message
            );
        }
    }

    #[test]
    fn test_dq002_file_nodes_handled_without_error() {
        // File nodes lack qualifiedName column; verify check_fqn_uniqueness
        // handles them gracefully (either via query failure → continue, or
        // empty results from NULL qualifiedName).
        let storage = fresh_storage();
        storage
            .save_nodes(
                &[
                    sample_file("file1", "demo", "/a.rs", "hash1"),
                    sample_file("file2", "demo", "/b.rs", "hash2"),
                ],
                NodeLabel::File,
            )
            .expect("save_nodes");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_fqn_uniqueness()
            .expect("check_fqn_uniqueness");
        assert!(
            violations.is_empty(),
            "File nodes without qualifiedName should not produce DQ-002 violations: {violations:?}"
        );
    }

    #[test]
    fn test_dq004_only_source_orphan_produces_single_violation() {
        // Exercises the DQ-004 source orphan branch (line 209) independently
        // from the target orphan branch. Only the source is missing.
        let storage = fresh_storage();
        storage
            .save_nodes(
                &[sample_function("f2", "demo", "helper", "demo.helper")],
                NodeLabel::Function,
            )
            .expect("save_nodes");
        // Edge from missing_src to existing f2.
        storage
            .save_edges(&[crate::model::Edge::builder(
                "missing_src",
                "f2",
                EdgeType::Calls,
                "demo",
            )
            .build()])
            .expect("save_edges");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_edge_integrity()
            .expect("check_edge_integrity");
        assert_eq!(
            violations.len(),
            1,
            "expected one DQ-004 violation for orphan source, got {violations:?}"
        );
        assert_eq!(violations[0].rule, "DQ-004");
        assert!(
            violations[0].message.contains("missing_src")
                && violations[0].message.contains("source"),
            "violation should mention orphan source: {}",
            violations[0].message
        );
    }

    #[test]
    fn test_dq004_only_target_orphan_produces_single_violation() {
        // Exercises the DQ-004 target orphan branch (line 216) independently.
        // Only the target is missing.
        let storage = fresh_storage();
        storage
            .save_nodes(
                &[sample_function("f1", "demo", "main", "demo.main")],
                NodeLabel::Function,
            )
            .expect("save_nodes");
        // Edge from existing f1 to missing_target.
        storage
            .save_edges(&[crate::model::Edge::builder(
                "f1",
                "missing_target",
                EdgeType::Calls,
                "demo",
            )
            .build()])
            .expect("save_edges");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_edge_integrity()
            .expect("check_edge_integrity");
        assert_eq!(
            violations.len(),
            1,
            "expected one DQ-004 violation for orphan target, got {violations:?}"
        );
        assert_eq!(violations[0].rule, "DQ-004");
        assert!(
            violations[0].message.contains("missing_target")
                && violations[0].message.contains("target"),
            "violation should mention orphan target: {}",
            violations[0].message
        );
    }

    #[test]
    fn test_dq005_violation_in_file_table() {
        // Exercises the DQ-005 violation construction (lines 279, 282, 284-285)
        // for a table other than Function. A File node with an unknown project
        // triggers an isolation violation in the File table.
        let storage = fresh_storage();
        storage
            .save_project(&sample_project("alpha", "alpha"))
            .expect("save_project");
        // File in known project "alpha".
        storage
            .save_nodes(
                &[sample_file("file1", "alpha", "/a.rs", "hash1")],
                NodeLabel::File,
            )
            .expect("save_nodes alpha file");
        // File in unknown project "ghost".
        storage
            .save_nodes(
                &[sample_file("file2", "ghost", "/b.rs", "hash2")],
                NodeLabel::File,
            )
            .expect("save_nodes ghost file");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_project_isolation()
            .expect("check_project_isolation");
        let file_violations: Vec<_> = violations
            .iter()
            .filter(|v| v.message.contains("File"))
            .collect();
        assert_eq!(
            file_violations.len(),
            1,
            "expected one DQ-005 violation for File table, got {violations:?}"
        );
        assert_eq!(file_violations[0].rule, "DQ-005");
        assert!(
            file_violations[0]
                .message
                .contains("total 2"),
            "total should be 2: {}",
            file_violations[0].message
        );
        assert!(
            file_violations[0]
                .message
                .contains("per-project sum 1"),
            "per-project sum should be 1: {}",
            file_violations[0].message
        );
    }

    #[test]
    fn test_dq006_multiple_empty_hash_files() {
        // Exercises the DQ-006 violation construction (line 317) with multiple
        // files having empty hashes.
        let storage = fresh_storage();
        storage
            .save_nodes(
                &[
                    sample_file("f1", "demo", "/a.rs", ""),
                    sample_file("f2", "demo", "/b.rs", ""),
                    sample_file("f3", "demo", "/c.rs", "sha256:valid"),
                ],
                NodeLabel::File,
            )
            .expect("save_nodes");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_hash_integrity()
            .expect("check_hash_integrity");
        assert_eq!(
            violations.len(),
            2,
            "expected two DQ-006 violations, got {violations:?}"
        );
        for v in &violations {
            assert_eq!(v.rule, "DQ-006");
            assert_eq!(v.project.as_deref(), Some("demo"));
            assert!(
                v.message.contains("empty hash"),
                "message should mention empty hash: {}",
                v.message
            );
        }
        let ids: Vec<&str> = violations
            .iter()
            .map(|v| {
                v.message
                    .split('\'')
                    .nth(1)
                    .unwrap_or("")
            })
            .collect();
        assert!(ids.contains(&"f1"), "should report f1: {ids:?}");
        assert!(ids.contains(&"f2"), "should report f2: {ids:?}");
    }

    #[test]
    fn test_run_all_with_all_four_violation_types() {
        // Exercises run_all aggregating violations from all four DQ rules,
        // ensuring each check's violation path is exercised end-to-end.
        let storage = fresh_storage();
        storage
            .save_project(&sample_project("alpha", "alpha"))
            .expect("save_project");

        // DQ-002: duplicate FQN.
        storage
            .save_nodes(
                &[
                    sample_function("f1", "alpha", "main", "alpha.main"),
                    sample_function("f2", "alpha", "alt", "alpha.main"),
                ],
                NodeLabel::Function,
            )
            .expect("save_nodes dup fqn");

        // DQ-004: orphan edge (both endpoints missing).
        storage
            .save_edges(&[crate::model::Edge::builder(
                "ghost_src",
                "ghost_tgt",
                EdgeType::Calls,
                "alpha",
            )
            .build()])
            .expect("save_edges orphan");

        // DQ-005: node in unknown project.
        storage
            .save_nodes(
                &[sample_function("g1", "ghost", "main", "ghost.main")],
                NodeLabel::Function,
            )
            .expect("save_nodes ghost");

        // DQ-006: file with empty hash.
        storage
            .save_nodes(
                &[sample_file("file1", "alpha", "/a.rs", "")],
                NodeLabel::File,
            )
            .expect("save_nodes empty hash file");

        let checker = QualityChecker::new(&*storage);
        let report = checker.run_all().expect("run_all");
        assert!(!report.is_clean());
        assert!(
            report.count_for_rule("DQ-002") >= 1,
            "should have DQ-002 violations: {:?}",
            report.violations
        );
        assert!(
            report.count_for_rule("DQ-004") >= 1,
            "should have DQ-004 violations: {:?}",
            report.violations
        );
        assert!(
            report.count_for_rule("DQ-005") >= 1,
            "should have DQ-005 violations: {:?}",
            report.violations
        );
        assert!(
            report.count_for_rule("DQ-006") >= 1,
            "should have DQ-006 violations: {:?}",
            report.violations
        );
    }

    #[test]
    fn test_dq005_multiple_projects_isolated_correctly() {
        // Exercises the per-project count summation (line 261) and total
        // comparison (line 277) with multiple known projects. Verifies no
        // false positive when isolation is maintained.
        let storage = fresh_storage();
        storage
            .save_project(&sample_project("alpha", "alpha"))
            .expect("save_project alpha");
        storage
            .save_project(&sample_project("beta", "beta"))
            .expect("save_project beta");
        storage
            .save_project(&sample_project("gamma", "gamma"))
            .expect("save_project gamma");

        storage
            .save_nodes(
                &[sample_function("a1", "alpha", "main", "alpha.main")],
                NodeLabel::Function,
            )
            .expect("save_nodes alpha");
        storage
            .save_nodes(
                &[
                    sample_function("b1", "beta", "main", "beta.main"),
                    sample_function("b2", "beta", "helper", "beta.helper"),
                ],
                NodeLabel::Function,
            )
            .expect("save_nodes beta");
        storage
            .save_nodes(
                &[sample_function("g1", "gamma", "main", "gamma.main")],
                NodeLabel::Function,
            )
            .expect("save_nodes gamma");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_project_isolation()
            .expect("check_project_isolation");
        assert!(
            violations.is_empty(),
            "three isolated projects should have no DQ-005 violations: {violations:?}"
        );
    }

    // --- Query error path tests: dropped table triggers Err(_) => continue ---

    #[test]
    fn test_dq002_skips_table_on_query_error() {
        // Drop the Variable table so its MATCH query errors (line 122 continue).
        // The check should still detect duplicate FQNs in the Function table.
        let storage = fresh_storage();
        storage
            .save_nodes(
                &[
                    sample_function("f1", "demo", "main", "demo.main"),
                    sample_function("f2", "demo", "other", "demo.main"),
                ],
                NodeLabel::Function,
            )
            .expect("save_nodes");
        storage
            .execute("DROP TABLE Variable;")
            .expect("drop table");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_fqn_uniqueness()
            .expect("check_fqn_uniqueness");
        assert_eq!(violations.len(), 1, "should still detect DQ-002 in Function table");
        assert_eq!(violations[0].rule, "DQ-002");
    }

    #[test]
    fn test_dq004_skips_table_on_query_error() {
        // Drop the Variable table so its node-id query errors (line 183 continue).
        // The check should still detect orphan edges using ids from other tables.
        let storage = fresh_storage();
        storage
            .save_nodes(
                &[sample_function("f1", "demo", "main", "demo.main")],
                NodeLabel::Function,
            )
            .expect("save_nodes");
        storage
            .save_edges(&[crate::model::Edge::builder(
                "f1",
                "missing_target",
                EdgeType::Calls,
                "demo",
            )
            .build()])
            .expect("save_edges");
        storage
            .execute("DROP TABLE Variable;")
            .expect("drop table");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_edge_integrity()
            .expect("check_edge_integrity");
        assert_eq!(violations.len(), 1, "should still detect DQ-004 orphan target");
        assert_eq!(violations[0].rule, "DQ-004");
        assert!(violations[0].message.contains("missing_target"));
    }

    #[test]
    fn test_dq005_skips_table_on_query_error() {
        // Drop the Variable table so both per-project count (line 253) and
        // total count (line 269) queries error. The check should still work
        // for the Function table.
        let storage = fresh_storage();
        storage
            .save_project(&sample_project("alpha", "alpha"))
            .expect("save_project");
        storage
            .save_nodes(
                &[sample_function("a1", "alpha", "main", "alpha.main")],
                NodeLabel::Function,
            )
            .expect("save_nodes alpha");
        storage
            .save_nodes(
                &[sample_function("g1", "ghost", "main", "ghost.main")],
                NodeLabel::Function,
            )
            .expect("save_nodes ghost");
        storage
            .execute("DROP TABLE Variable;")
            .expect("drop table");

        let checker = QualityChecker::new(&*storage);
        let violations = checker
            .check_project_isolation()
            .expect("check_project_isolation");
        assert!(
            violations
                .iter()
                .any(|v| v.rule == "DQ-005" && v.message.contains("Function")),
            "should still detect DQ-005 in Function table: {violations:?}"
        );
    }
}
