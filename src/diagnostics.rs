// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Archify-style structured diagnostics ("repair receipts").
//!
//! A [`Diagnostic`] pairs a stable rule code with the exact subject, measured
//! evidence, and a curated list of machine-executable repairs, so CLI/MCP
//! clients (and LLM agents driving them) can fix failures without guessing.
//! Receipts follow the archify discipline: a crash or unclassified failure
//! must never fabricate fixes — [`Receipt`] always carries the full
//! diagnostics list alongside the delivered artifact hashes.

use serde::{Deserialize, Serialize};

/// Severity of a [`Diagnostic`].
///
/// Ordering is by growing seriousness: `Warning < Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Advisory: the artifact/command output is usable but suboptimal.
    Warning,
    /// Blocking: the artifact was rejected or the command failed.
    Error,
}

/// A single structured diagnostic with a stable rule code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Stable rule identifier, `<domain>/<rule>` (e.g. `index/stale`).
    pub code: String,
    /// How serious the finding is.
    pub severity: Severity,
    /// Exact subject the finding applies to (symbol name, JSON Pointer,
    /// file path, ...).
    pub subject: String,
    /// Human-readable explanation.
    pub message: String,
    /// Measured evidence backing the finding (counts, hashes, candidates).
    pub evidence: serde_json::Value,
    /// Curated repairs a client can apply verbatim.
    pub supported_fixes: Vec<String>,
}

/// Content hash and byte size of a delivered artifact (or its specification).
///
/// `algorithm` names the hash (the project standardizes on BLAKE3 per
/// ADR-009, so delivery receipts read `"blake3"` — archify's SHA-256 is
/// deliberately not duplicated as a second hash family).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HashInfo {
    /// Hash algorithm identifier (`"blake3"`).
    pub algorithm: String,
    /// Lowercase hex digest.
    pub hash: String,
    /// Payload size in bytes.
    pub bytes: u64,
}

/// Summary of quality-gate validation for a delivered artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationSummary {
    /// Number of quality checks that passed.
    pub checks_passed: u32,
    /// Total number of quality checks executed.
    pub check_count: u32,
    /// Error-severity diagnostics emitted.
    pub errors: u32,
    /// Warning-severity diagnostics emitted.
    pub warnings: u32,
    /// Quality profile used for the gate (`standard` or `showcase`).
    pub quality_profile: String,
}

/// Git-verified source evidence summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSummary {
    /// Whether every declared source verified against the pinned revision.
    pub verified: bool,
    /// Repository URL when a git remote was resolved.
    pub repository: Option<String>,
    /// Pinned commit revision the sources were verified against.
    pub revision: Option<String>,
    /// Number of verified (source, path) references.
    pub reference_count: u32,
}

/// Delivery receipt for the `diagram` and `arch_diff` commands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Receipt {
    /// Command that produced this receipt (`diagram` or `arch_diff`).
    pub command: String,
    /// Hash/size of the canonical specification (diagram IR or delta input).
    pub specification: HashInfo,
    /// Hash/size of the delivered artifact file.
    pub artifact: HashInfo,
    /// Quality-gate summary.
    pub validation: ValidationSummary,
    /// Git evidence summary, when evidence verification ran.
    pub evidence: Option<EvidenceSummary>,
    /// All diagnostics emitted during validation and delivery.
    pub diagnostics: Vec<Diagnostic>,
}

/// Stable rule code emitted when a symbol name matches multiple graph nodes.
pub const AMBIGUOUS_SYMBOL_CODE: &str = "resolve/ambiguous-symbol";

