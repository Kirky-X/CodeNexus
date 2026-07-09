// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `setup` subcommand handler (H13, multi-agent-integration spec).
//!
//! Auto-detects installed AI coding agents (Claude Code, Cursor, Codex) by
//! inspecting well-known config directories under `$HOME`, then writes the MCP
//! server configuration pointing at `codenexus mcp` into each detected agent's
//! config file. Existing entries are never silently clobbered — a prompt is
//! issued when an entry pointing to a different binary already exists (unless
//! `--force` is given).
//!
//! # Agent detection
//!
//! Detection is deterministic (Rule 5 — no model involvement):
//!
//! | Agent       | Marker                  | Config file                |
//! |-------------|-------------------------|----------------------------|
//! | Claude Code | `$HOME/.claude/` exists | `$HOME/.claude.json`       |
//! | Cursor      | `$HOME/.cursor/` exists | `$HOME/.cursor/mcp.json`   |
//! | Codex       | `$HOME/.codex/` exists  | `$HOME/.codex/config.json` |
//!
//! # MCP config format
//!
//! All three agents consume the standard MCP server config shape:
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "codenexus": {
//!       "command": "codenexus",
//!       "args": ["mcp"]
//!     }
//!   }
//! }
//! ```
//!
//! The `codenexus` entry is merged into the existing config file (preserving
//! all other keys) so setup never destroys user customisations.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::args::SetupArgs;
use super::error::{CliError, Result};

/// MCP server entry written into agent config files.
///
/// Points the agent at `codenexus mcp` so the agent can launch CodeNexus as a
/// subprocess MCP server (see the `mcp` binary module in `src/mcp/mod.rs`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerEntry {
    /// Binary name (assumes `codenexus` is on `PATH`).
    pub command: String,
    /// Subcommand + flags passed to the binary.
    pub args: Vec<String>,
}

/// The canonical MCP server entry for CodeNexus.
///
/// `command = "codenexus"`, `args = ["mcp"]` — launch the MCP stdio server.
pub fn codenexus_mcp_entry() -> McpServerEntry {
    McpServerEntry {
        command: "codenexus".to_string(),
        args: vec!["mcp".to_string()],
    }
}

/// Supported AI coding agents (H13 spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    /// Anthropic Claude Code (`~/.claude/`).
    ClaudeCode,
    /// Cursor editor (`~/.cursor/`).
    Cursor,
    /// OpenAI Codex CLI (`~/.codex/`).
    Codex,
}

impl Agent {
    /// Human-readable agent name for confirmation messages.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "Claude Code",
            Agent::Cursor => "Cursor",
            Agent::Codex => "Codex",
        }
    }

    /// Marker directory used to detect this agent's installation.
    fn marker_dir(self, home: &Path) -> PathBuf {
        match self {
            Agent::ClaudeCode => home.join(".claude"),
            Agent::Cursor => home.join(".cursor"),
            Agent::Codex => home.join(".codex"),
        }
    }

    /// Config file path into which the MCP entry is merged.
    fn config_path(self, home: &Path) -> PathBuf {
        match self {
            Agent::ClaudeCode => home.join(".claude.json"),
            Agent::Cursor => home.join(".cursor").join("mcp.json"),
            Agent::Codex => home.join(".codex").join("config.json"),
        }
    }

    /// Returns `true` if this agent's marker directory exists under `home`.
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

/// JSON-serializable setup-command output (one entry per configured agent).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SetupOutput {
    /// Agents that were configured.
    pub configured: Vec<ConfiguredAgent>,
    /// Agents that were skipped (already configured with the same entry).
    pub skipped: Vec<SkippedAgent>,
}

/// An agent that received a fresh or updated MCP config entry.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConfiguredAgent {
    /// Agent name.
    pub agent: String,
    /// Config file path written.
    pub config_path: String,
}

/// An agent whose existing config already pointed at codenexus.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SkippedAgent {
    /// Agent name.
    pub agent: String,
    /// Config file path inspected.
    pub config_path: String,
    /// Reason the agent was skipped.
    pub reason: String,
}

/// Runs the `setup` subcommand.
///
/// Detects installed agents under `$HOME`, writes the MCP server config for
/// each, and prints a confirmation. Exits with [`CliError::InvalidInput`] when
/// no agents are detected (per spec: "exit non-zero").
///
/// # Errors
///
/// - [`CliError::InvalidInput`] if `HOME` is unset or no agents are detected.
/// - [`CliError::Io`] if a config file cannot be read or written.
/// - [`CliError::InvalidInput`] if an existing config file is not valid JSON.
pub fn run(args: &SetupArgs) -> Result<()> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| CliError::InvalidInput("HOME environment variable is not set".into()))?;
    run_with_home(
        &home,
        args.force,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout(),
    )
    .map(|_| ())
}

