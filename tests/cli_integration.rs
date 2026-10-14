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

// --- global `--format text|json` machine-readable output contract ---
// (sdforge `cli::output`; CodeNexus renders its own CLI boundary output, so
// the contract is pinned end to end here: stream choice and exit codes.)

/// Explicit `--format text` keeps the default behaviour byte for byte: the
/// human quick-start banner on stdout and exit 0.
#[test]
fn format_text_keeps_human_banner_and_exit_zero() {
    let (code, stdout, _) = run(&["--format", "text"]);
    assert_eq!(code, 0, "explicit text mode must keep the no-op exit code");
    assert!(
        stdout.contains("Quick start:"),
        "text mode must keep the banner, got: {stdout}"
    );
}

/// `--format json` turns the "no subcommand" condition into the contract's
/// UnifiedError JSON payload on stdout; the exit code follows the payload
/// class (`BAD_REQUEST` → 2), never a bare 1. The human banner would be
/// unparseable machine output.
#[test]
fn format_json_no_subcommand_emits_unified_error_json_on_stdout() {
    let (code, stdout, stderr) = run(&["--format", "json"]);
    assert_eq!(
        code, 2,
        "json usage error must exit with the payload's class code, stderr: {stderr}"
    );
    let payload: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("stdout must carry exactly the JSON payload, got {stdout:?}: {e}")
    });
    assert_eq!(payload["code"], "BAD_REQUEST");
    assert!(
        payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains("no subcommand"),
        "payload must explain the usage error, got: {payload}"
    );
    assert!(
        !stdout.contains("Quick start:"),
        "the human banner must not leak into json mode, got: {stdout}"
    );
}

/// `--format` is mounted with a `text|json` whitelist; anything else is
/// rejected by clap before any work happens.
#[test]
fn format_rejects_unknown_values() {
    let (code, _, stderr) = run(&["--format", "yaml", "list"]);
    assert_ne!(code, 0, "unknown --format value must be rejected");
    assert!(
        stderr.contains("yaml"),
        "clap must name the rejected value, stderr: {stderr}"
    );
}

/// Handler failures in `--format json` render the UnifiedError payload on
/// stdout (never the text `Error:` line on stderr), keeping CodeNexus's own
/// exit codes.
#[test]
fn format_json_handler_error_is_unified_error_on_stdout() {
    let tmp = tempfile::NamedTempFile::new().expect("create temp db file");
    let db_path = tmp.path().to_str().expect("db path to str");
    let (code, stdout, stderr) = run(&[
        "--db",
        db_path,
        "--format",
        "json",
        "query",
        "--cypher",
        "DEFINITELY NOT CYPHER",
    ]);
    assert_ne!(code, 0, "invalid cypher must fail, stderr: {stderr}");
    let payload: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("stdout must carry exactly the JSON payload, got {stdout:?}: {e}")
    });
    assert_eq!(
        payload["code"], "BAD_REQUEST",
        "invalid cypher is an input error, got: {payload}"
    );
    assert_eq!(
        code, 2,
        "exit code must match the payload class (BAD_REQUEST → 2)"
    );
    assert!(
        payload["message"].as_str().is_some_and(|m| !m.is_empty()),
        "payload must carry a message, got: {payload}"
    );
    assert!(
        !stderr.contains("Error:"),
        "json mode must not use the text error line, stderr: {stderr}"
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

/// `cn` 是 `codenexus` 的等形态别名二进制（同一源构建，同一 CLI 面）。
#[test]
fn cn_alias_binary_shares_the_cli_surface() {
    let cn = env!("CARGO_BIN_EXE_cn");
    let out = Command::new(cn)
        .arg("--version")
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn cn: {e}"));
    assert!(out.status.success(), "cn --version should exit 0");
    let version = String::from_utf8_lossy(&out.stdout);
    assert!(
        version.contains(env!("CARGO_PKG_VERSION")),
        "cn --version should carry the crate version, got: {version}"
    );

    let help = Command::new(cn)
        .arg("--help")
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn cn: {e}"));
    assert!(help.status.success(), "cn --help should exit 0");
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(
        help_text.contains("Usage:"),
        "cn --help must render the CLI"
    );

    // `cn webui --help`：启用 webui feature 时该子命令与 --port 参数可见
    if cfg!(feature = "webui") {
        let webui_help = Command::new(cn)
            .args(["webui", "--help"])
            .output()
            .unwrap_or_else(|e| panic!("failed to spawn cn webui: {e}"));
        assert!(webui_help.status.success(), "cn webui --help should exit 0");
        let text = String::from_utf8_lossy(&webui_help.stdout);
        assert!(
            text.contains("--port"),
            "cn webui --help 应列出 --port：{text}"
        );
    }
}
