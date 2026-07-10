// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! CLI integration tests — verifies the sdforge-based CLI boots, parses
//! arguments, and dispatches to service-layer handlers via inventory.

#![cfg(feature = "cli")]

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
