// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! CLI integration tests — verifies the sdforge-based CLI boots, parses
//! arguments, and dispatches to service-layer handlers via inventory.
use std::process::Command;

/// Returns the codenexus binary path.
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

#[test]
fn help_lists_all_commands() {
    let (code, stdout, _) = run(&["--help"]);
    assert_eq!(code, 0, " --help should exit 0");
    assert!(stdout.contains("Usage:"), "should show Usage line");
    assert!(stdout.contains("query"), "should list query command");
    assert!(stdout.contains("index"), "should list index command");
    assert!(stdout.contains("trace"), "should list trace command");
    assert!(stdout.contains("status"), "should list status command");
    assert!(stdout.contains("list"), "should list list command");
}

#[test]
fn version_flag_prints_version() {
    let (code, stdout, _) = run(&["--version"]);
    assert_eq!(code, 0, "--version should exit 0");
    assert!(
        stdout.contains("codenexus") || stdout.contains("CodeNexus"),
        "version output should contain crate name, got: {stdout}"
    );
}

#[test]
fn no_subcommand_exits_gracefully() {
    let (code, stdout, _) = run(&[]);
    assert_eq!(code, 0, "no subcommand should exit 0");
    assert!(
        stdout.contains("--help"),
        "should suggest --help, got: {stdout}"
    );
}

#[test]
fn list_command_works_with_empty_db() {
    let tmp = tempfile::NamedTempFile::new().expect("create temp db file");
    let db_path = tmp.path().to_str().expect("db path to str");
    let (code, _stdout, stderr) = run(&["--db", db_path, "list"]);
    assert_eq!(code, 0, "list should exit 0 on empty db, stderr: {stderr}");
}

#[test]
fn status_command_works_with_empty_db() {
    let tmp = tempfile::NamedTempFile::new().expect("create temp db file");
    let db_path = tmp.path().to_str().expect("db path to str");
    let (code, _, stderr) = run(&["--db", db_path, "status"]);
    assert_eq!(
        code, 0,
        "status should exit 0 on empty db, stderr: {stderr}"
    );
}

#[test]
fn query_command_returns_result() {
    let tmp = tempfile::NamedTempFile::new().expect("create temp db file");
    let db_path = tmp.path().to_str().expect("db path to str");
    let (code, stdout, stderr) = run(&["--db", db_path, "query", "--cypher", "RETURN 1 AS one"]);
    assert_eq!(code, 0, "query should exit 0, stderr: {stderr}");
    assert!(
        stdout.contains("\"one\"") || stdout.contains("one"),
        "query output should contain column 'one', got: {stdout}"
    );
}

#[test]
fn unknown_subcommand_exits_with_error() {
    let (code, _, stderr) = run(&["nonexistent_command"]);
    assert_ne!(
        code, 0,
        "unknown command should exit non-zero, stderr: {stderr}"
    );
}

#[test]
fn ci_gate_e2e_fails_on_high_risk_and_passes_with_fail_on_none() {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let root = tmp.path();

    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if !git(&["init"]) {
        eprintln!("skipping test: git unavailable");
        return;
    }

    // Commit 1: a hub function called by five callers across files.
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/hub.rs"),
        "pub fn hub_fn() -> i32 {\n    42\n}\n",
    )
    .unwrap();
    let mut callers = String::new();
    for i in 0..5 {
        callers.push_str(&format!(
            "pub fn caller_{i}() -> i32 {{\n    hub_fn()\n}}\n"
        ));
    }
    std::fs::write(root.join("src/callers.rs"), callers).unwrap();
    assert!(git(&["add", "."]));
    assert!(git(&[
        "-c",
        "user.email=t@t.com",
        "-c",
        "user.name=T",
        "commit",
        "-m",
        "init"
    ]));

    // Index the committed tree (binary writes .codenexus/<name>.lbug in cwd).
    let (code, _stdout, stderr) = run_in(root, &["index", "--path", ".", "--name", "gate_e2e"]);
    assert_eq!(code, 0, "index should exit 0, stderr: {stderr}");

    // Uncommitted edit inside hub_fn's line range.
    std::fs::write(
        root.join("src/hub.rs"),
        "pub fn hub_fn() -> i32 {\n    // changed\n    43\n}\n",
    )
    .unwrap();

    // Gate with fail_on=high: hub_fn has >=4 incoming CALLS edges → high risk
    // → verdict=fail and the process exits 2.
    let (code, stdout, stderr) = run_in(
        root,
        &[
            "--db",
            ".codenexus/gate_e2e.lbug",
            "ci",
            "--path",
            ".",
            "--base_mode",
            "unstaged",
            "--fail_on",
            "high",
        ],
    );
    let json_line = stdout
        .lines()
        .find(|l| l.starts_with('{'))
        .expect("ci should print a JSON receipt");
    let receipt: serde_json::Value = serde_json::from_str(json_line).expect("valid gate JSON");
    assert_eq!(receipt["verdict"], "fail", "gate must fail: {stderr}");
    assert_eq!(receipt["base_mode"], "unstaged");
    assert!(
        receipt["risk_counts"]["high"].as_u64().unwrap() >= 1,
        "hub_fn should be high risk: {receipt}"
    );
    assert!(
        receipt["markdown"]
            .as_str()
            .unwrap()
            .contains("CodeNexus Architecture Gate"),
        "markdown report must be present"
    );
    assert_eq!(code, 2, "failed gate must exit 2");

    // fail_on=none never fails → exit 0 even with the same diff.
    let (code, stdout, _) = run_in(
        root,
        &[
            "--db",
            ".codenexus/gate_e2e.lbug",
            "ci",
            "--path",
            ".",
            "--base_mode",
            "unstaged",
            "--fail_on",
            "none",
        ],
    );
    assert_eq!(code, 0, "fail_on=none must pass");
    let json_line = stdout
        .lines()
        .find(|l| l.starts_with('{'))
        .expect("gate JSON present");
    let receipt: serde_json::Value = serde_json::from_str(json_line).unwrap();
    assert_eq!(receipt["verdict"], "pass");
    assert_eq!(receipt["fail_on"], "none");
}

/// Runs the codenexus binary with `root` as the working directory.
fn run_in(root: &std::path::Path, args: &[&str]) -> (i32, String, String) {
    let output = Command::new(binary())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn codenexus: {e}"));
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}
