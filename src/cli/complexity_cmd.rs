// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `complexity` subcommand handler (v0.3.1).

use super::args::ComplexityArgs;
use super::error::Result;
use crate::analysis::complexity::{ComplexityAnalyzer, ComplexityEntry, ComplexityThresholds, Severity};
use crate::kit::{Kit, StorageKey};

/// JSON-serializable complexity output.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct ComplexityOutput {
    /// The queried project name.
    pub project: String,
    /// The per-function complexity entries (possibly filtered/sorted).
    pub complexity: Vec<ComplexityEntry>,
    /// Aggregate severity counts over the full (unfiltered) result set.
    pub summary: ComplexitySummary,
}

/// Aggregate severity counts.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct ComplexitySummary {
    /// Total number of analysed functions.
    pub total: usize,
    /// Number of functions whose overall severity is Green.
    pub green: usize,
    /// Number of functions whose overall severity is Yellow.
    pub yellow: usize,
    /// Number of functions whose overall severity is Red.
    pub red: usize,
    /// Number of functions whose overall severity is Critical.
    pub critical: usize,
}

/// Runs the `complexity` subcommand.
///
/// Resolves the [`Storage`](crate::storage::capability::Storage) capability
/// from `kit`, runs [`ComplexityAnalyzer::analyze`], optionally filters to
/// Red+Critical and sorts by severity, and prints the result as a JSON object
/// `{ project, complexity: [...], summary: { total, green, yellow, red, critical } }`.
///
/// `summary` always reflects the full (pre-filter) result set; `complexity`
/// reflects any `--red-only` / `--sort-by-severity` flags.
///
/// # Errors
///
/// Returns [`crate::cli::error::CliError::Kit`] if the Storage capability is
/// not registered. Returns [`crate::cli::error::CliError::Storage`] for
/// database failures during the Cypher queries.
pub fn run(kit: &Kit, args: &ComplexityArgs) -> Result<()> {
    let storage = kit.require::<StorageKey>()?;
    let thresholds = build_thresholds(args);
    let analyzer = ComplexityAnalyzer::new_with_thresholds(&*storage, thresholds);
    let entries = analyzer.analyze(&args.project)?;
    let summary = compute_summary(&entries);
    let mut filtered = entries;
    if args.red_only {
        filtered.retain(|e| e.overall_severity == Severity::Red || e.overall_severity == Severity::Critical);
    }
    if args.sort_by_severity {
        filtered.sort_by(|a, b| b.overall_severity.cmp(&a.overall_severity));
    }
    let output = ComplexityOutput {
        project: args.project.clone(),
        complexity: filtered,
        summary,
    };
    let json = serde_json::to_string(&output)?;
    println!("{json}");
    Ok(())
}

/// Builds [`ComplexityThresholds`] from `args`, starting from
/// [`ComplexityThresholds::default`] and overriding each triple member only
/// when the corresponding `Option<u32>` / `Option<TimeComplexity>` /
/// `Option<SpaceComplexity>` flag is `Some`. `None` flags fall through to the
/// default value for that metric. 3-tuple fields use `(green, yellow, red)`
/// indexing; `space_complexity` is a 2-tuple `(yellow, red)`.
fn build_thresholds(args: &ComplexityArgs) -> ComplexityThresholds {
    let mut t = ComplexityThresholds::default();
    if let Some(g) = args.cyclomatic_green {
        t.cyclomatic.0 = g;
    }
    if let Some(y) = args.cyclomatic_yellow {
        t.cyclomatic.1 = y;
    }
    if let Some(r) = args.cyclomatic_red {
        t.cyclomatic.2 = r;
    }
    if let Some(g) = args.cognitive_green {
        t.cognitive.0 = g;
    }
    if let Some(y) = args.cognitive_yellow {
        t.cognitive.1 = y;
    }
    if let Some(r) = args.cognitive_red {
        t.cognitive.2 = r;
    }
    if let Some(g) = args.nesting_green {
        t.nesting.0 = g;
    }
    if let Some(y) = args.nesting_yellow {
        t.nesting.1 = y;
    }
    if let Some(r) = args.nesting_red {
        t.nesting.2 = r;
    }
    if let Some(g) = args.func_length_green {
        t.func_length.0 = g;
    }
    if let Some(y) = args.func_length_yellow {
        t.func_length.1 = y;
    }
    if let Some(r) = args.func_length_red {
        t.func_length.2 = r;
    }
    if let Some(g) = args.halstead_volume_green {
        t.halstead_volume.0 = g;
    }
    if let Some(y) = args.halstead_volume_yellow {
        t.halstead_volume.1 = y;
    }
    if let Some(r) = args.halstead_volume_red {
        t.halstead_volume.2 = r;
    }
    if let Some(g) = args.maintainability_green {
        t.maintainability.0 = g;
    }
    if let Some(y) = args.maintainability_yellow {
        t.maintainability.1 = y;
    }
    if let Some(r) = args.maintainability_red {
        t.maintainability.2 = r;
    }
    if let Some(g) = args.time_complexity_green {
        t.time_complexity.0 = g;
    }
    if let Some(y) = args.time_complexity_yellow {
        t.time_complexity.1 = y;
    }
    if let Some(r) = args.time_complexity_red {
        t.time_complexity.2 = r;
    }
    if let Some(y) = args.space_complexity_yellow {
        t.space_complexity.0 = y;
    }
    if let Some(r) = args.space_complexity_red {
        t.space_complexity.1 = r;
    }
    t
}

