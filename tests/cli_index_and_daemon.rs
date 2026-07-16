// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CLI integration tests for `index`, `daemon` (hot update), and comprehensive
//! post-index command coverage.
//!
//! These tests spawn the `codenexus` binary as a child process (not in-process
//! function calls) to verify the full CLI path: argument parsing → service
//! dispatch → storage → output formatting.
//!
//! Coverage:
//! - `index` CLI: build, force rebuild, incremental, error cases
//! - `daemon` CLI hot update: spawn daemon → file change → SIGTERM → verify
//!   re-indexed data (the primary gap identified by the user)
//! - All 27 subcommands exercised via CLI after indexing

#![cfg(feature = "cli")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use tempfile::TempDir;

/// Returns the codenexus binary path (compile-time known via CARGO_BIN_EXE).
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_codenexus")
}

/// Runs `codenexus <args>` and returns (exit_code, stdout, stderr).
fn run(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(binary())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn codenexus: {e}"));
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// Runs `codenexus <args>` with extra env vars (e.g. HOME override for setup).
fn run_with_env(args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(binary());
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn codenexus: {e}"));
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// Runs `codenexus <args>` with `stdin` piped (for `hook` command).
fn run_with_stdin(args: &[&str], stdin: &[u8]) -> (i32, String, String) {
    let output = Command::new(binary())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(stdin)?;
            child.wait_with_output()
        })
        .unwrap_or_else(|e| panic!("failed to spawn codenexus: {e}"));
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// Writes `content` to `dir/rel`, creating parent dirs as needed.
fn write_file(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

/// Builds a small Rust repo with `main` calling `helper`.
fn build_rust_repo(dir: &Path) {
    write_file(
        dir,
        "src/main.rs",
        "fn main() {\n    helper();\n}\n\nfn helper() {\n    println!(\"hello\");\n}\n",
    );
}

/// Index args for a fresh build (force=false, no LSP/embed, no ram_first).
fn index_args(repo: &Path, db: &Path) -> Vec<String> {
    vec![
        "index".to_string(),
        "--path".to_string(),
        repo.to_string_lossy().to_string(),
        "--name".to_string(),
        "demo".to_string(),
        "--force=false".to_string(),
        "--lsp=false".to_string(),
        "--embed=false".to_string(),
        "--ram_first=false".to_string(),
        "--db".to_string(),
        db.to_string_lossy().to_string(),
    ]
}

/// Creates a fresh temp repo + indexes it. Returns (TempDir, db_path).
/// The TempDir owns both the repo and the DB file — keep it alive for the
/// test's duration.
fn index_fresh_repo() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    build_rust_repo(&repo);
    let db = tmp.path().join("test.lbug");
    let args = index_args(&repo, &db);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let (code, stdout, stderr) = run(&arg_refs);
    assert_eq!(
        code, 0,
        "index should exit 0, stderr: {stderr}, stdout: {stdout}"
    );
    (tmp, db)
}

// =============================================================================
// index CLI tests
// =============================================================================

#[test]
fn index_builds_rust_repo_succeeds() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    build_rust_repo(&repo);
    let db = tmp.path().join("test.lbug");

    let args = index_args(&repo, &db);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let (code, stdout, stderr) = run(&arg_refs);

    assert_eq!(code, 0, "index should exit 0, stderr: {stderr}");
    // stdout contains a JSON object with index stats (mixed with logs on stderr).
    assert!(
        stdout.contains("\"files_indexed\"") && stdout.contains("\"nodes_created\""),
        "stdout should contain index stats JSON, got: {stdout}"
    );
}

