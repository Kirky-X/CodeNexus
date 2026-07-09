// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! AST complexity analysis (T001-T004, v0.2.1).
//!
//! Provides per-function complexity metrics (cyclomatic, cognitive, nesting
//! depth, function length) with industry-standard severity classification
//! (Green / Yellow / Red).

use serde::Serialize;
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
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

/// Estimated time complexity class (T010). Variant declaration order defines
/// the derived `Ord` ordering: `O1 < OLogN < ON < ONLogN < ON2 < ON3 < O2N`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum TimeComplexity {
    O1,
    OLogN,
    ON,
    ONLogN,
    ON2,
    ON3,
    O2N,
}

impl fmt::Display for TimeComplexity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::O1 => write!(f, "O(1)"),
            Self::OLogN => write!(f, "O(log n)"),
            Self::ON => write!(f, "O(n)"),
            Self::ONLogN => write!(f, "O(n log n)"),
            Self::ON2 => write!(f, "O(n^2)"),
            Self::ON3 => write!(f, "O(n^3)"),
            Self::O2N => write!(f, "O(2^n)"),
        }
    }
}

impl FromStr for TimeComplexity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "O(1)" => Ok(Self::O1),
            "O(log n)" => Ok(Self::OLogN),
            "O(n)" => Ok(Self::ON),
            "O(n log n)" => Ok(Self::ONLogN),
            "O(n^2)" => Ok(Self::ON2),
            "O(n^3)" => Ok(Self::ON3),
            "O(2^n)" => Ok(Self::O2N),
            _ => Err(format!("unknown time complexity: {s}")),
        }
    }
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
    /// Halstead volume thresholds `(yellow, red)` — default `(1000, 8000)`.
    pub halstead_volume: (u32, u32),
    /// Maintainability Index thresholds `(yellow_min, red_min)` — default
    /// `(65, 85)`. MI is inverted (higher = better), so `from_maintainability`
    /// classifies `value >= red_min → Green`, `value >= yellow_min → Yellow`,
    /// else `Red`.
    pub maintainability: (u32, u32),
    /// Time complexity thresholds `(yellow, red)` as ordinal discriminants —
    /// default `(2, 4)` (ON=2, ON2=4). Replaced by strong `(TimeComplexity,
    /// TimeComplexity)` in T012.
    pub time_complexity: (u8, u8),
    /// Space complexity thresholds `(yellow, red)` as ordinal discriminants —
    /// default `(0, 1)` (O1=0, ON=1). Replaced by strong `(SpaceComplexity,
    /// SpaceComplexity)` in T015.
    pub space_complexity: (u8, u8),
}

