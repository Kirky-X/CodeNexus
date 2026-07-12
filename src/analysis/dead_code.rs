// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Dead code detection.
//!
//! Identifies `Function`/`Method` nodes with zero incoming `CALLS` edges that
//! are not entry points (e.g. `main`) or test functions (`test_*` / `*_test`).
//!
//! # Algorithm
//!
//! 1. Query all `Function`/`Method` nodes for the given project.
//! 2. Query all `CALLS` edges for the project and build a set of callee ids.
//! 3. A node is "dead" if its id is NOT in the callee set AND its name does
//!    not match any entry-point glob pattern AND its name does not match any
//!    default test-function pattern.
//! 4. The `language` field is resolved per-node by joining on the `File`
//!    table's `filePath` (polyglot projects are handled correctly).

use crate::model::EdgeType;
use crate::storage::capability::Storage;
use crate::storage::error::Result as StorageResult;
use crate::storage::schema::escape_cypher_string;
use serde::{Deserialize, Serialize};

/// Default glob patterns for functions that are NOT considered dead even with
/// zero incoming CALLS edges (test functions are always invoked by the test
/// runner, which is not modelled as a CALLS edge in the graph).
const DEFAULT_TEST_PATTERNS: &[&str] = &["test_*", "*_test", "*_spec"];

/// Default entry-point function names across common languages and platforms
/// (C/C++ main, C/C++ wmain, Python __main__, C# Main, Win32 WinMain, DLL
/// DLLMain).
const DEFAULT_ENTRY_PATTERNS: &[&str] =
    &["main", "Main", "__main__", "wmain", "WinMain", "DLLMain"];

/// Reason string recorded on every [`DeadCodeEntry`].
const REASON_ZERO_INCOMING_CALLS: &str = "zero incoming CALLS edges";

/// Configuration for dead-code detection.
///
/// Controls which edge types are consulted, whether exported/FFI functions are
/// excluded, and the default entry-point / test-function patterns.
#[derive(Debug, Clone)]
pub struct DeadCodeConfig {
    /// Glob patterns for function names that are always considered live
    /// (e.g. `"main"`, `"WinMain"`).
    pub entry_patterns: Vec<String>,
    /// Glob patterns for test-function names (e.g. `"test_*"`).
    pub test_patterns: Vec<String>,
    /// When `true`, `isExported=true` nodes are excluded from dead code.
    pub check_exported: bool,
    /// Reserved for future trait-object / dynamic-dispatch detection.
    pub check_dynamic_dispatch: bool,
    /// Reserved for future reflection / serde detection.
    pub check_reflection: bool,
    /// When `true`, signatures containing `extern "C"` / `#[no_mangle]` are
    /// treated as FFI entry points and excluded.
    pub check_ffi: bool,
    /// Edge types whose incoming edges mark a function as "used".
    pub edge_types: Vec<EdgeType>,
}

impl Default for DeadCodeConfig {
    fn default() -> Self {
        Self {
            entry_patterns: DEFAULT_ENTRY_PATTERNS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            test_patterns: DEFAULT_TEST_PATTERNS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            check_exported: true,
            check_dynamic_dispatch: false,
            check_reflection: false,
            check_ffi: true,
            edge_types: vec![
                EdgeType::Calls,
                EdgeType::FfiCalls,
                EdgeType::Implements,
                EdgeType::HandlesRoute,
                EdgeType::Usage,
                EdgeType::Tests,
                EdgeType::UsesType,
                EdgeType::HttpCalls,
                EdgeType::AsyncCalls,
            ],
        }
    }
}

/// Confidence level for a dead-code finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Confidence {
    /// All edge types have zero incoming edges.
    High,
    /// Non-CALLS edges exist but no CALLS incoming edge.
    Medium,
    /// Some edge types have incoming edges but coverage is incomplete.
    Low,
}

/// A single dead-code finding.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeadCodeEntry {
    /// Short function name (e.g. `parse_file`).
    pub name: String,
    /// Fully-qualified name (e.g. `demo.parse_file`).
    pub qualified_name: String,
    /// Source file path.
    pub file_path: String,
    /// 1-based start line.
    pub start_line: u32,
    /// Source language (resolved from the `File` node).
    pub language: String,
    /// Why this node is considered dead (e.g. `zero incoming CALLS edges`).
    pub reason: String,
    /// Confidence level of the finding.
    pub confidence: Confidence,
}

/// Detects dead code (zero-indegree CALLS functions) for a project.
pub struct DeadCodeDetector<'a> {
    storage: &'a dyn Storage,
    config: DeadCodeConfig,
}

impl<'a> DeadCodeDetector<'a> {
    /// Creates a new detector backed by the given storage capability, using
    /// the default [`DeadCodeConfig`].
    #[must_use]
    pub fn new(storage: &'a dyn Storage) -> Self {
        Self::with_config(storage, DeadCodeConfig::default())
    }

    /// Creates a new detector with the supplied [`DeadCodeConfig`].
    #[must_use]
    pub fn with_config(storage: &'a dyn Storage, config: DeadCodeConfig) -> Self {
        Self { storage, config }
    }

