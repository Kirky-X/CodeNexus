//! CodeNexus index invocation + stats extraction (tasks 3.1-3.4).
//!
//! - `run_index`: invoke `cargo run --bin codenexus -- index` via subprocess
//! - `extract_stats`: open the LadybugDB with `codenexus::storage::Repository`,
//!   run per-table `SELECT count(*)` for each of the 44 node types, and
//!   `SELECT type, count(*) GROUP BY type` on CodeRelation for edge counts
//! - `write_results`: serialize stats to `results/<name>.codenexus.json`
//! - Empty repo → zero-count JSON; index failure → non-zero exit

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// All 44 CodeNexus node table names (matches `NodeLabel::as_str` order).
/// Hardcoded so the verifier does not need to depend on CodeNexus internals
/// beyond `Repository::open` + raw SQL.
const NODE_TABLES: &[&str] = &[
    "Project", "Folder", "File", "Module", "Class", "Struct", "Enum", "Trait",
    "Impl", "Function", "Method", "Variable", "GlobalVar", "Parameter", "Const",
    "Static", "Macro", "TypeAlias", "Typedef", "Namespace", "Interface",
    "Constructor", "Property", "Record", "Delegate", "Annotation", "Template",
    "Union", "Variant", "Field", "Event", "Handler", "Middleware", "Service",
    "Endpoint", "Route", "Process", "Database", "Config", "Test", "Section",
    "Community", "Tool", "Embedding",
];

/// Stats extracted from a CodeNexus LadybugDB index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeNexusStats {
    pub name: String,
    pub node_counts_by_type: BTreeMap<String, u64>,
    pub edge_counts_by_type: BTreeMap<String, u64>,
    pub file_counts_by_language: BTreeMap<String, u64>,
}

impl CodeNexusStats {
    /// Build a zero-filled stats object (for empty-repo scenario).
    pub fn empty(name: &str) -> Self {
        Self {
            name: name.to_string(),
            node_counts_by_type: BTreeMap::new(),
            edge_counts_by_type: BTreeMap::new(),
            file_counts_by_language: BTreeMap::new(),
        }
    }
}

/// Default DB path used by `codenexus index` (matches CLI args.rs default).
const DEFAULT_DB: &str = "./codenexus.lbug";

/// `single` subcommand entry point — CodeNexus side only. Runs the full
/// index → extract → write flow for one repo and returns the stats so the
/// caller (main.rs orchestrator) can pass them to the report generator.
///
/// When `resume` is true: if the JSON results file exists, load it directly;
/// else if the DB file exists, extract stats from it without re-indexing;
/// else fall through to a full index run.
pub fn run_single(repo: &Path, name: &str, language: &str, resume: bool) -> Result<CodeNexusStats> {
    let results_dir = Path::new("tools/verification/results");
    std::fs::create_dir_all(results_dir)
        .with_context(|| format!("failed to create {}", results_dir.display()))?;

    let codenexus_json = results_dir.join(format!("{name}.codenexus.json"));
    let db_path = Path::new(DEFAULT_DB);

    let stats = if resume && codenexus_json.exists() {
        eprintln!("[resume] loading existing {codenexus_json:?}");
        serde_json::from_slice(&std::fs::read(&codenexus_json)?)?
    } else if resume && db_path.exists() {
        eprintln!("[resume] DB exists at {db_path:?}, extracting stats without re-indexing");
        let mut stats = extract_stats(db_path, name)?;
        stats.file_counts_by_language = count_files_by_language(repo, language);
        stats
    } else {
        // Task 3.1: run index
        run_index(repo, name)?;

        // Task 3.2: extract stats
        let mut stats = extract_stats(db_path, name)?;
        stats.file_counts_by_language = count_files_by_language(repo, language);
        stats
    };

    // Task 3.3: write results
    let path = write_results(name, &stats)?;
    eprintln!("[ok] wrote {path:?}");
    Ok(stats)
}

