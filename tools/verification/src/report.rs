//! Markdown report generator (tasks 6.1-6.4).
//!
//! Produces per-sample Markdown reports and an aggregate batch report. The
//! severity model (Rule 5: deterministic, not model-decided):
//!
//! - **critical**: query result set differs OR a comparable type is entirely
//!   missing from one side (count = 0 on one side, > 0 on the other).
//! - **major**: comparable type count delta > 10% (relative to the larger
//!   count; if both are 0, delta = 0).
//! - **minor**: comparable type count delta ≤ 10%.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::codenexus_stats::CodeNexusStats;
use crate::gitnexus_client::GitnexusStats;
use crate::query_compare::QueryDiff;
use crate::type_map::{CanonicalType, TypeMap};

/// Per-severity counter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeverityCounts {
    pub critical: usize,
    pub major: usize,
    pub minor: usize,
}

/// Aggregate per-sample summary used by `generate_aggregate_report`.
#[derive(Debug, Clone)]
pub struct SampleSummary {
    pub name: String,
    pub language: String,
    pub overall_pass: bool,
    pub severities: SeverityCounts,
}

/// Threshold (fraction) above which a count delta becomes `major`.
const MAJOR_DELTA_THRESHOLD: f64 = 0.10;

/// Classify the delta between two counts into a severity tier.
///
/// Returns `"critical"` if one side is zero and the other is non-zero,
/// `"major"` if the relative delta exceeds 10%, otherwise `"minor"`.
fn classify_delta(codenexus: u64, gitnexus: u64) -> &'static str {
    if codenexus == 0 && gitnexus > 0 {
        return "critical";
    }
    if gitnexus == 0 && codenexus > 0 {
        return "critical";
    }
    let larger = codenexus.max(gitnexus) as f64;
    let smaller = codenexus.min(gitnexus) as f64;
    if larger == 0.0 {
        return "minor";
    }
    let rel_delta = (larger - smaller) / larger;
    if rel_delta > MAJOR_DELTA_THRESHOLD {
        "major"
    } else {
        "minor"
    }
}

/// Format a percentage string with 2 decimals, e.g. `"3.45%"`.
fn pct(codenexus: u64, gitnexus: u64) -> String {
    let larger = codenexus.max(gitnexus) as f64;
    let smaller = codenexus.min(gitnexus) as f64;
    if larger == 0.0 {
        return "0.00%".to_string();
    }
    format!("{:.2}%", (larger - smaller) / larger * 100.0)
}

