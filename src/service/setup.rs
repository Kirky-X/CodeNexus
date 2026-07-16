// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `setup` service: auto-detect installed AI coding agents and write MCP
//! server config.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(feature = "cli")]
use crate::service::error::to_api_error;
#[cfg(feature = "cli")]
use crate::service::error::wrap_error;
use crate::service::error::CodeNexusError;

#[cfg(feature = "cli")]
use sdforge::forge;
#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;

/// MCP server entry written into agent config files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerEntry {
    pub command: String,
    pub args: Vec<String>,
}

/// The canonical MCP server entry for CodeNexus.
pub fn codenexus_mcp_entry() -> McpServerEntry {
    McpServerEntry {
        command: "codenexus".to_string(),
        args: vec!["mcp".to_string()],
    }
}

/// Supported AI coding agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    ClaudeCode,
    Cursor,
    Codex,
}

impl Agent {
    /// Human-readable agent name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
            Agent::Cursor => "Cursor",
            Agent::Codex => "Codex",
        }
    }

    fn marker_dir(self, home: &Path) -> PathBuf {
        match self {
            Agent::ClaudeCode => home.join(".claude"),
            Agent::Cursor => home.join(".cursor"),
            Agent::Codex => home.join(".codex"),
        }
    }

    fn config_path(self, home: &Path) -> PathBuf {
        match self {
            Agent::ClaudeCode => home.join(".claude.json"),
            Agent::Cursor => home.join(".cursor").join("mcp.json"),
            Agent::Codex => home.join(".codex").join("config.json"),
        }
    }

    fn is_installed(self, home: &Path) -> bool {
        self.marker_dir(home).is_dir()
    }

    /// Detects all installed agents under `home`, in canonical order.
    #[must_use]
    pub fn detect_all(home: &Path) -> Vec<Agent> {
        let all = [Agent::ClaudeCode, Agent::Cursor, Agent::Codex];
        all.into_iter().filter(|a| a.is_installed(home)).collect()
    }
}

/// JSON-serializable setup output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SetupOutput {
    pub configured: Vec<ConfiguredAgent>,
    pub skipped: Vec<SkippedAgent>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConfiguredAgent {
    pub agent: String,
    pub config_path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SkippedAgent {
    pub agent: String,
    pub config_path: String,
    pub reason: String,
}

/// Outcome of attempting to write the MCP entry into a single agent's config.
#[derive(Debug)]
enum WriteOutcome {
    Written,
    AlreadyConfigured,
    Declined,
}

/// Runs setup against a specific `home` directory (testable entry point).
pub fn run_with_home(
    home: &Path,
    force: bool,
    stdin: &mut dyn BufRead,
    stdout: &mut dyn Write,
) -> Result<SetupOutput, CodeNexusError> {
    let agents = Agent::detect_all(home);
    if agents.is_empty() {
        writeln!(
            stdout,
            "No supported AI coding agents detected under {home}.",
            home = home.display()
        )?;
        writeln!(
            stdout,
            "Supported agents: Claude Code (~/.claude/), Cursor (~/.cursor/), Codex (~/.codex/)."
        )?;
        writeln!(
            stdout,
            "Install one of the supported agents and re-run `codenexus setup`."
        )?;
        return Err(CodeNexusError::InvalidInput(
            "no supported agents detected".into(),
        ));
    }

    let entry = codenexus_mcp_entry();
    let mut configured = Vec::new();
    let mut skipped = Vec::new();

    for agent in &agents {
        let config_path = agent.config_path(home);
        match write_agent_config(&config_path, &entry, force, stdin, stdout)? {
            WriteOutcome::Written => {
                writeln!(
                    stdout,
                    "Configured {name} — wrote MCP entry to {path}",
                    name = agent.name(),
                    path = config_path.display()
                )?;
                configured.push(ConfiguredAgent {
                    agent: agent.name().to_string(),
                    config_path: config_path.to_string_lossy().into_owned(),
                });
            }
            WriteOutcome::AlreadyConfigured => {
                writeln!(
                    stdout,
                    "Skipped {name} — already configured at {path}",
                    name = agent.name(),
                    path = config_path.display()
                )?;
                skipped.push(SkippedAgent {
                    agent: agent.name().to_string(),
                    config_path: config_path.to_string_lossy().into_owned(),
                    reason: "already points at codenexus".to_string(),
                });
            }
            WriteOutcome::Declined => {
                writeln!(
                    stdout,
                    "Skipped {name} — user declined overwrite at {path}",
                    name = agent.name(),
                    path = config_path.display()
                )?;
                skipped.push(SkippedAgent {
                    agent: agent.name().to_string(),
                    config_path: config_path.to_string_lossy().into_owned(),
                    reason: "user declined overwrite".to_string(),
                });
            }
        }
    }

    Ok(SetupOutput {
        configured,
        skipped,
    })
}

/// Reads HOME, detects agents, writes MCP config. Testable entry point.
#[cfg(any(feature = "cli", test))]
fn run(force: bool) -> Result<SetupOutput, CodeNexusError> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| CodeNexusError::InvalidInput("HOME environment variable is not set".into()))?;
    run_with_home(
        &home,
        force,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout(),
    )
}