/// Builds the repair receipt for an ambiguous symbol resolution failure.
///
/// `candidates` carries the fully-qualified names of every matching node (the
/// same list rendered into `TraceError::AmbiguousSymbol`'s message). Each
/// candidate yields one curated fix telling the client to retry with that
/// qualified name — the machine-readable counterpart of the CLI's numbered
/// candidate list.
#[must_use]
pub fn ambiguous_symbol_diagnostics(symbol: &str, candidates: &[String]) -> Vec<Diagnostic> {
    let supported_fixes = candidates
        .iter()
        .map(|qn| format!("Use the qualified name `{qn}` instead of `{symbol}`"))
        .collect();
    vec![Diagnostic {
        code: AMBIGUOUS_SYMBOL_CODE.to_string(),
        severity: Severity::Error,
        subject: symbol.to_string(),
        message: format!(
            "ambiguous symbol '{symbol}': {} candidates found",
            candidates.len()
        ),
        evidence: serde_json::json!({
            "candidate_count": candidates.len(),
            "candidates": candidates,
        }),
        supported_fixes,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_serializes_exact_field_names() {
        let d = Diagnostic {
            code: "index/stale".to_string(),
            severity: Severity::Warning,
            subject: "demo".to_string(),
            message: "index is stale".to_string(),
            evidence: serde_json::json!({ "indexed_commit": "a", "current_head": "b" }),
            supported_fixes: vec!["Re-run: codenexus index".to_string()],
        };
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"code\":\"index/stale\""), "{json}");
        assert!(json.contains("\"severity\":\"warning\""), "{json}");
        assert!(json.contains("\"subject\""), "{json}");
        assert!(json.contains("\"message\""), "{json}");
        assert!(json.contains("\"evidence\""), "{json}");
        assert!(json.contains("\"supported_fixes\""), "{json}");
    }

    #[test]
    fn severity_serializes_and_orders_by_seriousness() {
        let json_warn = serde_json::to_string(&Severity::Warning).unwrap();
        let json_err = serde_json::to_string(&Severity::Error).unwrap();
        assert_eq!(json_warn, "\"warning\"");
        assert_eq!(json_err, "\"error\"");
        assert!(Severity::Warning < Severity::Error);
    }

    #[test]
    fn receipt_serializes_full_shape() {
        let receipt = Receipt {
            command: "diagram".to_string(),
            specification: HashInfo {
                algorithm: "blake3".to_string(),
                hash: "aa".to_string(),
                bytes: 1,
            },
            artifact: HashInfo {
                algorithm: "blake3".to_string(),
                hash: "bb".to_string(),
                bytes: 2,
            },
            validation: ValidationSummary {
                checks_passed: 7,
                check_count: 7,
                errors: 0,
                warnings: 0,
                quality_profile: "standard".to_string(),
            },
            evidence: None,
            diagnostics: vec![],
        };
        let json = serde_json::to_string(&receipt).unwrap();
        for field in [
            "\"command\"",
            "\"specification\"",
            "\"artifact\"",
            "\"validation\"",
            "\"evidence\"",
            "\"diagnostics\"",
            "\"checks_passed\"",
            "\"quality_profile\"",
        ] {
            assert!(json.contains(field), "missing {field} in {json}");
        }
        assert!(json.contains("\"evidence\":null"), "{json}");
        assert!(json.contains("\"algorithm\":\"blake3\""), "{json}");
    }

    #[test]
    fn ambiguous_symbol_diagnostics_lists_candidates_as_fixes() {
        let candidates = vec!["demo.a.handler".to_string(), "demo.b.handler".to_string()];
        let diags = ambiguous_symbol_diagnostics("handler", &candidates);
        assert_eq!(diags.len(), 1);
        let d = &diags[0];
        assert_eq!(d.code, "resolve/ambiguous-symbol");
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.subject, "handler");
        assert_eq!(d.evidence["candidate_count"], 2);
        assert_eq!(d.evidence["candidates"][0], "demo.a.handler");
        assert_eq!(d.supported_fixes.len(), 2);
        assert!(d.supported_fixes[0].contains("demo.a.handler"));
        assert!(d.supported_fixes[1].contains("demo.b.handler"));
    }

    #[test]
    fn ambiguous_symbol_diagnostics_empty_candidates_still_emits_receipt() {
        let diags = ambiguous_symbol_diagnostics("ghost", &[]);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].supported_fixes.is_empty());
        assert_eq!(diags[0].evidence["candidate_count"], 0);
    }
}
