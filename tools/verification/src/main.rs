//! codenexus-verify — Cross-validation harness for CodeNexus parsing.
//!
//! Indexes a target repo with CodeNexus, fetches the gitnexus reference index
//! for the same repo, compares node/edge counts and query result sets, and
//! emits a Markdown diff report. See
//! `openspec/changes/cross-validate-parsing-with-gitnexus/` for the full spec.

mod codenexus_stats;
mod gitnexus_client;
mod query_compare;
mod report;
mod type_map;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use query_compare::{QueryDiff, Side};
use type_map::TypeMap;

/// Cross-validate CodeNexus parsing against gitnexus reference indexes.
#[derive(Parser, Debug)]
#[command(name = "codenexus-verify", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Index a single repo with CodeNexus, fetch gitnexus reference, compare, report.
    Single {
        /// Path to the target repository root.
        #[arg(long)]
        repo: PathBuf,

        /// Project name (used as the CodeNexus index key and gitnexus repo name).
        #[arg(long)]
        name: String,

        /// Primary language of the repo (c / rust / fortran / python / typescript).
        #[arg(long)]
        language: String,

        /// Skip CodeNexus indexing if results/<name>.codenexus.json already exists.
        #[arg(long)]
        resume: bool,
    },

    /// Run the full index-compare-report flow over every entry in a corpus JSON.
    Batch {
        /// Path to samples.json.
        #[arg(long, default_value = "tools/verification/samples.json")]
        corpus: PathBuf,

        /// Skip CodeNexus indexing for samples that already have a results JSON.
        #[arg(long)]
        resume: bool,

        /// Run only the named sample (repeatable).
        #[arg(long = "only", action = clap::ArgAction::Append)]
        only: Vec<String>,
    },

    /// Clone/checkout sample repos listed in samples.json.
    FetchSamples {
        /// Path to samples.json.
        #[arg(long, default_value = "tools/verification/samples.json")]
        corpus: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Single {
            repo,
            name,
            language,
            resume,
        } => run_single_orchestrator(&repo, &name, &language, resume),
        Command::Batch {
            corpus,
            resume,
            only,
        } => run_batch(&corpus, resume, &only),
        Command::FetchSamples { corpus } => run_fetch_samples(&corpus),
    }
}

/// Default DB path used by `codenexus index` (matches CLI args.rs default).
const DEFAULT_DB: &str = "./codenexus.lbug";

/// Default type map path.
const TYPE_MAP_PATH: &str = "tools/verification/type_map.json";

/// Default queries directory.
const QUERIES_DIR: &str = "tools/verification/queries";

/// Task 8.1: Full single-sample orchestrator.
///
/// Wires together: CodeNexus index+extract → gitnexus reference fetch →
/// query comparison → report generation. Produces three output files:
/// - `results/<name>.codenexus.json`
/// - `results/<name>.gitnexus.json`
/// - `results/<name>.report.md`
fn run_single_orchestrator(
    repo: &Path,
    name: &str,
    language: &str,
    resume: bool,
) -> Result<()> {
    // 1. CodeNexus side: index + extract + write codenexus.json
    let codenexus_stats = codenexus_stats::run_single(repo, name, language, resume)?;

    // 2. gitnexus side: fetch reference + write gitnexus.json
    let gitnexus_stats = gitnexus_client::fetch_reference(name)?;
    let gn_path = gitnexus_client::write_reference(name, &gitnexus_stats)?;
    eprintln!("[ok] wrote {gn_path:?}");

    // 3. Load type map
    let type_map = TypeMap::load(&PathBuf::from(TYPE_MAP_PATH))?;

    // 4. Run all 8 query comparisons
    let query_diffs = run_query_comparisons(name)?;

    // 5. Generate + write report
    let markdown = report::generate_report(
        name,
        &codenexus_stats,
        &gitnexus_stats,
        &query_diffs,
        &type_map,
    );
    let report_path = report::write_report(name, &markdown)?;
    eprintln!("[ok] wrote {report_path:?}");

    // Print summary
    let overall_pass = query_diffs.iter().all(|(_, d)| matches!(d, QueryDiff::Match { .. }));
    eprintln!(
        "[done] {} — overall {}",
        name,
        if overall_pass { "PASS" } else { "FAIL" }
    );
    Ok(())
}