fn write_agent_config(
    path: &Path,
    entry: &McpServerEntry,
    force: bool,
    stdin: &mut dyn BufRead,
    stdout: &mut dyn Write,
) -> Result<WriteOutcome, CodeNexusError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if !path.exists() {
        let mut root = serde_json::Map::new();
        let mut servers = serde_json::Map::new();
        servers.insert("codenexus".to_string(), serde_json::to_value(entry)?);
        root.insert("mcpServers".to_string(), serde_json::Value::Object(servers));
        let json = serde_json::to_string_pretty(&serde_json::Value::Object(root))?;
        std::fs::write(path, json + "\n")?;
        return Ok(WriteOutcome::Written);
    }

    let raw = std::fs::read_to_string(path)?;
    let mut root: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        CodeNexusError::InvalidInput(format!(
            "failed to parse existing config at {path}: {e}",
            path = path.display()
        ))
    })?;

    let servers = root
        .as_object_mut()
        .ok_or_else(|| {
            CodeNexusError::InvalidInput(format!(
                "config at {path} is not a JSON object",
                path = path.display()
            ))
        })?
        .entry("mcpServers".to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            CodeNexusError::InvalidInput(format!(
                "config at {path} has a non-object `mcpServers`",
                path = path.display()
            ))
        })?;

    let existing = servers.get("codenexus");
    match existing {
        None => {
            servers.insert("codenexus".to_string(), serde_json::to_value(entry)?);
            write_pretty(path, &root)?;
            Ok(WriteOutcome::Written)
        }
        Some(curr) if curr == &serde_json::to_value(entry)? => Ok(WriteOutcome::AlreadyConfigured),
        Some(_) => {
            if !force {
                write!(
                    stdout,
                    "Existing codenexus MCP entry found at {path}. Overwrite? [y/N] ",
                    path = path.display()
                )?;
                stdout.flush()?;
                let mut line = String::new();
                stdin.read_line(&mut line)?;
                let answer = line.trim().to_ascii_lowercase();
                if answer != "y" && answer != "yes" {
                    return Ok(WriteOutcome::Declined);
                }
            }
            servers.insert("codenexus".to_string(), serde_json::to_value(entry)?);
            write_pretty(path, &root)?;
            Ok(WriteOutcome::Written)
        }
    }
}

