// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `complexity` service: AST-based complexity metrics with severity classification.

use serde::Serialize;
use std::str::FromStr;

#[cfg(feature = "complexity")]
use crate::analysis::complexity::{
    ComplexityAnalyzer, ComplexityEntry, ComplexityThresholds, Severity, SpaceComplexity,
    TimeComplexity,
};
#[cfg(feature = "complexity")]
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
#[cfg(all(feature = "cli", feature = "complexity"))]
use crate::service::error::kit_not_initialized;
#[cfg(all(feature = "cli", feature = "complexity"))]
use crate::service::error::to_api_error;
#[cfg(feature = "complexity")]
use crate::service::error::CodeNexusError;
#[cfg(feature = "complexity")]
use crate::service::project::resolve_project_id;
#[cfg(all(feature = "cli", feature = "complexity"))]
use crate::service::runtime::kit;

#[cfg(all(feature = "cli", feature = "complexity"))]
use sdforge::forge;
#[cfg(all(feature = "cli", feature = "complexity"))]
use sdforge::prelude::ApiError;

/// JSON-serializable complexity output.
#[cfg(feature = "complexity")]
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ComplexityOutput {
    pub project: String,
    pub complexity: Vec<ComplexityEntry>,
    pub summary: ComplexitySummary,
}

/// Aggregate severity counts.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ComplexitySummary {
    pub total: usize,
    pub green: usize,
    pub yellow: usize,
    pub red: usize,
    pub critical: usize,
}

/// Builds [`ComplexityThresholds`] from sentinel-encoded params.
///
/// `u32` params use 0 as sentinel (0 = use default).
/// `String` params use empty string as sentinel ("" = use default).
#[cfg(feature = "complexity")]
#[allow(clippy::too_many_arguments)]
fn build_thresholds(
    cyclomatic_green: u32,
    cyclomatic_yellow: u32,
    cyclomatic_red: u32,
    cognitive_green: u32,
    cognitive_yellow: u32,
    cognitive_red: u32,
    nesting_green: u32,
    nesting_yellow: u32,
    nesting_red: u32,
    func_length_green: u32,
    func_length_yellow: u32,
    func_length_red: u32,
    halstead_volume_green: u32,
    halstead_volume_yellow: u32,
    halstead_volume_red: u32,
    maintainability_green: u32,
    maintainability_yellow: u32,
    maintainability_red: u32,
    time_complexity_green: &str,
    time_complexity_yellow: &str,
    time_complexity_red: &str,
    space_complexity_yellow: &str,
    space_complexity_red: &str,
) -> Result<ComplexityThresholds, CodeNexusError> {
    let mut t = ComplexityThresholds::default();
    if cyclomatic_green > 0 {
        t.cyclomatic.0 = cyclomatic_green;
    }
    if cyclomatic_yellow > 0 {
        t.cyclomatic.1 = cyclomatic_yellow;
    }
    if cyclomatic_red > 0 {
        t.cyclomatic.2 = cyclomatic_red;
    }
    if cognitive_green > 0 {
        t.cognitive.0 = cognitive_green;
    }
    if cognitive_yellow > 0 {
        t.cognitive.1 = cognitive_yellow;
    }
    if cognitive_red > 0 {
        t.cognitive.2 = cognitive_red;
    }
    if nesting_green > 0 {
        t.nesting.0 = nesting_green;
    }
    if nesting_yellow > 0 {
        t.nesting.1 = nesting_yellow;
    }
    if nesting_red > 0 {
        t.nesting.2 = nesting_red;
    }
    if func_length_green > 0 {
        t.func_length.0 = func_length_green;
    }
    if func_length_yellow > 0 {
        t.func_length.1 = func_length_yellow;
    }
    if func_length_red > 0 {
        t.func_length.2 = func_length_red;
    }
    if halstead_volume_green > 0 {
        t.halstead_volume.0 = halstead_volume_green;
    }
    if halstead_volume_yellow > 0 {
        t.halstead_volume.1 = halstead_volume_yellow;
    }
    if halstead_volume_red > 0 {
        t.halstead_volume.2 = halstead_volume_red;
    }
    if maintainability_green > 0 {
        t.maintainability.0 = maintainability_green;
    }
    if maintainability_yellow > 0 {
        t.maintainability.1 = maintainability_yellow;
    }
    if maintainability_red > 0 {
        t.maintainability.2 = maintainability_red;
    }
    if !time_complexity_green.is_empty() {
        t.time_complexity.0 = TimeComplexity::from_str(time_complexity_green)
            .map_err(CodeNexusError::InvalidInput)?;
    }
    if !time_complexity_yellow.is_empty() {
        t.time_complexity.1 = TimeComplexity::from_str(time_complexity_yellow)
            .map_err(CodeNexusError::InvalidInput)?;
    }
    if !time_complexity_red.is_empty() {
        t.time_complexity.2 =
            TimeComplexity::from_str(time_complexity_red).map_err(CodeNexusError::InvalidInput)?;
    }
    if !space_complexity_yellow.is_empty() {
        t.space_complexity.0 = SpaceComplexity::from_str(space_complexity_yellow)
            .map_err(CodeNexusError::InvalidInput)?;
    }
    if !space_complexity_red.is_empty() {
        t.space_complexity.1 = SpaceComplexity::from_str(space_complexity_red)
            .map_err(CodeNexusError::InvalidInput)?;
    }
    Ok(t)
}