impl Default for ComplexityThresholds {
    fn default() -> Self {
        Self {
            cyclomatic: (20, 25),
            cognitive: (15, 20),
            nesting: (5, 6),
            func_length: (100, 200),
            halstead_volume: (1000, 8000),
            maintainability: (65, 85),
            time_complexity: (2, 4),
            space_complexity: (0, 1),
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

    /// Classifies Maintainability Index against `thresholds.maintainability`.
    ///
    /// MI is inverted (higher = more maintainable), so classification is
    /// reversed: `value >= red_min → Green`, `value >= yellow_min → Yellow`,
    /// else `Red`. `thresholds.maintainability = (yellow_min, red_min)`.
    pub fn from_maintainability(value: f64, thresholds: &ComplexityThresholds) -> Severity {
        let yellow_min = thresholds.maintainability.0 as f64;
        let red_min = thresholds.maintainability.1 as f64;
        if value >= red_min {
            Severity::Green
        } else if value >= yellow_min {
            Severity::Yellow
        } else {
            Severity::Red
        }
    }
}

/// A single function's complexity metrics with overall severity.
#[derive(Debug, Clone, Serialize, PartialEq)]
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
    /// Halstead complexity metrics (T007).
    pub halstead: HalsteadMetrics,
    /// Maintainability Index (Microsoft 2007, 0-100, higher=better) (T009).
    pub maintainability_index: f64,
}

/// Halstead complexity metrics (Halstead 1977). Tracks distinct and total
/// operator/operand counts plus derived volume, difficulty, effort, and
/// delivered-bug estimates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Default)]
pub struct HalsteadMetrics {
    /// Distinct operator count (n1).
    pub n1: u32,
    /// Distinct operand count (n2).
    pub n2: u32,
    /// Total operator occurrences (N1).
    pub n1_total: u32,
    /// Total operand occurrences (N2).
    pub n2_total: u32,
    /// Volume V = (N1+N2) * log2(n1+n2).
    pub volume: f64,
    /// Difficulty D = (n1/2) * (N2/n2).
    pub difficulty: f64,
    /// Effort E = D * V.
    pub effort: f64,
    /// Delivered bugs B = E^(2/3) / 3000.
    pub delivered_bugs: f64,
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
            Severity::from_maintainability(self.maintainability_index, thresholds),
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
                | "if_let_expression"
                | "match_expression"
                | "for_expression"
                | "while_expression"
                | "loop_expression"
                | "try_expression"
        ),
        #[cfg(feature = "lang-c")]
        Language::C => matches!(
            node_type,
            "if_statement" | "for_statement" | "while_statement" | "switch_statement"
        ),
        #[cfg(feature = "lang-cpp")]
        Language::Cpp => matches!(
            node_type,
            "if_statement"
                | "for_statement"
                | "while_statement"
                | "switch_statement"
                | "try_statement"
                | "catch_clause"
                | "throw_statement"
        ),
        #[cfg(feature = "lang-python")]
        Language::Python => matches!(
            node_type,
            "if_statement"
                | "for_statement"
                | "while_statement"
                | "try_statement"
                | "except_clause"
        ),
        #[cfg(feature = "lang-typescript")]
        Language::TypeScript => matches!(
            node_type,
            "if_statement"
                | "for_statement"
                | "while_statement"
                | "switch_case"
                | "try_statement"
                | "catch_clause"
        ),
        #[cfg(feature = "lang-go")]
        Language::Go => matches!(node_type, "if_statement" | "for_statement" | "switch"),
        #[cfg(feature = "lang-java")]
        Language::Java => matches!(
            node_type,
            "if_statement"
                | "for_statement"
                | "while_statement"
                | "switch_expression"
                | "try_statement"
                | "catch_clause"
        ),
        #[cfg(feature = "lang-fortran")]
        Language::Fortran => matches!(node_type, "if_statement" | "do_statement"),
        _ => false,
    }
}

/// Returns true if the given tree-sitter node type is an exit node (explicit
/// `return` / `break` / `continue`) for the specified language.
fn is_exit_node(language: Language, node_type: &str) -> bool {
    #[allow(unreachable_patterns)]
    match language {
        #[cfg(feature = "lang-rust")]
        Language::Rust => matches!(
            node_type,
            "return_expression" | "break_expression" | "continue_expression"
        ),
        _ => matches!(
            node_type,
            "return_statement" | "break_statement" | "continue_statement"
        ),
    }
}

/// Counts explicit exit nodes (`return` / `break` / `continue`) in the parse
/// tree. Per McCabe 1976, each explicit exit adds 1 to cyclomatic complexity.
/// Implicit returns (the trailing expression in a Rust block) are not counted.
pub fn count_exit_nodes(tree: &tree_sitter::Tree, language: Language) -> u32 {
    count_exit_nodes_recursive(tree.root_node(), language)
}

fn count_exit_nodes_recursive(node: Node<'_>, language: Language) -> u32 {
    let mut count = 0;
    if is_exit_node(language, node.kind()) {
        count += 1;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        count += count_exit_nodes_recursive(child, language);
    }
    count
}

// --- Halstead complexity (T006) ---

/// Returns the tree-sitter node kind for the function body block of `language`,
/// or `None` for languages without a clear body container (e.g. Fortran).
fn halstead_body_kind(language: Language) -> Option<&'static str> {
    #[allow(unreachable_patterns)]
    match language {
        #[cfg(feature = "lang-rust")]
        Language::Rust => Some("block"),
        #[cfg(feature = "lang-python")]
        Language::Python => Some("block"),
        #[cfg(feature = "lang-c")]
        Language::C => Some("compound_statement"),
        #[cfg(feature = "lang-cpp")]
        Language::Cpp => Some("compound_statement"),
        #[cfg(feature = "lang-java")]
        Language::Java => Some("block"),
        #[cfg(feature = "lang-go")]
        Language::Go => Some("block"),
        #[cfg(feature = "lang-typescript")]
        Language::TypeScript => Some("statement_block"),
        _ => None,
    }
}

