// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! AST complexity analysis (T001-T004, v0.2.1).
//!
//! Provides per-function complexity metrics (cyclomatic, cognitive, nesting
//! depth, function length) with industry-standard severity classification
//! (Green / Yellow / Red).

use serde::Serialize;
use tree_sitter::Node;

use crate::model::Language;
use crate::parse::parser_factory::ParserFactory;
use crate::storage::capability::Storage;
use crate::storage::error::Result as StorageResult;
use crate::storage::schema::escape_cypher_string;

/// Complexity severity level for a single metric.
///
/// Variant order matters: `Green < Yellow < Red` via derived `Ord`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Severity {
    Green,
    Yellow,
    Red,
}

/// Industry-standard complexity thresholds stored as `(yellow_max, red_max)`
/// tuples. `from_*` methods classify values against these thresholds:
/// `value <= green_max → Green`, `value <= yellow_max → Yellow`, else `Red`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComplexityThresholds {
    /// Cyclomatic complexity thresholds `(yellow, red)` — default `(20, 25)`.
    pub cyclomatic: (u32, u32),
    /// Cognitive complexity thresholds `(yellow, red)` — default `(15, 20)`.
    pub cognitive: (u32, u32),
    /// Nesting depth thresholds `(yellow, red)` — default `(5, 6)`.
    pub nesting: (u32, u32),
    /// Function length thresholds `(yellow, red)` — default `(100, 200)`.
    pub func_length: (u32, u32),
}

impl Default for ComplexityThresholds {
    fn default() -> Self {
        Self {
            cyclomatic: (20, 25),
            cognitive: (15, 20),
            nesting: (5, 6),
            func_length: (100, 200),
        }
    }
}

impl Severity {
    /// Classifies cyclomatic complexity against `thresholds.cyclomatic`.
    ///
    /// `green_max = yellow_max / 2` (at least 1) per design D1, keeping the
    /// historical `≤10` Green / `≤20` Yellow default behavior.
    pub fn from_cyclomatic(value: u32, thresholds: &ComplexityThresholds) -> Severity {
        let yellow_max = thresholds.cyclomatic.0;
        let green_max = (yellow_max / 2).max(1);
        if value <= green_max {
            Severity::Green
        } else if value <= yellow_max {
            Severity::Yellow
        } else {
            Severity::Red
        }
    }

    /// Classifies cognitive complexity against `thresholds.cognitive`.
    pub fn from_cognitive(value: u32, thresholds: &ComplexityThresholds) -> Severity {
        let yellow_max = thresholds.cognitive.0;
        let green_max = (yellow_max / 2).max(1);
        if value <= green_max {
            Severity::Green
        } else if value <= yellow_max {
            Severity::Yellow
        } else {
            Severity::Red
        }
    }

    /// Classifies nesting depth against `thresholds.nesting`.
    pub fn from_nesting(value: u32, thresholds: &ComplexityThresholds) -> Severity {
        let yellow_max = thresholds.nesting.0;
        let green_max = (yellow_max / 2).max(1);
        if value <= green_max {
            Severity::Green
        } else if value <= yellow_max {
            Severity::Yellow
        } else {
            Severity::Red
        }
    }

    /// Classifies function length against `thresholds.func_length`.
    pub fn from_func_length(value: u32, thresholds: &ComplexityThresholds) -> Severity {
        let yellow_max = thresholds.func_length.0;
        let green_max = (yellow_max / 2).max(1);
        if value <= green_max {
            Severity::Green
        } else if value <= yellow_max {
            Severity::Yellow
        } else {
            Severity::Red
        }
    }
}

