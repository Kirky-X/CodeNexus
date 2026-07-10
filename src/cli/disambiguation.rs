// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Ranked disambiguation for multi-match symbol queries (H14).
//!
//! When a symbol name (or FQN) matches multiple nodes in the graph, the
//! `trace`, `impact`, and `search` subcommands must surface a ranked
//! `ambiguous` list so the user can narrow with `--uid` / `--file` / `--kind`.
//!
//! # Scoring (deterministic)
//!
//! Each candidate receives four score components:
//!
//! | Component          | Source                                       |
//! |--------------------|----------------------------------------------|
//! | `exact_name_match` | `1.0` if `candidate.name == symbol`, else `0.0` |
//! | `kind_match`       | `1.0` if no `--kind` filter or matches, else `0.0` |
//! | `file_match`       | `1.0` if no `--file` filter or matches, else `0.0` |
//! | `confidence_score` | `1.0` (node-level default; edges carry tier) |
//!
//! `total_score = exact_name_match + kind_match + file_match + confidence_score`.
//! Ties are broken by FQN lexicographic order (ascending). Identical inputs
//! always produce identical orderings — no hash-map iteration.
//!
//! # Filtering
//!
//! `--uid` / `--file` / `--kind` filter the candidate set BEFORE scoring. If
//! exactly one candidate remains, the command proceeds; if zero or more than
//! one remain, the command fails loud with the filtered list.

use serde::Serialize;

use super::error::{CliError, Result};
use crate::kit::{Kit, StorageKey};
use crate::model::NodeLabel;
use crate::storage::schema::escape_cypher_string;

/// Symbol labels searched for disambiguation (mirrors `query::SYMBOL_LABELS`).
///
/// Kept as a private constant here so the disambiguation module does not depend
/// on the query subsystem's internals.
const SYMBOL_LABELS: &[NodeLabel] = &[
    NodeLabel::Module,
    NodeLabel::Class,
    NodeLabel::Struct,
    NodeLabel::Enum,
    NodeLabel::Trait,
    NodeLabel::Impl,
    NodeLabel::Function,
    NodeLabel::Method,
    NodeLabel::Variable,
    NodeLabel::GlobalVar,
    NodeLabel::Parameter,
    NodeLabel::Property,
    NodeLabel::Constructor,
    NodeLabel::Record,
    NodeLabel::Delegate,
    NodeLabel::Annotation,
    NodeLabel::Template,
];

/// Narrowing flags for disambiguation (H14 spec: `--uid` / `--file` / `--kind`).
///
/// All fields are `Option` — `None` means "no filter". When `Some`, the
/// candidate set is filtered before scoring.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DisambiguationFilters {
    /// Filter by node id (exact match).
    pub uid: Option<String>,
    /// Filter by file path (exact match).
    pub file: Option<String>,
    /// Filter by node label (exact match).
    pub kind: Option<NodeLabel>,
}

/// A single disambiguation candidate with score components.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Candidate {
    /// Node id (UID).
    pub uid: String,
    /// Short display name.
    pub name: String,
    /// Fully qualified name.
    pub qualified_name: String,
    /// Node label as a string (e.g. `"Function"`).
    pub label: String,
    /// Source file path, when available.
    pub file_path: Option<String>,
    /// 1-based start line, when available.
    pub start_line: Option<u32>,
    /// Score component: `1.0` if `name == symbol`, else `0.0`.
    pub exact_name_match: f32,
    /// Score component: `1.0` if no `--kind` filter or matches, else `0.0`.
    pub kind_match: f32,
    /// Score component: `1.0` if no `--file` filter or matches, else `0.0`.
    pub file_match: f32,
    /// Score component: node-level confidence (default `1.0`).
    pub confidence_score: f32,
    /// Total score (sum of the four components).
    pub total_score: f32,
}

/// Outcome of disambiguation.
#[derive(Debug, Clone)]
pub enum DisambiguationResult {
    /// Exactly one candidate matched (after filtering) — proceed with this.
    Single(Candidate),
    /// Multiple candidates matched — caller must surface the ranked list.
    Ambiguous(Vec<Candidate>),
    /// No candidates matched — caller may proceed (e.g. trace returns
    /// SymbolNotFound) or treat as an error depending on context.
    NotFound,
}