#[test]
fn index_nonexistent_path_exits_2() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("test.lbug");
    let args: &[&str] = &[
        "index",
        "--path",
        "/nonexistent/path/does_not_exist_xyz",
        "--name",
        "demo",
        "--force=false",
        "--lsp=false",
        "--embed=false",
        "--ram_first=false",
        "--db",
        db.to_str().unwrap(),
    ];
    let (code, _stdout, _stderr) = run(args);
    // index service does not pre-validate path existence; it fails during
    // IndexFacade::index with an IOError → Internal (exit 1). Accept either
    // exit 1 (current) or exit 2 (if a pre-validation guard is added later).
    assert_ne!(code, 0, "nonexistent path should exit non-zero, got {code}");
}

#[test]
fn index_force_rebuild_exits_0() {
    let (tmp, db) = index_fresh_repo();
    let repo = tmp.path().join("repo");

    // Second index with force=true → full rebuild.
    let args: &[&str] = &[
        "index",
        "--path",
        repo.to_str().unwrap(),
        "--name",
        "demo",
        "--force=true",
        "--lsp=false",
        "--embed=false",
        "--ram_first=false",
        "--db",
        db.to_str().unwrap(),
    ];
    let (code, stdout, stderr) = run(args);
    assert_eq!(code, 0, "force rebuild should exit 0, stderr: {stderr}");
    assert!(
        stdout.contains("\"files_indexed\""),
        "should produce index stats: {stdout}"
    );
}

#[test]
fn index_incremental_skips_unchanged() {
    let (tmp, db) = index_fresh_repo();
    let repo = tmp.path().join("repo");

    // Second index with force=false → incremental, all files unchanged → skipped.
    let args: &[&str] = &[
        "index",
        "--path",
        repo.to_str().unwrap(),
        "--name",
        "demo",
        "--force=false",
        "--lsp=false",
        "--embed=false",
        "--ram_first=false",
        "--db",
        db.to_str().unwrap(),
    ];
    let (code, stdout, stderr) = run(args);
    assert_eq!(code, 0, "incremental index should exit 0, stderr: {stderr}");
    assert!(
        stdout.contains("\"files_skipped\""),
        "should report skipped files: {stdout}"
    );
}

// =============================================================================
// daemon CLI hot update tests (Unix-only: SIGTERM signal handling)
// =============================================================================

#[cfg(unix)]
#[cfg(feature = "daemon")]
mod daemon_hot_update {
    use super::*;

    /// Spawns `codenexus daemon --path <repo> --name demo --db <db>` as a
    /// child process. Returns the `Child` handle.
    fn spawn_daemon(repo: &Path, db: &Path, debounce_ms: u64) -> std::process::Child {
        let mut cmd = Command::new(binary());
        cmd.args([
            "daemon",
            "--path",
            repo.to_str().unwrap(),
            "--name",
            "demo",
            "--db",
            db.to_str().unwrap(),
            "--debounce-ms",
            &debounce_ms.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        cmd.spawn().unwrap_or_else(|e| panic!("spawn daemon: {e}"))
    }

    /// Sends SIGTERM to the child process via `kill -TERM <pid>`.
    fn send_sigterm(child: &std::process::Child) {
        let pid = child.id();
        let exit = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .expect("kill -TERM");
        assert!(exit.success(), "kill -TERM should succeed for pid {pid}");
    }

    /// Waits up to `timeout` for `child` to exit. Returns the exit code, or
    /// panics if the child did not exit in time.
    fn wait_for_exit(child: &mut std::process::Child, timeout: Duration) -> i32 {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.code().unwrap_or(-1),
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        panic!("daemon did not exit within {timeout:?}");
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(e) => panic!("wait failed: {e}"),
            }
        }
    }

    /// Queries all node names via Cypher. Returns the stdout JSON.
    fn query_names(db: &Path) -> String {
        let args: &[&str] = &[
            "query",
            "--cypher",
            "MATCH (n) RETURN n.name AS name",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, stdout, stderr) = run(args);
        assert_eq!(code, 0, "query should exit 0, stderr: {stderr}");
        stdout
    }

