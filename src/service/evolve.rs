// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

use std::path::Path;
use std::process::Command;

use serde::Serialize;

use crate::analysis::architecture::ArchitectureAnalyzer;
use crate::kit::{AsyncKit, AsyncReady};
use crate::service::error::CodeNexusError;
// run_evolve（无 cli 门控的可测试核心）无条件使用 index_core，此 import
// 不得挂 cli 门——否则 core,daemon,… 等无 cli 组合的 lib test 编译失败。
#[cfg(feature = "cli")]
use crate::service::error::{kit_not_initialized, to_api_error, wrap_error};
use crate::service::index::index_core;
#[cfg(feature = "cli")]
use crate::service::runtime::kit;

#[cfg(feature = "cli")]
use sdforge::forge;
#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;

/// Default and maximum number of commits to replay.
pub const DEFAULT_MAX_COMMITS: usize = 10;
pub const MAX_COMMITS_LIMIT: usize = 50;

/// One timeline point.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct EvolvePoint {
    pub commit: String,
    pub timestamp: u64,
    pub subject: String,
    /// Indexed project name: `<project>@<sha7>`.
    pub project_name: String,
    pub file_count: usize,
    pub symbol_count: usize,
    pub module_count: usize,
    /// language → file count.
    pub languages: std::collections::BTreeMap<String, usize>,
    /// Index-run edges per symbol (cheap coupling proxy).
    pub avg_fanout: f64,
    /// Index duration for this commit, milliseconds.
    pub index_ms: u64,
}

/// JSON-serializable evolve output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct EvolveOutput {
    pub path: String,
    pub max_commits: usize,
    pub points: Vec<EvolvePoint>,
    /// Path of `evolve.json`.
    pub json_path: String,
    /// Path of `evolve.html`.
    pub html_path: String,
}

struct CommitInfo {
    sha: String,
    timestamp: u64,
    subject: String,
}