/// Resolves `symbol` against the graph, applying `filters` before scoring.
///
/// Queries each symbol-label table for nodes whose `name` or `qualifiedName`
/// equals `symbol`, applies the narrowing filters, scores the survivors, and
/// returns the [`DisambiguationResult`].
///
/// # Errors
///
/// Returns [`CliError::Storage`] if the underlying `storage.query()` fails.
pub fn resolve(
    kit: &Kit,
    symbol: &str,
    filters: &DisambiguationFilters,
) -> Result<DisambiguationResult> {
    let storage = kit.require::<StorageKey>()?;
    let raw = find_candidates(&*storage, symbol)?;
    let filtered = apply_filters(raw, filters);
    if filtered.is_empty() {
        return Ok(DisambiguationResult::NotFound);
    }
    let mut scored = score_candidates(filtered, symbol, filters);
    scored.sort_by(|a, b| {
        // Descending by total_score, then ascending by FQN (deterministic).
        b.total_score
            .partial_cmp(&a.total_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.qualified_name.cmp(&b.qualified_name))
    });
    if scored.len() == 1 {
        Ok(DisambiguationResult::Single(scored.into_iter().next().expect("single")))
    } else {
        Ok(DisambiguationResult::Ambiguous(scored))
    }
}

/// Raw candidate row from the database (before filtering/scoring).
#[derive(Debug, Clone)]
struct RawCandidate {
    id: String,
    name: String,
    qualified_name: String,
    label: NodeLabel,
    file_path: Option<String>,
    start_line: Option<u32>,
}

/// Queries each symbol-label table for exact `name` or `qualifiedName` matches.
fn find_candidates(
    storage: &dyn crate::storage::capability::Storage,
    symbol: &str,
) -> Result<Vec<RawCandidate>> {
    let escaped = escape_cypher_string(symbol);
    let mut out = Vec::new();
    for &label in SYMBOL_LABELS {
        let table = label.table_name();
        let cypher = format!(
            "MATCH (n:{table}) WHERE n.name = '{escaped}' OR n.qualifiedName = '{escaped}' \
             RETURN n.id AS id, n.name AS name, n.qualifiedName AS qn, \
             n.filePath AS filePath, n.startLine AS line;"
        );
        match storage.query(&cypher) {
            Ok(rows) => {
                for row in rows {
                    let id = row.first().and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if id.is_empty() {
                        continue;
                    }
                    let name = row
                        .get(1)
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let qn = row
                        .get(2)
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let file_path = row
                        .get(3)
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let start_line = row.get(4).and_then(|v| v.as_u64()).map(|n| n as u32);
                    out.push(RawCandidate {
                        id,
                        name,
                        qualified_name: qn,
                        label,
                        file_path,
                        start_line,
                    });
                }
            }
            // Some label tables may not exist or may lack a column — skip.
            Err(_) => continue,
        }
    }
    Ok(out)
}

/// Applies `--uid` / `--file` / `--kind` filters, removing non-matching
/// candidates.
fn apply_filters(
    candidates: Vec<RawCandidate>,
    filters: &DisambiguationFilters,
) -> Vec<RawCandidate> {
    candidates
        .into_iter()
        .filter(|c| {
            if let Some(ref uid) = filters.uid {
                if c.id != *uid {
                    return false;
                }
            }
            if let Some(ref file) = filters.file {
                if c.file_path.as_deref() != Some(file.as_str()) {
                    return false;
                }
            }
            if let Some(kind) = filters.kind {
                if c.label != kind {
                    return false;
                }
            }
            true
        })
        .collect()
}

/// Scores each candidate, producing the four components and the total.
fn score_candidates(
    candidates: Vec<RawCandidate>,
    symbol: &str,
    filters: &DisambiguationFilters,
) -> Vec<Candidate> {
    candidates
        .into_iter()
        .map(|c| {
            let exact_name_match = if c.name == symbol { 1.0 } else { 0.0 };
            // After filtering, kind_match and file_match are always 1.0 for
            // surviving candidates. We still compute them from the filter
            // state for transparency in the output.
            let kind_match = if filters.kind.is_none() || Some(c.label) == filters.kind {
                1.0
            } else {
                0.0
            };
            let file_match = if filters.file.is_none()
                || c.file_path.as_deref() == filters.file.as_deref()
            {
                1.0
            } else {
                0.0
            };
            // Node-level confidence default — edges carry tier-based scores.
            let confidence_score = 1.0;
            let total_score = exact_name_match + kind_match + file_match + confidence_score;
            Candidate {
                uid: c.id,
                name: c.name,
                qualified_name: c.qualified_name,
                label: c.label.to_string(),
                file_path: c.file_path,
                start_line: c.start_line,
                exact_name_match,
                kind_match,
                file_match,
                confidence_score,
                total_score,
            }
        })
        .collect()
}