    /// Returns the dead-code entries for `project`.
    ///
    /// `entry_patterns` are glob patterns (using `*` as the only wildcard)
    /// for function names that should NOT be considered dead even with zero
    /// incoming CALLS edges (e.g. `"main"`, `"__main__"`). Test-function
    /// patterns (`test_*`, `*_test`, `*_spec`) are always excluded.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if any Cypher query fails.
    pub fn detect(
        &self,
        project: &str,
        entry_patterns: &[&str],
    ) -> StorageResult<Vec<DeadCodeEntry>> {
        // (a) Load all Function/Method nodes for the project.
        let functions = self.load_functions(project)?;

        // (b) Load the set of referenced ids (targets of any configured edge type).
        let referenced_ids = self.load_referenced_ids(project)?;

        // (c) Load all edge targets split by CALLS vs non-CALLS for confidence.
        let (calls_targets, non_calls_targets) = self.load_edge_targets_by_category(project)?;

        // (d) Build a filePath -> language map from the File table.
        let file_languages = self.load_file_languages(project)?;

        // (e) Filter: zero-indegree + not an entry point + not a test function.
        let config_entry_patterns: Vec<&str> = self
            .config
            .entry_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let config_test_patterns: Vec<&str> = self
            .config
            .test_patterns
            .iter()
            .map(|s| s.as_str())
            .collect();
        let mut entries = Vec::new();
        for func in &functions {
            if referenced_ids.contains(&func.id) {
                continue;
            }
            if matches_any_pattern(&func.name, entry_patterns)
                || matches_any_pattern(&func.name, &config_entry_patterns)
            {
                continue;
            }
            if matches_any_pattern(&func.name, DEFAULT_TEST_PATTERNS)
                || matches_any_pattern(&func.name, &config_test_patterns)
            {
                continue;
            }
            if self.config.check_exported && self.is_exported(&func.id)? {
                continue;
            }
            if self.config.check_ffi && self.is_ffi_entry(&func.id)? {
                continue;
            }
            let language = file_languages
                .get(&func.file_path)
                .cloned()
                .unwrap_or_default();
            let confidence = if calls_targets.contains(&func.id) {
                Confidence::Low
            } else if non_calls_targets.contains(&func.id) {
                Confidence::Medium
            } else {
                Confidence::High
            };
            entries.push(DeadCodeEntry {
                name: func.name.clone(),
                qualified_name: func.qualified_name.clone(),
                file_path: func.file_path.clone(),
                start_line: func.start_line,
                language,
                reason: REASON_ZERO_INCOMING_CALLS.to_string(),
                confidence,
            });
        }
        // Stable order by qualified name for deterministic output.
        entries.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        Ok(entries)
    }

    /// Loads all `Function` and `Method` nodes for `project`.
    ///
    /// LadybugDB's Cypher subset does not support `WHERE (n:Function OR
    /// n:Method)` label expressions, so we issue two separate queries (one
    /// per label) and merge the results in Rust.
    fn load_functions(&self, project: &str) -> StorageResult<Vec<FunctionRow>> {
        let escaped = escape_cypher_string(project);
        let function_cypher = format!(
            "MATCH (n:Function) WHERE n.project = '{escaped}' \
             RETURN n.id AS id, n.name AS name, n.qualifiedName AS qualified_name, \
             n.filePath AS file_path, n.startLine AS start_line;"
        );
        let method_cypher = format!(
            "MATCH (n:Method) WHERE n.project = '{escaped}' \
             RETURN n.id AS id, n.name AS name, n.qualifiedName AS qualified_name, \
             n.filePath AS file_path, n.startLine AS start_line;"
        );
        let mut out = Vec::new();
        for cypher in [function_cypher, method_cypher] {
            let rows = self.storage.query(&cypher)?;
            for row in rows {
                if row.len() < 5 {
                    continue;
                }
                let id = row[0].as_str().unwrap_or_default().to_string();
                let name = row[1].as_str().unwrap_or_default().to_string();
                let qualified_name = row[2].as_str().unwrap_or_default().to_string();
                let file_path = row[3].as_str().unwrap_or_default().to_string();
                let start_line = row[4]
                    .as_i64()
                    .map(|v| v as u32)
                    .or_else(|| row[4].as_u64().map(|v| v as u32))
                    .unwrap_or(0);
                out.push(FunctionRow {
                    id,
                    name,
                    qualified_name,
                    file_path,
                    start_line,
                });
            }
        }
        Ok(out)
    }