/// Task 3.1: Invoke `cargo run --bin codenexus -- index <repo> --name <name>`.
///
/// Uses `cargo run` rather than a bare `codenexus` binary because the latter
/// is not installed in PATH in this environment. Captures stdout/stderr and
/// returns an explicit error on non-zero exit (Rule 12: fail loud).
///
/// **Database isolation**: deletes `./codenexus.lbug` before each index run
/// to prevent cross-project data pollution. Without this, all 8 batch samples
/// share the same DB file, and later samples inherit nodes/edges from earlier
/// ones (e.g. a pure-C project would show Rust `Impl`/`Trait` nodes from a
/// previous Rust sample). The `--force` flag alone does NOT clear other
/// projects' data — it only forces a full re-index of the *current* project.
pub fn run_index(repo_path: &Path, name: &str) -> Result<()> {
    // Delete the DB file to ensure a clean slate for this sample.
    let db_path = Path::new(DEFAULT_DB);
    if db_path.exists() {
        eprintln!("[index] removing existing DB at {db_path:?} to prevent cross-project pollution");
        std::fs::remove_file(db_path)
            .with_context(|| format!("failed to remove old DB at {}", db_path.display()))?;
    }

    eprintln!("[index] codenexus index {} --name {}", repo_path.display(), name);
    let output = Command::new("cargo")
        .args([
            "run",
            "--bin",
            "codenexus",
            "--",
            "index",
            repo_path.to_str().expect("repo_path is valid UTF-8"),
            "--name",
            name,
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .context("failed to spawn `cargo run --bin codenexus`")?;

    if !output.status.success() {
        // Rule 12: surface stderr explicitly, do not swallow.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        anyhow::bail!(
            "codenexus index failed (exit {:?})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
            output.status.code()
        );
    }

    // Print index stdout for visibility but do not treat as error.
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.is_empty() {
        eprintln!("[index] {stdout}");
    }
    Ok(())
}

/// Task 3.2: Open the LadybugDB index and extract node/edge counts by type.
///
/// CodeNexus stores each node type in a separate table (e.g. `Function`,
/// `Class`, ...) and all edges in a single `CodeRelation` table with a `type`
/// column. We run one `SELECT count(*) FROM <Table>` per node type and one
/// `SELECT type, count(*) FROM CodeRelation GROUP BY type` for edges.
pub fn extract_stats(db_path: &Path, name: &str) -> Result<CodeNexusStats> {
    let repo = codenexus::storage::repository::Repository::open(db_path)
        .with_context(|| format!("failed to open CodeNexus DB at {}", db_path.display()))?;
    let conn = repo.connection();

    let mut node_counts = BTreeMap::new();
    for table in NODE_TABLES {
        // Parameterized DDL not supported here; table name is from a
        // hardcoded trusted list (NODE_TABLES), not user input, so SQL
        // injection is not a concern.
        let cypher = format!("MATCH (n:{table}) RETURN count(n) AS c");
        match conn.query(&cypher) {
            Ok(rows) => {
                let raw = rows.first().and_then(|row| row.first());
                let count = raw.map(json_to_u64).unwrap_or(0);
                if *table == "Function" {
                    eprintln!("[debug] Function count raw value: {raw:?} → {count}");
                }
                if count > 0 {
                    node_counts.insert((*table).to_string(), count);
                }
            }
            Err(e) => {
                // Table may not exist if the label was never created; surface
                // as a warning but do not abort (Rule 12: visible, not fatal).
                eprintln!("[warn] count for {table} failed: {e}");
            }
        }
    }

    let mut edge_counts = BTreeMap::new();
    // CodeNexus stores CodeRelation as a NODE TABLE (not a REL TABLE), so we
    // must use `MATCH (r:CodeRelation)` — the `()-[r:CodeRelation]->()` REL
    // TABLE syntax is not valid against this schema.
    match conn.query("MATCH (r:CodeRelation) RETURN r.type AS t, count(*) AS c ORDER BY t") {
        Ok(rows) => {
            for row in &rows {
                if row.len() < 2 {
                    continue;
                }
                let t = row[0].as_str().unwrap_or("(unknown)").to_string();
                let c = json_to_u64(&row[1]);
                edge_counts.insert(t, c);
            }
        }
        Err(e) => {
            eprintln!("[warn] edge count query failed: {e}");
        }
    }

    Ok(CodeNexusStats {
        name: name.to_string(),
        node_counts_by_type: node_counts,
        edge_counts_by_type: edge_counts,
        file_counts_by_language: BTreeMap::new(),
    })
}

/// Convert a `serde_json::Value` to `u64`, handling the LadybugDB quirk where
/// `UInt64` values are serialized as JSON strings (see `connection.rs:267`).
/// Falls back to 0 on unparseable input (Rule 12: visible — caller sees 0
/// count in the report, which surfaces the gap rather than hiding it).
fn json_to_u64(v: &serde_json::Value) -> u64 {
    if let Some(u) = v.as_u64() {
        return u;
    }
    if let Some(i) = v.as_i64() {
        return u64::try_from(i).unwrap_or(0);
    }
    if let Some(s) = v.as_str() {
        return s.parse().unwrap_or(0);
    }
    0
}

/// Count source files by language based on file extensions in the repo.
/// (Task 3.2 sub-requirement: `file_counts_by_language`.)
fn count_files_by_language(repo_path: &Path, primary_lang: &str) -> BTreeMap<String, u64> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let walker = ignore::WalkBuilder::new(repo_path)
        .hidden(true)
        .git_ignore(true)
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let lang = match ext {
            "rs" => "rust",
            "c" | "h" => "c",
            "f" | "f90" | "f95" | "for" | "f03" | "f08" => "fortran",
            "py" => "python",
            "ts" | "tsx" => "typescript",
            "js" | "jsx" => "javascript",
            _ => continue,
        };
        *counts.entry(lang.to_string()).or_insert(0) += 1;
    }
    if counts.is_empty() {
        // Ensure the primary language at least appears with 0 if no files found.
        counts.insert(primary_lang.to_string(), 0);
    }
    counts
}