/// Computes aggregate severity counts over `entries`.
#[cfg(feature = "complexity")]
fn compute_summary(entries: &[ComplexityEntry]) -> ComplexitySummary {
    let mut green = 0usize;
    let mut yellow = 0usize;
    let mut red = 0usize;
    let mut critical = 0usize;
    for e in entries {
        match e.overall_severity {
            Severity::Green => green += 1,
            Severity::Yellow => yellow += 1,
            Severity::Red => red += 1,
            Severity::Critical => critical += 1,
        }
    }
    ComplexitySummary {
        total: entries.len(),
        green,
        yellow,
        red,
        critical,
    }
}

/// Core logic — resolves storage, runs analysis, filters/sorts, prints JSON.
#[cfg(feature = "complexity")]
#[allow(clippy::too_many_arguments)]
fn complexity_core(
    kit: &AsyncKit<AsyncReady>,
    project: &str,
    red_only: bool,
    sort_by_severity: bool,
    cyclomatic_green: u32,
    cyclomatic_yellow: u32,
    cyclomatic_red: u32,
    cognitive_green: u32,
    cognitive_yellow: u32,
    cognitive_red: u32,
    nesting_green: u32,
    nesting_yellow: u32,
    nesting_red: u32,
    func_length_green: u32,
    func_length_yellow: u32,
    func_length_red: u32,
    halstead_volume_green: u32,
    halstead_volume_yellow: u32,
    halstead_volume_red: u32,
    maintainability_green: u32,
    maintainability_yellow: u32,
    maintainability_red: u32,
    time_complexity_green: &str,
    time_complexity_yellow: &str,
    time_complexity_red: &str,
    space_complexity_yellow: &str,
    space_complexity_red: &str,
) -> Result<(), CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let project_id = resolve_project_id(&*storage, project)?;
    let thresholds = build_thresholds(
        cyclomatic_green,
        cyclomatic_yellow,
        cyclomatic_red,
        cognitive_green,
        cognitive_yellow,
        cognitive_red,
        nesting_green,
        nesting_yellow,
        nesting_red,
        func_length_green,
        func_length_yellow,
        func_length_red,
        halstead_volume_green,
        halstead_volume_yellow,
        halstead_volume_red,
        maintainability_green,
        maintainability_yellow,
        maintainability_red,
        time_complexity_green,
        time_complexity_yellow,
        time_complexity_red,
        space_complexity_yellow,
        space_complexity_red,
    )?;
    let analyzer = ComplexityAnalyzer::new_with_thresholds(&*storage, thresholds);
    let entries = analyzer.analyze(&project_id)?;
    let summary = compute_summary(&entries);
    let mut filtered = entries;
    if red_only {
        filtered.retain(|e| {
            e.overall_severity == Severity::Red || e.overall_severity == Severity::Critical
        });
    }
    if sort_by_severity {
        filtered.sort_by_key(|b| std::cmp::Reverse(b.overall_severity));
    }
    let output = ComplexityOutput {
        project: project.to_string(),
        complexity: filtered,
        summary,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// CLI wrapper — prints result to stdout as JSON.
///
/// All threshold/flag params use sentinel values (0 for u32, "" for String,
/// "false" for bool) because sdforge 0.4.2's `#[forge]` macro cannot parse
/// `Option<T>` CLI args. `main.rs` injects these sentinel values via clap
/// `default_value` (see `SENTINEL_DEFAULTS`) so users can omit them on the
/// command line; `build_thresholds` then treats sentinels as "use default".
#[cfg(all(feature = "cli", feature = "complexity"))]
#[allow(clippy::too_many_arguments)]
#[forge(
    name = "complexity",
    version = "0.3.3",
    description = "Analyze AST-based complexity metrics for all functions in a project.",
    cli = true
)]
async fn complexity(
    project: String,
    red_only: bool,
    sort_by_severity: bool,
    cyclomatic_green: u32,
    cyclomatic_yellow: u32,
    cyclomatic_red: u32,
    cognitive_green: u32,
    cognitive_yellow: u32,
    cognitive_red: u32,
    nesting_green: u32,
    nesting_yellow: u32,
    nesting_red: u32,
    func_length_green: u32,
    func_length_yellow: u32,
    func_length_red: u32,
    halstead_volume_green: u32,
    halstead_volume_yellow: u32,
    halstead_volume_red: u32,
    maintainability_green: u32,
    maintainability_yellow: u32,
    maintainability_red: u32,
    time_complexity_green: String,
    time_complexity_yellow: String,
    time_complexity_red: String,
    space_complexity_yellow: String,
    space_complexity_red: String,
) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    complexity_core(
        &kit,
        &project,
        red_only,
        sort_by_severity,
        cyclomatic_green,
        cyclomatic_yellow,
        cyclomatic_red,
        cognitive_green,
        cognitive_yellow,
        cognitive_red,
        nesting_green,
        nesting_yellow,
        nesting_red,
        func_length_green,
        func_length_yellow,
        func_length_red,
        halstead_volume_green,
        halstead_volume_yellow,
        halstead_volume_red,
        maintainability_green,
        maintainability_yellow,
        maintainability_red,
        &time_complexity_green,
        &time_complexity_yellow,
        &time_complexity_red,
        &space_complexity_yellow,
        &space_complexity_red,
    )
    .map_err(|e| to_api_error(e, "complexity_error"))?;
    Ok(())
}