/// DFS for the first node of `kind` in the subtree rooted at `node`.
fn find_first_node_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    if node.kind() == kind {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_first_node_of_kind(child, kind) {
            return Some(found);
        }
    }
    None
}

/// Returns true if `kind` is an operator-expression node whose anonymous
/// children are operator tokens (e.g. `+` in a `binary_expression`).
fn is_operator_expression(kind: &str) -> bool {
    matches!(
        kind,
        "binary_expression"
            | "unary_expression"
            | "assignment_expression"
            | "update_expression"
            | "binary_operator"
            | "boolean_operator"
            | "comparison_operator"
            | "not_operator"
            | "assignment"
            | "augmented_assignment"
    )
}

/// Returns true if `kind` is an operand node (identifier or literal).
fn is_operand_kind(kind: &str) -> bool {
    kind == "identifier" || kind.ends_with("_literal")
}

/// Computes Halstead complexity metrics for the given parse tree.
///
/// Traverses the function body (or full tree if no body is found) collecting
/// operator tokens from operator-expression nodes and operand text from
/// identifier/literal nodes. Derived metrics follow Halstead 1977:
/// `V = N * log2(n)`, `D = (n1/2) * (N2/n2)`, `E = D * V`, `B = E^(2/3)/3000`.
pub fn calc_halstead(
    tree: &tree_sitter::Tree,
    source: &[u8],
    language: Language,
) -> HalsteadMetrics {
    let root = tree.root_node();
    let start = halstead_body_kind(language)
        .and_then(|kind| find_first_node_of_kind(root, kind))
        .unwrap_or(root);

    let mut ops_distinct: HashSet<&'static str> = HashSet::new();
    let mut ops_total: u32 = 0;
    let mut operands_distinct: HashSet<String> = HashSet::new();
    let mut operands_total: u32 = 0;

    collect_halstead(
        start,
        source,
        &mut ops_distinct,
        &mut ops_total,
        &mut operands_distinct,
        &mut operands_total,
    );

    let n1 = ops_distinct.len() as u32;
    let n2 = operands_distinct.len() as u32;
    let n1_total = ops_total;
    let n2_total = operands_total;

    let n = (n1 + n2) as f64;
    let volume = if n > 0.0 {
        (n1_total + n2_total) as f64 * n.log2()
    } else {
        0.0
    };
    let difficulty = if n2 > 0 {
        (n1 as f64 / 2.0) * (n2_total as f64 / n2 as f64)
    } else {
        0.0
    };
    let effort = difficulty * volume;
    let delivered_bugs = effort.powf(2.0 / 3.0) / 3000.0;

    HalsteadMetrics {
        n1,
        n2,
        n1_total,
        n2_total,
        volume,
        difficulty,
        effort,
        delivered_bugs,
    }
}

fn collect_halstead(
    node: Node<'_>,
    source: &[u8],
    ops_distinct: &mut HashSet<&'static str>,
    ops_total: &mut u32,
    operands_distinct: &mut HashSet<String>,
    operands_total: &mut u32,
) {
    let kind = node.kind();

    if is_operator_expression(kind) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if !child.is_named() {
                // Anonymous child = operator token (e.g. `+`, `==`, `=`).
                ops_distinct.insert(child.kind());
                *ops_total += 1;
            } else {
                collect_halstead(
                    child,
                    source,
                    ops_distinct,
                    ops_total,
                    operands_distinct,
                    operands_total,
                );
            }
        }
    } else if is_operand_kind(kind) {
        if let Ok(text) = node.utf8_text(source) {
            operands_distinct.insert(text.to_string());
            *operands_total += 1;
        }
    } else {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            collect_halstead(
                child,
                source,
                ops_distinct,
                ops_total,
                operands_distinct,
                operands_total,
            );
        }
    }
}

// --- Maintainability Index (T008) ---