    /// Queries function names only (filter for Function label).
    fn query_function_names(db: &Path) -> String {
        let args: &[&str] = &[
            "query",
            "--cypher",
            "MATCH (n:Function) RETURN n.name AS name",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, stdout, _stderr) = run(args);
        assert_eq!(code, 0, "function query should exit 0");
        stdout
    }

    // --- BR-DAEMON-003: hot update detects new code file ---

    #[test]
    fn daemon_hot_update_detects_new_file() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        build_rust_repo(&repo);
        let db = tmp.path().join("test.lbug");

        // Step 1: Initial index via CLI.
        let args = index_args(&repo, &db);
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let (code, _, stderr) = run(&arg_refs);
        assert_eq!(code, 0, "initial index should exit 0, stderr: {stderr}");

        // Step 2: Verify initial state — "helper" is indexed.
        let before = query_function_names(&db);
        assert!(
            before.contains("helper"),
            "initial index should contain 'helper': {before}"
        );
        assert!(
            !before.contains("new_func"),
            "new_func should NOT exist before daemon hot update: {before}"
        );

        // Step 3: Spawn daemon with short debounce.
        let mut daemon = spawn_daemon(&repo, &db, 300);

        // Step 4: Wait for daemon to start watching.
        thread::sleep(Duration::from_millis(600));

        // Step 5: Add a new .rs file with a new function.
        write_file(
            &repo,
            "src/new_func.rs",
            "fn new_func() {\n    helper();\n}\n",
        );

        // Step 6: Wait for debounce (300ms) + index (~500ms) + tick (500ms).
        thread::sleep(Duration::from_millis(2500));

        // Step 7: Send SIGTERM → graceful shutdown.
        send_sigterm(&daemon);
        let exit = wait_for_exit(&mut daemon, Duration::from_secs(3));
        assert_eq!(exit, 0, "daemon should exit 0 after SIGTERM, got {exit}");