    /// Loads the set of node ids that are targets of any edge type listed in
    /// [`DeadCodeConfig::edge_types`] for `project`.
    ///
    /// A function is "used" if it appears as the `target` of at least one
    /// CodeRelation edge whose type is in the configured set (CALLS, USAGE,
    /// HANDLES_ROUTE, TESTS, etc.).
    fn load_referenced_ids(
        &self,
        project: &str,
    ) -> StorageResult<std::collections::HashSet<String>> {
        if self.config.edge_types.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        let escaped = escape_cypher_string(project);
        // EdgeType::as_db_type returns static UPPERCASE DDL strings from a
        // controlled enum, so they are safe to embed in Cypher without extra
        // escaping. Collapsing the former per-type loop into a single `IN`
        // query reduces 9 round-trips (default config) to 1.
        let in_list = self
            .config
            .edge_types
            .iter()
            .map(|t| format!("'{}'", t.as_db_type()))
            .collect::<Vec<_>>()
            .join(", ");
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type IN [{in_list}] AND e.project = '{escaped}' \
             RETURN e.target AS target;"
        );
        let mut set = std::collections::HashSet::new();
        let rows = self.storage.query(&cypher)?;
        for row in rows {
            if let Some(target) = row.first().and_then(|v| v.as_str()) {
                set.insert(target.to_string());
            }
        }
        Ok(set)
    }

    /// Loads all CodeRelation targets for `project`, split into two sets:
    /// - `calls_targets`: ids that are targets of `CALLS` edges.
    /// - `non_calls_targets`: ids that are targets of any non-`CALLS` edge.
    ///
    /// Used for confidence scoring: a dead function with only non-CALLS
    /// incoming edges gets `Medium` confidence; one with CALLS gets `Low`.
    fn load_edge_targets_by_category(
        &self,
        project: &str,
    ) -> StorageResult<(
        std::collections::HashSet<String>,
        std::collections::HashSet<String>,
    )> {
        let escaped = escape_cypher_string(project);
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.project = '{escaped}' \
             RETURN e.target AS target, e.type AS type;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut calls_targets = std::collections::HashSet::new();
        let mut non_calls_targets = std::collections::HashSet::new();
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let target = row[0].as_str().unwrap_or_default().to_string();
            let edge_type = row[1].as_str().unwrap_or_default();
            if edge_type == "CALLS" {
                calls_targets.insert(target);
            } else {
                non_calls_targets.insert(target);
            }
        }
        Ok((calls_targets, non_calls_targets))
    }

    /// Checks whether `func_id` has any incoming edge of `edge_type`.
    ///
    /// No project filter is applied — `func_id` is globally unique.
    ///
    /// Unlike [`load_referenced_ids`](Self::load_referenced_ids), which builds a
    /// project-wide set of referenced targets aggregated across every configured
    /// edge type, this method answers a single-func, single-edge-type question.
    /// Currently exercised by unit tests in `mod tests`; reserved for future
    /// single-edge-type diagnostics (e.g. "is this function tested?" via
    /// `EdgeType::Tests`) when `load_referenced_ids` is overkill.
    fn has_incoming_edge(&self, func_id: &str, edge_type: EdgeType) -> StorageResult<bool> {
        let escaped_id = escape_cypher_string(func_id);
        let type_str = edge_type.as_db_type();
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = '{type_str}' AND e.target = '{escaped_id}' \
             RETURN e.id AS id LIMIT 1;"
        );
        Ok(!self.storage.query(&cypher)?.is_empty())
    }

    /// Returns `true` if the node with `func_id` has `isExported = true`.
    ///
    /// Queries both `Function` and `Method` labels since LadybugDB does not
    /// support `OR` label expressions.
    fn is_exported(&self, func_id: &str) -> StorageResult<bool> {
        let escaped_id = escape_cypher_string(func_id);
        for label in ["Function", "Method"] {
            let cypher = format!(
                "MATCH (n:{label}) WHERE n.id = '{escaped_id}' \
                 RETURN n.isExported AS is_exported LIMIT 1;"
            );
            let rows = self.storage.query(&cypher)?;
            if let Some(row) = rows.into_iter().next() {
                if let Some(val) = row.first() {
                    return Ok(val.as_bool().unwrap_or(false));
                }
            }
        }
        Ok(false)
    }

    /// Returns `true` if the node's `signature` contains FFI markers
    /// (`extern "C"` or `#[no_mangle]`), marking it as an FFI entry point.
    fn is_ffi_entry(&self, func_id: &str) -> StorageResult<bool> {
        let escaped_id = escape_cypher_string(func_id);
        for label in ["Function", "Method"] {
            let cypher = format!(
                "MATCH (n:{label}) WHERE n.id = '{escaped_id}' \
                 RETURN n.signature AS signature LIMIT 1;"
            );
            let rows = self.storage.query(&cypher)?;
            if let Some(row) = rows.into_iter().next() {
                if let Some(sig) = row.first().and_then(|v| v.as_str()) {
                    if sig.contains(r#"extern "C""#) || sig.contains("#[no_mangle]") {
                        return Ok(true);
                    }
                    return Ok(false);
                }
            }
        }
        Ok(false)
    }

    /// Builds a `filePath -> language` map from the `File` table for `project`.
    fn load_file_languages(
        &self,
        project: &str,
    ) -> StorageResult<std::collections::HashMap<String, String>> {
        let escaped = escape_cypher_string(project);
        let cypher = format!(
            "MATCH (f:File) WHERE f.project = '{escaped}' \
             RETURN f.filePath AS file_path, f.language AS language;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut map = std::collections::HashMap::with_capacity(rows.len());
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let file_path = row[0].as_str().unwrap_or_default().to_string();
            let language = row[1].as_str().unwrap_or_default().to_string();
            map.insert(file_path, language);
        }
        Ok(map)
    }
}

/// Internal row representation for a Function/Method node.
struct FunctionRow {
    id: String,
    name: String,
    qualified_name: String,
    file_path: String,
    start_line: u32,
}

/// Returns `true` if `name` matches any of the glob `patterns`.
///
/// Supports `*` as the only wildcard (matches any sequence of characters,
/// including the empty sequence). All other characters match literally.
fn matches_any_pattern(name: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| glob_match(p, name))
}

/// Simple glob matcher where `*` matches any sequence of characters.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_helper(&p, &t)
}

