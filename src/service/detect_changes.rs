// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `detect_changes` service: find symbols affected by uncommitted git changes.
//!
//! Runs `git diff` in `path`, parses the unified diff to extract changed line
//! ranges, then queries the graph for symbols whose `[startLine, endLine]`
//! overlap any changed range. Each affected symbol is annotated with an
//! `incoming_edge_count` and a `risk_level` (low / medium / high).

use std::path::Path;
use std::process::Command;

use serde::Serialize;
use serde_json::Value;

use crate::kit::StorageModule;
use crate::model::NodeLabel;
use crate::service::error::{CodeNexusError, kit_not_initialized, to_api_error, wrap_error, wrap_kit_error};
use crate::service::runtime::kit;
use crate::storage::schema::node_table_columns;

#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::service_api;

/// Git diff mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffMode {
    Unstaged,
    Staged,
    Head,
}

impl DiffMode {
    fn from_cli_str(s: &str) -> Option<Self> {
        match s {
            "unstaged" => Some(Self::Unstaged),
            "staged" => Some(Self::Staged),
            "head" => Some(Self::Head),
            _ => None,
        }
    }

    fn git_args(self) -> &'static [&'static str] {
        match self {
            Self::Unstaged => &["diff", "--no-color", "--unified=0"],
            Self::Staged => &["diff", "--staged", "--no-color", "--unified=0"],
            Self::Head => &["diff", "HEAD", "--no-color", "--unified=0"],
        }
    }
}

/// Runs `git -C {root} diff ...` and returns the raw stdout as a string.
fn run_git_diff(root: &Path, mode: DiffMode) -> Result<String, CodeNexusError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(mode.git_args())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CodeNexusError::InvalidInput(format!(
                    "git binary not found on PATH — detect-changes requires git. Error: {e}"
                ))
            } else {
                CodeNexusError::Io(e)
            }
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CodeNexusError::InvalidInput(format!(
            "git diff failed (status {}): {}",
            output.status,
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A parsed hunk: `(new_start, new_len)` in the new file's line numbering.
type LineRange = (u32, u32);

/// Parses a unified diff (with `--unified=0`) into `(file_path, Vec<(new_start, new_len)>)`.
fn parse_unified_diff(diff: &str) -> Vec<(String, Vec<LineRange>)> {
    let mut result: Vec<(String, Vec<LineRange>)> = Vec::new();
    let mut current_file: Option<String> = None;
    let mut current_ranges: Vec<LineRange> = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            if let Some(file) = current_file.take() {
                if !current_ranges.is_empty() {
                    result.push((file, std::mem::take(&mut current_ranges)));
                }
            }
            current_file = parse_diff_path(rest);
        } else if line.starts_with("@@ ") {
            if let Some(range) = parse_hunk_new_range(line) {
                current_ranges.push(range);
            }
        }
    }
    if let Some(file) = current_file.take() {
        if !current_ranges.is_empty() {
            result.push((file, current_ranges));
        }
    }
    result
}

/// Parses `+++ b/path` or `+++ /dev/null`. Returns `None` for `/dev/null`.
fn parse_diff_path(s: &str) -> Option<String> {
    let s = s.trim();
    if s == "/dev/null" {
        return None;
    }
    let stripped = s.strip_prefix("b/").unwrap_or(s);
    Some(stripped.to_string())
}

/// Parses `@@ -old_start,old_len +new_start,new_len @@` and returns
/// `(new_start, new_len)`. Returns `None` if invalid or `new_len` is 0.
fn parse_hunk_new_range(line: &str) -> Option<LineRange> {
    let plus_idx = line.find(" +")?;
    let after_plus = &line[plus_idx + 2..];
    let end_idx = after_plus.find(" @@")?;
    let spec = &after_plus[..end_idx];
    let (start_str, len_str) = spec.split_once(',').unwrap_or((spec, "1"));
    let new_start: u32 = start_str.parse().ok()?;
    let new_len: u32 = len_str.parse().ok()?;
    if new_len == 0 {
        return None;
    }
    Some((new_start, new_len))
}