/// A single function's complexity metrics with overall severity.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ComplexityEntry {
    /// Short function name (e.g. `parse_file`).
    pub name: String,
    /// Fully-qualified name (e.g. `demo.parse_file`).
    pub qualified_name: String,
    /// Source file path.
    pub file_path: String,
    /// 1-based start line.
    pub start_line: u32,
    /// 1-based end line.
    pub end_line: u32,
    /// Source language.
    pub language: String,
    /// Cyclomatic complexity.
    pub cyclomatic: u32,
    /// Cognitive complexity.
    pub cognitive: u32,
    /// Maximum nesting depth.
    pub nesting_depth: u32,
    /// Function length in lines.
    pub function_length: u32,
    /// Highest severity across all four metrics.
    pub overall_severity: Severity,
}

impl ComplexityEntry {
    /// Computes the overall severity as the highest individual metric severity
    /// (`Red > Yellow > Green`) by calling the `from_*` classifiers with
    /// `thresholds`.
    pub fn compute_overall_severity(&self, thresholds: &ComplexityThresholds) -> Severity {
        [
            Severity::from_cyclomatic(self.cyclomatic, thresholds),
            Severity::from_cognitive(self.cognitive, thresholds),
            Severity::from_nesting(self.nesting_depth, thresholds),
            Severity::from_func_length(self.function_length, thresholds),
        ]
        .into_iter()
        .max()
        .unwrap_or(Severity::Green)
    }
}

/// Returns true if the given tree-sitter node type is a branch/decision node
/// for the specified language.
pub fn is_branch_node(language: Language, node_type: &str) -> bool {
    #[allow(unreachable_patterns)]
    match language {
        #[cfg(feature = "lang-rust")]
        Language::Rust => matches!(
            node_type,
            "if_expression"
                | "match_expression"
                | "for_expression"
                | "while_expression"
                | "loop_expression"
        ),
        #[cfg(feature = "lang-c")]
        Language::C => matches!(
            node_type,
            "if_statement" | "for_statement" | "while_statement" | "switch_statement"
        ),
        #[cfg(feature = "lang-cpp")]
        Language::Cpp => matches!(
            node_type,
            "if_statement" | "for_statement" | "while_statement" | "switch_statement"
        ),
        #[cfg(feature = "lang-python")]
        Language::Python => matches!(
            node_type,
            "if_statement" | "for_statement" | "while_statement" | "try_statement"
        ),
        #[cfg(feature = "lang-typescript")]
        Language::TypeScript => matches!(
            node_type,
            "if_statement" | "for_statement" | "while_statement" | "switch_case"
        ),
        #[cfg(feature = "lang-go")]
        Language::Go => matches!(node_type, "if_statement" | "for_statement" | "switch"),
        #[cfg(feature = "lang-java")]
        Language::Java => matches!(
            node_type,
            "if_statement" | "for_statement" | "while_statement" | "switch_expression"
        ),
        #[cfg(feature = "lang-fortran")]
        Language::Fortran => matches!(node_type, "if_statement" | "do_statement"),
        _ => false,
    }
}

/// Computes cyclomatic complexity (McCabe) for the given parse tree.
///
/// Starts at CC=1 (entry point) and adds 1 for each branch node, each `&&`/`||`
/// operator in binary expressions, and each `match_arm` beyond the first.
pub fn calc_cyclomatic(tree: &tree_sitter::Tree, language: Language) -> u32 {
    1 + cyclomatic_count(tree.root_node(), language)
}

fn cyclomatic_count(node: Node<'_>, language: Language) -> u32 {
    let mut count = 0;
    let kind = node.kind();

    if is_branch_node(language, kind) {
        count += 1;
    }

    let is_binary = kind == "binary_expression";
    let is_match = kind == "match_expression";
    let mut arm_count = 0u32;

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if is_binary && (child.kind() == "&&" || child.kind() == "||") {
            count += 1;
        }
        if is_match && child.kind() == "match_arm" {
            arm_count += 1;
        }
        count += cyclomatic_count(child, language);
    }

    if is_match {
        count += arm_count.saturating_sub(1);
    }

    count
}