/// Runs setup against a specific `home` directory (testable entry point).
///
/// `stdin` provides confirmation input; `stdout` receives human-readable
/// progress messages. Returns the structured [`SetupOutput`] for callers that
/// want to inspect what was written.
pub fn run_with_home(
    home: &Path,
    force: bool,
    stdin: &mut dyn BufRead,
    stdout: &mut dyn Write,
) -> Result<SetupOutput> {
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
        return Err(CliError::InvalidInput(
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

    Ok(SetupOutput { configured, skipped })
}

/// Outcome of attempting to write the MCP entry into a single agent's config.
#[derive(Debug)]
enum WriteOutcome {
    /// The entry was freshly written or updated.
    Written,
    /// The existing config already pointed at codenexus — no change needed.
    AlreadyConfigured,
    /// The user declined the overwrite prompt.
    Declined,
}

/// Merges the codenexus MCP entry into the config file at `path`.
///
/// - If the file does not exist, creates it with just the codenexus entry.
/// - If the file exists but has no `mcpServers.codenexus` key, adds the entry.
/// - If `mcpServers.codenexus` already matches the canonical entry, no-op.
/// - If `mcpServers.codenexus` points to a different binary, prompt the user
///   (unless `force`); on decline, return [`WriteOutcome::Declined`].
fn write_agent_config(
    path: &Path,
    entry: &McpServerEntry,
    force: bool,
    stdin: &mut dyn BufRead,
    stdout: &mut dyn Write,
) -> Result<WriteOutcome> {
    // Ensure the parent directory exists (e.g. ~/.cursor/ for mcp.json).
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if !path.exists() {
        // Fresh config file — write the codenexus entry directly.
        let mut root = serde_json::Map::new();
        let mut servers = serde_json::Map::new();
        servers.insert("codenexus".to_string(), serde_json::to_value(entry)?);
        root.insert("mcpServers".to_string(), serde_json::Value::Object(servers));
        let json = serde_json::to_string_pretty(&serde_json::Value::Object(root))?;
        std::fs::write(path, json + "\n")?;
        return Ok(WriteOutcome::Written);
    }

    // Existing config — parse, merge, and conditionally overwrite.
    let raw = std::fs::read_to_string(path)?;
    let mut root: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        CliError::InvalidInput(format!(
            "failed to parse existing config at {path}: {e}",
            path = path.display()
        ))
    })?;

    // Navigate to root.mcpServers.codenexus (creating intermediate objects).
    let servers = root
        .as_object_mut()
        .ok_or_else(|| {
            CliError::InvalidInput(format!(
                "config at {path} is not a JSON object",
                path = path.display()
            ))
        })?
        .entry("mcpServers".to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            CliError::InvalidInput(format!(
                "config at {path} has a non-object `mcpServers`",
                path = path.display()
            ))
        })?;

    let existing = servers.get("codenexus");
    match existing {
        None => {
            // No codenexus entry — add it.
            servers.insert("codenexus".to_string(), serde_json::to_value(entry)?);
            write_pretty(path, &root)?;
            Ok(WriteOutcome::Written)
        }
        Some(curr) if curr == &serde_json::to_value(entry)? => {
            // Same entry already present — nothing to do.
            Ok(WriteOutcome::AlreadyConfigured)
        }
        Some(_) => {
            // Different entry — prompt unless --force.
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

/// Writes `value` as pretty-printed JSON + trailing newline to `path`.
fn write_pretty(path: &Path, value: &serde_json::Value) -> Result<()> {
    let json = serde_json::to_string_pretty(value)?;
    std::fs::write(path, json + "\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Creates a fake `$HOME` with the given agent marker directories.
    fn fake_home(agents: &[Agent]) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        for agent in agents {
            std::fs::create_dir_all(agent.marker_dir(dir.path())).unwrap();
        }
        dir
    }

    /// stdin that answers "y" to the overwrite prompt.
    fn yes_stdin() -> Cursor<Vec<u8>> {
        Cursor::new(b"y\n".to_vec())
    }

    /// stdin that answers "n" to the overwrite prompt.
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
        assert_eq!(
            agents,
            vec![Agent::ClaudeCode, Agent::Cursor, Agent::Codex]
        );
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

    // --- write_agent_config: no existing codenexus entry ---

    #[test]
    fn write_agent_config_adds_entry_to_existing_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        // Pre-existing config with another server but no codenexus.
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
        // codenexus was added
        assert_eq!(v["mcpServers"]["codenexus"]["command"], "codenexus");
        // other was preserved
        assert_eq!(v["mcpServers"]["other"]["command"], "other");
    }

    // --- write_agent_config: existing matching entry ---

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

    // --- write_agent_config: existing different entry ---

    #[test]
    fn write_agent_config_prompts_when_different_entry_exists() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        // Existing entry points to a different binary.
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
        // File unchanged
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
        // Empty stdin — force should not read from it.
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let outcome = write_agent_config(&path, &entry, true, &mut stdin, &mut stdout).unwrap();
        assert!(matches!(outcome, WriteOutcome::Written));
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["mcpServers"]["codenexus"]["command"], "codenexus");
    }

    // --- run_with_home: no agents ---

    #[test]
    fn run_with_home_errors_when_no_agents_detected() {
        let home = tempfile::TempDir::new().unwrap();
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let err = run_with_home(home.path(), false, &mut stdin, &mut stdout)
            .expect_err("no agents should error");
        assert!(matches!(err, CliError::InvalidInput(_)));
        assert!(!stdout.is_empty(), "should print guidance");
    }

    // --- run_with_home: single agent ---

    #[test]
    fn run_with_home_configures_single_agent() {
        let home = fake_home(&[Agent::ClaudeCode]);
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let output = run_with_home(home.path(), false, &mut stdin, &mut stdout).unwrap();
        assert_eq!(output.configured.len(), 1);
        assert_eq!(output.configured[0].agent, "Claude Code");
        assert!(output.skipped.is_empty());
        // Config file was created.
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

    // --- run_with_home: idempotent ---

    #[test]
    fn run_with_home_skips_already_configured_agent() {
        let home = fake_home(&[Agent::ClaudeCode]);
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        // First run writes the config.
        run_with_home(home.path(), false, &mut stdin, &mut stdout).unwrap();
        // Second run should skip.
        let mut stdin2 = Cursor::new(Vec::new());
        let mut stdout2 = Vec::new();
        let output = run_with_home(home.path(), false, &mut stdin2, &mut stdout2).unwrap();
        assert!(output.configured.is_empty());
        assert_eq!(output.skipped.len(), 1);
        assert_eq!(output.skipped[0].agent, "Claude Code");
    }

    // --- run_with_home: preserves existing config ---

    #[test]
    fn run_with_home_preserves_existing_mcp_servers() {
        let home = fake_home(&[Agent::ClaudeCode]);
        let config_path = home.path().join(".claude.json");
        // Pre-existing config with another server.
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

    // --- run() wrapper (lines 164-174) ---
    //
    // These tests set/unset `HOME` process-wide. They are safe within this
    // module because no other test here reads `HOME`. The original value is
    // always restored.

    #[test]
    fn run_succeeds_when_home_has_fresh_agent() {
        let home = fake_home(&[Agent::ClaudeCode]);
        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", home.path());
        let args = SetupArgs { force: false };
        let result = run(&args);
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
        let original_home = std::env::var("HOME").ok();
        std::env::remove_var("HOME");
        let args = SetupArgs { force: false };
        let err = run(&args).expect_err("HOME unset should error");
        match original_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        assert!(matches!(err, CliError::InvalidInput(_)));
    }

    // --- run_with_home: Declined branch (lines 241, 244-245, 247-250) ---

    #[test]
    fn run_with_home_declines_overwrite_returns_skipped() {
        let home = fake_home(&[Agent::ClaudeCode]);
        let config_path = home.path().join(".claude.json");
        // Pre-existing config with a different entry → user says "no".
        std::fs::write(
            &config_path,
            r#"{"mcpServers":{"codenexus":{"command":"/old/codenexus","args":["mcp"]}}}"#,
        )
        .unwrap();
        let mut stdin = no_stdin();
        let mut stdout = Vec::new();
        let output = run_with_home(home.path(), false, &mut stdin, &mut stdout).unwrap();
        assert!(output.configured.is_empty(), "nothing should be configured");
        assert_eq!(output.skipped.len(), 1, "agent should be skipped");
        assert_eq!(output.skipped[0].agent, "Claude Code");
        assert_eq!(output.skipped[0].reason, "user declined overwrite");
        // File unchanged.
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["codenexus"]["command"], "/old/codenexus");
    }

    // --- write_agent_config: invalid JSON (lines 302, 304) ---

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
        assert!(matches!(err, CliError::InvalidInput(_)));
    }

    // --- write_agent_config: non-object root (lines 312, 314) ---

    #[test]
    fn write_agent_config_non_object_root_returns_invalid_input() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        // Valid JSON but an array, not an object.
        std::fs::write(&path, r#"[1, 2, 3]"#).unwrap();
        let entry = codenexus_mcp_entry();
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let err = write_agent_config(&path, &entry, false, &mut stdin, &mut stdout)
            .expect_err("non-object root should error");
        assert!(matches!(err, CliError::InvalidInput(_)));
    }

    // --- write_agent_config: non-object mcpServers (lines 321, 323) ---

    #[test]
    fn write_agent_config_non_object_mcp_servers_returns_invalid_input() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        // mcpServers is a string, not an object.
        std::fs::write(&path, r#"{"mcpServers":"not-an-object"}"#).unwrap();
        let entry = codenexus_mcp_entry();
        let mut stdin = Cursor::new(Vec::new());
        let mut stdout = Vec::new();
        let err = write_agent_config(&path, &entry, false, &mut stdin, &mut stdout)
            .expect_err("non-object mcpServers should error");
        assert!(matches!(err, CliError::InvalidInput(_)));
    }
}
