// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! AST complexity analysis (T001-T004, v0.2.1).
//!
//! Provides per-function complexity metrics (cyclomatic, cognitive, nesting
//! depth, function length) with industry-standard severity classification
//! (Green / Yellow / Red).

use serde::Serialize;

use crate::model::Language;

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
    /// Classifies cyclomatic complexity: `≤10` Green, `≤20` Yellow, else Red.
    pub fn from_cyclomatic(value: u32) -> Severity {
        if value <= 10 {
            Severity::Green
        } else if value <= 20 {
            Severity::Yellow
        } else {
            Severity::Red
        }
    }

    /// Classifies cognitive complexity: `≤10` Green, `≤15` Yellow, else Red.
    pub fn from_cognitive(value: u32) -> Severity {
        if value <= 10 {
            Severity::Green
        } else if value <= 15 {
            Severity::Yellow
        } else {
            Severity::Red
        }
    }

    /// Classifies nesting depth: `≤3` Green, `≤5` Yellow, else Red.
    pub fn from_nesting(value: u32) -> Severity {
        if value <= 3 {
            Severity::Green
        } else if value <= 5 {
            Severity::Yellow
        } else {
            Severity::Red
        }
    }

    /// Classifies function length: `≤30` Green, `≤100` Yellow, else Red.
    pub fn from_func_length(value: u32) -> Severity {
        if value <= 30 {
            Severity::Green
        } else if value <= 100 {
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
    /// (`Red > Yellow > Green`) by calling the `from_*` classifiers.
    pub fn compute_overall_severity(&self, _thresholds: &ComplexityThresholds) -> Severity {
        [
            Severity::from_cyclomatic(self.cyclomatic),
            Severity::from_cognitive(self.cognitive),
            Severity::from_nesting(self.nesting_depth),
            Severity::from_func_length(self.function_length),
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

#[cfg(test)]
mod tests {
    use super::*;

    // --- T005: is_branch_node tests ---

    #[test]
    fn from_cyclomatic_classification() {
        assert_eq!(Severity::from_cyclomatic(5), Severity::Green);
        assert_eq!(Severity::from_cyclomatic(15), Severity::Yellow);
        assert_eq!(Severity::from_cyclomatic(30), Severity::Red);
    }

    #[test]
    fn from_cognitive_classification() {
        assert_eq!(Severity::from_cognitive(5), Severity::Green);
        assert_eq!(Severity::from_cognitive(12), Severity::Yellow);
        assert_eq!(Severity::from_cognitive(25), Severity::Red);
    }

    #[test]
    fn from_nesting_classification() {
        assert_eq!(Severity::from_nesting(2), Severity::Green);
        assert_eq!(Severity::from_nesting(4), Severity::Yellow);
        assert_eq!(Severity::from_nesting(7), Severity::Red);
    }

    #[test]
    fn from_func_length_classification() {
        assert_eq!(Severity::from_func_length(20), Severity::Green);
        assert_eq!(Severity::from_func_length(50), Severity::Yellow);
        assert_eq!(Severity::from_func_length(150), Severity::Red);
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
}