/// Computes cognitive complexity for the given parse tree.
///
/// Increments by `(1 + nesting_level)` for each branch node and each `&&`/`||`
/// operator. Nesting level increases when descending into a branch node's body.
pub fn calc_cognitive(tree: &tree_sitter::Tree, language: Language) -> u32 {
    cognitive_count(tree.root_node(), language, 0)
}

fn cognitive_count(node: Node<'_>, language: Language, nesting: u32) -> u32 {
    let mut count = 0;
    let kind = node.kind();
    let is_branch = is_branch_node(language, kind);
    let is_binary = kind == "binary_expression";

    if is_branch {
        count += 1 + nesting;
    }

    let child_nesting = if is_branch { nesting + 1 } else { nesting };
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if is_binary && (child.kind() == "&&" || child.kind() == "||") {
            count += 1 + nesting;
        }
        count += cognitive_count(child, language, child_nesting);
    }

    count
}

/// Computes the maximum nesting depth of branch nodes in the parse tree.
///
/// Returns the deepest level of nested branch nodes (e.g. an `if` inside an
/// `if` inside an `if` has depth 3).
pub fn calc_nesting_depth(tree: &tree_sitter::Tree, language: Language) -> u32 {
    nesting_depth_count(tree.root_node(), language, 0)
}

fn nesting_depth_count(node: Node<'_>, language: Language, current_depth: u32) -> u32 {
    let is_branch = is_branch_node(language, node.kind());
    let this_depth = if is_branch { current_depth + 1 } else { current_depth };
    let mut max_depth = if is_branch { this_depth } else { 0 };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        max_depth = max_depth.max(nesting_depth_count(child, language, this_depth));
    }

    max_depth
}

/// Analyzes per-function complexity metrics over a Storage graph.
///
/// Loads `Function`/`Method` nodes for a project, parses each node's
/// `content` with tree-sitter, and computes cyclomatic/cognitive/nesting/
/// length metrics via the functions in this module.
pub struct ComplexityAnalyzer<'a> {
    storage: &'a dyn Storage,
    thresholds: ComplexityThresholds,
}

impl<'a> ComplexityAnalyzer<'a> {
    /// Creates a new analyzer backed by the given storage capability, using
    /// the default [`ComplexityThresholds`].
    #[must_use]
    pub fn new(storage: &'a dyn Storage) -> Self {
        Self {
            storage,
            thresholds: ComplexityThresholds::default(),
        }
    }