/// Parses a `--kind` string into a [`NodeLabel`].
///
/// # Errors
/// Returns [`CliError::InvalidInput`] if the string is not a valid label.
pub fn parse_kind(s: &str) -> Result<NodeLabel> {
    s.parse::<NodeLabel>()
        .map_err(|e| CliError::InvalidInput(format!("invalid --kind '{s}': {e}")))
}

/// Finds a single node by UID across all symbol-label tables.
///
/// Used by `search --uid <UID>` to look up a node directly by id (bypassing
/// the text search). Returns `None` if no node with that id exists.
///
/// # Errors
/// Returns [`CliError::Storage`] if the underlying `storage.query()` fails.
pub fn find_by_uid(kit: &Kit, uid: &str) -> Result<Option<Candidate>> {
    let storage = kit.require::<StorageKey>()?;
    let escaped = escape_cypher_string(uid);
    for &label in SYMBOL_LABELS {
        let table = label.table_name();
        let cypher = format!(
            "MATCH (n:{table}) WHERE n.id = '{escaped}' \
             RETURN n.id AS id, n.name AS name, n.qualifiedName AS qn, \
             n.filePath AS filePath, n.startLine AS line;"
        );
        if let Ok(rows) = storage.query(&cypher) {
            if let Some(row) = rows.into_iter().next() {
                let id = row
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !id.is_empty() {
                    let name = row
                        .get(1)
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let qn = row
                        .get(2)
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let file_path = row
                        .get(3)
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let start_line = row.get(4).and_then(|v| v.as_u64()).map(|n| n as u32);
                    return Ok(Some(Candidate {
                        uid: id,
                        name,
                        qualified_name: qn,
                        label: label.to_string(),
                        file_path,
                        start_line,
                        exact_name_match: 1.0,
                        kind_match: 1.0,
                        file_match: 1.0,
                        confidence_score: 1.0,
                        total_score: 4.0,
                    }));
                }
            }
        }
    }
    Ok(None)
}

/// JSON-serializable output for the `ambiguous` array.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AmbiguousOutput {
    /// The queried symbol.
    pub symbol: String,
    /// The number of candidates.
    pub count: usize,
    /// The ranked candidates (highest score first).
    pub ambiguous: Vec<Candidate>,
}