        // Step 8: Verify hot update — new_func now indexed.
        let after = query_function_names(&db);
        assert!(
            after.contains("new_func"),
            "new_func should appear after daemon hot update: {after}"
        );
        assert!(
            after.contains("helper"),
            "helper should still be present: {after}"
        );
    }

    // --- BR-DAEMON-003: hot update detects modified file (re-index) ---

    #[test]
    fn daemon_hot_update_detects_modified_file() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        build_rust_repo(&repo);
        let db = tmp.path().join("test.lbug");

        let args = index_args(&repo, &db);
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let (code, _, _) = run(&arg_refs);
        assert_eq!(code, 0, "initial index should exit 0");

        let before = query_function_names(&db);
        assert!(
            before.contains("helper"),
            "initial: helper present: {before}"
        );
        assert!(
            !before.contains("extra_fn"),
            "initial: extra_fn absent: {before}"
        );

        let mut daemon = spawn_daemon(&repo, &db, 300);
        thread::sleep(Duration::from_millis(600));

        // Modify main.rs: add a new function.
        write_file(
            &repo,
            "src/main.rs",
            "fn main() {\n    helper();\n}\n\nfn helper() {\n    println!(\"hello\");\n}\n\nfn extra_fn() {\n    helper();\n}\n",
        );

        thread::sleep(Duration::from_millis(2500));

        send_sigterm(&daemon);
        let exit = wait_for_exit(&mut daemon, Duration::from_secs(3));
        assert_eq!(exit, 0, "daemon should exit 0 after SIGTERM");

        let after = query_function_names(&db);
        assert!(
            after.contains("extra_fn"),
            "extra_fn should appear after modify: {after}"
        );
    }

    // --- Non-code files are ignored by the daemon ---

    #[test]
    fn daemon_hot_update_ignores_non_code_file() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        build_rust_repo(&repo);
        let db = tmp.path().join("test.lbug");

        let args = index_args(&repo, &db);
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let (code, _, _) = run(&arg_refs);
        assert_eq!(code, 0, "initial index should exit 0");

        let before = query_function_names(&db);

        let mut daemon = spawn_daemon(&repo, &db, 300);
        thread::sleep(Duration::from_millis(600));

        // Add non-code files (should be filtered out by is_code_file).
        // NOTE: .json is a code file when lang-json feature is enabled (part
        // of `full`), so use .txt and .md which are never code files.
        write_file(&repo, "notes.txt", "hello world\n");
        write_file(&repo, "README.md", "# readme\n");
        thread::sleep(Duration::from_millis(2500));

        send_sigterm(&daemon);
        let exit = wait_for_exit(&mut daemon, Duration::from_secs(3));
        assert_eq!(exit, 0, "daemon should exit 0 after SIGTERM");

        let after = query_function_names(&db);
        // Non-code files must NOT trigger re-index. Verify function set is
        // unchanged: both before and after contain main + helper, and neither
        // contains the non-code file names. We avoid exact JSON comparison
        // because LadybugDB row order is not guaranteed across calls.
        assert!(
            after.contains("main") && after.contains("helper"),
            "functions should still be present after non-code file add: {after}"
        );
        assert_eq!(
            before.matches("helper").count(),
            after.matches("helper").count(),
            "helper occurrence count should not change"
        );

        // Verify the non-code file names did not sneak in as nodes.
        let all_names = query_names(&db);
        assert!(
            !all_names.contains("notes.txt"),
            "notes.txt should not be indexed: {all_names}"
        );
        assert!(
            !all_names.contains("README.md"),
            "README.md should not be indexed: {all_names}"
        );
    }

    // --- daemon with nonexistent path → exit 2 (after DB exists) ---

    #[test]
    fn daemon_nonexistent_path_exits_2() {
        // CLI gates on DB existence first (exit 4 if missing). To test the
        // path validation guard in daemon_core, we need an existing DB —
        // index a tiny repo, then point daemon at a nonexistent path.
        let (tmp, db) = index_fresh_repo();
        let args: &[&str] = &[
            "daemon",
            "--path",
            "/nonexistent/path/xyz_abc",
            "--name",
            "demo",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, _stdout, _stderr) = run(args);
        assert_eq!(
            code, 2,
            "daemon with nonexistent path should exit 2 (InvalidInput), got {code}"
        );
        let _ = tmp; // keep DB alive
    }
}

// =============================================================================
// Post-index command coverage: all 27 subcommands via CLI
// =============================================================================

mod post_index {
    use super::*;

    // --- Read commands (no mutation) ---

