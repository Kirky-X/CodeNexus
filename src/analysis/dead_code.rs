// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Dead code detection (T005, v0.1.5).
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

use crate::storage::capability::Storage;
use crate::storage::error::Result as StorageResult;
use crate::storage::schema::escape_cypher_string;
use serde::Serialize;

/// Default glob patterns for functions that are NOT considered dead even with
/// zero incoming CALLS edges (test functions are always invoked by the test
/// runner, which is not modelled as a CALLS edge in the graph).
const DEFAULT_TEST_PATTERNS: &[&str] = &["test_*", "*_test", "*_spec"];

/// Reason string recorded on every [`DeadCodeEntry`].
const REASON_ZERO_INCOMING_CALLS: &str = "zero incoming CALLS edges";

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
    /// Always `REASON_ZERO_INCOMING_CALLS`.
    pub reason: String,
}

/// Detects dead code (zero-indegree CALLS functions) for a project.
pub struct DeadCodeDetector<'a> {
    storage: &'a dyn Storage,
}

impl<'a> DeadCodeDetector<'a> {
    /// Creates a new detector backed by the given storage capability.
    #[must_use]
    pub fn new(storage: &'a dyn Storage) -> Self {
        Self { storage }
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

        // (b) Load the set of callee ids (targets of CALLS edges).
        let callees = self.load_callees(project)?;

        // (c) Build a filePath -> language map from the File table.
        let file_languages = self.load_file_languages(project)?;

        // (d) Filter: zero-indegree + not an entry point + not a test function.
        let mut entries = Vec::new();
        for func in &functions {
            if callees.contains(&func.id) {
                continue;
            }
            if matches_any_pattern(&func.name, entry_patterns) {
                continue;
            }
            if matches_any_pattern(&func.name, DEFAULT_TEST_PATTERNS) {
                continue;
            }
            let language = file_languages
                .get(&func.file_path)
                .cloned()
                .unwrap_or_default();
            entries.push(DeadCodeEntry {
                name: func.name.clone(),
                qualified_name: func.qualified_name.clone(),
                file_path: func.file_path.clone(),
                start_line: func.start_line,
                language,
                reason: REASON_ZERO_INCOMING_CALLS.to_string(),
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

    /// Loads the set of callee node ids (targets of CALLS edges) for `project`.
    fn load_callees(&self, project: &str) -> StorageResult<std::collections::HashSet<String>> {
        let escaped = escape_cypher_string(project);
        let cypher = format!(
            "MATCH (e:CodeRelation) WHERE e.type = 'CALLS' AND e.project = '{escaped}' \
             RETURN e.target AS target;"
        );
        let rows = self.storage.query(&cypher)?;
        let mut set = std::collections::HashSet::with_capacity(rows.len());
        for row in rows {
            if let Some(target) = row.first().and_then(|v| v.as_str()) {
                set.insert(target.to_string());
            }
        }
        Ok(set)
    }

    /// Builds a `filePath -> language` map from the `File` table for `project`.
    fn load_file_languages(&self, project: &str) -> StorageResult<std::collections::HashMap<String, String>> {
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
    use crate::kit::{build_kit, KitBootstrapConfig, Kit, StorageKey};
    use tempfile::TempDir;

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("dead_code_testdb");
        std::mem::forget(dir);
        path
    }

    /// Builds a Kit backed by an on-disk database at `db`.
    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    /// Returns the `dyn Storage` capability from `kit`.
    fn storage(kit: &Kit) -> std::sync::Arc<dyn crate::storage::capability::Storage> {
        kit.require::<StorageKey>().expect("require_storage")
    }

    /// Creates a Function node via direct Cypher.
    fn create_function(kit: &Kit, id: &str, project: &str, name: &str, qn: &str, file: &str, line: u32) {
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

    /// Creates a Method node via direct Cypher.
    fn create_method(kit: &Kit, id: &str, project: &str, name: &str, qn: &str, file: &str, line: u32) {
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
    fn create_calls_edge(kit: &Kit, edge_id: &str, caller_id: &str, callee_id: &str, project: &str) {
        let storage = storage(kit);
        let cypher = format!(
            "CREATE (:CodeRelation {{id: '{}', source: '{}', target: '{}', type: 'CALLS', \
             confidence: 1.0, confidenceTier: 'High', reason: '', startLine: 1, project: '{}'}});",
            escape_cypher_string(edge_id),
            escape_cypher_string(caller_id),
            escape_cypher_string(callee_id),
            escape_cypher_string(project),
        );
        storage.execute(&cypher).expect("create calls edge");
    }

    /// Creates a File node (for language resolution).
    fn create_file(kit: &Kit, id: &str, project: &str, file_path: &str, language: &str) {
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
        create_function(&kit, "f_main", "demo", "main", "demo.main", "/src/main.rs", 1);
        create_file(&kit, "file1", "demo", "/src/lib.rs", "rust");
        create_file(&kit, "file2", "demo", "/src/main.rs", "rust");

        let storage = storage(&kit);
        let detector = DeadCodeDetector::new(&*storage);
        let result = detector.detect("demo", &["main"]).expect("detect");
        let names: Vec<&str> = result.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"foo"), "foo should be dead: {:?}", names);
        assert!(!names.contains(&"main"), "main should be excluded: {:?}", names);
    }

    #[test]
    fn detect_excludes_entry_points() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function(&kit, "f_main", "demo", "main", "demo.main", "/src/main.rs", 1);
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
        create_function(&kit, "f_test_foo", "demo", "test_foo", "demo.test_foo", "/src/lib.rs", 1);
        create_function(&kit, "f_foo_test", "demo", "foo_test", "demo.foo_test", "/src/lib.rs", 10);
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
        create_method(&kit, "m_1", "demo", "helper", "demo.Class.helper", "/src/lib.rs", 5);
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
        create_function(&kit, "f_main", "demo", "main", "demo.main", "/src/main.rs", 1);
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
        let entry = result.iter().find(|e| e.name == "foo").expect("foo should be dead");
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
        let entry = result.iter().find(|e| e.name == "foo").expect("foo should be dead");
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
}