/// Execute all .cql files in the queries directory against both sides and
/// collect the diffs.
fn run_query_comparisons(name: &str) -> Result<Vec<(String, QueryDiff)>> {
    let queries_dir = Path::new(QUERIES_DIR);
    let db_path = Path::new(DEFAULT_DB);

    // Look up the CodeNexus project_id by name so CQL queries can filter
    // by `__PID__` to prevent cross-project data contamination (project
    // memory hard constraint).
    let repo = codenexus::storage::repository::Repository::open(db_path)
        .with_context(|| format!("failed to open CodeNexus DB at {}", db_path.display()))?;
    let project_id = codenexus_stats::lookup_project_id(repo.connection(), name)?;
    eprintln!("[query] project '{name}' → id {project_id}");

    let mut cql_files: Vec<PathBuf> = std::fs::read_dir(queries_dir)
        .with_context(|| format!("failed to read queries dir {}", queries_dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |ext| ext == "cql"))
        .collect();
    cql_files.sort();

    let mut diffs = Vec::with_capacity(cql_files.len());
    for cql_path in &cql_files {
        let query_name = cql_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        eprintln!("[query] {query_name}");

        let content = std::fs::read_to_string(cql_path)
            .with_context(|| format!("failed to read {}", cql_path.display()))?;

        let cn_cql = query_compare::extract_query_for_side(&content, Side::Codenexus)?;
        let gn_cql = query_compare::extract_query_for_side(&content, Side::Gitnexus)?;

        let cn_results = query_compare::execute_codenexus_query(db_path, &cn_cql, Some(&project_id))
            .with_context(|| format!("CodeNexus query failed for {query_name}"))?;
        let gn_results = query_compare::execute_gitnexus_query(name, &gn_cql)
            .with_context(|| format!("gitnexus query failed for {query_name}"))?;

        let diff = query_compare::compare_query_results(&cn_results, &gn_results);
        eprintln!(
            "  CodeNexus={}, gitnexus={}, diff={}",
            cn_results.len(),
            gn_results.len(),
            match &diff {
                QueryDiff::Match { count } => format!("MATCH({count})"),
                QueryDiff::CriticalDiff { missing_in_codenexus, missing_in_gitnexus } =>
                    format!("CRITICAL(missing_cn={}, missing_gn={})", missing_in_codenexus.len(), missing_in_gitnexus.len()),
            }
        );
        diffs.push((query_name, diff));
    }
    Ok(diffs)
}

/// Batch dispatcher (task 8.7).
fn run_batch(corpus: &PathBuf, resume: bool, only: &[String]) -> Result<()> {
    let content = std::fs::read_to_string(corpus)
        .with_context(|| format!("failed to read corpus {}", corpus.display()))?;
    let corpus: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse corpus JSON {}", corpus.display()))?;

    let samples = corpus
        .get("samples")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("corpus JSON missing `samples` array"))?;

    let mut summaries = Vec::new();

    for sample in samples {
        let name = sample
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("sample missing `name`"))?;
        let language = sample
            .get("language")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("sample missing `language`"))?;
        let repo_path = sample
            .get("repo_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("sample missing `repo_path`"))?;

        if !only.is_empty() && !only.iter().any(|o| o == name) {
            eprintln!("[skip] {name} (not in --only filter)");
            continue;
        }

        eprintln!("\n=== batch: {name} ({language}) ===");
        match run_single_orchestrator(Path::new(repo_path), name, language, resume) {
            Ok(()) => {
                // Read the generated report to extract severity counts.
                let report_path = Path::new("tools/verification/results")
                    .join(format!("{name}.report.md"));
                let severities = read_severity_counts(&report_path);
                let overall_pass = severities.critical == 0;
                summaries.push(report::SampleSummary {
                    name: name.to_string(),
                    language: language.to_string(),
                    overall_pass,
                    severities,
                });
            }
            Err(e) => {
                eprintln!("[error] {name} failed: {e:#}");
                summaries.push(report::SampleSummary {
                    name: name.to_string(),
                    language: language.to_string(),
                    overall_pass: false,
                    severities: report::SeverityCounts {
                        critical: 1,
                        major: 0,
                        minor: 0,
                    },
                });
            }
        }
    }

    let aggregate_md = report::generate_aggregate_report(&summaries);
    let agg_path = report::write_aggregate_report(&aggregate_md)?;
    eprintln!("\n[done] aggregate report: {agg_path:?}");
    Ok(())
}

/// Read a per-sample report file and extract severity counts from the Summary table.
fn read_severity_counts(report_path: &Path) -> report::SeverityCounts {
    let content = std::fs::read_to_string(report_path).unwrap_or_default();
    let mut critical = 0usize;
    let mut major = 0usize;
    let mut minor = 0usize;
    for line in content.lines() {
        if line.contains("Critical discrepancies") {
            if let Some(val) = line.split('|').nth(2) {
                critical = val.trim().parse().unwrap_or(0);
            }
        }
        if line.contains("Major discrepancies") {
            if let Some(val) = line.split('|').nth(2) {
                major = val.trim().parse().unwrap_or(0);
            }
        }
        if line.contains("Minor discrepancies") {
            if let Some(val) = line.split('|').nth(2) {
                minor = val.trim().parse().unwrap_or(0);
            }
        }
    }
    report::SeverityCounts { critical, major, minor }
}

/// Fetch-samples subcommand: delegates to the bash script.
fn run_fetch_samples(corpus: &Path) -> Result<()> {
    let script = Path::new("tools/verification/fetch_samples.sh");
    if !script.exists() {
        anyhow::bail!("fetch_samples.sh not found at {}", script.display());
    }
    let status = StdCommand::new("bash")
        .arg(script)
        .arg(corpus)
        .status()
        .context("failed to spawn fetch_samples.sh")?;
    if !status.success() {
        anyhow::bail!("fetch_samples.sh exited with {status}");
    }
    Ok(())
}