/// Computes the Maintainability Index (Microsoft 2007 revision).
///
/// `MI = max(0, min(100, (171 - 5.2*ln(V) - 0.23*CC - 16.2*ln(LOC)) * 100/171))`
///
/// `halstead_volume` and `loc` are clamped to a minimum of `1.0` / `1` before
/// taking the logarithm, preventing `ln(0)` from producing `NaN`/`-inf`. Higher
/// MI indicates better maintainability (0=worst, 100=best).
pub fn calc_maintainability_index(cyclomatic: u32, halstead_volume: f64, loc: u32) -> f64 {
    let v = halstead_volume.max(1.0);
    let loc_f = loc.max(1) as f64;
    let raw = 171.0 - 5.2 * v.ln() - 0.23 * cyclomatic as f64 - 16.2 * loc_f.ln();
    (raw * 100.0 / 171.0).clamp(0.0, 100.0)
}

// --- Time complexity estimation (T011) ---

/// Returns the tree-sitter node kind for `while` loops in `language`.
fn while_kind(language: Language) -> &'static str {
    #[allow(unreachable_patterns)]
    match language {
        #[cfg(feature = "lang-rust")]
        Language::Rust => "while_expression",
        _ => "while_statement",
    }
}

/// Returns true if `kind` is a loop construct (`for`/`while`) for `language`.
fn is_loop_node(language: Language, kind: &str) -> bool {
    #[allow(unreachable_patterns)]
    match language {
        #[cfg(feature = "lang-rust")]
        Language::Rust => matches!(kind, "for_expression" | "while_expression"),
        #[cfg(feature = "lang-fortran")]
        Language::Fortran => matches!(kind, "do_statement"),
        _ => matches!(kind, "for_statement" | "while_statement"),
    }
}

/// Computes the maximum nesting depth of loop nodes in the subtree rooted at
/// `node`. A non-loop node contributes 0; a loop node contributes 1 plus the
/// max depth of its children.
fn max_loop_depth(node: Node<'_>, language: Language) -> u32 {
    let this_depth = if is_loop_node(language, node.kind()) {
        1
    } else {
        0
    };
    let mut max_child = 0u32;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        max_child = max_child.max(max_loop_depth(child, language));
    }
    this_depth + max_child
}