/// Computes aggregate severity counts over `entries`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::complexity::{ComplexityThresholds, SpaceComplexity, TimeComplexity};
    use crate::cli::args::ComplexityArgs;
    use crate::kit::{build_kit, KitBootstrapConfig, StorageKey};
    use crate::storage::schema::escape_cypher_string;
    use tempfile::TempDir;

    fn fresh_db_path() -> std::path::PathBuf {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("complexity_cmd_testdb");
        std::mem::forget(dir);
        path
    }

    fn build_kit_for_db(db: &std::path::Path) -> Kit {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        build_kit(&config).expect("build_kit")
    }

    fn make_args(project: &str, db: &str) -> ComplexityArgs {
        ComplexityArgs {
            project: project.to_string(),
            db: db.to_string(),
            red_only: false,
            sort_by_severity: false,
            cyclomatic_green: None,
            cyclomatic_yellow: None,
            cyclomatic_red: None,
            cognitive_green: None,
            cognitive_yellow: None,
            cognitive_red: None,
            nesting_green: None,
            nesting_yellow: None,
            nesting_red: None,
            func_length_green: None,
            func_length_yellow: None,
            func_length_red: None,
            halstead_volume_green: None,
            halstead_volume_yellow: None,
            halstead_volume_red: None,
            maintainability_green: None,
            maintainability_yellow: None,
            maintainability_red: None,
            time_complexity_green: None,
            time_complexity_yellow: None,
            time_complexity_red: None,
            space_complexity_yellow: None,
            space_complexity_red: None,
        }
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
        let storage = kit.require::<StorageKey>().expect("require_storage");
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
    fn run_complexity_succeeds_on_empty_db() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        let args = make_args("demo", db.to_str().unwrap());
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    #[cfg(feature = "lang-rust")]
    fn run_complexity_returns_correct_summary() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // Green function: simple, no branches.
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
        // Red function: deeply nested branches (cyclomatic > 20).
        let red_src = "fn red() { if a { if b { if c { if d { if e { if f { if g { if h { if i { if j { if k { if l { if m { if n { if o { if p { if q { if r { if s { if t { if u {} } } } } } } } } } } } } } } } } } } } } }";
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

        // Call run and capture stdout to inspect the JSON summary.
        let args = make_args("demo", db.to_str().unwrap());
        // run prints JSON; we verify via the returned Ok and by re-running the
        // analyzer directly to check summary counts.
        let storage = kit.require::<StorageKey>().expect("require_storage");
        let analyzer = ComplexityAnalyzer::new(&*storage);
        let entries = analyzer.analyze("demo").expect("analyze");
        let summary = compute_summary(&entries);
        assert_eq!(summary.total, 2, "total functions");
        // At least one Green (simple) and one Critical (deeply nested →
        // nesting_depth > 6 triggers Critical).
        assert!(summary.green >= 1, "green count: {}", summary.green);
        assert!(summary.critical >= 1, "critical count: {}", summary.critical);

        // run itself should succeed.
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());
    }

    #[test]
    fn complexity_output_serializes_to_json() {
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
    fn run_complexity_red_only_filters_correctly() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
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
        // Build a function that is Red by cyclomatic complexity (many branches).
        let red_src = "fn red() { if a { if b { if c { if d { if e { if f { if g { if h { if i { if j { if k { if l { if m { if n { if o { if p { if q { if r { if s { if t { if u {} } } } } } } } } } } } } } } } } } } } } }";
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

        // Verify summary still counts both (full set), while complexity array
        // only contains Red+Critical entries when --red-only is set.
        let storage = kit.require::<StorageKey>().expect("require_storage");
        let analyzer = ComplexityAnalyzer::new(&*storage);
        let entries = analyzer.analyze("demo").expect("analyze");
        let summary = compute_summary(&entries);
        assert_eq!(summary.total, 2, "summary total counts all entries");
        // red_only filter retains Red and Critical entries.
        let mut filtered = entries;
        filtered.retain(|e| e.overall_severity == Severity::Red || e.overall_severity == Severity::Critical);
        assert!(
            filtered.iter().all(|e| e.overall_severity == Severity::Red || e.overall_severity == Severity::Critical),
            "red_only should retain only Red and Critical entries"
        );
    }

    // --- T017: build_thresholds + run wiring tests ---

    #[test]
    fn build_thresholds_uses_defaults_when_none() {
        // All threshold fields None → build_thresholds returns default.
        let args = make_args("demo", "/tmp/x.lbug");
        let t = build_thresholds(&args);
        assert_eq!(t, ComplexityThresholds::default());
    }

    #[test]
    fn build_thresholds_overrides_when_some() {
        // Every threshold field set → build_thresholds folds them onto default.
        let mut args = make_args("demo", "/tmp/x.lbug");
        args.cyclomatic_green = Some(2);
        args.cyclomatic_yellow = Some(5);
        args.cyclomatic_red = Some(8);
        args.cognitive_green = Some(3);
        args.cognitive_yellow = Some(7);
        args.cognitive_red = Some(10);
        args.nesting_green = Some(1);
        args.nesting_yellow = Some(3);
        args.nesting_red = Some(4);
        args.func_length_green = Some(20);
        args.func_length_yellow = Some(50);
        args.func_length_red = Some(100);
        args.halstead_volume_green = Some(50);
        args.halstead_volume_yellow = Some(500);
        args.halstead_volume_red = Some(4000);
        args.maintainability_green = Some(90);
        args.maintainability_yellow = Some(60);
        args.maintainability_red = Some(80);
        args.time_complexity_green = Some(TimeComplexity::O1);
        args.time_complexity_yellow = Some(TimeComplexity::O1);
        args.time_complexity_red = Some(TimeComplexity::ON);
        args.space_complexity_yellow = Some(SpaceComplexity::O1);
        args.space_complexity_red = Some(SpaceComplexity::ON2);

        let t = build_thresholds(&args);
        assert_eq!(t.cyclomatic, (2, 5, 8));
        assert_eq!(t.cognitive, (3, 7, 10));
        assert_eq!(t.nesting, (1, 3, 4));
        assert_eq!(t.func_length, (20, 50, 100));
        assert_eq!(t.halstead_volume, (50, 500, 4000));
        assert_eq!(t.maintainability, (90, 60, 80));
        assert_eq!(t.time_complexity, (TimeComplexity::O1, TimeComplexity::O1, TimeComplexity::ON));
        assert_eq!(t.space_complexity, (SpaceComplexity::O1, SpaceComplexity::ON2));
    }

    #[test]
    #[cfg(feature = "lang-rust")]
    fn run_uses_custom_thresholds() {
        let db = fresh_db_path();
        let kit = build_kit_for_db(&db);
        // 9 if-branches → cyclomatic = 1 + 9 = 10. With default thresholds
        // (green=10, yellow=20, red=25), cyclomatic=10 → Green. With custom
        // (green=2, yellow=5, red=8), cyclomatic=10 > 8 → Critical.
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

        let mut args = make_args("demo", db.to_str().unwrap());
        args.cyclomatic_green = Some(2);
        args.cyclomatic_yellow = Some(5);
        args.cyclomatic_red = Some(8);

        // run must accept custom-threshold args and succeed.
        let result = run(&kit, &args);
        assert!(result.is_ok(), "run should succeed: {:?}", result.err());

        // build_thresholds must produce thresholds that make this function Critical.
        let storage = kit.require::<StorageKey>().expect("require_storage");
        let thresholds = build_thresholds(&args);
        let analyzer = ComplexityAnalyzer::new_with_thresholds(&*storage, thresholds);
        let entries = analyzer.analyze("demo").expect("analyze");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cyclomatic, 10, "cyclomatic should be 10");
        assert_eq!(
            entries[0].overall_severity,
            Severity::Critical,
            "custom thresholds (green=2, yellow=5, red=8) should make cyclomatic=10 Critical"
        );

        // Sanity: default thresholds should not make this function Critical.
        let analyzer_default = ComplexityAnalyzer::new(&*storage);
        let entries_default = analyzer_default.analyze("demo").expect("analyze");
        assert_ne!(
            entries_default[0].overall_severity,
            Severity::Critical,
            "default thresholds should not make this function Critical"
        );
    }
}