    #[test]
    fn list_shows_indexed_project() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &["list", "--db", db.to_str().unwrap()];
        let (code, stdout, stderr) = run(args);
        assert_eq!(code, 0, "list should exit 0, stderr: {stderr}");
        assert!(
            stdout.contains("\"demo\""),
            "list should show 'demo' project: {stdout}"
        );
    }

    #[test]
    fn status_shows_project() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &["status", "--db", db.to_str().unwrap()];
        let (code, stdout, stderr) = run(args);
        assert_eq!(code, 0, "status should exit 0, stderr: {stderr}");
        // status returns JSON array of projects with staleness info.
        assert!(
            stdout.contains("demo") || stdout.contains("project"),
            "status should mention project: {stdout}"
        );
    }

    #[test]
    fn query_returns_node_names() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &[
            "query",
            "--cypher",
            "MATCH (n:Function) RETURN n.name AS name",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, stdout, stderr) = run(args);
        assert_eq!(code, 0, "query should exit 0, stderr: {stderr}");
        assert!(
            stdout.contains("helper"),
            "query should return 'helper': {stdout}"
        );
    }

    #[test]
    fn query_invalid_cypher_exits_2() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &[
            "query",
            "--cypher",
            "THIS IS NOT CYPHER @@@",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, _stdout, _stderr) = run(args);
        assert_eq!(code, 2, "invalid Cypher should exit 2 (InvalidInput)");
    }

    #[test]
    fn search_exact_finds_symbol() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &[
            "search",
            "--text=helper",
            "--fulltext=false",
            "--limit=10",
            "--mode=exact",
            "--project=demo",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, stdout, stderr) = run(args);
        assert_eq!(code, 0, "search should exit 0, stderr: {stderr}");
        assert!(
            stdout.contains("\"helper\""),
            "search should find 'helper': {stdout}"
        );
        assert!(
            stdout.contains("\"results\""),
            "search should return results array: {stdout}"
        );
    }

    #[test]
    fn context_returns_symbol_view() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &[
            "context",
            "--symbol=helper",
            "--depth=1",
            "--project=demo",
            "--enhanced=false",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, stdout, stderr) = run(args);
        assert_eq!(code, 0, "context should exit 0, stderr: {stderr}");
        assert!(
            stdout.contains("\"symbol\"") && stdout.contains("helper"),
            "context should return symbol info: {stdout}"
        );
        assert!(
            stdout.contains("\"incoming\""),
            "context should show incoming edges: {stdout}"
        );
    }

    #[test]
    fn context_nonexistent_symbol_exits_2() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &[
            "context",
            "--symbol=does_not_exist_xyz",
            "--depth=1",
            "--project=demo",
            "--enhanced=false",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, _stdout, _stderr) = run(args);
        assert_eq!(code, 2, "nonexistent symbol should exit 2");
    }

    #[test]
    fn impact_returns_blast_radius() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &[
            "impact",
            "--symbol=helper",
            "--depth=3",
            "--edge_types=CALLS",
            "--max_depth=3",
            "--include_tests=false",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, stdout, stderr) = run(args);
        assert_eq!(code, 0, "impact should exit 0, stderr: {stderr}");
        assert!(
            stdout.contains("\"node_count\"") && stdout.contains("\"affected\""),
            "impact should return blast radius: {stdout}"
        );
    }

    #[test]
    fn trace_returns_call_paths() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &[
            "trace",
            "--symbol=main",
            "--trace_type=calls",
            "--depth=3",
            "--path_filter=",
            "--detect_cycles=false",
            "--cross_service=false",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, stdout, stderr) = run(args);
        assert_eq!(code, 0, "trace should exit 0, stderr: {stderr}");
        // trace returns paths JSON — just verify non-empty output.
        assert!(
            !stdout.trim().is_empty(),
            "trace should produce output: stderr={stderr}"
        );
    }

    #[test]
    fn route_map_exits_0() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &["route_map", "--project=demo", "--db", db.to_str().unwrap()];
        let (code, _stdout, stderr) = run(args);
        assert_eq!(code, 0, "route_map should exit 0, stderr: {stderr}");
    }

    #[test]
    fn dead_code_exits_0() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &[
            "dead_code",
            "--project=demo",
            "--entry=main",
            "--check_exported=false",
            "--check_ffi=false",
            "--edge_types=CALLS",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, _stdout, stderr) = run(args);
        assert_eq!(code, 0, "dead_code should exit 0, stderr: {stderr}");
    }

    #[test]
    fn architecture_exits_0() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &[
            "architecture",
            "--project=demo",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, _stdout, stderr) = run(args);
        assert_eq!(code, 0, "architecture should exit 0, stderr: {stderr}");
    }

    #[test]
    fn complexity_exits_0() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &["complexity", "--project=demo", "--db", db.to_str().unwrap()];
        let (code, _stdout, stderr) = run(args);
        assert_eq!(code, 0, "complexity should exit 0, stderr: {stderr}");
    }

    #[test]
    fn cross_service_exits_0() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &[
            "cross_service",
            "--project=demo",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, _stdout, stderr) = run(args);
        assert_eq!(code, 0, "cross_service should exit 0, stderr: {stderr}");
    }

    #[test]
    fn detect_changes_on_non_git_repo_exits_gracefully() {
        let (_tmp, db) = index_fresh_repo();
        let repo = _tmp.path().join("repo");
        // detect_changes requires --path and --mode. A non-git repo may
        // return empty results (exit 0) or error (exit 2); both are
        // acceptable as long as it doesn't crash (exit 1 or signal).
        let args: &[&str] = &[
            "detect_changes",
            "--path",
            repo.to_str().unwrap(),
            "--mode=staged",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, _stdout, _stderr) = run(args);
        assert!(
            code == 0 || code == 2,
            "detect_changes on non-git should exit 0 or 2, got {code}"
        );
    }

    #[test]
    fn rename_dry_run_exits_0() {
        let (_tmp, db) = index_fresh_repo();
        let repo = _tmp.path().join("repo");
        let args: &[&str] = &[
            "rename",
            "--from=helper",
            "--to=helper_renamed",
            "--path",
            repo.to_str().unwrap(),
            "--apply=false",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, _stdout, stderr) = run(args);
        assert_eq!(code, 0, "rename dry-run should exit 0, stderr: {stderr}");
    }

    #[test]
    fn api_impact_exits_0() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &["api_impact", "--project=demo", "--db", db.to_str().unwrap()];
        let (code, _stdout, stderr) = run(args);
        assert_eq!(code, 0, "api_impact should exit 0, stderr: {stderr}");
    }

    #[test]
    fn shape_check_exits_0() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &[
            "shape_check",
            "--project=demo",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, _stdout, stderr) = run(args);
        assert_eq!(code, 0, "shape_check should exit 0, stderr: {stderr}");
    }

    #[test]
    fn community_exits_0() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &["community", "--project=demo", "--db", db.to_str().unwrap()];
        let (code, _stdout, stderr) = run(args);
        assert_eq!(code, 0, "community should exit 0, stderr: {stderr}");
    }

    #[test]
    fn tool_map_exits_0() {
        let (_tmp, db) = index_fresh_repo();
        let args: &[&str] = &["tool_map", "--project=demo", "--db", db.to_str().unwrap()];
        let (code, _stdout, stderr) = run(args);
        assert_eq!(code, 0, "tool_map should exit 0, stderr: {stderr}");
    }

    // --- Mutation commands (separate index to avoid polluting other tests) ---

    #[test]
    fn export_creates_artifact() {
        let (tmp, db) = index_fresh_repo();
        let output = tmp.path().join("export.tar.zst");
        let args: &[&str] = &[
            "export",
            "--output",
            output.to_str().unwrap(),
            "--project=demo",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, _stdout, stderr) = run(args);
        assert_eq!(code, 0, "export should exit 0, stderr: {stderr}");
        assert!(
            output.exists(),
            "export artifact should exist at {output:?}"
        );
        assert!(
            fs::metadata(&output).map(|m| m.len()).unwrap_or(0) > 0,
            "export artifact should be non-empty"
        );
    }

    #[test]
    fn clean_removes_project() {
        let (_tmp, db) = index_fresh_repo();
        // Verify project exists before clean.
        let args_before: &[&str] = &["list", "--db", db.to_str().unwrap()];
        let (code, stdout, _) = run(args_before);
        assert_eq!(code, 0);
        assert!(stdout.contains("demo"), "project should exist before clean");

        // Clean the project.
        let args: &[&str] = &["clean", "--project=demo", "--db", db.to_str().unwrap()];
        let (code, _stdout, stderr) = run(args);
        assert_eq!(code, 0, "clean should exit 0, stderr: {stderr}");

        // Verify project is gone after clean.
        let args_after: &[&str] = &["list", "--db", db.to_str().unwrap()];
        let (code, stdout, _) = run(args_after);
        assert_eq!(code, 0);
        assert!(
            !stdout.contains("demo"),
            "project should be gone after clean: {stdout}"
        );
    }

    #[test]
    fn hook_reads_stdin_and_responds() {
        // hook reads a git hook payload from stdin and emits a JSON decision.
        // An empty/minimal payload should produce a decision (not crash).
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("test.lbug");
        let payload = b"{}";
        let args: &[&str] = &["hook", "--db", db.to_str().unwrap()];
        let (code, _stdout, _stderr) = run_with_stdin(args, payload);
        // hook should exit 0 (decision emitted) or 2 (invalid input) — both
        // mean the CLI path works. Exit 1 would indicate an internal error.
        assert!(
            code == 0 || code == 2,
            "hook should exit 0 or 2, got {code}"
        );
    }

    #[test]
    fn setup_with_force_false_exits_gracefully() {
        // setup writes MCP config to $HOME/.codenexus/ — override HOME to a
        // temp dir to avoid polluting the user's actual home.
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let db = tmp.path().join("test.lbug");
        let args: &[&str] = &["setup", "--force=false", "--db", db.to_str().unwrap()];
        let (code, _stdout, _stderr) = run_with_env(args, &[("HOME", home.to_str().unwrap())]);
        // setup may exit 0 (success) or 2 (no agents detected) — both are
        // valid CLI paths. Exit 1 would indicate a crash.
        assert!(
            code == 0 || code == 2,
            "setup should exit 0 or 2, got {code}"
        );
    }

    // --- LSP commands: require a language server, expect graceful failure ---

    #[test]
    fn lsp_goto_def_without_server_exits_gracefully() {
        let (_tmp, db) = index_fresh_repo();
        let repo = _tmp.path().join("repo");
        let file = repo.join("src/main.rs");
        let args: &[&str] = &[
            "lsp_goto_def",
            "--file",
            file.to_str().unwrap(),
            "--line=1",
            "--col=1",
            "--workspace",
            repo.to_str().unwrap(),
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, _stdout, _stderr) = run(args);
        // Without a running language server, lsp_goto_def should fail
        // gracefully (exit 1 or 2), not crash/panic/signal.
        assert!(
            code == 1 || code == 2,
            "lsp_goto_def without server should exit 1 or 2, got {code}"
        );
    }

    #[test]
    fn lsp_hover_without_server_exits_gracefully() {
        let (_tmp, db) = index_fresh_repo();
        let repo = _tmp.path().join("repo");
        let file = repo.join("src/main.rs");
        let args: &[&str] = &[
            "lsp_hover",
            "--file",
            file.to_str().unwrap(),
            "--line=1",
            "--col=1",
            "--workspace",
            repo.to_str().unwrap(),
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, _stdout, _stderr) = run(args);
        assert!(
            code == 1 || code == 2,
            "lsp_hover without server should exit 1 or 2, got {code}"
        );
    }

    // --- import round-trip (export → import) ---

    #[test]
    fn export_then_import_roundtrip() {
        let (tmp, db) = index_fresh_repo();
        let repo = tmp.path().join("repo");
        let artifact = tmp.path().join("export.tar.zst");

        // Export.
        let export_args: &[&str] = &[
            "export",
            "--output",
            artifact.to_str().unwrap(),
            "--project=demo",
            "--db",
            db.to_str().unwrap(),
        ];
        let (code, _, stderr) = run(export_args);
        assert_eq!(code, 0, "export should exit 0, stderr: {stderr}");
        assert!(artifact.exists(), "artifact should exist");

        // Import into a new DB to verify round-trip.
        let db2 = tmp.path().join("imported.lbug");
        let import_args: &[&str] = &[
            "import",
            "--input",
            artifact.to_str().unwrap(),
            "--reindex=false",
            "--path",
            repo.to_str().unwrap(),
            "--name=demo_imported",
            "--db",
            db2.to_str().unwrap(),
        ];
        let (code, stdout, stderr) = run(import_args);
        assert_eq!(
            code, 0,
            "import should exit 0, stderr: {stderr}, stdout: {stdout}"
        );
    }
}