/// Builds an [`AmbiguousOutput`] from a ranked candidate list and prints it
/// as JSON to stdout. Returns [`CliError::InvalidInput`] so the caller can
/// propagate the exit-code-1 signal.
pub fn fail_loud(symbol: &str, candidates: Vec<Candidate>) -> CliError {
    let output = AmbiguousOutput {
        symbol: symbol.to_string(),
        count: candidates.len(),
        ambiguous: candidates,
    };
    // Print the ambiguous list to stdout so the user (and --json consumers)
    // can see the candidates. Errors are returned to trigger exit code 1.
    if let Ok(json) = serde_json::to_string(&output) {
        println!("{json}");
    }
    CliError::InvalidInput(format!(
        "ambiguous symbol '{}' matched {} candidates; use --uid/--file/--kind to narrow",
        symbol,
        output.count
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("disambig_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &str) -> Kit {
        let config = KitBootstrapConfig::new(PathBuf::from(db));
        build_kit(&config).expect("build_kit")
    }

    /// Seeds two functions named `handle` in different files + one `handleEvent`.
    fn seed_ambiguous(kit: &Kit) {
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage
            .execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'handle', qualifiedName: 'demo.handle', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .expect("create f1");
        storage
            .execute("CREATE (:Function {id: 'f2', project: 'demo', name: 'handle', qualifiedName: 'demo.handle', filePath: '/src/b.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .expect("create f2");
        storage
            .execute("CREATE (:Function {id: 'f3', project: 'demo', name: 'handleEvent', qualifiedName: 'demo.handleEvent', filePath: '/src/c.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .expect("create f3");
    }

    // --- DisambiguationFilters ---

    #[test]
    fn filters_default_is_empty() {
        let f = DisambiguationFilters::default();
        assert!(f.uid.is_none());
        assert!(f.file.is_none());
        assert!(f.kind.is_none());
    }

    #[test]
    fn filters_clone_eq() {
        let f = DisambiguationFilters {
            uid: Some("f1".into()),
            file: Some("/src/a.rs".into()),
            kind: Some(NodeLabel::Function),
        };
        assert_eq!(f, f.clone());
    }

    // --- resolve: single candidate ---

    #[test]
    fn resolve_unique_name_returns_single() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_ambiguous(&kit);
        let filters = DisambiguationFilters::default();
        let result = resolve(&kit, "handleEvent", &filters).expect("resolve");
        match result {
            DisambiguationResult::Single(c) => {
                assert_eq!(c.name, "handleEvent");
                assert_eq!(c.uid, "f3");
            }
            other => panic!("expected Single, got {other:?}"),
        }
    }

    // --- resolve: multiple candidates ---

    #[test]
    fn resolve_ambiguous_returns_ranked_list() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_ambiguous(&kit);
        let filters = DisambiguationFilters::default();
        let result = resolve(&kit, "handle", &filters).expect("resolve");
        match result {
            DisambiguationResult::Ambiguous(candidates) => {
                // f1 and f2 have name "handle" (exact match), f3 has "handleEvent"
                // (not exact). f3 should not appear because we query for exact
                // name or FQN match.
                assert_eq!(candidates.len(), 2, "two exact-name matches");
                // Both have exact_name_match=1.0, so tie broken by FQN asc.
                assert_eq!(candidates[0].qualified_name, "demo.handle");
                assert_eq!(candidates[1].qualified_name, "demo.handle");
                // Both should have exact_name_match=1.0
                assert!((candidates[0].exact_name_match - 1.0).abs() < f32::EPSILON);
                assert!((candidates[1].exact_name_match - 1.0).abs() < f32::EPSILON);
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    // --- resolve: not found ---

    #[test]
    fn resolve_missing_symbol_returns_not_found() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_ambiguous(&kit);
        let filters = DisambiguationFilters::default();
        let result = resolve(&kit, "nonexistent", &filters).expect("resolve");
        assert!(matches!(result, DisambiguationResult::NotFound));
    }

    // --- resolve: --uid filter narrows to single ---

    #[test]
    fn resolve_uid_filter_narrows_to_single() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_ambiguous(&kit);
        let filters = DisambiguationFilters {
            uid: Some("f1".into()),
            ..Default::default()
        };
        let result = resolve(&kit, "handle", &filters).expect("resolve");
        match result {
            DisambiguationResult::Single(c) => assert_eq!(c.uid, "f1"),
            other => panic!("expected Single, got {other:?}"),
        }
    }

    // --- resolve: --file filter narrows to single ---

    #[test]
    fn resolve_file_filter_narrows_to_single() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_ambiguous(&kit);
        let filters = DisambiguationFilters {
            file: Some("/src/a.rs".into()),
            ..Default::default()
        };
        let result = resolve(&kit, "handle", &filters).expect("resolve");
        match result {
            DisambiguationResult::Single(c) => {
                assert_eq!(c.uid, "f1");
                assert_eq!(c.file_path.as_deref(), Some("/src/a.rs"));
            }
            other => panic!("expected Single, got {other:?}"),
        }
    }

    // --- resolve: --file filter leaves multiple -> Ambiguous ---

    #[test]
    fn resolve_file_filter_multiple_remains_ambiguous() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        // Seed two "handle" in the same file.
        let storage = kit.require::<StorageKey>().expect("require_storage");
        storage
            .execute("CREATE (:Function {id: 'g1', project: 'demo', name: 'handle', qualifiedName: 'demo.handle', filePath: '/src/same.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .expect("create g1");
        storage
            .execute("CREATE (:Function {id: 'g2', project: 'demo', name: 'handle', qualifiedName: 'demo.handle', filePath: '/src/same.rs', startLine: 10, endLine: 15, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .expect("create g2");
        let filters = DisambiguationFilters {
            file: Some("/src/same.rs".into()),
            ..Default::default()
        };
        let result = resolve(&kit, "handle", &filters).expect("resolve");
        assert!(
            matches!(result, DisambiguationResult::Ambiguous(ref cs) if cs.len() == 2),
            "expected Ambiguous(2), got {result:?}"
        );
    }

    // --- resolve: --kind filter ---

    #[test]
    fn resolve_kind_filter_narrows_by_label() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageKey>().expect("require_storage");
        // Same name "foo" but different labels.
        storage
            .execute("CREATE (:Function {id: 'fn1', project: 'demo', name: 'foo', qualifiedName: 'demo.foo', filePath: '/src/a.rs', startLine: 1, endLine: 5, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});")
            .expect("create fn1");
        storage
            .execute("CREATE (:Class {id: 'cls1', project: 'demo', name: 'foo', qualifiedName: 'demo.Foo', filePath: '/src/b.rs', startLine: 1, endLine: 5, isExported: true, docstring: '', content: '', parentQn: ''});")
            .expect("create cls1");
        let filters = DisambiguationFilters {
            kind: Some(NodeLabel::Class),
            ..Default::default()
        };
        let result = resolve(&kit, "foo", &filters).expect("resolve");
        match result {
            DisambiguationResult::Single(c) => {
                assert_eq!(c.uid, "cls1");
                assert_eq!(c.label, "Class");
            }
            other => panic!("expected Single, got {other:?}"),
        }
    }

    // --- resolve: deterministic ordering ---

    #[test]
    fn resolve_is_deterministic_across_calls() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_ambiguous(&kit);
        let filters = DisambiguationFilters::default();
        let r1 = resolve(&kit, "handle", &filters).expect("resolve");
        let r2 = resolve(&kit, "handle", &filters).expect("resolve");
        // Both calls should produce identical orderings.
        match (&r1, &r2) {
            (DisambiguationResult::Ambiguous(a), DisambiguationResult::Ambiguous(b)) => {
                assert_eq!(a.len(), b.len());
                for (x, y) in a.iter().zip(b.iter()) {
                    assert_eq!(x.uid, y.uid);
                    assert_eq!(x.total_score, y.total_score);
                }
            }
            _ => panic!("both should be Ambiguous"),
        }
    }

    // --- score components ---

    #[test]
    fn exact_name_match_is_one_for_exact() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        seed_ambiguous(&kit);
        let filters = DisambiguationFilters::default();
        let result = resolve(&kit, "handle", &filters).expect("resolve");
        if let DisambiguationResult::Ambiguous(cs) = result {
            for c in &cs {
                assert!((c.exact_name_match - 1.0).abs() < f32::EPSILON);
                assert!((c.kind_match - 1.0).abs() < f32::EPSILON);
                assert!((c.file_match - 1.0).abs() < f32::EPSILON);
                assert!((c.confidence_score - 1.0).abs() < f32::EPSILON);
                assert!((c.total_score - 4.0).abs() < f32::EPSILON);
            }
        } else {
            panic!("expected Ambiguous");
        }
    }

    // --- fail_loud ---

    #[test]
    fn fail_loud_returns_invalid_input() {
        let candidates = vec![Candidate {
            uid: "x".into(),
            name: "x".into(),
            qualified_name: "demo.x".into(),
            label: "Function".into(),
            file_path: None,
            start_line: None,
            exact_name_match: 1.0,
            kind_match: 1.0,
            file_match: 1.0,
            confidence_score: 1.0,
            total_score: 4.0,
        }];
        let err = fail_loud("x", candidates);
        assert!(matches!(err, CliError::InvalidInput(_)));
        assert_eq!(err.exit_code(), 2);
    }

    // --- AmbiguousOutput serialization ---

    #[test]
    fn ambiguous_output_serializes_to_json() {
        let out = AmbiguousOutput {
            symbol: "handle".into(),
            count: 1,
            ambiguous: vec![Candidate {
                uid: "f1".into(),
                name: "handle".into(),
                qualified_name: "demo.handle".into(),
                label: "Function".into(),
                file_path: Some("/src/a.rs".into()),
                start_line: Some(1),
                exact_name_match: 1.0,
                kind_match: 1.0,
                file_match: 1.0,
                confidence_score: 1.0,
                total_score: 4.0,
            }],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"symbol\":\"handle\""));
        assert!(json.contains("\"count\":1"));
        assert!(json.contains("\"ambiguous\""));
        assert!(json.contains("\"exact_name_match\":1.0"));
        assert!(json.contains("\"kind_match\":1.0"));
        assert!(json.contains("\"file_match\":1.0"));
        assert!(json.contains("\"confidence_score\":1.0"));
    }

    // --- apply_filters ---

    #[test]
    fn apply_filters_no_filters_keeps_all() {
        let candidates = vec![RawCandidate {
            id: "f1".into(),
            name: "x".into(),
            qualified_name: "demo.x".into(),
            label: NodeLabel::Function,
            file_path: Some("/a.rs".into()),
            start_line: Some(1),
        }];
        let filters = DisambiguationFilters::default();
        let result = apply_filters(candidates, &filters);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn apply_filters_uid_removes_non_matching() {
        let candidates = vec![
            RawCandidate {
                id: "f1".into(),
                name: "x".into(),
                qualified_name: "demo.x".into(),
                label: NodeLabel::Function,
                file_path: None,
                start_line: None,
            },
            RawCandidate {
                id: "f2".into(),
                name: "x".into(),
                qualified_name: "demo.x".into(),
                label: NodeLabel::Function,
                file_path: None,
                start_line: None,
            },
        ];
        let filters = DisambiguationFilters {
            uid: Some("f1".into()),
            ..Default::default()
        };
        let result = apply_filters(candidates, &filters);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "f1");
    }
}