/// Task 6.1: Generate a per-sample Markdown report.
///
/// The report contains:
/// 1. Summary header (OVERALL PASS/FAIL + per-severity counts)
/// 2. Node type comparison table (comparable types only)
/// 3. Edge type comparison table (comparable types only)
/// 4. Query comparison table
/// 5. Unmapped types list (codenexus_only / gitnexus_only / analysis_artifact)
pub fn generate_report(
    name: &str,
    codenexus_stats: &CodeNexusStats,
    gitnexus_stats: &GitnexusStats,
    query_diffs: &[(String, QueryDiff)],
    type_map: &TypeMap,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Verification Report: {name}\n\n"));

    // --- Severity tally ---
    let mut severities = SeverityCounts::default();

    // --- Node comparison (comparable types) ---
    out.push_str("## Node Type Comparison (Comparable Types)\n\n");
    out.push_str("| Type | CodeNexus | gitnexus | Delta | Severity |\n");
    out.push_str("|------|-----------|----------|-------|----------|\n");

    // Collect all canonical comparable node types from the type map.
    let mut comparable_nodes: BTreeMap<String, ()> = BTreeMap::new();
    for raw in codenexus_stats.node_counts_by_type.keys() {
        if let CanonicalType::Comparable(canon) = type_map.normalize_codenexus_node(raw) {
            comparable_nodes.insert(canon, ());
        }
    }
    for raw in gitnexus_stats.node_counts_by_label.keys() {
        if let CanonicalType::Comparable(canon) = type_map.normalize_gitnexus_node(raw) {
            comparable_nodes.insert(canon, ());
        }
    }

    for canon in comparable_nodes.keys() {
        let cn = sum_counts_canonical_node(codenexus_stats, type_map, canon);
        let gn = sum_counts_canonical_node_gitnexus(gitnexus_stats, type_map, canon);
        let severity = classify_delta(cn, gn);
        increment_severity(&mut severities, severity);
        out.push_str(&format!(
            "| {canon} | {cn} | {gn} | {} | {severity} |\n",
            pct(cn, gn),
        ));
    }
    out.push('\n');

    // --- Edge comparison (comparable types) ---
    out.push_str("## Edge Type Comparison (Comparable Types)\n\n");
    out.push_str("| Type | CodeNexus | gitnexus | Delta | Severity |\n");
    out.push_str("|------|-----------|----------|-------|----------|\n");

    let mut comparable_edges: BTreeMap<String, ()> = BTreeMap::new();
    for raw in codenexus_stats.edge_counts_by_type.keys() {
        if let CanonicalType::Comparable(canon) = type_map.normalize_codenexus_edge(raw) {
            comparable_edges.insert(canon, ());
        }
    }
    for raw in gitnexus_stats.edge_counts_by_type.keys() {
        if let CanonicalType::Comparable(canon) = type_map.normalize_gitnexus_edge(raw) {
            comparable_edges.insert(canon, ());
        }
    }

    for canon in comparable_edges.keys() {
        let cn = sum_counts_canonical_edge(codenexus_stats, type_map, canon);
        let gn = sum_counts_canonical_edge_gitnexus(gitnexus_stats, type_map, canon);
        let severity = classify_delta(cn, gn);
        increment_severity(&mut severities, severity);
        out.push_str(&format!(
            "| {canon} | {cn} | {gn} | {} | {severity} |\n",
            pct(cn, gn),
        ));
    }
    out.push('\n');

    // --- Query comparison ---
    out.push_str("## Query Comparison\n\n");
    out.push_str("| Query | Result | Missing in CodeNexus | Missing in gitnexus |\n");
    out.push_str("|-------|--------|----------------------|---------------------|\n");
    for (query_name, diff) in query_diffs {
        match diff {
            QueryDiff::Match { count } => {
                out.push_str(&format!("| {query_name} | MATCH ({count}) | 0 | 0 |\n"));
            }
            QueryDiff::CriticalDiff {
                missing_in_codenexus,
                missing_in_gitnexus,
            } => {
                severities.critical += 1;
                out.push_str(&format!(
                    "| {query_name} | CRITICAL | {} | {} |\n",
                    missing_in_codenexus.len(),
                    missing_in_gitnexus.len(),
                ));
            }
        }
    }
    out.push('\n');

    // --- Unmapped types (informational) ---
    out.push_str("## Unmapped Types (Informational)\n\n");
    out.push_str("### CodeNexus-only (not indexed by gitnexus)\n\n");
    out.push_str("| Type | Count |\n|------|-------|\n");
    for (raw, count) in &codenexus_stats.node_counts_by_type {
        if matches!(
            type_map.normalize_codenexus_node(raw),
            CanonicalType::CodenexusOnly(_)
        ) {
            out.push_str(&format!("| {raw} | {count} |\n"));
        }
    }
    out.push('\n');
    out.push_str("### gitnexus-only (not indexed by CodeNexus)\n\n");
    out.push_str("| Type | Count |\n|------|-------|\n");
    for (raw, count) in &gitnexus_stats.node_counts_by_label {
        if matches!(
            type_map.normalize_gitnexus_node(raw),
            CanonicalType::GitnexusOnly(_)
        ) {
            out.push_str(&format!("| {raw} | {count} |\n"));
        }
    }
    out.push('\n');
    out.push_str("### Analysis Artifacts (excluded from comparison)\n\n");
    out.push_str("| Type | CodeNexus | gitnexus |\n|------|-----------|----------|\n");
    let mut analysis_artifacts: BTreeMap<String, ()> = BTreeMap::new();
    for raw in codenexus_stats.node_counts_by_type.keys() {
        if matches!(
            type_map.normalize_codenexus_node(raw),
            CanonicalType::AnalysisArtifact(_)
        ) {
            analysis_artifacts.insert(raw.clone(), ());
        }
    }
    for raw in gitnexus_stats.node_counts_by_label.keys() {
        if matches!(
            type_map.normalize_gitnexus_node(raw),
            CanonicalType::AnalysisArtifact(_)
        ) {
            analysis_artifacts.insert(raw.clone(), ());
        }
    }
    for raw in analysis_artifacts.keys() {
        let cn = codenexus_stats
            .node_counts_by_type
            .get(raw)
            .copied()
            .unwrap_or(0);
        let gn = gitnexus_stats
            .node_counts_by_label
            .get(raw)
            .copied()
            .unwrap_or(0);
        out.push_str(&format!("| {raw} | {cn} | {gn} |\n"));
    }
    out.push('\n');

    // --- Summary header (prepend after we know the totals) ---
    let overall_pass = severities.critical == 0;
    let mut header = String::new();
    header.push_str("## Summary\n\n");
    header.push_str("| Metric | Value |\n|--------|-------|\n");
    header.push_str(&format!(
        "| Overall | {} |\n",
        if overall_pass { "PASS" } else { "FAIL" },
    ));
    header.push_str(&format!(
        "| Critical discrepancies | {} |\n",
        severities.critical
    ));
    header.push_str(&format!("| Major discrepancies | {} |\n", severities.major));
    header.push_str(&format!("| Minor discrepancies | {} |\n", severities.minor));
    header.push_str(&format!(
        "| CodeNexus file_count (by lang) | {} |\n",
        codenexus_stats
            .file_counts_by_language
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", "),
    ));
    header.push_str(&format!(
        "| gitnexus file_count | {} |\n",
        gitnexus_stats.file_count
    ));
    header.push('\n');

    // Splice header after the title.
    let title_end = out.find("\n\n").unwrap_or(out.len());
    let mut full = String::new();
    full.push_str(&out[..title_end + 2]);
    full.push_str(&header);
    full.push_str(&out[title_end + 2..]);
    full
}