fn glob_helper(p: &[char], t: &[char]) -> bool {
    match (p.first(), t.first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some('*'), None) => glob_helper(&p[1..], t),
        (Some('*'), Some(_)) => glob_helper(&p[1..], t) || glob_helper(p, &t[1..]),
        // Non-`*` pattern char with empty text: cannot match.
        (Some(_), None) => false,
        (Some(pc), Some(tc)) => *pc == *tc && glob_helper(&p[1..], &t[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("dead_code_testdb");
        std::mem::forget(dir);
        path
    }

    /// Builds a Kit backed by an on-disk database at `db`.
    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    /// Returns the `dyn Storage` capability from `kit`.
    fn storage(
        kit: &AsyncKit<AsyncReady>,
    ) -> std::sync::Arc<dyn crate::storage::capability::Storage> {
        kit.require::<StorageModule>().expect("require_storage")
    }

    /// Creates a Function node via direct Cypher.
    fn create_function(
        kit: &AsyncKit<AsyncReady>,
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        line: u32,
    ) {
        let storage = storage(kit);
        let end_line = line + 10;
        let cypher = format!(
            "CREATE (:Function {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '', returnType: '', \
             isExported: false, docstring: '', content: '', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(qn),
            escape_cypher_string(file),
            line,
            end_line,
        );
        storage.execute(&cypher).expect("create function");
    }

    /// Creates a Function node with `isExported = true` and optional `signature`.
    fn create_function_with_flags(
        kit: &AsyncKit<AsyncReady>,
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        line: u32,
        is_exported: bool,
        signature: &str,
    ) {
        let storage = storage(kit);
        let end_line = line + 10;
        let cypher = format!(
            "CREATE (:Function {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '{}', returnType: '', \
             isExported: {}, docstring: '', content: '', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(qn),
            escape_cypher_string(file),
            line,
            end_line,
            escape_cypher_string(signature),
            is_exported,
        );
        storage
            .execute(&cypher)
            .expect("create function with flags");
    }

    /// Creates a Method node via direct Cypher.
    fn create_method(
        kit: &AsyncKit<AsyncReady>,
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        line: u32,
    ) {
        let storage = storage(kit);
        let end_line = line + 10;
        let cypher = format!(
            "CREATE (:Method {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '', returnType: '', \
             isExported: false, docstring: '', content: '', parameterCount: 0, parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(qn),
            escape_cypher_string(file),
            line,
            end_line,
        );
        storage.execute(&cypher).expect("create method");
    }

    /// Creates a CALLS edge from `caller_id` to `callee_id`.
    fn create_calls_edge(
        kit: &AsyncKit<AsyncReady>,
        edge_id: &str,
        caller_id: &str,
        callee_id: &str,
        project: &str,
    ) {
        create_edge(kit, edge_id, caller_id, callee_id, project, "CALLS");
    }

    /// Creates a CodeRelation edge of `edge_type` (DDL string, e.g. `"USAGE"`)
    /// from `source_id` to `target_id`.
    fn create_edge(
        kit: &AsyncKit<AsyncReady>,
        edge_id: &str,
        source_id: &str,
        target_id: &str,
        project: &str,
        edge_type: &str,
    ) {
        let storage = storage(kit);
        let cypher = format!(
            "CREATE (:CodeRelation {{id: '{}', source: '{}', target: '{}', type: '{}', \
             confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: '{}'}});",
            escape_cypher_string(edge_id),
            escape_cypher_string(source_id),
            escape_cypher_string(target_id),
            escape_cypher_string(edge_type),
            escape_cypher_string(project),
        );
        storage.execute(&cypher).expect("create edge");
    }

    /// Creates a File node (for language resolution).
    fn create_file(
        kit: &AsyncKit<AsyncReady>,
        id: &str,
        project: &str,
        file_path: &str,
        language: &str,
    ) {
        let storage = storage(kit);
        let cypher = format!(
            "CREATE (:File {{id: '{}', project: '{}', name: '{}', filePath: '{}', \
             language: '{}', hash: '', lineCount: 0}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(file_path.split('/').next_back().unwrap_or("file")),
            escape_cypher_string(file_path),
            escape_cypher_string(language),
        );
        storage.execute(&cypher).expect("create file");
    }

    // --- glob_match unit tests ---

    #[test]
    fn glob_match_exact() {
        assert!(glob_match("main", "main"));
        assert!(!glob_match("main", "main2"));
    }

    #[test]
    fn glob_match_prefix_wildcard() {
        assert!(glob_match("test_*", "test_foo"));
        assert!(glob_match("test_*", "test_"));
        assert!(!glob_match("test_*", "foo_test"));
    }

    #[test]
    fn glob_match_suffix_wildcard() {
        assert!(glob_match("*_test", "foo_test"));
        assert!(glob_match("*_test", "_test"));
        assert!(!glob_match("*_test", "test_foo"));
    }

    #[test]
    fn glob_match_middle_wildcard() {
        assert!(glob_match("test_*_spec", "test_foo_spec"));
        assert!(!glob_match("test_*_spec", "test_foo"));
    }

    #[test]
    fn glob_match_star_only() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
    }

    // --- DeadCodeDetector tests ---

    #[test]
    fn detect_returns_empty_for_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        assert!(result.is_empty(), "empty DB should yield no dead code");
    }

    #[test]
    fn detect_finds_dead_function() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // `foo` has no incoming CALLS edges; `main` also has no incoming
        // edges but is excluded as an entry point.
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
        create_function(
            &kit,
            "f_main",
            "demo",
            "main",
            "demo.main",
            "/src/main.rs",
            1,
        );
        create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");
        create_file(&kit, "file2", "demo", "/src/main.rs", "rust");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"foo"), "foo should be dead: {:?}", names);
        assert!(
            !names.contains(&"main"),
            "main should be excluded: {:?}",
            names
        );
    }

    #[test]
    fn detect_excludes_entry_points() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_main",
            "demo",
            "main",
            "demo.main",
            "/src/main.rs",
            1,
        );
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
        create_file(&kit, "file1", "demo", "/src/main.rs", "rust");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&"main"), "main is an entry point");
        assert!(names.contains(&"foo"), "foo is not an entry point");
    }

    #[test]
    fn detect_excludes_test_functions() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // `test_foo` matches the default `test_*` pattern and is excluded
        // even though it has zero incoming CALLS edges.
        create_function(
            &kit,
            "f_test_foo",
            "demo",
            "test_foo",
            "demo.test_foo",
            "/src/lib.rs",
            1,
        );
        create_function(
            &kit,
            "f_foo_test",
            "demo",
            "foo_test",
            "demo.foo_test",
            "/src/lib.rs",
            10,
        );
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 20);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&"test_foo"), "test_foo matches test_*");
        assert!(!names.contains(&"foo_test"), "foo_test matches *_test");
        assert!(names.contains(&"foo"), "foo is dead");
    }

    #[test]
    fn detect_handles_method_nodes() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // A Method node with zero incoming CALLS edges is dead code.
        create_method(
            &kit,
            "m_1",
            "demo",
            "helper",
            "demo.Class.helper",
            "/src/lib.rs",
            5,
        );
        create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"helper"), "Method helper should be dead");
    }

    #[test]
    fn detect_excludes_functions_with_incoming_calls() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // `main` calls `foo`; `foo` has an incoming CALLS edge and is NOT dead.
        create_function(
            &kit,
            "f_main",
            "demo",
            "main",
            "demo.main",
            "/src/main.rs",
            1,
        );
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
        create_calls_edge(&kit, "e1", "f_main", "f_foo", "demo");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&"foo"), "foo is called by main, not dead");
        assert!(!names.contains(&"main"), "main is an entry point");
    }

    #[test]
    fn detect_resolves_language_from_file_table() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.py", 1);
        create_file(&kit, "file1", "demo", "/src/lib.py", "python");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let entry = result
            .iter()
            .find(|e| e.name == "foo")
            .expect("foo should be dead");
        assert_eq!(entry.language, "python");
    }

    #[test]
    fn detect_includes_reason_field() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let entry = result
            .iter()
            .find(|e| e.name == "foo")
            .expect("foo should be dead");
        assert_eq!(entry.reason, "zero incoming CALLS edges");
    }

    #[test]
    fn detect_all_dead_when_no_edges_and_no_entries() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/a.rs", 1);
        create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/b.rs", 1);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        // No entry patterns, no CALLS edges → everything is dead.
        let result = detector.detect("demo", &[]).expect("detect");
        assert_eq!(result.len(), 2, "both functions should be dead");
    }

    #[test]
    fn detect_filters_by_project() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/a.rs", 1);
        create_function(&kit, "f_b", "other", "b", "other.b", "/src/b.rs", 1);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"a"), "a is in demo project");
        assert!(!names.contains(&"b"), "b is in other project");
    }

    // --- T002: DeadCodeConfig / Confidence tests ---

    #[test]
    fn dead_code_config_default_values() {
        let cfg = DeadCodeConfig::default();
        // Entry patterns default to the 6 multi-language entry points.
        assert_eq!(
            cfg.entry_patterns,
            vec![
                "main".to_string(),
                "Main".to_string(),
                "__main__".to_string(),
                "wmain".to_string(),
                "WinMain".to_string(),
                "DLLMain".to_string(),
            ]
        );
        // Test patterns mirror DEFAULT_TEST_PATTERNS.
        assert_eq!(cfg.test_patterns.len(), 3);
        assert!(cfg.test_patterns.contains(&"test_*".to_string()));
        assert!(cfg.test_patterns.contains(&"*_test".to_string()));
        assert!(cfg.test_patterns.contains(&"*_spec".to_string()));
        // Exported / FFI checks are on by default.
        assert!(cfg.check_exported, "check_exported should default to true");
        assert!(cfg.check_ffi, "check_ffi should default to true");
        // Dynamic-dispatch / reflection checks are off (reserved).
        assert!(!cfg.check_dynamic_dispatch);
        assert!(!cfg.check_reflection);
        // Edge types must include all variants used for "used" detection
        // per R-dead_code-001.
        assert!(cfg.edge_types.contains(&EdgeType::Calls));
        assert!(cfg.edge_types.contains(&EdgeType::FfiCalls));
        assert!(cfg.edge_types.contains(&EdgeType::Implements));
        assert!(cfg.edge_types.contains(&EdgeType::HandlesRoute));
        assert!(cfg.edge_types.contains(&EdgeType::Usage));
        assert!(cfg.edge_types.contains(&EdgeType::Tests));
        assert!(cfg.edge_types.contains(&EdgeType::UsesType));
        assert!(cfg.edge_types.contains(&EdgeType::HttpCalls));
        assert!(cfg.edge_types.contains(&EdgeType::AsyncCalls));
    }

    #[test]
    fn confidence_serializes_high_medium_low() {
        // Variant name is the JSON representation (serde default).
        assert_eq!(
            serde_json::to_string(&Confidence::High).unwrap(),
            "\"High\""
        );
        assert_eq!(
            serde_json::to_string(&Confidence::Medium).unwrap(),
            "\"Medium\""
        );
        assert_eq!(serde_json::to_string(&Confidence::Low).unwrap(), "\"Low\"");
        // Roundtrip every variant.
        for c in [Confidence::High, Confidence::Medium, Confidence::Low] {
            let json = serde_json::to_string(&c).unwrap();
            let parsed: Confidence = serde_json::from_str(&json).unwrap();
            assert_eq!(c, parsed, "roundtrip failed for {json}");
        }
    }

    #[test]
    fn confidence_rejects_invalid_variant() {
        assert!(serde_json::from_str::<Confidence>("\"Critical\"").is_err());
        assert!(serde_json::from_str::<Confidence>("\"high\"").is_err());
    }

    #[test]
    fn detect_sets_confidence_high_for_zero_incoming() {
        // Until T007 refines scoring, zero-incoming entries are High.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let entry = result
            .iter()
            .find(|e| e.name == "foo")
            .expect("foo should be dead");
        assert_eq!(entry.confidence, Confidence::High);
    }

    #[test]
    fn with_config_accepts_custom_config() {
        // with_config must not panic and must produce a working detector.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/a.rs", 1);
        let storage = storage(&kit);
        let cfg = DeadCodeConfig {
            entry_patterns: vec![],
            test_patterns: vec![],
            check_exported: false,
            check_dynamic_dispatch: false,
            check_reflection: false,
            check_ffi: false,
            edge_types: vec![EdgeType::Calls],
        };
        let detector = DeadCodeDetector::with_config(&*storage, cfg);
        let result = detector.detect("demo", &[]).expect("detect");
        assert_eq!(result.len(), 1, "a should be dead with empty patterns");
    }

    // --- T003: multi-edge-type reference detection tests ---

    #[test]
    fn detect_usage_edge_prevents_dead_code() {
        // R-dead_code-001: a USAGE edge marks the target as used.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
        create_function(&kit, "f_bar", "demo", "bar", "demo.bar", "/src/lib.rs", 5);
        // bar uses foo → foo is not dead.
        create_edge(&kit, "e1", "f_bar", "f_foo", "demo", "USAGE");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&"foo"), "foo has USAGE incoming edge");
        assert!(names.contains(&"bar"), "bar has no incoming edges");
    }

    #[test]
    fn detect_handles_route_edge_prevents_dead_code() {
        // R-dead_code-001: a HANDLES_ROUTE edge marks the target as used.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_handler",
            "demo",
            "handler",
            "demo.handler",
            "/src/lib.rs",
            1,
        );
        create_function(&kit, "f_reg", "demo", "reg", "demo.reg", "/src/lib.rs", 5);
        // reg -> handler (HANDLES_ROUTE) → handler is not dead.
        create_edge(&kit, "e1", "f_reg", "f_handler", "demo", "HANDLES_ROUTE");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"handler"),
            "handler has HANDLES_ROUTE incoming edge"
        );
        assert!(names.contains(&"reg"), "reg has no incoming edges");
    }

    #[test]
    fn detect_tests_edge_prevents_dead_code() {
        // R-dead_code-001: a TESTS edge marks the target as used.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_target",
            "demo",
            "target",
            "demo.target",
            "/src/lib.rs",
            1,
        );
        create_function(
            &kit,
            "f_ttest",
            "demo",
            "ttest",
            "demo.ttest",
            "/src/lib.rs",
            5,
        );
        // ttest tests target → target is not dead.
        create_edge(&kit, "e1", "f_ttest", "f_target", "demo", "TESTS");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&"target"), "target has TESTS incoming edge");
        assert!(names.contains(&"ttest"), "ttest has no incoming edges");
    }

    #[test]
    fn detect_all_edge_types_exhaustive_no_incoming_is_dead() {
        // R-dead_code-001: a function with no incoming edges of ANY type is dead.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_lone",
            "demo",
            "lone",
            "demo.lone",
            "/src/lib.rs",
            1,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"lone"),
            "lone has zero incoming edges → dead"
        );
    }

    #[test]
    fn has_incoming_edge_returns_true_for_existing_edge() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);
        create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/lib.rs", 5);
        create_edge(&kit, "e1", "f_a", "f_b", "demo", "USAGE");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        assert!(
            detector
                .has_incoming_edge("f_b", EdgeType::Usage)
                .expect("has_incoming_edge"),
            "f_b should have USAGE incoming edge"
        );
    }

    #[test]
    fn has_incoming_edge_returns_false_for_missing_edge() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        assert!(
            !detector
                .has_incoming_edge("f_a", EdgeType::Calls)
                .expect("has_incoming_edge"),
            "f_a has no CALLS incoming edge"
        );
    }

    #[test]
    fn has_incoming_edge_distinguishes_edge_types() {
        // A function with a USAGE edge but no CALLS edge: USAGE=true, CALLS=false.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_a", "demo", "a", "demo.a", "/src/lib.rs", 1);
        create_function(&kit, "f_b", "demo", "b", "demo.b", "/src/lib.rs", 5);
        create_edge(&kit, "e1", "f_a", "f_b", "demo", "USAGE");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        assert!(
            detector
                .has_incoming_edge("f_b", EdgeType::Usage)
                .expect("has_incoming_edge"),
            "f_b has USAGE edge"
        );
        assert!(
            !detector
                .has_incoming_edge("f_b", EdgeType::Calls)
                .expect("has_incoming_edge"),
            "f_b has no CALLS edge"
        );
    }

    #[test]
    fn load_referenced_ids_collects_targets_across_multiple_edge_types() {
        // Verifies the single IN-clause query captures targets across all
        // configured edge types (CALLS, USAGE, TESTS) in one pass.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_tgt_calls",
            "demo",
            "tgt_calls",
            "demo.tgt_calls",
            "/src/lib.rs",
            1,
        );
        create_function(
            &kit,
            "f_tgt_usage",
            "demo",
            "tgt_usage",
            "demo.tgt_usage",
            "/src/lib.rs",
            5,
        );
        create_function(
            &kit,
            "f_tgt_tests",
            "demo",
            "tgt_tests",
            "demo.tgt_tests",
            "/src/lib.rs",
            10,
        );
        create_function(&kit, "f_src_a", "demo", "src_a", "demo.src_a", "/src/lib.rs", 20);
        create_function(&kit, "f_src_b", "demo", "src_b", "demo.src_b", "/src/lib.rs", 25);
        create_function(&kit, "f_src_c", "demo", "src_c", "demo.src_c", "/src/lib.rs", 30);
        create_edge(&kit, "e1", "f_src_a", "f_tgt_calls", "demo", "CALLS");
        create_edge(&kit, "e2", "f_src_b", "f_tgt_usage", "demo", "USAGE");
        create_edge(&kit, "e3", "f_src_c", "f_tgt_tests", "demo", "TESTS");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let referenced = detector
            .load_referenced_ids("demo")
            .expect("load_referenced_ids");
        assert!(
            referenced.contains("f_tgt_calls"),
            "CALLS target should be referenced"
        );
        assert!(
            referenced.contains("f_tgt_usage"),
            "USAGE target should be referenced"
        );
        assert!(
            referenced.contains("f_tgt_tests"),
            "TESTS target should be referenced"
        );
        assert_eq!(
            referenced.len(),
            3,
            "exactly 3 targets should be referenced"
        );
    }

    // --- T004: exported function detection tests ---

    #[test]
    fn detect_excludes_exported_functions() {
        // R-dead_code-002: isExported=true with no incoming edges → NOT dead.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_flags(
            &kit,
            "f_pub",
            "demo",
            "pub_fn",
            "demo.pub_fn",
            "/src/lib.rs",
            1,
            true,
            "",
        );
        create_function(
            &kit,
            "f_priv",
            "demo",
            "priv_fn",
            "demo.priv_fn",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"pub_fn"),
            "exported pub_fn should NOT be dead"
        );
        assert!(names.contains(&"priv_fn"), "private priv_fn should be dead");
    }

    #[test]
    fn detect_includes_exported_when_check_exported_false() {
        // When check_exported=false, exported functions ARE dead code.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_flags(
            &kit,
            "f_pub",
            "demo",
            "pub_fn",
            "demo.pub_fn",
            "/src/lib.rs",
            1,
            true,
            "",
        );

        let storage = storage(&kit);
        let cfg = DeadCodeConfig {
            check_exported: false,
            ..DeadCodeConfig::default()
        };
        let detector = DeadCodeDetector::with_config(&*storage, cfg);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"pub_fn"),
            "with check_exported=false, pub_fn IS dead"
        );
    }

    #[test]
    fn is_exported_returns_correct_value() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_flags(
            &kit,
            "f_pub",
            "demo",
            "pub_fn",
            "demo.pub_fn",
            "/src/lib.rs",
            1,
            true,
            "",
        );
        create_function(
            &kit,
            "f_priv",
            "demo",
            "priv_fn",
            "demo.priv_fn",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        assert!(
            detector.is_exported("f_pub").expect("is_exported"),
            "f_pub should be exported"
        );
        assert!(
            !detector.is_exported("f_priv").expect("is_exported"),
            "f_priv should NOT be exported"
        );
    }

    // --- T005: FFI entry point detection tests ---

    #[test]
    fn detect_excludes_ffi_entry_extern_c() {
        // R-dead_code-003: signature with `extern "C"` → NOT dead.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_flags(
            &kit,
            "f_ffi",
            "demo",
            "ffi_fn",
            "demo.ffi_fn",
            "/src/lib.rs",
            1,
            false,
            r#"pub extern "C" fn ffi_fn(x: i32) -> i32"#,
        );
        create_function(
            &kit,
            "f_plain",
            "demo",
            "plain",
            "demo.plain",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"ffi_fn"),
            "ffi_fn is an FFI entry → not dead"
        );
        assert!(names.contains(&"plain"), "plain has no FFI markers → dead");
    }

    #[test]
    fn detect_excludes_ffi_entry_no_mangle() {
        // R-dead_code-003: signature with `#[no_mangle]` → NOT dead.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_flags(
            &kit,
            "f_nm",
            "demo",
            "native_fn",
            "demo.native_fn",
            "/src/lib.rs",
            1,
            false,
            "#[no_mangle]\npub fn native_fn() -> u32",
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"native_fn"),
            "native_fn has #[no_mangle] → not dead"
        );
    }

    #[test]
    fn detect_includes_ffi_when_check_ffi_false() {
        // When check_ffi=false, FFI functions ARE dead code.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_flags(
            &kit,
            "f_ffi",
            "demo",
            "ffi_fn",
            "demo.ffi_fn",
            "/src/lib.rs",
            1,
            false,
            r#"extern "C" fn ffi_fn()"#,
        );

        let storage = storage(&kit);
        let cfg = DeadCodeConfig {
            check_ffi: false,
            ..DeadCodeConfig::default()
        };
        let detector = DeadCodeDetector::with_config(&*storage, cfg);
        let result = detector.detect("demo", &[]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"ffi_fn"),
            "with check_ffi=false, ffi_fn IS dead"
        );
    }

    #[test]
    fn is_ffi_entry_distinguishes_ffi_from_plain() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_flags(
            &kit,
            "f_ffi",
            "demo",
            "ffi_fn",
            "demo.ffi_fn",
            "/src/lib.rs",
            1,
            false,
            r#"extern "C" fn ffi_fn()"#,
        );
        create_function(
            &kit,
            "f_plain",
            "demo",
            "plain",
            "demo.plain",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        assert!(
            detector.is_ffi_entry("f_ffi").expect("is_ffi_entry"),
            "f_ffi should be FFI entry"
        );
        assert!(
            !detector.is_ffi_entry("f_plain").expect("is_ffi_entry"),
            "f_plain should NOT be FFI entry"
        );
    }

    // --- T006: expanded entry point pattern tests ---

    #[test]
    fn detect_excludes_all_default_entry_patterns() {
        // R-dead_code-004: all 6 default entry patterns must be excluded.
        for entry_name in ["main", "Main", "__main__", "wmain", "WinMain", "DLLMain"] {
            let db = fresh_db_path();
            let kit = build_kit_for_db(&db);
            create_function(
                &kit,
                "f_entry",
                "demo",
                entry_name,
                &format!("demo.{entry_name}"),
                "/src/lib.rs",
                1,
            );
            // Also create a control function that IS dead.
            create_function(
                &kit,
                "f_dead",
                "demo",
                "dead_fn",
                "demo.dead_fn",
                "/src/lib.rs",
                5,
            );

            let storage = storage(&kit);
            let detector = DeadCodeDetector::new(&*storage);
            // Pass empty entry_patterns — config defaults should still apply.
            let result = detector.detect("demo", &[]).expect("detect");
            let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
            assert!(
                !names.contains(&entry_name),
                "{entry_name} should be excluded by default config patterns"
            );
            assert!(names.contains(&"dead_fn"), "dead_fn should still be dead");
        }
    }

    #[test]
    fn detect_excludes_custom_entry_patterns_parameter() {
        // R-dead_code-004: custom entry_patterns parameter still works.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(
            &kit,
            "f_h",
            "demo",
            "handler",
            "demo.handler",
            "/src/lib.rs",
            1,
        );
        create_function(
            &kit,
            "f_d",
            "demo",
            "dead_fn",
            "demo.dead_fn",
            "/src/lib.rs",
            5,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["handler"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"handler"),
            "handler matches custom pattern"
        );
        assert!(names.contains(&"dead_fn"), "dead_fn is still dead");
    }

    #[test]
    fn detect_merges_parameter_and_config_entry_patterns() {
        // Both the parameter and config patterns are checked.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // "main" is in config defaults; "custom_entry" is passed via parameter.
        create_function(&kit, "f_m", "demo", "main", "demo.main", "/src/lib.rs", 1);
        create_function(
            &kit,
            "f_c",
            "demo",
            "custom_entry",
            "demo.custom_entry",
            "/src/lib.rs",
            5,
        );
        create_function(
            &kit,
            "f_d",
            "demo",
            "dead_fn",
            "demo.dead_fn",
            "/src/lib.rs",
            10,
        );

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["custom_entry"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&"main"), "main excluded by config");
        assert!(
            !names.contains(&"custom_entry"),
            "custom_entry excluded by parameter"
        );
        assert!(names.contains(&"dead_fn"), "dead_fn is dead");
    }

    // --- T007: confidence scoring tests ---

    #[test]
    fn detect_confidence_high_for_zero_incoming_edges() {
        // R-dead_code-005: no incoming edges of ANY type → High.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let entry = result
            .iter()
            .find(|e| e.name == "foo")
            .expect("foo should be dead");
        assert_eq!(
            entry.confidence,
            Confidence::High,
            "zero incoming edges → High"
        );
    }

    #[test]
    fn detect_confidence_medium_for_non_calls_edge_only() {
        // R-dead_code-005: has USAGE but no CALLS → Medium.
        // Config with edge_types=[Calls] only: USAGE doesn't count as "used".
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
        create_function(&kit, "f_bar", "demo", "bar", "demo.bar", "/src/lib.rs", 5);
        // bar uses foo → foo has a USAGE incoming edge.
        create_edge(&kit, "e1", "f_bar", "f_foo", "demo", "USAGE");

        let storage = storage(&kit);
        let cfg = DeadCodeConfig {
            edge_types: vec![EdgeType::Calls],
            ..DeadCodeConfig::default()
        };
        let detector = DeadCodeDetector::with_config(&*storage, cfg);
        let result = detector.detect("demo", &[]).expect("detect");
        // foo is dead because USAGE is not in config.edge_types.
        let foo_entry = result
            .iter()
            .find(|e| e.name == "foo")
            .expect("foo should be dead (USAGE not in config.edge_types)");
        assert_eq!(
            foo_entry.confidence,
            Confidence::Medium,
            "USAGE but no CALLS → Medium"
        );
    }

    #[test]
    fn detect_confidence_low_for_calls_edge_with_empty_config() {
        // R-dead_code-005: has CALLS but config doesn't check CALLS → Low.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);
        create_function(&kit, "f_bar", "demo", "bar", "demo.bar", "/src/lib.rs", 5);
        // bar calls foo → foo has a CALLS incoming edge.
        create_calls_edge(&kit, "e1", "f_bar", "f_foo", "demo");

        let storage = storage(&kit);
        let cfg = DeadCodeConfig {
            edge_types: vec![], // empty: nothing counts as "used"
            ..DeadCodeConfig::default()
        };
        let detector = DeadCodeDetector::with_config(&*storage, cfg);
        let result = detector.detect("demo", &[]).expect("detect");
        let foo_entry = result
            .iter()
            .find(|e| e.name == "foo")
            .expect("foo should be dead (empty edge_types)");
        assert_eq!(
            foo_entry.confidence,
            Confidence::Low,
            "has CALLS incoming edge → Low"
        );
    }

    #[test]
    fn detect_confidence_serializes_in_dead_code_entry() {
        // Confidence field must appear in serialized JSON.
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_foo", "demo", "foo", "demo.foo", "/src/lib.rs", 1);

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &[]).expect("detect");
        let entry = result
            .iter()
            .find(|e| e.name == "foo")
            .expect("foo should be dead");
        let json = serde_json::to_string(entry).expect("serialize");
        assert!(
            json.contains("\"confidence\":\"High\""),
            "JSON should contain confidence field: {json}"
        );
    }
}