    /// Returns complexity entries for every `Function`/`Method` node in
    /// `project` whose `content` is non-empty and whose language is supported.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`](crate::storage::error::StorageError) if any
    /// Cypher query fails.
    pub fn analyze(&self, project: &str) -> StorageResult<Vec<ComplexityEntry>> {
        let escaped = escape_cypher_string(project);
        // LadybugDB's Cypher subset does not support `WHERE (n:Function OR
        // n:Method)` label expressions, so we issue two separate queries and
        // merge the results in Rust (same pattern as DeadCodeDetector).
        let function_cypher = format!(
            "MATCH (n:Function) WHERE n.project = '{escaped}' \
             RETURN n.name AS name, n.qualifiedName AS qualified_name, \
             n.filePath AS file_path, n.startLine AS start_line, \
             n.endLine AS end_line, n.content AS content;"
        );
        let method_cypher = format!(
            "MATCH (n:Method) WHERE n.project = '{escaped}' \
             RETURN n.name AS name, n.qualifiedName AS qualified_name, \
             n.filePath AS file_path, n.startLine AS start_line, \
             n.endLine AS end_line, n.content AS content;"
        );

        let mut entries = Vec::new();
        for cypher in [function_cypher, method_cypher] {
            let rows = self.storage.query(&cypher)?;
            for row in rows {
                if row.len() < 6 {
                    continue;
                }
                let name = row[0].as_str().unwrap_or_default().to_string();
                let qualified_name = row[1].as_str().unwrap_or_default().to_string();
                let file_path = row[2].as_str().unwrap_or_default().to_string();
                let start_line = row[3]
                    .as_i64()
                    .map(|v| v as u32)
                    .or_else(|| row[3].as_u64().map(|v| v as u32))
                    .unwrap_or(0);
                let end_line = row[4]
                    .as_i64()
                    .map(|v| v as u32)
                    .or_else(|| row[4].as_u64().map(|v| v as u32))
                    .unwrap_or(0);
                let content = row[5].as_str().unwrap_or_default().to_string();

                // Skip nodes with empty content (nothing to parse).
                if content.is_empty() {
                    eprintln!(
                        "warning: skipping {qualified_name} ({file_path}): empty content"
                    );
                    continue;
                }

                // Resolve language from the file path extension.
                let language = match detect_language(&file_path) {
                    Some(lang) => lang,
                    None => {
                        eprintln!(
                            "warning: skipping {qualified_name} ({file_path}): unknown language"
                        );
                        continue;
                    }
                };

                // Create parser; skip if the language grammar is not enabled.
                let mut parser = match ParserFactory::create_parser(language) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!(
                            "warning: skipping {qualified_name} ({file_path}): parser unavailable: {e}"
                        );
                        continue;
                    }
                };

                // Parse content; skip if parsing yields no tree.
                let tree = match parser.parse(&content, None) {
                    Some(t) => t,
                    None => {
                        eprintln!(
                            "warning: skipping {qualified_name} ({file_path}): parse failed"
                        );
                        continue;
                    }
                };

                let cyclomatic = calc_cyclomatic(&tree, language);
                let cognitive = calc_cognitive(&tree, language);
                let nesting_depth = calc_nesting_depth(&tree, language);
                let function_length = end_line.saturating_sub(start_line) + 1;

                let mut entry = ComplexityEntry {
                    name,
                    qualified_name,
                    file_path,
                    start_line,
                    end_line,
                    language: language.to_string(),
                    cyclomatic,
                    cognitive,
                    nesting_depth,
                    function_length,
                    overall_severity: Severity::Green,
                };
                entry.overall_severity = entry.compute_overall_severity(&self.thresholds);
                entries.push(entry);
            }
        }

        // Stable order by qualified name for deterministic output.
        entries.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        Ok(entries)
    }
}