/// Task 6.3: Write a per-sample Markdown report to `results/<name>.report.md`.
pub fn write_report(name: &str, markdown: &str) -> Result<PathBuf> {
    let dir = Path::new("tools/verification/results");
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{name}.report.md"));
    std::fs::write(&path, markdown)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

/// Task 6.4: Generate an aggregate batch report ranking samples by severity.
///
/// Writes to `tools/verification/results/_aggregate.report.md`.
pub fn generate_aggregate_report(summaries: &[SampleSummary]) -> String {
    let mut out = String::new();
    out.push_str("# Aggregate Verification Report\n\n");
    out.push_str("## Per-Sample Summary\n\n");
    out.push_str("| Sample | Language | Overall | Critical | Major | Minor |\n");
    out.push_str("|--------|----------|---------|----------|-------|-------|\n");
    for s in summaries {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            s.name,
            s.language,
            if s.overall_pass { "PASS" } else { "FAIL" },
            s.severities.critical,
            s.severities.major,
            s.severities.minor,
        ));
    }
    out.push('\n');

    // Ranking (by critical desc, then major desc, then minor desc).
    let mut ranked: Vec<&SampleSummary> = summaries.iter().collect();
    ranked.sort_by(|a, b| {
        b.severities
            .critical
            .cmp(&a.severities.critical)
            .then_with(|| b.severities.major.cmp(&a.severities.major))
            .then_with(|| b.severities.minor.cmp(&a.severities.minor))
    });
    out.push_str("## Ranking (by critical count desc)\n\n");
    for (i, s) in ranked.iter().enumerate() {
        out.push_str(&format!(
            "{}. **{}** — critical={}, major={}, minor={}, overall={}\n",
            i + 1,
            s.name,
            s.severities.critical,
            s.severities.major,
            s.severities.minor,
            if s.overall_pass { "PASS" } else { "FAIL" },
        ));
    }

    let total_pass = summaries.iter().filter(|s| s.overall_pass).count();
    let total_fail = summaries.len() - total_pass;
    out.push_str(&format!(
        "\n**Total: {} pass, {} fail (of {} samples)**\n",
        total_pass,
        total_fail,
        summaries.len(),
    ));
    out
}