/// Task 3.3: Write stats to `tools/verification/results/<name>.codenexus.json`.
pub fn write_results(name: &str, stats: &CodeNexusStats) -> Result<PathBuf> {
    let dir = Path::new("tools/verification/results");
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{name}.codenexus.json"));
    let json = serde_json::to_string_pretty(stats)?;
    std::fs::write(&path, json + "\n")
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stats_has_no_counts() {
        let s = CodeNexusStats::empty("test");
        assert!(s.node_counts_by_type.is_empty());
        assert!(s.edge_counts_by_type.is_empty());
        assert!(s.file_counts_by_language.is_empty());
        assert_eq!(s.name, "test");
    }

    #[test]
    fn node_tables_has_44_entries() {
        assert_eq!(NODE_TABLES.len(), 44, "CodeNexus defines 44 node types");
    }

    #[test]
    fn node_tables_cover_expected_types() {
        assert!(NODE_TABLES.contains(&"Function"));
        assert!(NODE_TABLES.contains(&"Class"));
        assert!(NODE_TABLES.contains(&"Struct"));
        assert!(NODE_TABLES.contains(&"Trait"));
        assert!(NODE_TABLES.contains(&"Impl"));
        assert!(NODE_TABLES.contains(&"Embedding"));
    }

    #[test]
    fn write_and_read_results_json() {
        // In-memory JSON roundtrip: covers serde Serialize/Deserialize impls
        // for CodeNexusStats. End-to-end file write is exercised in task 8.1.
        let stats = CodeNexusStats {
            name: "demo".to_string(),
            node_counts_by_type: {
                let mut m = BTreeMap::new();
                m.insert("Function".to_string(), 42);
                m
            },
            edge_counts_by_type: {
                let mut m = BTreeMap::new();
                m.insert("CALLS".to_string(), 100);
                m
            },
            file_counts_by_language: {
                let mut m = BTreeMap::new();
                m.insert("rust".to_string(), 5);
                m
            },
        };
        let json = serde_json::to_string_pretty(&stats).unwrap();
        let read_back: CodeNexusStats = serde_json::from_str(&json).unwrap();
        assert_eq!(read_back, stats);
    }
}