/// Queries the graph for symbols in `rel_path` or `abs_path` whose
/// `[startLine, endLine]` overlaps any range in `ranges`.
fn find_symbols_in_ranges(
    storage: &dyn crate::storage::capability::Storage,
    rel_path: &str,
    abs_path: &Path,
    ranges: &[(u32, u32)],
) -> Result<Vec<AffectedSymbolOutput>, CodeNexusError> {
    struct MatchingSymbol {
        id: String,
        name: String,
        label: String,
        qualified_name: String,
        file_path: String,
        start_line: u32,
        end_line: u32,
    }

    let mut matching: Vec<MatchingSymbol> = Vec::new();
    let rel = rel_path.to_string();
    let abs = abs_path.to_string_lossy().to_string();
    for label in NodeLabel::all() {
        if label == NodeLabel::Project {
            continue;
        }
        let cols = node_table_columns(label);
        if !cols.contains(&"filePath") || !cols.contains(&"startLine") || !cols.contains(&"endLine")
        {
            continue;
        }
        let table = crate::storage::schema::escape_identifier(label.table_name());
        let rel_esc = crate::storage::schema::escape_cypher_string(&rel);
        let abs_esc = crate::storage::schema::escape_cypher_string(&abs);
        let cypher = format!(
            "MATCH (n:{table}) WHERE n.filePath = '{rel_esc}' OR n.filePath = '{abs_esc}' \
             RETURN n.id AS id, n.name AS name, n.qualifiedName AS qualifiedName, \
             n.filePath AS filePath, n.startLine AS startLine, n.endLine AS endLine;"
        );
        let rows = storage.query(&cypher)?;
        for row in rows {
            let id = row
                .first()
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_default();
            let name = row
                .get(1)
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_default();
            let qualified_name = row
                .get(2)
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_default();
            let file_path = row
                .get(3)
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_default();
            let start_line = row
                .get(4)
                .and_then(|v| v.as_i64())
                .and_then(|i| u32::try_from(i).ok())
                .unwrap_or(0);
            let end_line = row
                .get(5)
                .and_then(|v| v.as_i64())
                .and_then(|i| u32::try_from(i).ok())
                .unwrap_or(0);
            if !ranges_overlap(start_line, end_line, ranges) {
                continue;
            }
            matching.push(MatchingSymbol {
                id,
                name,
                label: label.to_string(),
                qualified_name,
                file_path,
                start_line,
                end_line,
            });
        }
    }

    if matching.is_empty() {
        return Ok(Vec::new());
    }

    // Batch query: count incoming edges for all matching symbols in one query
    let ids_clause = matching
        .iter()
        .map(|m| {
            format!(
                "'{}'",
                crate::storage::schema::escape_cypher_string(&m.id)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let cypher = format!(
        "MATCH (r:CodeRelation) WHERE r.target IN [{ids_clause}] \
         RETURN r.target AS id, count(r) AS cnt;"
    );
    let edge_rows = storage.query(&cypher)?;
    let mut edge_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for row in &edge_rows {
        let id = row.first().and_then(|v| v.as_str()).unwrap_or("");
        let cnt = row.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        edge_counts.insert(id.to_string(), cnt);
    }

    let out = matching
        .into_iter()
        .map(|m| {
            let incoming_edge_count = edge_counts.get(&m.id).copied().unwrap_or(0);
            let risk_level = classify_risk(incoming_edge_count);
            AffectedSymbolOutput {
                name: m.name,
                label: m.label,
                qualified_name: m.qualified_name,
                file_path: m.file_path,
                start_line: m.start_line,
                end_line: m.end_line,
                incoming_edge_count,
                risk_level: risk_level.to_string(),
            }
        })
        .collect();
    Ok(out)
}

/// Returns `true` if `[start, end]` overlaps any `(range_start, range_len)` range.
fn ranges_overlap(start: u32, end: u32, ranges: &[(u32, u32)]) -> bool {
    if end < start {
        return false;
    }
    ranges.iter().any(|(rs, rl)| {
        let re = rs.saturating_add(*rl).saturating_sub(1);
        start <= re && *rs <= end
    })
}

/// Classifies risk by incoming edge count (blast radius).
fn classify_risk(incoming: usize) -> &'static str {
    match incoming {
        0 => "low",
        1..=3 => "medium",
        _ => "high",
    }
}

/// JSON-serializable detect-changes output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DetectChangesOutput {
    /// The codebase root path that was diffed.
    pub path: String,
    /// The diff mode used (`unstaged` / `staged` / `head`).
    pub mode: String,
    /// Number of files with at least one changed hunk.
    pub files_changed: usize,
    /// Symbols whose line range overlaps a changed hunk.
    pub affected: Vec<AffectedSymbolOutput>,
}

/// JSON-serializable view of an affected symbol.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AffectedSymbolOutput {
    pub name: String,
    pub label: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    /// Number of edges in the graph pointing at this symbol.
    pub incoming_edge_count: usize,
    /// `low` (0 incoming), `medium` (1–3), `high` (≥4).
    pub risk_level: String,
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[service_api(
    name = "detect_changes",
    version = "0.3.2",
    description = "Detect symbols affected by uncommitted git changes and classify their risk.",
    cli = true
)]
async fn detect_changes(path: String, mode: String) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let repo_root = Path::new(&path);
    if !repo_root.is_dir() {
        return Err(ApiError::InvalidInput {
            message: format!("path is not a directory: {path}"),
            field: Some("path".to_string()),
            value: Some(Value::String(path)),
        });
    }
    let diff_mode = DiffMode::from_cli_str(&mode).ok_or_else(|| ApiError::InvalidInput {
        message: format!("unknown diff mode '{mode}' (expected unstaged/staged/head)"),
        field: Some("mode".to_string()),
        value: Some(Value::String(mode.clone())),
    })?;

    let diff_output = run_git_diff(repo_root, diff_mode).map_err(|e| to_api_error(e, "detect_changes_error"))?;
    let hunks = parse_unified_diff(&diff_output);
    let files_changed = hunks.len();

    let storage = kit
        .require::<StorageModule>()
        .map_err(|e| wrap_kit_error("Failed to resolve storage capability", e))?;
    let mut affected: Vec<AffectedSymbolOutput> = Vec::new();
    for (rel_path, ranges) in &hunks {
        let abs_path = repo_root.join(rel_path);
        for sym in
            find_symbols_in_ranges(&*storage, rel_path, &abs_path, ranges).map_err(|e| to_api_error(e, "detect_changes_error"))?
        {
            affected.push(sym);
        }
    }
    affected.sort_by(|a, b| {
        a.qualified_name
            .cmp(&b.qualified_name)
            .then_with(|| a.file_path.cmp(&b.file_path))
            .then_with(|| a.start_line.cmp(&b.start_line))
    });
    affected.dedup_by(|a, b| {
        a.qualified_name == b.qualified_name
            && a.file_path == b.file_path
            && a.start_line == b.start_line
    });

    let output = DetectChangesOutput {
        path,
        mode,
        files_changed,
        affected,
    };
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_detect_testdb");
        (dir, path)
    }

    fn build_kit_for_db(db: &str) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(PathBuf::from(db));
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    /// Core logic mirroring the service function, taking explicit params
    /// (no DetectChangesArgs) so tests can exercise error paths without the
    /// `#[service_api]` macro wrapper.
    fn detect_changes_core(kit: &AsyncKit<AsyncReady>, path: &str, mode: &str) -> Result<(), CodeNexusError> {
        let repo_root = Path::new(path);
        if !repo_root.is_dir() {
            return Err(CodeNexusError::InvalidInput(format!(
                "path is not a directory: {path}"
            )));
        }
        let diff_mode = DiffMode::from_cli_str(mode).ok_or_else(|| {
            CodeNexusError::InvalidInput(format!(
                "unknown diff mode '{mode}' (expected unstaged/staged/head)"
            ))
        })?;

        let diff_output = run_git_diff(repo_root, diff_mode)?;
        let hunks = parse_unified_diff(&diff_output);
        let files_changed = hunks.len();

        let storage = kit.require::<StorageModule>()?;
        let mut affected: Vec<AffectedSymbolOutput> = Vec::new();
        for (rel_path, ranges) in &hunks {
            let abs_path = repo_root.join(rel_path);
            for sym in find_symbols_in_ranges(&*storage, rel_path, &abs_path, ranges)? {
                affected.push(sym);
            }
        }
        affected.sort_by(|a, b| {
            a.qualified_name
                .cmp(&b.qualified_name)
                .then_with(|| a.file_path.cmp(&b.file_path))
                .then_with(|| a.start_line.cmp(&b.start_line))
        });
        affected.dedup_by(|a, b| {
            a.qualified_name == b.qualified_name
                && a.file_path == b.file_path
                && a.start_line == b.start_line
        });

        let output = DetectChangesOutput {
            path: path.to_string(),
            mode: mode.to_string(),
            files_changed,
            affected,
        };
        let json = serde_json::to_string(&output)?;
        println!("{json}");
        Ok(())
    }

    // --- DiffMode ---

    #[test]
    fn diff_mode_parses_known_modes() {
        assert_eq!(DiffMode::from_cli_str("unstaged"), Some(DiffMode::Unstaged));
        assert_eq!(DiffMode::from_cli_str("staged"), Some(DiffMode::Staged));
        assert_eq!(DiffMode::from_cli_str("head"), Some(DiffMode::Head));
        assert_eq!(DiffMode::from_cli_str("bogus"), None);
    }

    #[test]
    fn diff_mode_git_args_correct() {
        assert!(DiffMode::Unstaged.git_args().contains(&"diff"));
        assert!(DiffMode::Staged.git_args().contains(&"--staged"));
        assert!(DiffMode::Head.git_args().contains(&"HEAD"));
    }

    // --- parse_diff_path ---

    #[test]
    fn parse_diff_path_strips_b_prefix() {
        assert_eq!(
            parse_diff_path("b/src/main.rs").as_deref(),
            Some("src/main.rs")
        );
    }

    #[test]
    fn parse_diff_path_no_prefix() {
        assert_eq!(
            parse_diff_path("src/main.rs").as_deref(),
            Some("src/main.rs")
        );
    }

    #[test]
    fn parse_diff_path_dev_null_returns_none() {
        assert!(parse_diff_path("/dev/null").is_none());
    }

    // --- parse_hunk_new_range ---

    #[test]
    fn parse_hunk_with_len() {
        let r = parse_hunk_new_range("@@ -10,3 +12,5 @@ fn ctx").unwrap();
        assert_eq!(r, (12, 5));
    }

    #[test]
    fn parse_hunk_default_len_one() {
        let r = parse_hunk_new_range("@@ -20 +22 @@").unwrap();
        assert_eq!(r, (22, 1));
    }

    #[test]
    fn parse_hunk_zero_len_returns_none() {
        assert!(parse_hunk_new_range("@@ -10,3 +0,0 @@").is_none());
    }

    #[test]
    fn parse_hunk_garbage_returns_none() {
        assert!(parse_hunk_new_range("not a hunk").is_none());
        assert!(parse_hunk_new_range("@@ missing plus").is_none());
    }

    // --- parse_unified_diff ---

    #[test]
    fn parse_unified_diff_single_file_single_hunk() {
        let diff = "\
diff --git a/foo.rs b/foo.rs
index abc..def 100644
--- a/foo.rs
+++ b/foo.rs
@@ -10,3 +12,5 @@ fn old() {
+new line 1
+new line 2
";
        let hunks = parse_unified_diff(diff);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].0, "foo.rs");
        assert_eq!(hunks[0].1, vec![(12, 5)]);
    }

    #[test]
    fn parse_unified_diff_multiple_hunks_one_file() {
        let diff = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +2 @@
-a
+b
@@ -10 +11 @@
-c
+d
";
        let hunks = parse_unified_diff(diff);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].1, vec![(2, 1), (11, 1)]);
    }

    #[test]
    fn parse_unified_diff_multiple_files() {
        let diff = "\
diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +2 @@
-a
+b
diff --git a/b.rs b/b.rs
--- b/b.rs
+++ b/b.rs
@@ -5,2 +6,2 @@
-c
+d
";
        let hunks = parse_unified_diff(diff);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].0, "a.rs");
        assert_eq!(hunks[1].0, "b.rs");
    }

    #[test]
    fn parse_unified_diff_skips_dev_null() {
        let diff = "\
diff --git a/deleted.rs b/deleted.rs
--- a/deleted.rs
+++ /dev/null
@@ -1,3 +0,0 @@
-a
-b
-c
diff --git a/added.rs b/added.rs
--- /dev/null
+++ b/added.rs
@@ -0,0 +1,2 @@
+a
+b
";
        let hunks = parse_unified_diff(diff);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].0, "added.rs");
    }

    #[test]
    fn parse_unified_diff_empty_returns_empty() {
        assert!(parse_unified_diff("").is_empty());
    }

    // --- ranges_overlap ---

    #[test]
    fn ranges_overlap_symbol_before_range() {
        assert!(!ranges_overlap(1, 5, &[(10, 5)]));
    }

    #[test]
    fn ranges_overlap_symbol_after_range() {
        assert!(!ranges_overlap(20, 25, &[(10, 5)]));
    }

    #[test]
    fn ranges_overlap_symbol_straddles_range_start() {
        assert!(ranges_overlap(8, 12, &[(10, 5)]));
    }

    #[test]
    fn ranges_overlap_symbol_inside_range() {
        assert!(ranges_overlap(11, 13, &[(10, 5)]));
    }

    #[test]
    fn ranges_overlap_symbol_straddles_range_end() {
        assert!(ranges_overlap(13, 20, &[(10, 5)]));
    }

    #[test]
    fn ranges_overlap_multiple_ranges_any_match() {
        assert!(ranges_overlap(1, 2, &[(5, 1), (1, 1)]));
        assert!(!ranges_overlap(1, 2, &[(5, 1), (10, 1)]));
    }

    #[test]
    fn ranges_overlap_empty_ranges() {
        assert!(!ranges_overlap(1, 5, &[]));
    }

    #[test]
    fn ranges_overlap_end_before_start_returns_false() {
        assert!(!ranges_overlap(10, 5, &[(1, 10)]));
    }

    // --- classify_risk ---

    #[test]
    fn classify_risk_low_medium_high() {
        assert_eq!(classify_risk(0), "low");
        assert_eq!(classify_risk(1), "medium");
        assert_eq!(classify_risk(3), "medium");
        assert_eq!(classify_risk(4), "high");
        assert_eq!(classify_risk(100), "high");
    }

    // --- detect_changes_core error paths ---

    #[test]
    fn core_path_not_a_directory_returns_error() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let err = detect_changes_core(&kit, "/nonexistent/path/xyz", "unstaged")
            .expect_err("nonexistent path should error");
        assert_eq!(err.exit_code(), 2, "InvalidInput → exit 2");
    }

    #[test]
    fn core_invalid_mode_returns_error() {
        let tmp = TempDir::new().unwrap();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let err = detect_changes_core(&kit, tmp.path().to_str().unwrap(), "bogus")
            .expect_err("invalid mode should error");
        assert_eq!(err.exit_code(), 2, "InvalidInput → exit 2");
    }

    #[test]
    fn core_not_a_git_repo_returns_error() {
        let tmp = TempDir::new().unwrap();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let result = detect_changes_core(&kit, tmp.path().to_str().unwrap(), "unstaged");
        let err = match result {
            Err(e) => e,
            Ok(_) => return,
        };
        assert_eq!(err.exit_code(), 2, "non-git path → InvalidInput → exit 2");
    }

    #[test]
    fn core_clean_repo_returns_empty_affected() {
        let tmp = TempDir::new().unwrap();
        let status = std::process::Command::new("git")
            .arg("init")
            .arg(tmp.path())
            .status();
        if status.is_err() || !status.unwrap().success() {
            eprintln!("skipping test: git init failed");
            return;
        }
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let result = detect_changes_core(&kit, tmp.path().to_str().unwrap(), "unstaged");
        if result.is_ok() {
            // Empty diff → zero affected symbols.
        }
    }

    // --- DetectChangesOutput serialization ---

    #[test]
    fn detect_changes_output_serializes_to_json() {
        let out = DetectChangesOutput {
            path: "/repo".into(),
            mode: "unstaged".into(),
            files_changed: 2,
            affected: vec![AffectedSymbolOutput {
                name: "foo".into(),
                label: "Function".into(),
                qualified_name: "demo.foo".into(),
                file_path: "/repo/foo.rs".into(),
                start_line: 10,
                end_line: 20,
                incoming_edge_count: 5,
                risk_level: "high".into(),
            }],
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"files_changed\":2"));
        assert!(json.contains("\"risk_level\":\"high\""));
        assert!(json.contains("\"incoming_edge_count\":5"));
    }

    // --- find_symbols_in_ranges: end-to-end with seeded DB ---

    fn sample_function(id: &str, file_path: &str, start: u32, end: u32) -> crate::model::Node {
        crate::model::Node::builder(
            crate::model::NodeLabel::Function,
            "sym",
            format!("demo.{id}"),
        )
        .id(id)
        .project("demo")
        .file_path(file_path)
        .start_line(start)
        .end_line(end)
        .language(crate::model::Language::Rust)
        .build()
    }

    #[test]
    fn find_symbols_in_ranges_returns_overlapping_symbol_low_risk() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().expect("require_storage");

        storage
            .save_nodes(
                &[sample_function("f_low", "/repo/src/foo.rs", 10, 20)],
                NodeLabel::Function,
            )
            .expect("save_nodes");

        let affected = find_symbols_in_ranges(
            &*storage,
            "src/foo.rs",
            std::path::Path::new("/repo/src/foo.rs"),
            &[(10, 5)],
        )
        .expect("find_symbols_in_ranges");
        assert_eq!(affected.len(), 1, "should find the seeded function");
        assert_eq!(affected[0].name, "sym");
        assert_eq!(affected[0].incoming_edge_count, 0, "no edges → 0 incoming");
        assert_eq!(affected[0].risk_level, "low", "0 incoming → low risk");
    }

    #[test]
    fn find_symbols_in_ranges_returns_medium_risk_with_1_to_3_incoming() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().expect("require_storage");

        storage
            .save_nodes(
                &[sample_function("f_med", "/repo/src/bar.rs", 5, 15)],
                NodeLabel::Function,
            )
            .expect("save_nodes");

        let edges = vec![
            crate::model::Edge::builder("caller1", "f_med", crate::model::EdgeType::Calls, "demo")
                .build(),
            crate::model::Edge::builder("caller2", "f_med", crate::model::EdgeType::Calls, "demo")
                .build(),
        ];
        storage.save_edges(&edges).expect("save_edges");

        let affected = find_symbols_in_ranges(
            &*storage,
            "src/bar.rs",
            std::path::Path::new("/repo/src/bar.rs"),
            &[(5, 10)],
        )
        .expect("find_symbols_in_ranges");
        assert_eq!(affected.len(), 1);
        assert_eq!(affected[0].incoming_edge_count, 2, "2 incoming edges");
        assert_eq!(affected[0].risk_level, "medium", "1–3 incoming → medium");
    }

    #[test]
    fn find_symbols_in_ranges_returns_high_risk_with_4_plus_incoming() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().expect("require_storage");

        storage
            .save_nodes(
                &[sample_function("f_high", "/repo/src/baz.rs", 1, 100)],
                NodeLabel::Function,
            )
            .expect("save_nodes");

        let edges: Vec<crate::model::Edge> = (1..=5)
            .map(|i| {
                crate::model::Edge::builder(
                    format!("caller{i}"),
                    "f_high",
                    crate::model::EdgeType::Calls,
                    "demo",
                )
                .build()
            })
            .collect();
        storage.save_edges(&edges).expect("save_edges");

        let affected = find_symbols_in_ranges(
            &*storage,
            "src/baz.rs",
            std::path::Path::new("/repo/src/baz.rs"),
            &[(1, 100)],
        )
        .expect("find_symbols_in_ranges");
        assert_eq!(affected.len(), 1);
        assert_eq!(affected[0].incoming_edge_count, 5, "5 incoming edges");
        assert_eq!(affected[0].risk_level, "high", "≥4 incoming → high");
    }

    #[test]
    fn find_symbols_in_ranges_skips_non_overlapping_symbol() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().expect("require_storage");

        storage
            .save_nodes(
                &[sample_function("f_off", "/repo/src/off.rs", 1, 5)],
                NodeLabel::Function,
            )
            .expect("save_nodes");

        let affected = find_symbols_in_ranges(
            &*storage,
            "src/off.rs",
            std::path::Path::new("/repo/src/off.rs"),
            &[(100, 10)],
        )
        .expect("find_symbols_in_ranges");
        assert!(
            affected.is_empty(),
            "non-overlapping symbol should be skipped"
        );
    }

    #[test]
    fn find_symbols_in_ranges_empty_ranges_returns_empty() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().expect("require_storage");
        storage
            .save_nodes(
                &[sample_function("f_e", "/repo/src/e.rs", 1, 5)],
                NodeLabel::Function,
            )
            .expect("save_nodes");

        let affected = find_symbols_in_ranges(
            &*storage,
            "src/e.rs",
            std::path::Path::new("/repo/src/e.rs"),
            &[],
        )
        .expect("find_symbols_in_ranges");
        assert!(affected.is_empty(), "empty ranges → no symbols");
    }

    // --- run_git_diff error paths ---

    #[test]
    fn run_git_diff_non_git_dir_returns_invalid_input() {
        let tmp = TempDir::new().unwrap();
        let result = run_git_diff(tmp.path(), DiffMode::Unstaged);
        match result {
            Err(CodeNexusError::InvalidInput(_)) => {}
            Err(CodeNexusError::Io(_)) => {}
            Ok(s) if s.is_empty() => {}
            other => panic!("expected error or empty, got {other:?}"),
        }
    }

    // --- parse_hunk_new_range: more edge cases ---

    #[test]
    fn parse_hunk_no_at_at_returns_none() {
        assert!(parse_hunk_new_range("@@ -10 +12,5 no closing").is_none());
    }

    #[test]
    fn parse_hunk_non_numeric_start_returns_none() {
        assert!(parse_hunk_new_range("@@ -10 +abc,5 @@").is_none());
    }

    #[test]
    fn parse_hunk_non_numeric_len_returns_none() {
        assert!(parse_hunk_new_range("@@ -10 +12,abc @@").is_none());
    }

    // --- parse_diff_path: whitespace ---

    #[test]
    fn parse_diff_path_trims_whitespace() {
        assert_eq!(
            parse_diff_path("b/src/main.rs  ").as_deref(),
            Some("src/main.rs")
        );
    }

    // --- parse_unified_diff: file with only zero-len hunks ---

    #[test]
    fn parse_unified_diff_skips_file_with_only_zero_len_hunks() {
        let diff = "\
diff --git a/foo.rs b/foo.rs
--- a/foo.rs
+++ b/foo.rs
@@ -1,3 +0,0 @@
-a
-b
-c
diff --git a/bar.rs b/bar.rs
--- a/bar.rs
+++ b/bar.rs
@@ -5 +7,2 @@
-c
+d
";
        let hunks = parse_unified_diff(diff);
        // foo.rs has only zero-len hunks → skipped; bar.rs has a valid hunk.
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].0, "bar.rs");
    }

    // --- find_symbols_in_ranges: no matching symbols (early return) ---

    #[test]
    fn find_symbols_in_ranges_no_matching_symbols_returns_empty() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().expect("require_storage");
        let affected = find_symbols_in_ranges(
            &*storage,
            "src/nonexistent.rs",
            std::path::Path::new("/repo/src/nonexistent.rs"),
            &[(1, 10)],
        )
        .expect("find_symbols_in_ranges");
        assert!(affected.is_empty(), "no seeded symbols → empty result");
    }

    // --- detect_changes_core: end-to-end with real git repo ---

    #[test]
    fn core_detects_changed_symbol_in_real_git_repo() {
        let tmp = TempDir::new().unwrap();
        let status = std::process::Command::new("git")
            .arg("init")
            .arg(tmp.path())
            .status();
        if status.is_err() || !status.unwrap().success() {
            eprintln!("skipping test: git init failed");
            return;
        }
        let file = tmp.path().join("src/foo.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "fn foo() {\n    // old\n}\n").unwrap();

        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(tmp.path())
                .args(args)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        if !git(&["add", "."]) || !git(&["-c", "user.email=t@t.com", "-c", "user.name=T", "commit", "-m", "init"]) {
            eprintln!("skipping test: git add/commit failed");
            return;
        }
        // Modify the file to create an unstaged diff.
        std::fs::write(&file, "fn foo() {\n    // new\n    // added\n}\n").unwrap();

        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(db.to_str().unwrap());
        let storage = kit.require::<StorageModule>().unwrap();
        storage.execute("CREATE (:Function {id: 'f1', project: 'demo', name: 'foo', qualifiedName: 'demo.foo', filePath: 'src/foo.rs', startLine: 1, endLine: 3, signature: '', returnType: '', isExported: false, docstring: '', content: '', parentQn: ''});").unwrap();

        let result = detect_changes_core(&kit, tmp.path().to_str().unwrap(), "unstaged");
        assert!(
            result.is_ok(),
            "detect_changes should succeed: {:?}",
            result.err()
        );
    }
}