/// Detects the source language from a file path's extension.
///
/// Extracts the extension (lowercased) and delegates to
/// [`Language::from_extension`]. Returns `None` for unknown extensions or
/// paths without an extension.
#[must_use]
pub fn detect_language(file_path: &str) -> Option<Language> {
    std::path::Path::new(file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(Language::from_extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::kit::{build_kit, Kit, KitBootstrapConfig, StorageKey};
    use crate::parse::parser_factory::ParserFactory;
    use tempfile::TempDir;

    // --- T005: is_branch_node tests ---

    #[test]
    fn from_cyclomatic_classification() {
        let t = ComplexityThresholds::default();
        assert_eq!(Severity::from_cyclomatic(5, &t), Severity::Green);
        assert_eq!(Severity::from_cyclomatic(15, &t), Severity::Yellow);
        assert_eq!(Severity::from_cyclomatic(30, &t), Severity::Red);
    }

    #[test]
    fn from_cognitive_classification() {
        let t = ComplexityThresholds::default();
        assert_eq!(Severity::from_cognitive(5, &t), Severity::Green);
        assert_eq!(Severity::from_cognitive(12, &t), Severity::Yellow);
        assert_eq!(Severity::from_cognitive(25, &t), Severity::Red);
    }

    #[test]
    fn from_nesting_classification() {
        let t = ComplexityThresholds::default();
        assert_eq!(Severity::from_nesting(2, &t), Severity::Green);
        assert_eq!(Severity::from_nesting(4, &t), Severity::Yellow);
        assert_eq!(Severity::from_nesting(7, &t), Severity::Red);
    }

    #[test]
    fn from_func_length_classification() {
        let t = ComplexityThresholds::default();
        assert_eq!(Severity::from_func_length(20, &t), Severity::Green);
        // green_max = 100/2 = 50, so 50 is Green; 75 falls in Yellow range.
        assert_eq!(Severity::from_func_length(50, &t), Severity::Green);
        assert_eq!(Severity::from_func_length(75, &t), Severity::Yellow);
        assert_eq!(Severity::from_func_length(150, &t), Severity::Red);
    }

    #[test]
    fn severity_uses_custom_thresholds() {
        // Custom thresholds: cyclomatic (yellow=10, red=12). green_max = 10/2 = 5.
        let mut custom = ComplexityThresholds::default();
        custom.cyclomatic = (10, 12);
        // value 15 > yellow_max(10) → Red. Old hardcoded impl returned Yellow.
        assert_eq!(Severity::from_cyclomatic(15, &custom), Severity::Red);
        // value 5 <= green_max(5) → Green.
        assert_eq!(Severity::from_cyclomatic(5, &custom), Severity::Green);
        // value 8: 8 > 5, 8 <= 10 → Yellow.
        assert_eq!(Severity::from_cyclomatic(8, &custom), Severity::Yellow);
    }

    #[test]
    fn thresholds_default_industry_values() {
        let t = ComplexityThresholds::default();
        assert_eq!(t.cyclomatic, (20, 25));
        assert_eq!(t.cognitive, (15, 20));
        assert_eq!(t.nesting, (5, 6));
        assert_eq!(t.func_length, (100, 200));
    }

    /// Builds a `ComplexityEntry` with the given metric values and placeholder
    /// metadata. `overall_severity` is set to `Green` and should be recomputed
    /// via `compute_overall_severity` in the test.
    fn make_entry(cyclomatic: u32, cognitive: u32, nesting: u32, length: u32) -> ComplexityEntry {
        ComplexityEntry {
            name: "f".to_string(),
            qualified_name: "demo.f".to_string(),
            file_path: "/src/lib.rs".to_string(),
            start_line: 1,
            end_line: 1 + length,
            language: "rust".to_string(),
            cyclomatic,
            cognitive,
            nesting_depth: nesting,
            function_length: length,
            overall_severity: Severity::Green,
        }
    }

    #[test]
    fn compute_overall_severity_red_when_any_red() {
        let mut entry = make_entry(30, 5, 2, 20);
        let thresholds = ComplexityThresholds::default();
        entry.overall_severity = entry.compute_overall_severity(&thresholds);
        assert_eq!(entry.overall_severity, Severity::Red);
    }

    #[test]
    fn compute_overall_severity_green_when_all_green() {
        let mut entry = make_entry(5, 5, 2, 20);
        let thresholds = ComplexityThresholds::default();
        entry.overall_severity = entry.compute_overall_severity(&thresholds);
        assert_eq!(entry.overall_severity, Severity::Green);
    }

    #[test]
    fn compute_overall_severity_yellow_when_any_yellow() {
        let mut entry = make_entry(15, 5, 2, 20);
        let thresholds = ComplexityThresholds::default();
        entry.overall_severity = entry.compute_overall_severity(&thresholds);
        assert_eq!(entry.overall_severity, Severity::Yellow);
    }

    // --- T005: is_branch_node tests ---

    #[cfg(feature = "lang-rust")]
    #[test]
    fn is_branch_node_rust() {
        assert!(is_branch_node(Language::Rust, "if_expression"));
        assert!(is_branch_node(Language::Rust, "match_expression"));
        assert!(is_branch_node(Language::Rust, "for_expression"));
        assert!(is_branch_node(Language::Rust, "while_expression"));
        assert!(is_branch_node(Language::Rust, "loop_expression"));
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn is_branch_node_python() {
        assert!(is_branch_node(Language::Python, "if_statement"));
        assert!(is_branch_node(Language::Python, "for_statement"));
        assert!(is_branch_node(Language::Python, "while_statement"));
    }

    #[cfg(feature = "lang-typescript")]
    #[test]
    fn is_branch_node_typescript() {
        assert!(is_branch_node(Language::TypeScript, "if_statement"));
        assert!(is_branch_node(Language::TypeScript, "for_statement"));
        assert!(is_branch_node(Language::TypeScript, "while_statement"));
        assert!(is_branch_node(Language::TypeScript, "switch_case"));
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn is_branch_node_non_branch_returns_false() {
        assert!(!is_branch_node(Language::Rust, "identifier"));
        assert!(!is_branch_node(Language::Rust, "string_literal"));
        assert!(!is_branch_node(Language::Rust, "totally_made_up_node"));
    }

    // --- T007: calc_cyclomatic tests ---

    #[cfg(feature = "lang-rust")]
    #[test]
    fn calc_cyclomatic_empty_function() {
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse("fn empty() {}", None).unwrap();
        assert_eq!(calc_cyclomatic(&tree, Language::Rust), 1);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn calc_cyclomatic_branches() {
        let src = r#"
fn complex(x: i32) {
    if x > 0 {
        for i in 0..x {
            if i % 2 == 0 {
                println!("{}", i);
            }
        }
    }
}
"#;
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        assert_eq!(calc_cyclomatic(&tree, Language::Rust), 4);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn calc_cyclomatic_logical_operators() {
        let src = "fn logic(a: bool, b: bool) { if a && b || a { } }";
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        assert_eq!(calc_cyclomatic(&tree, Language::Rust), 4);
    }

    // --- T009: calc_cognitive tests ---

    #[cfg(feature = "lang-rust")]
    #[test]
    fn calc_cognitive_nested_if() {
        let src = r#"
fn nested(a: i32, b: i32, c: i32) {
    if a > 0 {
        if b > 0 {
            if c > 0 {
                println!("deep");
            }
        }
    }
}
"#;
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        assert_eq!(calc_cognitive(&tree, Language::Rust), 6);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn calc_cognitive_sequential_ifs() {
        let src = r#"
fn flat(a: i32) {
    if a > 0 { }
    if a > 1 { }
    if a > 2 { }
    if a > 3 { }
    if a > 4 { }
}
"#;
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        assert_eq!(calc_cognitive(&tree, Language::Rust), 5);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn calc_cognitive_empty_function() {
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse("fn empty() {}", None).unwrap();
        assert_eq!(calc_cognitive(&tree, Language::Rust), 0);
    }

    // --- T011: calc_nesting_depth tests ---

    #[cfg(feature = "lang-rust")]
    #[test]
    fn calc_nesting_depth_four_levels() {
        let src = r#"
fn deep(a: i32) {
    if a > 0 {
        if a > 1 {
            if a > 2 {
                if a > 3 { }
            }
        }
    }
}
"#;
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        assert_eq!(calc_nesting_depth(&tree, Language::Rust), 4);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn calc_nesting_depth_no_branches() {
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse("fn flat() {}", None).unwrap();
        assert_eq!(calc_nesting_depth(&tree, Language::Rust), 0);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn calc_nesting_depth_parallel_branches() {
        let src = r#"
fn parallel(a: i32) {
    if a > 0 {
        if a > 1 { }
    }
    if a > 2 {
        if a > 3 {
            if a > 4 { }
        }
    }
}
"#;
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        assert_eq!(calc_nesting_depth(&tree, Language::Rust), 3);
    }

    // --- T013: ComplexityAnalyzer tests ---

    /// Returns a fresh on-disk database path inside a temp dir.
    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("complexity_testdb");
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

    /// Creates a Function node with the given `content` via direct Cypher.
    fn create_function_with_content(
        kit: &Kit,
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        start_line: u32,
        end_line: u32,
        content: &str,
    ) {
        let storage = storage(kit);
        let cypher = format!(
            "CREATE (:Function {{id: '{}', project: '{}', name: '{}', qualifiedName: '{}', \
             filePath: '{}', startLine: {}, endLine: {}, signature: '', returnType: '', \
             isExported: false, docstring: '', content: '{}', parentQn: ''}});",
            escape_cypher_string(id),
            escape_cypher_string(project),
            escape_cypher_string(name),
            escape_cypher_string(qn),
            escape_cypher_string(file),
            start_line,
            end_line,
            escape_cypher_string(content),
        );
        storage.execute(&cypher).expect("create function");
    }

    #[test]
    fn analyze_returns_empty_for_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = storage(&kit);
        let analyzer = ComplexityAnalyzer::new(&*storage);
        let result = analyzer.analyze("demo").expect("analyze");
        assert!(result.is_empty(), "empty DB should yield no complexity entries");
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn analyze_returns_correct_metrics() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Simple function: no branches.
        create_function_with_content(
            &kit,
            "f_foo",
            "demo",
            "foo",
            "demo.foo",
            "/src/lib.rs",
            1,
            1,
            "fn foo() {}",
        );
        // Complex function: if > for > if (3 branch nodes, 3 levels deep).
        create_function_with_content(
            &kit,
            "f_bar",
            "demo",
            "bar",
            "demo.bar",
            "/src/lib.rs",
            1,
            5,
            "fn bar() { if x { for i in 0..n { if i % 2 == 0 {} } } }",
        );

        let storage = storage(&kit);
        let analyzer = ComplexityAnalyzer::new(&*storage);
        let mut result = analyzer.analyze("demo").expect("analyze");
        // Stable order by qualified name for deterministic assertions.
        result.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));

        assert_eq!(result.len(), 2, "should analyze both functions");

        let foo = result.iter().find(|e| e.name == "foo").expect("foo entry");
        assert_eq!(foo.cyclomatic, 1, "foo cyclomatic");
        assert_eq!(foo.cognitive, 0, "foo cognitive");
        assert_eq!(foo.nesting_depth, 0, "foo nesting");
        assert_eq!(foo.function_length, 1, "foo length");
        assert_eq!(foo.overall_severity, Severity::Green, "foo severity");

        let bar = result.iter().find(|e| e.name == "bar").expect("bar entry");
        assert_eq!(bar.cyclomatic, 4, "bar cyclomatic");
        assert!(bar.cognitive > 0, "bar cognitive should be > 0, got {}", bar.cognitive);
        assert_eq!(bar.nesting_depth, 3, "bar nesting (if>for>if = 3 levels)");
        assert_eq!(bar.function_length, 5, "bar length");
    }

    // --- T015: detect_language tests ---

    #[cfg(all(
        feature = "lang-rust",
        feature = "lang-python",
        feature = "lang-typescript",
        feature = "lang-c",
        feature = "lang-cpp",
        feature = "lang-go",
        feature = "lang-java",
        feature = "lang-fortran"
    ))]
    #[test]
    fn detect_language_maps_known_extensions() {
        assert_eq!(detect_language("/src/lib.rs"), Some(Language::Rust));
        assert_eq!(detect_language("/src/main.py"), Some(Language::Python));
        assert_eq!(detect_language("/src/index.ts"), Some(Language::TypeScript));
        assert_eq!(detect_language("/src/main.c"), Some(Language::C));
        assert_eq!(detect_language("/src/main.cpp"), Some(Language::Cpp));
        assert_eq!(detect_language("/src/main.go"), Some(Language::Go));
        assert_eq!(detect_language("/src/Main.java"), Some(Language::Java));
        assert_eq!(detect_language("/src/program.f90"), Some(Language::Fortran));
    }

    #[test]
    fn detect_language_returns_none_for_unknown() {
        assert_eq!(detect_language("/src/unknown.xyz"), None);
        assert_eq!(detect_language("no_extension"), None);
    }
}