/// Write the aggregate report to `results/_aggregate.report.md`.
pub fn write_aggregate_report(markdown: &str) -> Result<PathBuf> {
    let dir = Path::new("tools/verification/results");
    std::fs::create_dir_all(dir)?;
    let path = dir.join("_aggregate.report.md");
    std::fs::write(&path, markdown)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

// -- helpers --

fn increment_severity(severities: &mut SeverityCounts, tier: &str) {
    match tier {
        "critical" => severities.critical += 1,
        "major" => severities.major += 1,
        "minor" => severities.minor += 1,
        _ => {}
    }
}

/// Sum CodeNexus node counts that map to a given canonical type.
fn sum_counts_canonical_node(stats: &CodeNexusStats, type_map: &TypeMap, canon: &str) -> u64 {
    stats
        .node_counts_by_type
        .iter()
        .filter(|(raw, _)| {
            matches!(type_map.normalize_codenexus_node(raw), CanonicalType::Comparable(c) if c == canon)
        })
        .map(|(_, v)| v)
        .sum()
}

/// Sum gitnexus node counts that map to a given canonical type.
fn sum_counts_canonical_node_gitnexus(
    stats: &GitnexusStats,
    type_map: &TypeMap,
    canon: &str,
) -> u64 {
    stats
        .node_counts_by_label
        .iter()
        .filter(|(raw, _)| {
            matches!(type_map.normalize_gitnexus_node(raw), CanonicalType::Comparable(c) if c == canon)
        })
        .map(|(_, v)| v)
        .sum()
}

/// Sum CodeNexus edge counts that map to a given canonical type.
fn sum_counts_canonical_edge(stats: &CodeNexusStats, type_map: &TypeMap, canon: &str) -> u64 {
    stats
        .edge_counts_by_type
        .iter()
        .filter(|(raw, _)| {
            matches!(type_map.normalize_codenexus_edge(raw), CanonicalType::Comparable(c) if c == canon)
        })
        .map(|(_, v)| v)
        .sum()
}

/// Sum gitnexus edge counts that map to a given canonical type.
fn sum_counts_canonical_edge_gitnexus(
    stats: &GitnexusStats,
    type_map: &TypeMap,
    canon: &str,
) -> u64 {
    stats
        .edge_counts_by_type
        .iter()
        .filter(|(raw, _)| {
            matches!(type_map.normalize_gitnexus_edge(raw), CanonicalType::Comparable(c) if c == canon)
        })
        .map(|(_, v)| v)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::type_map::TypeMap;
    use std::path::PathBuf;

    fn load_type_map() -> TypeMap {
        let path = PathBuf::from("tools/verification/type_map.json");
        TypeMap::load(&path).expect("type_map.json must load")
    }

    fn sample_codenexus_stats() -> CodeNexusStats {
        let mut node_counts = BTreeMap::new();
        node_counts.insert("Function".to_string(), 100);
        node_counts.insert("Class".to_string(), 0);
        node_counts.insert("GlobalVar".to_string(), 45); // codenexus_only
        node_counts.insert("Community".to_string(), 10); // analysis_artifact
        let mut edge_counts = BTreeMap::new();
        edge_counts.insert("CALLS".to_string(), 500);
        edge_counts.insert("FFI_CALLS".to_string(), 50); // codenexus_only
        CodeNexusStats {
            name: "demo".to_string(),
            node_counts_by_type: node_counts,
            edge_counts_by_type: edge_counts,
            file_counts_by_language: {
                let mut m = BTreeMap::new();
                m.insert("rust".to_string(), 50);
                m
            },
        }
    }

    fn sample_gitnexus_stats() -> GitnexusStats {
        let mut node_counts = BTreeMap::new();
        node_counts.insert("Function".to_string(), 102); // 2% delta → minor
        node_counts.insert("Class".to_string(), 10); // codenexus 0 → critical
        node_counts.insert("CodeElement".to_string(), 200); // gitnexus_only
        node_counts.insert("Community".to_string(), 10); // analysis_artifact
        let mut edge_counts = BTreeMap::new();
        edge_counts.insert("CALLS".to_string(), 600); // 16.7% delta → major
        GitnexusStats {
            name: "demo".to_string(),
            node_counts_by_label: node_counts,
            edge_counts_by_type: edge_counts,
            file_count: 50,
        }
    }

    #[test]
    fn classify_delta_critical_when_one_side_zero() {
        assert_eq!(classify_delta(0, 10), "critical");
        assert_eq!(classify_delta(10, 0), "critical");
    }

    #[test]
    fn classify_delta_minor_when_both_zero() {
        assert_eq!(classify_delta(0, 0), "minor");
    }

    #[test]
    fn classify_delta_major_when_above_threshold() {
        // 100 vs 120 → 16.67% delta → major
        assert_eq!(classify_delta(100, 120), "major");
        assert_eq!(classify_delta(120, 100), "major");
    }

    #[test]
    fn classify_delta_minor_when_at_or_below_threshold() {
        // 100 vs 105 → 5% delta → minor
        assert_eq!(classify_delta(100, 105), "minor");
        // 100 vs 110 → exactly 10% → minor (≤ 10%)
        assert_eq!(classify_delta(100, 110), "minor");
    }

    #[test]
    fn generate_report_includes_summary_header() {
        let tm = load_type_map();
        let cn = sample_codenexus_stats();
        let gn = sample_gitnexus_stats();
        let diffs = vec![(
            "callers_of_function".to_string(),
            QueryDiff::Match { count: 50 },
        )];
        let report = generate_report("demo", &cn, &gn, &diffs, &tm);
        assert!(report.contains("# Verification Report: demo"));
        assert!(report.contains("## Summary"));
        assert!(report.contains("Overall"));
        // Class is comparable, codenexus=0, gitnexus=10 → critical
        assert!(report.contains("| class | 0 | 10 | 100.00% | critical |"));
        // Function: 100 vs 102 → 2% delta → minor
        assert!(report.contains("| function | 100 | 102 | 1.96% | minor |"));
        // CALLS: 500 vs 600 → 16.67% → major
        assert!(report.contains("| calls | 500 | 600 | 16.67% | major |"));
        // Query match row
        assert!(report.contains("| callers_of_function | MATCH (50) | 0 | 0 |"));
        // Unmapped types section
        assert!(report.contains("### CodeNexus-only"));
        assert!(report.contains("| GlobalVar | 45 |"));
        assert!(report.contains("### gitnexus-only"));
        assert!(report.contains("| CodeElement | 200 |"));
        assert!(report.contains("### Analysis Artifacts"));
    }

    #[test]
    fn generate_report_marks_fail_when_critical_present() {
        let tm = load_type_map();
        let cn = sample_codenexus_stats();
        let gn = sample_gitnexus_stats();
        let diffs = vec![(
            "extends_chain".to_string(),
            QueryDiff::CriticalDiff {
                missing_in_codenexus: vec![("foo".to_string(), "a.rs".to_string())],
                missing_in_gitnexus: vec![],
            },
        )];
        let report = generate_report("demo", &cn, &gn, &diffs, &tm);
        assert!(report.contains("| Overall | FAIL |"));
        assert!(report.contains("| extends_chain | CRITICAL | 1 | 0 |"));
    }

    #[test]
    fn generate_aggregate_report_ranks_by_critical_desc() {
        let summaries = vec![
            SampleSummary {
                name: "alpha".to_string(),
                language: "rust".to_string(),
                overall_pass: true,
                severities: SeverityCounts {
                    critical: 0,
                    major: 2,
                    minor: 5,
                },
            },
            SampleSummary {
                name: "bravo".to_string(),
                language: "python".to_string(),
                overall_pass: false,
                severities: SeverityCounts {
                    critical: 3,
                    major: 1,
                    minor: 0,
                },
            },
            SampleSummary {
                name: "charlie".to_string(),
                language: "c".to_string(),
                overall_pass: false,
                severities: SeverityCounts {
                    critical: 1,
                    major: 0,
                    minor: 1,
                },
            },
        ];
        let report = generate_aggregate_report(&summaries);
        assert!(report.contains("# Aggregate Verification Report"));
        assert!(report.contains("## Ranking (by critical count desc)"));
        // bravo (3 critical) must rank before charlie (1 critical) before alpha (0 critical)
        let bravo_idx = report.find("1. **bravo**").unwrap_or(usize::MAX);
        let charlie_idx = report.find("3. **alpha**").unwrap_or(usize::MAX);
        let alpha_idx = report.find("2. **charlie**").unwrap_or(usize::MAX);
        assert!(bravo_idx < alpha_idx);
        assert!(alpha_idx < charlie_idx);
        assert!(report.contains("**Total: 1 pass, 2 fail (of 3 samples)**"));
    }
}
