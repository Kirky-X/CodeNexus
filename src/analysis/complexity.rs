// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! AST complexity analysis (T001-T004, v0.2.1).
//!
//! Provides per-function complexity metrics (cyclomatic, cognitive, nesting
//! depth, function length) with industry-standard severity classification
//! (Green / Yellow / Red).

use serde::Serialize;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