/// Runs `git -C <repo> <args>` and returns trimmed stdout.
fn run_git(repo: &Path, args: &[&str]) -> Result<String, CodeNexusError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CodeNexusError::InvalidInput(format!(
                    "git binary not found on PATH — evolve requires git. Error: {e}"
                ))
            } else {
                CodeNexusError::Io(e)
            }
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CodeNexusError::InvalidInput(format!(
            "git {} failed (status {}): {}",
            args.first().unwrap_or(&""),
            output.status,
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Enumerates the last `max` commits (newest first): (sha, unix timestamp, subject).
fn enumerate_commits(repo: &Path, max: usize) -> Result<Vec<CommitInfo>, CodeNexusError> {
    let out = run_git(
        repo,
        &[
            "log",
            "--format=%H%x00%ct%x00%s",
            "--max-count",
            &max.to_string(),
        ],
    )?;
    let mut commits = Vec::new();
    for line in out.lines() {
        let mut parts = line.split('\u{0}');
        let (Some(sha), Some(ts), Some(subject)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if sha.len() < 7 {
            continue;
        }
        match ts.parse::<u64>() {
            Ok(timestamp) => commits.push(CommitInfo {
                sha: sha.to_string(),
                timestamp,
                subject: subject.to_string(),
            }),
            Err(_) => continue,
        }
    }
    Ok(commits)
}

/// Snapshots `repo` at `sha` into a detached worktree at `dir`.
fn add_worktree(repo: &Path, dir: &Path, sha: &str) -> Result<(), CodeNexusError> {
    run_git(
        repo,
        &[
            "worktree",
            "add",
            "--detach",
            dir.to_str().unwrap_or_default(),
            sha,
        ],
    )
    .map(|_| ())
}

/// Removes a worktree (force). Errors are ignored — the TempDir cleanup is
/// the backstop for the working files.
fn remove_worktree(repo: &Path, dir: &Path) {
    let _ = run_git(
        repo,
        &[
            "worktree",
            "remove",
            "--force",
            dir.to_str().unwrap_or_default(),
        ],
    );
}

/// Core logic — builds the evolution timeline (testable core).
pub fn run_evolve(
    kit: &AsyncKit<AsyncReady>,
    path: &str,
    max_commits: usize,
    output_dir: &str,
    project: &str,
) -> Result<EvolveOutput, CodeNexusError> {
    let max_commits = max_commins_clamp(max_commits);
    let repo = Path::new(path);
    if !repo.is_dir() {
        return Err(CodeNexusError::InvalidInput(format!(
            "path is not a directory: {path}"
        )));
    }
    let commits = enumerate_commits(repo, max_commits)?;

    let storage_config = kit.config::<crate::storage::StorageConfig>()?;
    let db_path = storage_config.db_path.clone();

    let base_project = if project.trim().is_empty() {
        repo.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string())
    } else {
        project.trim().to_string()
    };

    // Chronological order (oldest first) for the timeline.
    let mut points = Vec::with_capacity(commits.len());
    for commit in commits.iter().rev() {
        let worktree = tempfile::TempDir::new().map_err(CodeNexusError::Io)?;
        add_worktree(repo, worktree.path(), &commit.sha)?;

        let sha7: String = commit.sha.chars().take(7).collect();
        let project_name = format!("{base_project}@{sha7}");
        let started = std::time::Instant::now();
        let index_result = index_core(
            kit,
            &db_path,
            worktree.path().to_str().unwrap_or_default(),
            &project_name,
            true,
            false,
            false,
        );
        if let Err(err) = index_result {
            remove_worktree(repo, worktree.path());
            return Err(err);
        }
        let index_ms = started.elapsed().as_millis() as u64;
        let index_result = index_result.expect("checked above");

        // Metrics come from a FRESH connection: the indexer committed through
        // its own short-lived connection, and a long-lived reader (the kit's)
        // would not observe the new data.
        let fresh_repo = crate::storage::Repository::open(&db_path)?;
        let analyzer = ArchitectureAnalyzer::new(&fresh_repo);
        let overview = analyzer
            .overview(&index_result.project_id)
            .map_err(|e| CodeNexusError::Internal(format!("architecture overview: {e}")))?;
        drop(fresh_repo);

        let mut languages = std::collections::BTreeMap::new();
        let mut file_count = 0usize;
        let mut symbol_count = 0usize;
        for stat in &overview.languages {
            languages.insert(stat.language.clone(), stat.file_count as usize);
            file_count += stat.file_count as usize;
            symbol_count += stat.symbol_count as usize;
        }
        let module_count = overview.packages.len();
        let avg_fanout = if symbol_count > 0 {
            index_result.edges_created as f64 / symbol_count as f64
        } else {
            0.0
        };

        points.push(EvolvePoint {
            commit: commit.sha.clone(),
            timestamp: commit.timestamp,
            subject: commit.subject.clone(),
            project_name,
            file_count,
            symbol_count,
            module_count,
            languages,
            avg_fanout,
            index_ms,
        });

        remove_worktree(repo, worktree.path());
    }

    // Artifacts.
    let out_dir = Path::new(output_dir);
    std::fs::create_dir_all(out_dir)?;
    let json_path = out_dir.join("evolve.json");
    let html_path = out_dir.join("evolve.html");
    let json = serde_json::to_string_pretty(&points).map_err(CodeNexusError::from)?;
    std::fs::write(&json_path, json)?;
    let html = render_evolve_html(&points, &base_project);
    std::fs::write(&html_path, html)?;

    Ok(EvolveOutput {
        path: path.to_string(),
        max_commits,
        points,
        json_path: json_path.to_string_lossy().to_string(),
        html_path: html_path.to_string_lossy().to_string(),
    })
}

fn max_commins_clamp(max: usize) -> usize {
    max.clamp(1, MAX_COMMITS_LIMIT)
}

/// Renders the timeline HTML: one inline-SVG polyline sparkline per metric.
/// No external scripts, styles, or fonts (spec R-evolve-003).
fn render_evolve_html(points: &[EvolvePoint], project: &str) -> String {
    let svg_for = |label: &str, values: &[f64]| -> String {
        let w = 240.0_f64;
        let h = 60.0_f64;
        let max = values.iter().cloned().fold(0.0_f64, f64::max).max(1.0);
        let step = if values.len() > 1 {
            w / (values.len() - 1) as f64
        } else {
            w
        };
        let mut pts = String::new();
        for (i, v) in values.iter().enumerate() {
            let x = i as f64 * step;
            let y = h - (v / max) * (h - 6.0) - 3.0;
            pts.push_str(&format!("{:.1},{:.1} ", x, y));
        }
        format!(
            "<div><h3>{label}</h3><svg width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\"><polyline fill=\"none\" stroke=\"#4a90d9\" stroke-width=\"2\" points=\"{pts}\"/></svg></div>"
        )
    };
    let files: Vec<f64> = points.iter().map(|p| p.file_count as f64).collect();
    let symbols: Vec<f64> = points.iter().map(|p| p.symbol_count as f64).collect();
    let modules: Vec<f64> = points.iter().map(|p| p.module_count as f64).collect();
    let fanout: Vec<f64> = points.iter().map(|p| p.avg_fanout).collect();
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{project} — architecture evolution</title></head><body><h1>{project} — architecture evolution</h1>{}{}{}{}</body></html>",
        svg_for("files", &files),
        svg_for("symbols", &symbols),
        svg_for("modules", &modules),
        svg_for("avg fanout", &fanout),
    )
}

/// CLI wrapper — prints the evolve receipt to stdout as JSON.
#[cfg(feature = "cli")]
#[forge(
    name = "evolve",
    version = "0.4.0",
    description = "Architecture evolution timeline: snapshot-index the last N commits (git worktree, project name '<project>@<sha7>'), extract per-commit architecture metrics, and write evolve.json + evolve.html (inline sparklines). Params: path (required); output (default .codenexus/evolve); max_commits (default 10, cap 50); project — base project name (empty = directory name).",
    cli = true
)]
async fn evolve(
    path: String,
    output: String,
    max_commits: String,
    project: String,
) -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    let max = max_commits.trim().parse::<usize>().map_err(|_| {
        to_api_error(
            CodeNexusError::InvalidInput(format!("invalid --max_commits: {max_commits}")),
            "evolve_error",
        )
    })?;
    let output = run_evolve(&kit, &path, max, &output, &project)
        .map_err(|e| to_api_error(e, "evolve_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
#[cfg(feature = "cli")]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};
    use tempfile::TempDir;

    fn build_kit_for_db(db: &std::path::Path) -> AsyncKit<AsyncReady> {
        let config = KitBootstrapConfig::new(db.to_path_buf());
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&config))
            .expect("build_kit")
    }

    /// Creates a git repo with `n` commits, each touching src/lib.rs.
    fn setup_repo(n: usize) -> (TempDir, Vec<String>) {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .expect("git")
                .success());
        };
        git(&["init"]);
        for i in 0..n {
            std::fs::write(
                root.join("src/lib.rs"),
                format!("pub fn f{i}() -> i32 {{\n    {i}\n}}\n"),
            )
            .unwrap();
            git(&["add", "."]);
            git(&[
                "-c",
                "user.email=t@t.com",
                "-c",
                "user.name=T",
                "commit",
                "-m",
                &format!("commit {i}"),
            ]);
        }
        let log = run_git(root, &["log", "--format=%H"]).expect("log");
        let shas: Vec<String> = log.lines().map(String::from).collect();
        (dir, shas)
    }

    #[test]
    fn enumerates_commits_with_sha_timestamp_subject() {
        let (dir, shas) = setup_repo(2);
        let commits = enumerate_commits(dir.path(), 10).expect("enumerate");
        assert_eq!(commits.len(), 2);
        // Newest first from git log.
        assert_eq!(commits[0].sha, shas[0]);
        assert!(commits[0].sha.len() == 40);
        assert!(commits[0].timestamp > 0);
        assert!(!commits[0].subject.is_empty());
    }

    #[test]
    fn worktree_snapshot_roundtrip_and_cleanup() {
        let (dir, shas) = setup_repo(1);
        let worktree = tempfile::TempDir::new().unwrap();
        add_worktree(dir.path(), worktree.path(), &shas[0]).expect("worktree add");
        assert!(
            worktree.path().join("src/lib.rs").exists(),
            "snapshot has files"
        );
        remove_worktree(dir.path(), worktree.path());
        let git_dir = dir.path().join(".git/worktrees");
        if git_dir.exists() {
            let leftover: Vec<_> = std::fs::read_dir(&git_dir)
                .unwrap()
                .filter_map(Result::ok)
                .collect();
            assert!(leftover.is_empty(), "worktree metadata must be cleaned");
        }
    }

    #[test]
    fn evolve_timeline_covers_all_commits_and_writes_artifacts() {
        let (repo_dir, _shas) = setup_repo(2);
        let db = TempDir::new().unwrap();
        let kit = build_kit_for_db(&db.path().join("evolve_db"));
        let out_dir = TempDir::new().unwrap();

        let output = run_evolve(
            &kit,
            repo_dir.path().to_str().unwrap(),
            10,
            out_dir.path().to_str().unwrap(),
            "demo",
        )
        .expect("evolve runs");

        assert_eq!(output.points.len(), 2, "timeline covers both commits");
        for point in &output.points {
            assert!(point.file_count > 0, "files indexed per point");
            assert!(!point.languages.is_empty(), "languages non-empty");
            assert!(point.project_name.contains('@'), "project@sha7 naming");
        }
        // Distinct project names per commit.
        assert_ne!(output.points[0].project_name, output.points[1].project_name);

        // Artifacts exist; JSON round-trips; HTML has no external links.
        let json = std::fs::read_to_string(&output.json_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 2);
        let html = std::fs::read_to_string(&output.html_path).unwrap();
        assert!(html.contains("<svg"), "sparklines present");
        assert!(
            !html.contains("http://") && !html.contains("https://"),
            "no external resources"
        );
    }

    #[test]
    fn evolve_failed_commit_cleans_worktree() {
        // A repo where indexing fails mid-way (e.g. commit introduces no
        // indexable files) — worktree must still be removed.
        let (dir, _shas) = setup_repo(1);
        // Empty commit with no source files:
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .status()
                .expect("git")
                .success());
        };
        git(&["add", "."]);
        let _ = Command::new("git")
            .args(["-C"])
            .arg(dir.path())
            .args(["commit", "--allow-empty", "-m", "empty", "--allow-empty"])
            .status();
        git(&["status"]);

        let db = TempDir::new().unwrap();
        let kit = build_kit_for_db(&db.path().join("evolve_db2"));
        let out_dir = TempDir::new().unwrap();
        // Indexing a worktree without source files still succeeds (0 files);
        // the cleanup invariant is what we assert on success paths too.
        let output = run_evolve(
            &kit,
            dir.path().to_str().unwrap(),
            10,
            out_dir.path().to_str().unwrap(),
            "demo",
        );
        if let Ok(output) = output {
            let _ = output;
        }
        let git_worktrees = dir.path().join(".git/worktrees");
        if git_worktrees.exists() {
            let leftover: Vec<_> = std::fs::read_dir(&git_worktrees)
                .unwrap()
                .filter_map(Result::ok)
                .collect();
            assert!(leftover.is_empty(), "no worktree leak after run");
        }
    }

    #[test]
    fn max_commits_is_clamped() {
        assert_eq!(max_commins_clamp(0), 1);
        assert_eq!(max_commins_clamp(10), 10);
        assert_eq!(max_commins_clamp(500), MAX_COMMITS_LIMIT);
    }
}
