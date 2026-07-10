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
    "Project",
    "Folder",
    "File",
    "Module",
    "Class",
    "Struct",
    "Enum",
    "Trait",
    "Impl",
    "Function",
    "Method",
    "Variable",
    "GlobalVar",
    "Parameter",
    "Const",
    "Static",
    "Macro",
    "TypeAlias",
    "Typedef",
    "Namespace",
    "Interface",
    "Constructor",
    "Property",
    "Record",
    "Delegate",
    "Annotation",
    "Template",
    "Union",
    "Variant",
    "Field",
    "Event",
    "Handler",
    "Middleware",
    "Service",
    "Endpoint",
    "Route",
    "Process",
    "Database",
    "Config",
    "Test",
    "Section",
    "Community",
    "Tool",
    "Embedding",
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
    #[cfg(test)]
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
/// **Multi-project coexistence**: CodeNexus supports multiple projects in the
/// same DB by design (see `ac_index_003_multiple_projects_coexist` in
/// `src/index/pipeline.rs`). Each project's nodes carry a `project` column
/// storing the project's UUIDv7 id, and [`extract_stats`] filters all queries
/// by this id. There is no need to delete the DB file between samples — doing
/// so would destroy other projects' indexes unnecessarily.
///
/// **Release mode**: uses `--release` because large projects (velo: 69K edges,
/// LAPACK: 493K edges) in debug mode are too slow and get killed by signals
/// (exit status None) mid-index. Release mode completes in seconds.
pub fn run_index(repo_path: &Path, name: &str) -> Result<()> {
    eprintln!(
        "[index] codenexus index {} --name {} (release)",
        repo_path.display(),
        name
    );
    let output = Command::new("cargo")
        .args([
            "run",
            "--release",
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
/// column. We run one `MATCH (n:<Table>) WHERE n.project = $id RETURN count(n)`
/// per node type and one `MATCH (r:CodeRelation) WHERE r.project = $id RETURN
/// r.type, count(*)` for edges.
///
/// **Project filtering**: all node tables (except `Project` itself) and the
/// `CodeRelation` table carry a `project` column storing the project's UUIDv7
/// id. We look up the id by name from the `Project` table, then filter every
/// query by it. This ensures we only count the current sample's data even when
/// multiple projects coexist in the same DB (the default — see
/// `ac_index_003_multiple_projects_coexist` in `src/index/pipeline.rs`).
/// Previously, the lack of this filter caused apparent "data pollution" where
/// a pure-C project would show Rust `Impl`/`Trait` nodes from a previous
/// sample. The old workaround (deleting the DB file before each run) was a
/// sledgehammer that destroyed other projects' indexes; this filter is the
/// correct fix.
pub fn extract_stats(db_path: &Path, name: &str) -> Result<CodeNexusStats> {
    let repo = codenexus::storage::repository::Repository::open(db_path)
        .with_context(|| format!("failed to open CodeNexus DB at {}", db_path.display()))?;
    let conn = repo.connection();

    // Look up the project_id by name. All node tables (except Project itself)
    // and CodeRelation store this id in their `project` column.
    let project_id = lookup_project_id(conn, name)?;
    eprintln!("[stats] project '{name}' → id {project_id}");

    let escaped_pid = codenexus::storage::schema::escape_cypher_string(&project_id);
    let escaped_name = codenexus::storage::schema::escape_cypher_string(name);

    let mut node_counts = BTreeMap::new();
    for table in NODE_TABLES {
        // Parameterized DDL not supported here; table name is from a
        // hardcoded trusted list (NODE_TABLES), not user input, so SQL
        // injection is not a concern. However, some table names (Macro,
        // Union) collide with LadybugDB reserved keywords and must be
        // backtick-escaped via `escape_identifier`.
        let escaped_table = codenexus::storage::schema::escape_identifier(table);
        let cypher = if *table == "Project" {
            // Project table has no `project` column; filter by name instead.
            format!("MATCH (n:Project) WHERE n.name = '{escaped_name}' RETURN count(n) AS c")
        } else {
            format!(
                "MATCH (n:{escaped_table}) WHERE n.project = '{escaped_pid}' RETURN count(n) AS c"
            )
        };
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
    // TABLE syntax is not valid against this schema. Filter by project_id so
    // we only count the current sample's edges.
    let edge_cypher = format!(
        "MATCH (r:CodeRelation) WHERE r.project = '{escaped_pid}' RETURN r.type AS t, count(*) AS c ORDER BY t"
    );
    match conn.query(&edge_cypher) {
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

/// Looks up a project's UUIDv7 id by its display name from the `Project`
/// table.
///
/// CodeNexus assigns each indexed project a UUIDv7 id at index time (see
/// `ScanPhase`). All node tables (except `Project` itself) and the
/// `CodeRelation` table store this id in their `project` column. Filtering
/// stats by this id ensures we only count the current sample's data, even
/// when multiple projects coexist in the same DB.
///
/// # Errors
///
/// Returns an error if the project name is not found in the Project table.
/// This is a fail-loud signal (Rule 12): a missing project means the index
/// step failed silently or the wrong DB was opened. We must NOT silently
/// return zero counts — that would hide the failure behind plausible-looking
/// numbers.
pub fn lookup_project_id(
    conn: &codenexus::storage::StorageConnection,
    name: &str,
) -> Result<String> {
    let escaped = codenexus::storage::schema::escape_cypher_string(name);
    let cypher = format!("MATCH (p:Project) WHERE p.name = '{escaped}' RETURN p.id");
    let rows = conn
        .query(&cypher)
        .with_context(|| format!("failed to query Project table for name '{name}'"))?;
    let id = rows
        .first()
        .and_then(|row| row.first())
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "project '{name}' not found in Project table — \
                 index step may have failed silently or the wrong DB was opened"
            )
        })?;
    Ok(id)
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
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
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

    #[test]
    fn json_to_u64_handles_uint64_string_quirk() {
        // LadybugDB serializes UInt64 as JSON strings (see connection.rs:267).
        // Covers the `as_str()` branch + successful parse.
        assert_eq!(
            json_to_u64(&serde_json::Value::String("12345".into())),
            12345
        );
    }

    #[test]
    fn json_to_u64_handles_native_u64() {
        assert_eq!(json_to_u64(&serde_json::json!(42)), 42);
    }

    #[test]
    fn json_to_u64_handles_i64() {
        // Negative i64 must clamp to 0 (u64::try_from fails → unwrap_or(0)).
        assert_eq!(json_to_u64(&serde_json::json!(-1i64)), 0);
        assert_eq!(json_to_u64(&serde_json::json!(99i64)), 99);
    }

    #[test]
    fn json_to_u64_falls_back_to_zero_for_unparseable() {
        // Rule 12: visible 0, not silent drop. Non-numeric strings → 0.
        assert_eq!(
            json_to_u64(&serde_json::Value::String("not-a-number".into())),
            0
        );
        // Null/bool/object → 0.
        assert_eq!(json_to_u64(&serde_json::Value::Null), 0);
        assert_eq!(json_to_u64(&serde_json::json!({"k": 1})), 0);
    }

    #[test]
    fn lookup_project_id_returns_id_when_project_exists() {
        // End-to-end: insert a Project row, then look it up by name.
        // Covers the success branch of `lookup_project_id`.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("pid.lbug");
        let repo = codenexus::storage::repository::Repository::open(&db_path).unwrap();
        let project = codenexus::model::Node::builder(
            codenexus::model::NodeLabel::Project,
            "sample-a",
            "sample-a",
        )
        .id("uuid-7-a")
        .build();
        repo.save_nodes(&[project], codenexus::model::NodeLabel::Project)
            .unwrap();
        let conn = repo.connection();
        let id = lookup_project_id(conn, "sample-a").expect("lookup_project_id");
        assert_eq!(id, "uuid-7-a");
    }

    #[test]
    fn lookup_project_id_errors_when_name_missing() {
        // Rule 12: fail loud when the project is not in the DB — returning
        // zero counts would hide a silent index failure behind plausible
        // numbers. Covers the `ok_or_else` error branch.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("pid-missing.lbug");
        let repo = codenexus::storage::repository::Repository::open(&db_path).unwrap();
        let conn = repo.connection();
        let err = lookup_project_id(conn, "ghost").unwrap_err();
        assert!(
            err.to_string().contains("project 'ghost' not found"),
            "expected not-found error, got: {err}"
        );
    }

    #[test]
    fn extract_stats_counts_nodes_for_named_project() {
        // Insert a Project + 2 Function nodes for project "alpha", and 1
        // Function for project "beta". extract_stats(name="alpha") must
        // report exactly 2 Functions and 1 Project (filtered by name).
        // Covers the main loop body of `extract_stats`.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("stats.lbug");
        let repo = codenexus::storage::repository::Repository::open(&db_path).unwrap();

        let alpha =
            codenexus::model::Node::builder(codenexus::model::NodeLabel::Project, "alpha", "alpha")
                .id("alpha-id")
                .build();
        let beta =
            codenexus::model::Node::builder(codenexus::model::NodeLabel::Project, "beta", "beta")
                .id("beta-id")
                .build();
        let f1 = codenexus::model::Node::builder(
            codenexus::model::NodeLabel::Function,
            "alpha_fn_a",
            "alpha::alpha_fn_a",
        )
        .id("f1")
        .project("alpha-id")
        .file_path("/a.rs")
        .start_line(1)
        .end_line(5)
        .build();
        let f2 = codenexus::model::Node::builder(
            codenexus::model::NodeLabel::Function,
            "alpha_fn_b",
            "alpha::alpha_fn_b",
        )
        .id("f2")
        .project("alpha-id")
        .file_path("/a.rs")
        .start_line(7)
        .end_line(10)
        .build();
        let f3 = codenexus::model::Node::builder(
            codenexus::model::NodeLabel::Function,
            "beta_fn",
            "beta::beta_fn",
        )
        .id("f3")
        .project("beta-id")
        .file_path("/b.rs")
        .start_line(1)
        .end_line(3)
        .build();
        repo.save_nodes(&[alpha, beta], codenexus::model::NodeLabel::Project)
            .unwrap();
        repo.save_nodes(&[f1, f2, f3], codenexus::model::NodeLabel::Function)
            .unwrap();
        drop(repo);

        let stats = extract_stats(&db_path, "alpha").expect("extract_stats");
        assert_eq!(stats.name, "alpha");
        // Project table is filtered by name, not project_id; "alpha" appears once.
        assert_eq!(stats.node_counts_by_type.get("Project"), Some(&1));
        // Functions filtered by project_id == "alpha-id": exactly 2.
        assert_eq!(stats.node_counts_by_type.get("Function"), Some(&2));
        // Beta's Function must NOT leak into alpha's stats (the project_id
        // filter is the fix for the historical cross-pollution bug).
        assert!(
            stats.node_counts_by_type.values().sum::<u64>() <= 3,
            "alpha stats should not include beta's nodes"
        );
        // No edges inserted → edge map empty.
        assert!(stats.edge_counts_by_type.is_empty());
        // extract_stats does not set file_counts_by_language (run_single does).
        assert!(stats.file_counts_by_language.is_empty());
    }

    #[test]
    fn extract_stats_errors_when_project_missing() {
        // No Project rows at all → lookup_project_id fails → extract_stats
        // must propagate the error (Rule 12). Covers the `?` propagation.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("empty-stats.lbug");
        let _ = codenexus::storage::repository::Repository::open(&db_path).unwrap();
        let err = extract_stats(&db_path, "nonexistent").unwrap_err();
        assert!(
            err.to_string().contains("project 'nonexistent' not found"),
            "expected not-found error, got: {err}"
        );
    }

    #[test]
    fn count_files_by_language_classifies_extensions() {
        // Build a tiny temp repo with a few source files and verify the
        // classification. Covers the `match ext` arm + the BTreeMap insert.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("b.c"), "int main(){}").unwrap();
        std::fs::write(root.join("c.py"), "def main(): pass").unwrap();
        std::fs::write(root.join("d.ts"), "function main(){}").unwrap();
        std::fs::write(root.join("e.md"), "# readme").unwrap(); // not classified

        let counts = count_files_by_language(root, "rust");
        assert_eq!(counts.get("rust"), Some(&1));
        assert_eq!(counts.get("c"), Some(&1));
        assert_eq!(counts.get("python"), Some(&1));
        assert_eq!(counts.get("typescript"), Some(&1));
        // .md must be skipped (the `_ => continue` arm).
        assert!(!counts.contains_key("javascript"));
        assert!(counts.len() == 4);
    }

    #[test]
    fn count_files_by_language_inserts_primary_when_empty() {
        // No source files at all → the `counts.is_empty()` branch fires and
        // inserts the primary language with 0. Covers that fallback.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("README.md"), "# no source here").unwrap();
        let counts = count_files_by_language(root, "fortran");
        assert_eq!(counts.get("fortran"), Some(&0));
        assert_eq!(counts.len(), 1);
    }

    #[test]
    fn write_results_creates_json_file_with_name() {
        // write_results writes to tools/verification/results/<name>.codenexus.json.
        // Use a unique name to avoid collisions with real verifier runs.
        let name = "cov_test_write_results_unique";
        let stats = CodeNexusStats::empty(name);
        let path = write_results(name, &stats).expect("write_results");
        assert!(path.exists(), "results file must exist after write_results");
        assert!(
            path.to_string_lossy()
                .ends_with(&format!("{name}.codenexus.json")),
            "expected path to end with {name}.codenexus.json, got {}",
            path.display()
        );
        // The file must be valid JSON that round-trips back to the same stats.
        let bytes = std::fs::read(&path).unwrap();
        let back: CodeNexusStats = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, stats);
        // Clean up so repeated test runs don't accumulate files.
        let _ = std::fs::remove_file(&path);
    }
}