/// Returns true if `function_name` is invoked directly within the subtree
/// (direct recursion). Matches `f()` and `self.f()` style calls.
fn has_direct_recursion(node: Node<'_>, source: &[u8], function_name: &str) -> bool {
    if node.kind() == "call_expression" {
        if let Some(func) = node.child_by_field_name("function") {
            if let Ok(text) = func.utf8_text(source) {
                if text == function_name || text.ends_with(&format!(".{function_name}")) {
                    return true;
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if has_direct_recursion(child, source, function_name) {
            return true;
        }
    }
    false
}

/// Returns true if a `while` loop in the subtree contains a binary-search
/// halving pattern (`(left + right) / 2` or `(left + right) >> 1`). Heuristic
/// source-text check: the while node text contains `+` and (`/ 2` or `>> 1`).
fn has_binary_search_pattern(node: Node<'_>, source: &[u8], language: Language) -> bool {
    if node.kind() == while_kind(language) {
        if let Ok(text) = node.utf8_text(source) {
            if text.contains('+') && (text.contains("/ 2") || text.contains(">> 1")) {
                return true;
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if has_binary_search_pattern(child, source, language) {
            return true;
        }
    }
    false
}

/// Returns true if the source text contains a `.sort(` method call (heuristic).
fn has_sort_call(source_text: &str) -> bool {
    source_text.contains(".sort(")
}

/// Estimates the time complexity class of a function via AST pattern matching
/// (design D5). Priority cascade (first match wins):
/// 1. Direct recursion → `O(2^n)` (conservative upper bound).
/// 2. Binary-search halving inside a `while` loop → `O(log n)`.
/// 3. `.sort(` call → `O(n log n)`.
/// 4. Loop nesting depth: 0→`O(1)`, 1→`O(n)`, 2→`O(n^2)`, 3+→`O(n^3)`.
pub fn estimate_time_complexity(
    tree: &tree_sitter::Tree,
    source: &[u8],
    language: Language,
    function_name: &str,
) -> TimeComplexity {
    let root = tree.root_node();

    if has_direct_recursion(root, source, function_name) {
        return TimeComplexity::O2N;
    }

    if has_binary_search_pattern(root, source, language) {
        return TimeComplexity::OLogN;
    }

    if let Ok(source_text) = std::str::from_utf8(source) {
        if has_sort_call(source_text) {
            return TimeComplexity::ONLogN;
        }
    }

    let depth = max_loop_depth(root, language);
    match depth {
        0 => TimeComplexity::O1,
        1 => TimeComplexity::ON,
        2 => TimeComplexity::ON2,
        _ => TimeComplexity::ON3,
    }
}

/// Computes cyclomatic complexity (McCabe) for the given parse tree.
///
/// Starts at CC=1 (entry point) and adds 1 for each branch node, each `&&`/`||`
/// operator in binary expressions, each `match_arm` beyond the first, and each
/// explicit exit node (`return` / `break` / `continue`) per McCabe 1976.
pub fn calc_cyclomatic(tree: &tree_sitter::Tree, language: Language) -> u32 {
    1 + cyclomatic_count(tree.root_node(), language) + count_exit_nodes(tree, language)
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

    /// Creates a new analyzer with caller-supplied `thresholds`, overriding
    /// the defaults used by [`new`](Self::new).
    #[must_use]
    pub fn new_with_thresholds(storage: &'a dyn Storage, thresholds: ComplexityThresholds) -> Self {
        Self { storage, thresholds }
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
                let halstead = calc_halstead(&tree, content.as_bytes(), language);
                let maintainability_index =
                    calc_maintainability_index(cyclomatic, halstead.volume, function_length);

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
                    halstead,
                    maintainability_index,
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

    #[test]
    fn thresholds_default_includes_new_fields() {
        let t = ComplexityThresholds::default();
        assert_eq!(t.halstead_volume, (1000, 8000));
        assert_eq!(t.maintainability, (65, 85));
        assert_eq!(t.time_complexity, (2, 4));
        assert_eq!(t.space_complexity, (0, 1));
    }

    /// Builds a `ComplexityEntry` with the given metric values and placeholder
    /// metadata. `overall_severity` is set to `Green` and should be recomputed
    /// via `compute_overall_severity` in the test. `maintainability_index` is
    /// set to `100.0` (neutral Green) so it does not pollute the overall
    /// severity in tests focused on other metrics.
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
            halstead: HalsteadMetrics::default(),
            maintainability_index: 100.0,
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

    #[cfg(feature = "lang-rust")]
    #[test]
    fn is_branch_node_rust_try_expression() {
        assert!(is_branch_node(Language::Rust, "try_expression"));
        assert!(is_branch_node(Language::Rust, "if_let_expression"));
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn is_branch_node_python_except_clause() {
        assert!(is_branch_node(Language::Python, "except_clause"));
        assert!(is_branch_node(Language::Python, "try_statement"));
    }

    #[cfg(feature = "lang-cpp")]
    #[test]
    fn is_branch_node_cpp_throw_statement() {
        assert!(is_branch_node(Language::Cpp, "throw_statement"));
        assert!(is_branch_node(Language::Cpp, "try_statement"));
        assert!(is_branch_node(Language::Cpp, "catch_clause"));
    }

    #[cfg(feature = "lang-typescript")]
    #[test]
    fn is_branch_node_typescript_try_catch() {
        assert!(is_branch_node(Language::TypeScript, "try_statement"));
        assert!(is_branch_node(Language::TypeScript, "catch_clause"));
    }

    #[cfg(feature = "lang-java")]
    #[test]
    fn is_branch_node_java_try_catch() {
        assert!(is_branch_node(Language::Java, "try_statement"));
        assert!(is_branch_node(Language::Java, "catch_clause"));
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

    #[cfg(feature = "lang-rust")]
    #[test]
    fn count_exit_nodes_rust_return_break_continue() {
        let src = "fn f() { if x { return 1; } for i in 0..n { if i == 0 { break; } else { continue; } } }";
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        // 1 return + 1 break + 1 continue = 3 exit nodes.
        assert_eq!(count_exit_nodes(&tree, Language::Rust), 3);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn calc_cyclomatic_with_exits() {
        // 1 (entry) + 1 (if_expression branch) + 1 (return_expression exit) = 3.
        // Old impl without exit counting returned 2.
        let src = "fn f() { if x { return 1; } }";
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        assert_eq!(calc_cyclomatic(&tree, Language::Rust), 3);
    }

    // --- T006: calc_halstead tests ---

    #[cfg(feature = "lang-rust")]
    #[test]
    fn calc_halstead_rust_simple_addition() {
        let src = "fn add(a: i32, b: i32) -> i32 { a + b }";
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        let m = calc_halstead(&tree, src.as_bytes(), Language::Rust);
        // `+` is the one distinct operator → n1 >= 1, N1 >= 1.
        assert!(m.n1 >= 1, "n1 (distinct operators) should be >= 1, got {}", m.n1);
        assert!(m.n1_total >= 1, "N1 (total operators) should be >= 1, got {}", m.n1_total);
        // `a` and `b` are operands → n2 >= 2, N2 >= 2.
        assert!(m.n2 >= 2, "n2 (distinct operands) should be >= 2, got {}", m.n2);
        assert!(m.n2_total >= 2, "N2 (total operands) should be >= 2, got {}", m.n2_total);
        assert!(m.volume > 0.0, "volume should be > 0, got {}", m.volume);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn calc_halstead_empty_function() {
        let src = "fn empty() {}";
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        let m = calc_halstead(&tree, src.as_bytes(), Language::Rust);
        // Empty body → no operators/operands → all-zero default.
        assert_eq!(m, HalsteadMetrics::default());
    }

    // --- T008: calc_maintainability_index tests ---

    #[test]
    fn mi_simple_function_high_score() {
        // CC=1, V=10, LOC=5 → MI ≈ 77.6 (simple, maintainable).
        let mi = calc_maintainability_index(1, 10.0, 5);
        assert!(mi.is_finite(), "MI should be finite, got {mi}");
        assert!(
            (70.0..=100.0).contains(&mi),
            "simple function MI should be 70-100, got {mi}"
        );
    }

    #[test]
    fn mi_complex_function_low_score() {
        // CC=30, V=5000, LOC=200 → MI ≈ 19.9 (complex, hard to maintain).
        let mi = calc_maintainability_index(30, 5000.0, 200);
        assert!(mi.is_finite(), "MI should be finite, got {mi}");
        assert!(
            mi < 65.0,
            "complex function MI should be < 65, got {mi}"
        );
    }

    #[test]
    fn mi_zero_volume_clamped() {
        // V=0 would cause ln(0)=-inf without clamping.
        let mi = calc_maintainability_index(1, 0.0, 1);
        assert!(!mi.is_nan(), "MI must not be NaN for zero volume, got {mi}");
        assert!(mi.is_finite(), "MI should be finite, got {mi}");
        assert!(mi >= 0.0 && mi <= 100.0, "MI out of range: {mi}");
    }

    #[test]
    fn mi_zero_loc_clamped() {
        // LOC=0 would cause ln(0)=-inf without clamping.
        let mi = calc_maintainability_index(1, 10.0, 0);
        assert!(!mi.is_nan(), "MI must not be NaN for zero LOC, got {mi}");
        assert!(mi.is_finite(), "MI should be finite, got {mi}");
        assert!(mi >= 0.0 && mi <= 100.0, "MI out of range: {mi}");
    }

    // --- T010: TimeComplexity tests ---

    #[test]
    fn time_complexity_display_format() {
        assert_eq!(TimeComplexity::O1.to_string(), "O(1)");
        assert_eq!(TimeComplexity::OLogN.to_string(), "O(log n)");
        assert_eq!(TimeComplexity::ON.to_string(), "O(n)");
        assert_eq!(TimeComplexity::ONLogN.to_string(), "O(n log n)");
        assert_eq!(TimeComplexity::ON2.to_string(), "O(n^2)");
        assert_eq!(TimeComplexity::ON3.to_string(), "O(n^3)");
        assert_eq!(TimeComplexity::O2N.to_string(), "O(2^n)");
    }

    #[test]
    fn time_complexity_fromstr_parses() {
        assert_eq!("O(1)".parse::<TimeComplexity>().unwrap(), TimeComplexity::O1);
        assert_eq!("O(log n)".parse::<TimeComplexity>().unwrap(), TimeComplexity::OLogN);
        assert_eq!("O(n)".parse::<TimeComplexity>().unwrap(), TimeComplexity::ON);
        assert_eq!(
            "O(n log n)".parse::<TimeComplexity>().unwrap(),
            TimeComplexity::ONLogN
        );
        assert_eq!("O(n^2)".parse::<TimeComplexity>().unwrap(), TimeComplexity::ON2);
        assert_eq!("O(n^3)".parse::<TimeComplexity>().unwrap(), TimeComplexity::ON3);
        assert_eq!("O(2^n)".parse::<TimeComplexity>().unwrap(), TimeComplexity::O2N);
        // Unknown string → error.
        assert!("O(n!)".parse::<TimeComplexity>().is_err());
    }

    #[test]
    fn time_complexity_ord_ordering() {
        // Variant declaration order defines Ord: O1 < OLogN < ON < ONLogN < ON2 < ON3 < O2N.
        assert!(TimeComplexity::O1 < TimeComplexity::ON);
        assert!(TimeComplexity::ON < TimeComplexity::ON2);
        assert!(TimeComplexity::O1 < TimeComplexity::ON2);
        assert!(TimeComplexity::OLogN < TimeComplexity::ON);
        assert!(TimeComplexity::ON2 < TimeComplexity::ON3);
        assert!(TimeComplexity::ON3 < TimeComplexity::O2N);
    }

    // --- T011: estimate_time_complexity tests ---

    #[cfg(feature = "lang-rust")]
    #[test]
    fn tc_empty_function_is_o1() {
        let src = "fn f() {}";
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        assert_eq!(
            estimate_time_complexity(&tree, src.as_bytes(), Language::Rust, "f"),
            TimeComplexity::O1
        );
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn tc_single_loop_is_on() {
        let src = "fn f() { for i in 0..n { } }";
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        assert_eq!(
            estimate_time_complexity(&tree, src.as_bytes(), Language::Rust, "f"),
            TimeComplexity::ON
        );
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn tc_nested_loops_is_on2() {
        let src = "fn f() { for i in 0..n { for j in 0..n { } } }";
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        assert_eq!(
            estimate_time_complexity(&tree, src.as_bytes(), Language::Rust, "f"),
            TimeComplexity::ON2
        );
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn tc_triple_nested_loops_is_on3() {
        let src = "fn f() { for i in 0..n { for j in 0..n { for k in 0..n { } } } }";
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        assert_eq!(
            estimate_time_complexity(&tree, src.as_bytes(), Language::Rust, "f"),
            TimeComplexity::ON3
        );
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn tc_binary_search_is_ologn() {
        let src = r#"
fn f(arr: &[i32], target: i32) -> i32 {
    let mut left = 0;
    let mut right = arr.len() as i32 - 1;
    while left <= right {
        let mid = (left + right) / 2;
        if arr[mid as usize] == target { return mid; }
        if arr[mid as usize] < target { left = mid + 1; }
        else { right = mid - 1; }
    }
    -1
}
"#;
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        assert_eq!(
            estimate_time_complexity(&tree, src.as_bytes(), Language::Rust, "f"),
            TimeComplexity::OLogN
        );
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn tc_sort_then_binary_search_is_onlogn() {
        let src = r#"
fn f(arr: &mut Vec<i32>, x: i32) -> bool {
    arr.sort();
    arr.binary_search(&x).is_ok()
}
"#;
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        assert_eq!(
            estimate_time_complexity(&tree, src.as_bytes(), Language::Rust, "f"),
            TimeComplexity::ONLogN
        );
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn tc_recursive_is_o2n() {
        let src = "fn f(n: i32) { f(n - 1); }";
        let mut parser = ParserFactory::create_parser(Language::Rust).unwrap();
        let tree = parser.parse(src, None).unwrap();
        assert_eq!(
            estimate_time_complexity(&tree, src.as_bytes(), Language::Rust, "f"),
            TimeComplexity::O2N
        );
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
    fn new_with_thresholds_overrides_default() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // 9 if-branches → cyclomatic = 1 + 9 = 10. With default thresholds
        // (yellow=20, green=10), cyclomatic=10 → Green. With custom
        // (yellow=5, red=8), green_max=2, cyclomatic=10 > 5 → Red.
        let src = "fn f() { if a {} if b {} if c {} if d {} if e {} \
                   if f {} if g {} if h {} if i {} }";
        create_function_with_content(
            &kit,
            "f_thresh",
            "demo",
            "f",
            "demo.f",
            "/src/lib.rs",
            1,
            1,
            src,
        );

        let storage = storage(&kit);
        let mut custom = ComplexityThresholds::default();
        custom.cyclomatic = (5, 8);
        let analyzer = ComplexityAnalyzer::new_with_thresholds(&*storage, custom);
        let result = analyzer.analyze("demo").expect("analyze");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].cyclomatic, 10, "cyclomatic should be 10");
        assert_eq!(
            result[0].overall_severity,
            Severity::Red,
            "custom thresholds should make cyclomatic=10 Red"
        );

        // Sanity: with default thresholds, cyclomatic=10 → Green but cognitive=9
        // → Yellow (green_max=7), so overall = Yellow (not Red).
        let analyzer_default = ComplexityAnalyzer::new(&*storage);
        let result_default = analyzer_default.analyze("demo").expect("analyze");
        assert_ne!(
            result_default[0].overall_severity,
            Severity::Red,
            "default thresholds should not make this function Red"
        );
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

    #[cfg(feature = "lang-rust")]
    #[test]
    fn analyze_includes_halstead_metrics() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Function with an operator (`+`) → halstead.volume > 0.
        create_function_with_content(
            &kit,
            "f_add",
            "demo",
            "add",
            "demo.add",
            "/src/lib.rs",
            1,
            1,
            "fn add(a: i32, b: i32) -> i32 { a + b }",
        );
        // Empty function → halstead all-zero (default).
        create_function_with_content(
            &kit,
            "f_empty",
            "demo",
            "empty",
            "demo.empty",
            "/src/lib.rs",
            1,
            1,
            "fn empty() {}",
        );

        let storage = storage(&kit);
        let analyzer = ComplexityAnalyzer::new(&*storage);
        let mut result = analyzer.analyze("demo").expect("analyze");
        result.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));

        let add = result.iter().find(|e| e.name == "add").expect("add entry");
        assert!(
            add.halstead.volume > 0.0,
            "add should have volume > 0, got {}",
            add.halstead.volume
        );
        assert!(add.halstead.n2 >= 2, "add should have >= 2 operands");

        let empty = result.iter().find(|e| e.name == "empty").expect("empty entry");
        assert_eq!(
            empty.halstead,
            HalsteadMetrics::default(),
            "empty function should have all-zero halstead"
        );
    }

    // --- T009: from_maintainability + analyze_includes_mi tests ---

    #[test]
    fn from_maintainability_high_is_green() {
        // Default thresholds: (yellow_min=65, red_min=85). value=90 >= 85 → Green.
        let t = ComplexityThresholds::default();
        assert_eq!(Severity::from_maintainability(90.0, &t), Severity::Green);
        // Boundary: value=85 == red_min → Green.
        assert_eq!(Severity::from_maintainability(85.0, &t), Severity::Green);
    }

    #[test]
    fn from_maintainability_mid_is_yellow() {
        // value=70: 70 >= 65 (yellow_min) but < 85 (red_min) → Yellow.
        let t = ComplexityThresholds::default();
        assert_eq!(Severity::from_maintainability(70.0, &t), Severity::Yellow);
        // Boundary: value=65 == yellow_min → Yellow.
        assert_eq!(Severity::from_maintainability(65.0, &t), Severity::Yellow);
    }

    #[test]
    fn from_maintainability_low_is_red() {
        // value=50 < 65 (yellow_min) → Red.
        let t = ComplexityThresholds::default();
        assert_eq!(Severity::from_maintainability(50.0, &t), Severity::Red);
        assert_eq!(Severity::from_maintainability(0.0, &t), Severity::Red);
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn analyze_includes_mi() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        create_function_with_content(
            &kit,
            "f_mi",
            "demo",
            "mi",
            "demo.mi",
            "/src/lib.rs",
            1,
            1,
            "fn mi(a: i32, b: i32) -> i32 { a + b }",
        );

        let storage = storage(&kit);
        let analyzer = ComplexityAnalyzer::new(&*storage);
        let result = analyzer.analyze("demo").expect("analyze");
        assert_eq!(result.len(), 1);
        let mi = &result[0];
        assert!(
            (0.0..=100.0).contains(&mi.maintainability_index),
            "MI should be 0-100, got {}",
            mi.maintainability_index
        );
        assert!(
            mi.maintainability_index.is_finite(),
            "MI should be finite, got {}",
            mi.maintainability_index
        );
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