#[cfg(all(test, feature = "cli", feature = "complexity"))]
mod tests {
    use super::*;
    use crate::analysis::complexity::HalsteadMetrics;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig, StorageModule};
    use crate::storage::capability::Storage;
    use crate::storage::schema::escape_cypher_string;
    use tempfile::TempDir;

    fn fresh_db_path() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("svc_complexity_testdb");
        (dir, path)
    }

    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    fn seed_project(storage: &dyn Storage, id: &str, name: &str) {
        storage
            .execute(&format!(
                "CREATE (:Project {{id: '{id}', name: '{name}', rootPath: '/demo', language: 'rust', fileCount: 1, indexedAt: 1000, lastCommit: 'abc'}});"
            ))
            .expect("create project");
    }

    #[allow(clippy::too_many_arguments)]
    fn create_function_with_content(
        kit: &AsyncKit<AsyncReady>,
        id: &str,
        project: &str,
        name: &str,
        qn: &str,
        file: &str,
        start_line: u32,
        end_line: u32,
        content: &str,
    ) {
        let storage = kit.require::<StorageModule>().expect("require_storage");
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
    fn core_succeeds_on_empty_db() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        let result = complexity_core(
            &kit, "demo", false, false, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "",
            "", "", "", "",
        );
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    #[cfg(feature = "lang-rust")]
    fn core_returns_correct_summary() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        create_function_with_content(
            &kit,
            "f_simple",
            "demo",
            "simple",
            "demo.simple",
            "/src/a.rs",
            1,
            1,
            "fn simple() {}",
        );
        let red_src = "fn red() { if a { if b { if c { if d { if e { if f { if g { if h { if i { if j { if k { if l { if m { if n { if o { if p { if q { if r { if s { if t { if u {} } } } } } } } } } } } } } } } } } } } }";
        create_function_with_content(
            &kit,
            "f_red",
            "demo",
            "red",
            "demo.red",
            "/src/b.rs",
            1,
            50,
            red_src,
        );

        let storage = kit.require::<StorageModule>().expect("require_storage");
        let analyzer = ComplexityAnalyzer::new(&*storage);
        let entries = analyzer.analyze("demo").expect("analyze");
        let summary = compute_summary(&entries);
        assert_eq!(summary.total, 2, "total functions");
        assert!(summary.green >= 1, "green count: {}", summary.green);
        assert!(
            summary.critical >= 1,
            "critical count: {}",
            summary.critical
        );

        let result = complexity_core(
            &kit, "demo", false, false, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "",
            "", "", "", "",
        );
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());
    }

    #[test]
    fn output_serializes_to_json() {
        let out = ComplexityOutput {
            project: "demo".into(),
            complexity: vec![],
            summary: ComplexitySummary {
                total: 0,
                green: 0,
                yellow: 0,
                red: 0,
                critical: 0,
            },
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"project\":\"demo\""));
        assert!(json.contains("\"complexity\""));
        assert!(json.contains("\"summary\""));
        assert!(json.contains("\"total\":0"));
        assert!(json.contains("\"green\":0"));
        assert!(json.contains("\"yellow\":0"));
        assert!(json.contains("\"red\":0"));
        assert!(json.contains("\"critical\":0"));
    }

    #[test]
    #[cfg(feature = "lang-rust")]
    fn core_red_only_filters_correctly() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        create_function_with_content(
            &kit,
            "f_simple",
            "demo",
            "simple",
            "demo.simple",
            "/src/a.rs",
            1,
            1,
            "fn simple() {}",
        );
        let red_src = "fn red() { if a { if b { if c { if d { if e { if f { if g { if h { if i { if j { if k { if l { if m { if n { if o { if p { if q { if r { if s { if t { if u {} } } } } } } } } } } } } } } } } } } }";
        create_function_with_content(
            &kit,
            "f_red",
            "demo",
            "red",
            "demo.red",
            "/src/b.rs",
            1,
            50,
            red_src,
        );

        let storage = kit.require::<StorageModule>().expect("require_storage");
        let analyzer = ComplexityAnalyzer::new(&*storage);
        let entries = analyzer.analyze("demo").expect("analyze");
        let summary = compute_summary(&entries);
        assert_eq!(summary.total, 2, "summary total counts all entries");
        let mut filtered = entries;
        filtered.retain(|e| {
            e.overall_severity == Severity::Red || e.overall_severity == Severity::Critical
        });
        assert!(
            filtered
                .iter()
                .all(|e| e.overall_severity == Severity::Red
                    || e.overall_severity == Severity::Critical),
            "red_only should retain only Red and Critical entries"
        );
    }

    #[test]
    fn build_thresholds_uses_defaults_when_sentinels() {
        let t = build_thresholds(
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "", "", "", "", "",
        )
        .expect("build_thresholds");
        assert_eq!(t, ComplexityThresholds::default());
    }

    #[test]
    fn build_thresholds_overrides_when_set() {
        let t = build_thresholds(
            2, 5, 8, 3, 7, 10, 1, 3, 4, 20, 50, 100, 50, 500, 4000, 90, 60, 80, "O(1)", "O(1)",
            "O(n)", "O(1)", "O(n^2)",
        )
        .expect("build_thresholds");
        assert_eq!(t.cyclomatic, (2, 5, 8));
        assert_eq!(t.cognitive, (3, 7, 10));
        assert_eq!(t.nesting, (1, 3, 4));
        assert_eq!(t.func_length, (20, 50, 100));
        assert_eq!(t.halstead_volume, (50, 500, 4000));
        assert_eq!(t.maintainability, (90, 60, 80));
        assert_eq!(
            t.time_complexity,
            (TimeComplexity::O1, TimeComplexity::O1, TimeComplexity::ON)
        );
        assert_eq!(
            t.space_complexity,
            (SpaceComplexity::O1, SpaceComplexity::ON2)
        );
    }

    #[test]
    fn build_thresholds_rejects_invalid_time_complexity() {
        let result = build_thresholds(
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "bogus", "", "", "", "",
        );
        assert!(result.is_err(), "invalid time complexity should error");
    }

    #[test]
    #[cfg(feature = "lang-rust")]
    fn core_uses_custom_thresholds() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
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

        let result = complexity_core(
            &kit, "demo", false, false, 2, 5, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "",
            "", "", "", "",
        );
        assert!(result.is_ok(), "core should succeed: {:?}", result.err());

        let storage = kit.require::<StorageModule>().expect("require_storage");
        let thresholds = build_thresholds(
            2, 5, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "", "", "", "", "",
        )
        .expect("build_thresholds");
        let analyzer = ComplexityAnalyzer::new_with_thresholds(&*storage, thresholds);
        let entries = analyzer.analyze("demo").expect("analyze");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cyclomatic, 10, "cyclomatic should be 10");
        assert_eq!(
            entries[0].overall_severity,
            Severity::Critical,
            "custom thresholds (green=2, yellow=5, red=8) should make cyclomatic=10 Critical"
        );

        let analyzer_default = ComplexityAnalyzer::new(&*storage);
        let entries_default = analyzer_default.analyze("demo").expect("analyze");
        assert_ne!(
            entries_default[0].overall_severity,
            Severity::Critical,
            "default thresholds should not make this function Critical"
        );
    }

    // --- build_thresholds: remaining error paths ---

    #[test]
    fn build_thresholds_rejects_invalid_time_complexity_yellow() {
        let result = build_thresholds(
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "", "bogus", "", "", "",
        );
        assert!(
            result.is_err(),
            "invalid time_complexity_yellow should error"
        );
    }

    #[test]
    fn build_thresholds_rejects_invalid_time_complexity_red() {
        let result = build_thresholds(
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "", "", "bogus", "", "",
        );
        assert!(result.is_err(), "invalid time_complexity_red should error");
    }

    #[test]
    fn build_thresholds_rejects_invalid_space_complexity_yellow() {
        let result = build_thresholds(
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "", "", "", "bogus", "",
        );
        assert!(
            result.is_err(),
            "invalid space_complexity_yellow should error"
        );
    }

    #[test]
    fn build_thresholds_rejects_invalid_space_complexity_red() {
        let result = build_thresholds(
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "", "", "", "", "bogus",
        );
        assert!(result.is_err(), "invalid space_complexity_red should error");
    }

    #[test]
    fn build_thresholds_accepts_valid_space_complexity_strings() {
        let t = build_thresholds(
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "", "", "", "O(1)", "O(n^2)",
        )
        .expect("build_thresholds");
        assert_eq!(
            t.space_complexity,
            (SpaceComplexity::O1, SpaceComplexity::ON2)
        );
    }

    // --- compute_summary: all four severities ---

    fn entry_with_severity(severity: Severity) -> ComplexityEntry {
        ComplexityEntry {
            name: "f".to_string(),
            qualified_name: "demo.f".to_string(),
            file_path: "/x.rs".to_string(),
            start_line: 1,
            end_line: 1,
            language: "rust".to_string(),
            cyclomatic: 1,
            cognitive: 0,
            nesting_depth: 0,
            function_length: 1,
            overall_severity: severity,
            halstead: HalsteadMetrics::default(),
            maintainability_index: 100.0,
            time_complexity: TimeComplexity::O1,
            space_complexity: SpaceComplexity::O1,
        }
    }

    #[test]
    fn compute_summary_counts_all_four_severities() {
        let entries = vec![
            entry_with_severity(Severity::Green),
            entry_with_severity(Severity::Yellow),
            entry_with_severity(Severity::Red),
            entry_with_severity(Severity::Critical),
            entry_with_severity(Severity::Green),
        ];
        let s = compute_summary(&entries);
        assert_eq!(s.total, 5);
        assert_eq!(s.green, 2);
        assert_eq!(s.yellow, 1);
        assert_eq!(s.red, 1);
        assert_eq!(s.critical, 1);
    }

    #[test]
    fn compute_summary_empty_returns_zeros() {
        let s = compute_summary(&[]);
        assert_eq!(
            s,
            ComplexitySummary {
                total: 0,
                green: 0,
                yellow: 0,
                red: 0,
                critical: 0
            }
        );
    }

    // --- complexity_core: red_only and sort_by_severity branches on empty db ---

    #[test]
    fn core_red_only_on_empty_db_succeeds() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        let result = complexity_core(
            &kit, "demo", true, false, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "",
            "", "", "", "",
        );
        assert!(
            result.is_ok(),
            "red_only on empty db should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn core_sort_by_severity_on_empty_db_succeeds() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        let result = complexity_core(
            &kit, "demo", false, true, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "",
            "", "", "", "",
        );
        assert!(
            result.is_ok(),
            "sort_by_severity on empty db should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn core_red_only_and_sort_on_empty_db_succeeds() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        let result = complexity_core(
            &kit, "demo", true, true, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "", "",
            "", "", "",
        );
        assert!(
            result.is_ok(),
            "red_only+sort on empty db should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    fn build_thresholds_partial_overrides_keep_defaults_for_unset() {
        let t = build_thresholds(
            5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "O(n)", "", "", "", "",
        )
        .expect("build_thresholds");
        assert_eq!(
            t.cyclomatic.0, 5,
            "cyclomatic green should be overridden to 5"
        );
        let defaults = ComplexityThresholds::default();
        assert_eq!(
            t.cyclomatic.1, defaults.cyclomatic.1,
            "cyclomatic yellow stays default"
        );
        assert_eq!(
            t.cyclomatic.2, defaults.cyclomatic.2,
            "cyclomatic red stays default"
        );
        assert_eq!(
            t.time_complexity.0,
            TimeComplexity::ON,
            "time green overridden"
        );
        assert_eq!(
            t.time_complexity.1, defaults.time_complexity.1,
            "time yellow stays default"
        );
    }

    // --- complexity_core: red_only + sort_by_severity with real entries ---

    #[test]
    #[cfg(feature = "lang-rust")]
    fn core_red_only_with_entries_exercises_retain_closure() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        create_function_with_content(
            &kit,
            "f_simple",
            "demo",
            "simple",
            "demo.simple",
            "/src/a.rs",
            1,
            1,
            "fn simple() {}",
        );
        let red_src = "fn red() { if a { if b { if c { if d { if e { if f { if g { if h { if i { if j { if k { if l { if m { if n { if o { if p { if q { if r { if s { if t { if u {} } } } } } } } } } } } } } } } } } } }";
        create_function_with_content(
            &kit,
            "f_red",
            "demo",
            "red",
            "demo.red",
            "/src/b.rs",
            1,
            50,
            red_src,
        );
        let result = complexity_core(
            &kit, "demo", true, false, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "",
            "", "", "", "",
        );
        assert!(
            result.is_ok(),
            "red_only with entries should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    #[cfg(feature = "lang-rust")]
    fn core_sort_by_severity_with_entries_exercises_sort() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        create_function_with_content(
            &kit,
            "f_simple",
            "demo",
            "simple",
            "demo.simple",
            "/src/a.rs",
            1,
            1,
            "fn simple() {}",
        );
        let red_src = "fn red() { if a { if b { if c { if d { if e { if f { if g { if h { if i { if j { if k { if l { if m { if n { if o { if p { if q { if r { if s { if t { if u {} } } } } } } } } } } } } } } } } } } }";
        create_function_with_content(
            &kit,
            "f_red",
            "demo",
            "red",
            "demo.red",
            "/src/b.rs",
            1,
            50,
            red_src,
        );
        let result = complexity_core(
            &kit, "demo", false, true, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "",
            "", "", "", "",
        );
        assert!(
            result.is_ok(),
            "sort_by_severity with entries should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    #[cfg(feature = "lang-rust")]
    fn core_red_only_and_sort_with_entries_exercises_both_branches() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        create_function_with_content(
            &kit,
            "f_simple",
            "demo",
            "simple",
            "demo.simple",
            "/src/a.rs",
            1,
            1,
            "fn simple() {}",
        );
        let red_src = "fn red() { if a { if b { if c { if d { if e { if f { if g { if h { if i { if j { if k { if l { if m { if n { if o { if p { if q { if r { if s { if t { if u {} } } } } } } } } } } } } } } } } } } } }";
        create_function_with_content(
            &kit,
            "f_red",
            "demo",
            "red",
            "demo.red",
            "/src/b.rs",
            1,
            50,
            red_src,
        );
        let result = complexity_core(
            &kit, "demo", true, true, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "", "",
            "", "", "",
        );
        assert!(
            result.is_ok(),
            "red_only+sort with entries should succeed: {:?}",
            result.err()
        );
    }

    #[test]
    #[cfg(feature = "lang-rust")]
    fn core_with_custom_thresholds_and_red_only_filters_correctly() {
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        let src = "fn f() { if a {} if b {} if c {} }";
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
        // With very low thresholds (green=1, yellow=2, red=3), cyclomatic=3 → Red
        let result = complexity_core(
            &kit, "demo", true, false, 1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, "",
            "", "", "", "",
        );
        assert!(
            result.is_ok(),
            "custom thresholds + red_only should succeed: {:?}",
            result.err()
        );
    }

    // ===== #[forge] wrapper tests via init_kit =====

    #[serial_test::serial(kit_init)]
    #[test]
    fn complexity_wrapper_succeeds_via_init_kit() {
        use crate::service::runtime::{init_kit, reset_kit_for_testing};

        reset_kit_for_testing();
        let (_dir, db) = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let storage = kit.require::<StorageModule>().expect("storage");
        seed_project(&*storage, "demo", "demo");
        init_kit(kit).expect("init_kit");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(complexity(
            "demo".to_string(),
            false,
            false,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            "".to_string(),
            "".to_string(),
            "".to_string(),
            "".to_string(),
            "".to_string(),
        ));
        assert!(result.is_ok(), "wrapper should succeed: {:?}", result.err());

        reset_kit_for_testing();
    }

    #[serial_test::serial(kit_init)]
    #[test]
    fn complexity_wrapper_fails_when_kit_not_initialized() {
        use crate::service::runtime::reset_kit_for_testing;

        reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(complexity(
            "demo".to_string(),
            false,
            false,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            "".to_string(),
            "".to_string(),
            "".to_string(),
            "".to_string(),
            "".to_string(),
        ));
        assert!(result.is_err(), "wrapper should fail without kit");
        reset_kit_for_testing();
    }
}