fn write_pretty(path: &Path, value: &serde_json::Value) -> Result<(), CodeNexusError> {
    let json = serde_json::to_string_pretty(value)?;
    std::fs::write(path, json + "\n")?;
    Ok(())
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[forge(
    name = "setup",
    version = "0.3.4",
    description = "Auto-detect installed AI coding agents and write MCP server config.",
    cli = true
)]
async fn setup(force: bool) -> Result<(), ApiError> {
    let output = run(force).map_err(|e| to_api_error(e, "setup_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fake_home(agents: &[Agent]) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        for agent in agents {
            std::fs::create_dir_all(agent.marker_dir(dir.path())).unwrap();
        }
        dir
    }

    fn yes_stdin() -> Cursor<Vec<u8>> {
        Cursor::new(b"y\n".to_vec())
    }

    fn no_stdin() -> Cursor<Vec<u8>> {
        Cursor::new(b"n\n".to_vec())
    }

    // --- Agent detection ---

    #[test]
    fn detect_all_returns_empty_when_no_agents_installed() {
        let home = tempfile::TempDir::new().unwrap();
        assert!(Agent::detect_all(home.path()).is_empty());
    }

    #[test]
    fn detect_all_finds_claude_code() {
        let home = fake_home(&[Agent::ClaudeCode]);
        let agents = Agent::detect_all(home.path());
        assert_eq!(agents, vec![Agent::ClaudeCode]);
    }

    #[test]
    fn detect_all_finds_cursor() {
        let home = fake_home(&[Agent::Cursor]);
        let agents = Agent::detect_all(home.path());
        assert_eq!(agents, vec![Agent::Cursor]);
    }

    #[test]
    fn detect_all_finds_codex() {
        let home = fake_home(&[Agent::Codex]);
        let agents = Agent::detect_all(home.path());
        assert_eq!(agents, vec![Agent::Codex]);
    }

    #[test]
    fn detect_all_returns_canonical_order() {
        let home = fake_home(&[Agent::Codex, Agent::Cursor, Agent::ClaudeCode]);
        let agents = Agent::detect_all(home.path());
        assert_eq!(agents, vec![Agent::ClaudeCode, Agent::Cursor, Agent::Codex]);
    }

    // --- config_path ---

    #[test]
    fn config_path_for_claude_code() {
        let home = Path::new("/home/user");
        assert_eq!(
            Agent::ClaudeCode.config_path(home),
            Path::new("/home/user/.claude.json")
        );
    }

    #[test]
    fn config_path_for_cursor() {
        let home = Path::new("/home/user");
        assert_eq!(
            Agent::Cursor.config_path(home),
            Path::new("/home/user/.cursor/mcp.json")
        );
    }

    #[test]
    fn config_path_for_codex() {
        let home = Path::new("/home/user");
        assert_eq!(
            Agent::Codex.config_path(home),
            Path::new("/home/user/.codex/config.json")
        );
    }

    // --- write_agent_config: fresh file ---

    #[test]
    fn write_agent_config_creates_fresh_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        let entry = codenexus_mcp_entry();
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let outcome = write_agent_config(&path, &entry, false, &mut stdin, &mut stdout).unwrap();
        assert!(matches!(outcome, WriteOutcome::Written));
        assert!(path.exists());
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            v["mcpServers"]["codenexus"]["command"],
            serde_json::json!("codenexus")
        );
        assert_eq!(
            v["mcpServers"]["codenexus"]["args"],
            serde_json::json!(["mcp"])
        );
    }

    #[test]
    fn write_agent_config_adds_entry_to_existing_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"other":{"command":"other","args":[]}}}"#,
        )
        .unwrap();
        let entry = codenexus_mcp_entry();
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let outcome = write_agent_config(&path, &entry, false, &mut stdin, &mut stdout).unwrap();
        assert!(matches!(outcome, WriteOutcome::Written));
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["mcpServers"]["codenexus"]["command"], "codenexus");
        assert_eq!(v["mcpServers"]["other"]["command"], "other");
    }

    #[test]
    fn write_agent_config_skips_when_already_configured() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        let entry = codenexus_mcp_entry();
        let json = serde_json::json!({
            "mcpServers": {"codenexus": {"command": "codenexus", "args": ["mcp"]}}
        });
        std::fs::write(&path, serde_json::to_string(&json).unwrap()).unwrap();
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let outcome = write_agent_config(&path, &entry, false, &mut stdin, &mut stdout).unwrap();
        assert!(matches!(outcome, WriteOutcome::AlreadyConfigured));
    }

    #[test]
    fn write_agent_config_prompts_when_different_entry_exists() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"codenexus":{"command":"/old/codenexus","args":["mcp"]}}}"#,
        )
        .unwrap();
        let entry = codenexus_mcp_entry();
        let mut stdin = yes_stdin();
        let mut stdout = Vec::new();
        let outcome = write_agent_config(&path, &entry, false, &mut stdin, &mut stdout).unwrap();
        assert!(matches!(outcome, WriteOutcome::Written));
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["mcpServers"]["codenexus"]["command"], "codenexus");
    }

    #[test]
    fn write_agent_config_declines_overwrite_when_user_says_no() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"codenexus":{"command":"/old/codenexus","args":["mcp"]}}}"#,
        )
        .unwrap();
        let entry = codenexus_mcp_entry();
        let mut stdin = no_stdin();
        let mut stdout = Vec::new();
        let outcome = write_agent_config(&path, &entry, false, &mut stdin, &mut stdout).unwrap();
        assert!(matches!(outcome, WriteOutcome::Declined));
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["mcpServers"]["codenexus"]["command"], "/old/codenexus");
    }

    #[test]
    fn write_agent_config_force_overwrites_without_prompt() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"codenexus":{"command":"/old/codenexus","args":["mcp"]}}}"#,
        )
        .unwrap();
        let entry = codenexus_mcp_entry();
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let outcome = write_agent_config(&path, &entry, true, &mut stdin, &mut stdout).unwrap();
        assert!(matches!(outcome, WriteOutcome::Written));
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["mcpServers"]["codenexus"]["command"], "codenexus");
    }

    // --- run_with_home ---

    #[test]
    fn run_with_home_errors_when_no_agents_detected() {
        let home = tempfile::TempDir::new().unwrap();
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let err = run_with_home(home.path(), false, &mut stdin, &mut stdout)
            .expect_err("no agents should error");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
        assert!(!stdout.is_empty(), "should print guidance");
    }

    #[test]
    fn run_with_home_configures_single_agent() {
        let home = fake_home(&[Agent::ClaudeCode]);
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let output = run_with_home(home.path(), false, &mut stdin, &mut stdout).unwrap();
        assert_eq!(output.configured.len(), 1);
        assert_eq!(output.configured[0].agent, "Claude Code");
        assert!(output.skipped.is_empty());
        let config_path = home.path().join(".claude.json");
        assert!(config_path.exists());
    }

    #[test]
    fn run_with_home_configures_multiple_agents() {
        let home = fake_home(&[Agent::ClaudeCode, Agent::Cursor]);
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let output = run_with_home(home.path(), false, &mut stdin, &mut stdout).unwrap();
        assert_eq!(output.configured.len(), 2);
        assert!(output.skipped.is_empty());
    }

    #[test]
    fn run_with_home_skips_already_configured_agent() {
        let home = fake_home(&[Agent::ClaudeCode]);
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        run_with_home(home.path(), false, &mut stdin, &mut stdout).unwrap();
        let mut stdin2 = Cursor::new(Vec::new());
        let mut stdout2 = Vec::new();
        let output = run_with_home(home.path(), false, &mut stdin2, &mut stdout2).unwrap();
        assert!(output.configured.is_empty());
        assert_eq!(output.skipped.len(), 1);
        assert_eq!(output.skipped[0].agent, "Claude Code");
    }

    #[test]
    fn run_with_home_preserves_existing_mcp_servers() {
        let home = fake_home(&[Agent::ClaudeCode]);
        let config_path = home.path().join(".claude.json");
        std::fs::write(
            &config_path,
            r#"{"mcpServers":{"other":{"command":"other","args":[]}}}"#,
        )
        .unwrap();
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        run_with_home(home.path(), false, &mut stdin, &mut stdout).unwrap();
        let raw = std::fs::read_to_string(&config_path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["mcpServers"]["other"]["command"], "other");
        assert_eq!(v["mcpServers"]["codenexus"]["command"], "codenexus");
    }

    #[test]
    fn run_with_home_declines_overwrite_returns_skipped() {
        let home = fake_home(&[Agent::ClaudeCode]);
        let config_path = home.path().join(".claude.json");
        std::fs::write(
            &config_path,
            r#"{"mcpServers":{"codenexus":{"command":"/old/codenexus","args":["mcp"]}}}"#,
        )
        .unwrap();
        let mut stdin = no_stdin();
        let mut stdout = Vec::new();
        let output = run_with_home(home.path(), false, &mut stdin, &mut stdout).unwrap();
        assert!(output.configured.is_empty());
        assert_eq!(output.skipped.len(), 1);
        assert_eq!(output.skipped[0].agent, "Claude Code");
        assert_eq!(output.skipped[0].reason, "user declined overwrite");
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["codenexus"]["command"], "/old/codenexus");
    }

    // --- write_agent_config: error paths ---

    #[test]
    fn write_agent_config_invalid_json_returns_invalid_input() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, "not valid json {").unwrap();
        let entry = codenexus_mcp_entry();
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let err = write_agent_config(&path, &entry, false, &mut stdin, &mut stdout)
            .expect_err("invalid JSON should error");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
    }

    #[test]
    fn write_agent_config_non_object_root_returns_invalid_input() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, r#"[1, 2, 3]"#).unwrap();
        let entry = codenexus_mcp_entry();
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let err = write_agent_config(&path, &entry, false, &mut stdin, &mut stdout)
            .expect_err("non-object root should error");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
    }

    #[test]
    fn write_agent_config_non_object_mcp_servers_returns_invalid_input() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, r#"{"mcpServers":"not-an-object"}"#).unwrap();
        let entry = codenexus_mcp_entry();
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let err = write_agent_config(&path, &entry, false, &mut stdin, &mut stdout)
            .expect_err("non-object mcpServers should error");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
    }

    // --- codenexus_mcp_entry ---

    #[test]
    fn codenexus_mcp_entry_has_command_and_args() {
        let entry = codenexus_mcp_entry();
        assert_eq!(entry.command, "codenexus");
        assert_eq!(entry.args, vec!["mcp".to_string()]);
    }

    #[test]
    fn codenexus_mcp_entry_serializes_to_expected_json() {
        let entry = codenexus_mcp_entry();
        let v = serde_json::to_value(&entry).unwrap();
        assert_eq!(v, serde_json::json!({"command":"codenexus","args":["mcp"]}));
    }

    // --- Agent::name ---

    #[test]
    fn agent_name_returns_human_readable() {
        assert_eq!(Agent::ClaudeCode.name(), "Claude Code");
        assert_eq!(Agent::Cursor.name(), "Cursor");
        assert_eq!(Agent::Codex.name(), "Codex");
    }

    // --- run() wrapper ---

    /// Serializes tests that mutate the `HOME` environment variable.
    static HOME_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn run_succeeds_when_home_has_fresh_agent() {
        let _lock = HOME_TEST_MUTEX.lock().unwrap();
        let home = fake_home(&[Agent::ClaudeCode]);
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", home.path());
        let result = run(false);
        match original_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        assert!(
            result.is_ok(),
            "run should succeed with a fresh agent: {:?}",
            result.err()
        );
        let config_path = home.path().join(".claude.json");
        assert!(config_path.exists(), "config file should be created");
    }

    #[test]
    fn run_returns_error_when_home_unset() {
        let _lock = HOME_TEST_MUTEX.lock().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::remove_var("HOME");
        let err = run(false).expect_err("HOME unset should error");
        match original_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
    }

    // --- setup() CLI wrapper ---

    #[cfg(feature = "cli")]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn setup_succeeds_when_home_has_fresh_agent() {
        let _lock = HOME_TEST_MUTEX.lock().unwrap();
        let home = fake_home(&[Agent::ClaudeCode]);
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", home.path());
        let result = setup(false).await;
        match original_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        assert!(result.is_ok(), "setup should succeed: {:?}", result.err());
        let config_path = home.path().join(".claude.json");
        assert!(config_path.exists(), "config file should be created");
    }

    #[cfg(feature = "cli")]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn setup_returns_error_when_home_unset() {
        let _lock = HOME_TEST_MUTEX.lock().unwrap();
        let original_home = std::env::var("HOME").ok();
        std::env::remove_var("HOME");
        let result = setup(false).await;
        match original_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        assert!(result.is_err(), "setup should error when HOME is unset");
    }
}
