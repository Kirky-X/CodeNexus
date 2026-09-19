// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::service::error::CodeNexusError;
use crate::service::setup::Agent;

#[cfg(any(feature = "cli", feature = "mcp"))]
use crate::service::error::{to_api_error, wrap_error};
#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::forge;
#[cfg(any(feature = "cli", feature = "mcp"))]
use sdforge::prelude::ApiError;

/// Embedded skill entrypoint.
const SKILL_MD: &str = include_str!("../../skill/SKILL.md");
/// Embedded command reference.
const COMMANDS_MD: &str = include_str!("../../skill/references/commands.md");
/// Embedded appendix.
const APPENDIX_MD: &str = include_str!("../../skill/references/appendix.md");
/// Embedded storage-model reference.
const STORAGE_MODEL_MD: &str = include_str!("../../skill/references/storage-model.md");
/// Embedded workflows reference.
const WORKFLOWS_MD: &str = include_str!("../../skill/references/workflows.md");

/// `(relative path, embedded content)` for every synced file.
const SKILL_FILES: &[(&str, &str)] = &[
    ("SKILL.md", SKILL_MD),
    ("references/commands.md", COMMANDS_MD),
    ("references/appendix.md", APPENDIX_MD),
    ("references/storage-model.md", STORAGE_MODEL_MD),
    ("references/workflows.md", WORKFLOWS_MD),
];

/// Version stamped into every synced file header.
fn version_stamp() -> String {
    format!("<!-- codenexus-sync: v{} -->", env!("CARGO_PKG_VERSION"))
}

/// Content for one synced file: stamp + embedded body.
fn stamped_content(body: &str) -> String {
    format!("{}\n{}", version_stamp(), body)
}

/// JSON-serializable skill-sync output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SkillSyncOutput {
    /// Files written this run (5 entries per synced agent).
    pub written: Vec<SkillFileTarget>,
    /// Files already up to date (identical stamped content).
    pub skipped: Vec<SkillFileTarget>,
    /// Agents whose differing docs were not updated (user declined).
    pub declined: Vec<SkillAgentTarget>,
}

/// One synced file's record.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SkillFileTarget {
    pub agent: String,
    pub file: String,
    pub path: String,
}

/// One declined agent's record.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SkillAgentTarget {
    pub agent: String,
    pub skill_dir: String,
}

/// Outcome of syncing a single agent.
#[derive(Debug)]
enum SyncOutcome {
    Written,
    Skipped,
    Declined,
}

/// Parses the `--target` CLI value: `auto` (default) detects installed
/// agents; otherwise a case-insensitive agent name ("claude-code",
/// "cursor", "codex").
fn parse_targets(raw: &str, home: &Path) -> Result<Vec<Agent>, CodeNexusError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
        return Ok(Agent::detect_all(home));
    }
    let agent = match trimmed.to_ascii_lowercase().as_str() {
        "claude-code" | "claudecode" | "claude" | "claude code" => Agent::ClaudeCode,
        "cursor" => Agent::Cursor,
        "codex" => Agent::Codex,
        other => {
            return Err(CodeNexusError::InvalidInput(format!(
                "unknown --target '{other}' (expected auto|claude-code|cursor|codex)"
            )))
        }
    };
    Ok(vec![agent])
}

/// Runs the sync against a specific `home` directory with injected I/O
/// (testable core).
pub fn run_skill_sync_with_io(
    home: &Path,
    force: bool,
    targets: &[Agent],
    stdin: &mut dyn BufRead,
    stdout: &mut dyn Write,
) -> Result<SkillSyncOutput, CodeNexusError> {
    let mut output = SkillSyncOutput {
        written: Vec::new(),
        skipped: Vec::new(),
        declined: Vec::new(),
    };
    for agent in targets {
        let outcome = sync_agent(home, *agent, force, stdin, stdout)?;
        let dir = agent.skill_dir(home);
        match outcome {
            SyncOutcome::Written => {
                for (file, _) in SKILL_FILES {
                    output.written.push(SkillFileTarget {
                        agent: agent.name().to_string(),
                        file: (*file).to_string(),
                        path: dir.join(file).to_string_lossy().to_string(),
                    });
                }
            }
            SyncOutcome::Skipped => {
                for (file, _) in SKILL_FILES {
                    output.skipped.push(SkillFileTarget {
                        agent: agent.name().to_string(),
                        file: (*file).to_string(),
                        path: dir.join(file).to_string_lossy().to_string(),
                    });
                }
            }
            SyncOutcome::Declined => output.declined.push(SkillAgentTarget {
                agent: agent.name().to_string(),
                skill_dir: dir.to_string_lossy().to_string(),
            }),
        }
    }
    Ok(output)
}

/// Syncs one agent's skill directory.
fn sync_agent(
    home: &Path,
    agent: Agent,
    force: bool,
    stdin: &mut dyn BufRead,
    stdout: &mut dyn Write,
) -> Result<SyncOutcome, CodeNexusError> {
    let dir = agent.skill_dir(home);
    let desired: Vec<(String, String)> = SKILL_FILES
        .iter()
        .map(|(file, body)| ((*file).to_string(), stamped_content(body)))
        .collect();

    let all_current = desired.iter().all(|(file, content)| {
        let path = dir.join(file);
        std::fs::read_to_string(&path).is_ok_and(|existing| existing == *content)
    });
    if all_current {
        return Ok(SyncOutcome::Skipped);
    }

    if !force {
        write!(
            stdout,
            "Install/update CodeNexus skill docs at {dir}? [y/N] ",
            dir = dir.display()
        )?;
        stdout.flush()?;
        let mut line = String::new();
        stdin.read_line(&mut line)?;
        let answer = line.trim().to_ascii_lowercase();
        if answer != "y" && answer != "yes" {
            return Ok(SyncOutcome::Declined);
        }
    }

    for (file, content) in &desired {
        let path = dir.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
    }
    Ok(SyncOutcome::Written)
}

/// Runs the sync against the real user home (non-interactive entry).
pub fn run_skill_sync(
    home: &Path,
    force: bool,
    targets: &[Agent],
) -> Result<SkillSyncOutput, CodeNexusError> {
    run_skill_sync_with_io(
        home,
        force,
        targets,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout(),
    )
}

/// CLI wrapper — prints result to stdout as JSON.
#[cfg(feature = "cli")]
#[forge(
    name = "skill",
    version = "0.4.0",
    description = "Sync the bundled CodeNexus skill docs into installed AI agents' global skill directories (~/.claude, ~/.cursor, ~/.codex). Params: force — overwrite differing docs without prompting; target — auto|claude-code|cursor|codex (default auto = all detected).",
    cli = true
)]
async fn skill(force: bool, target: String) -> Result<(), ApiError> {
    let home = home_dir().map_err(|e| to_api_error(e, "skill_error"))?;
    let targets = parse_targets(&target, &home).map_err(|e| to_api_error(e, "skill_error"))?;
    let output =
        run_skill_sync(&home, force, &targets).map_err(|e| to_api_error(e, "skill_error"))?;
    let json =
        serde_json::to_string(&output).map_err(|e| wrap_error("JSON serialization failed", e))?;
    println!("{json}");
    Ok(())
}

/// Resolves the user home directory (`$HOME`).
fn home_dir() -> Result<PathBuf, CodeNexusError> {
    std::env::var("HOME").map(PathBuf::from).map_err(|_| {
        CodeNexusError::InvalidInput("HOME environment variable is not set".to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::Mutex;

    /// Serializes tests that touch process-global stdin/stdout state.
    static IO_LOCK: Mutex<()> = Mutex::new(());

    fn fake_home(agents: &[Agent]) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        for agent in agents {
            std::fs::create_dir_all(agent.marker_dir(dir.path())).unwrap();
        }
        dir
    }

    fn sync(
        home: &Path,
        force: bool,
        targets: &[Agent],
        answers: &str,
    ) -> Result<SkillSyncOutput, CodeNexusError> {
        let mut stdin = Cursor::new(answers.to_string());
        let mut stdout = Vec::new();
        run_skill_sync_with_io(home, force, targets, &mut stdin, &mut stdout)
    }

    #[test]
    fn writes_to_detected_agent_only() {
        let _guard = IO_LOCK.lock().unwrap();
        let home = fake_home(&[Agent::ClaudeCode]);
        let targets = parse_targets("auto", home.path()).expect("auto detect");
        assert_eq!(
            targets,
            vec![Agent::ClaudeCode],
            "auto must detect installed agents"
        );
        let out = sync(home.path(), true, &targets, "").expect("sync");
        assert_eq!(out.written.len(), 5, "SKILL.md + 4 references");
        assert!(out.written.iter().all(|w| w.agent == "Claude Code"));
        assert!(out.skipped.is_empty());
        assert!(out.declined.is_empty());
        let skill_md = home.path().join(".claude/skills/codenexus/SKILL.md");
        let content = std::fs::read_to_string(skill_md).unwrap();
        assert!(
            content.starts_with("<!-- codenexus-sync: v"),
            "stamp must be the first line"
        );
        assert!(content.contains("codenexus"), "embedded skill body present");
    }

    #[test]
    fn identical_resync_is_skipped() {
        let _guard = IO_LOCK.lock().unwrap();
        let home = fake_home(&[Agent::Codex]);
        let targets = [Agent::Codex];
        sync(home.path(), true, &targets, "").expect("first sync");
        let out = sync(home.path(), true, &targets, "").expect("second sync");
        assert!(out.written.is_empty());
        assert_eq!(out.skipped.len(), 5);
        assert!(out.declined.is_empty());
    }

    #[test]
    fn differing_docs_declined_without_force_and_written_with_force() {
        let _guard = IO_LOCK.lock().unwrap();
        let home = fake_home(&[Agent::Cursor]);
        let targets = [Agent::Cursor];
        sync(home.path(), true, &targets, "").expect("first sync");
        // Mutate one file so contents differ.
        let target = home
            .path()
            .join(".cursor/skills/codenexus/references/commands.md");
        std::fs::write(&target, "outdated").unwrap();

        let declined = sync(home.path(), false, &targets, "").expect("non-interactive sync");
        assert!(declined.written.is_empty());
        assert_eq!(declined.declined.len(), 1);
        assert_eq!(declined.declined[0].agent, "Cursor");

        let forced = sync(home.path(), true, &targets, "").expect("forced sync");
        assert_eq!(forced.written.len(), 5);
        assert!(forced.declined.is_empty());
    }

    #[test]
    fn interactive_yes_overwrites() {
        let _guard = IO_LOCK.lock().unwrap();
        let home = fake_home(&[Agent::Codex]);
        let targets = [Agent::Codex];
        sync(home.path(), true, &targets, "").expect("first sync");
        let target = home.path().join(".codex/skills/codenexus/SKILL.md");
        std::fs::write(&target, "stale").unwrap();
        let out = sync(home.path(), false, &targets, "y\n").expect("answered sync");
        assert_eq!(out.written.len(), 5);
        assert_eq!(
            std::fs::read_to_string(target)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
            version_stamp()
        );
    }

    #[test]
    fn explicit_target_bypasses_detection() {
        let _guard = IO_LOCK.lock().unwrap();
        let home = fake_home(&[]); // no agents installed
        let targets = parse_targets("cursor", home.path()).expect("parse target");
        assert_eq!(targets, vec![Agent::Cursor]);
        let out = sync(home.path(), true, &targets, "").expect("sync");
        assert_eq!(out.written.len(), 5);
        assert!(home
            .path()
            .join(".cursor/skills/codenexus/SKILL.md")
            .exists());
    }

    #[test]
    fn unknown_target_is_invalid_input() {
        let home = fake_home(&[]);
        let err = parse_targets("bob", home.path()).expect_err("unknown target");
        assert!(matches!(err, CodeNexusError::InvalidInput(_)));
        assert!(err.to_string().contains("unknown --target"));
    }

    #[test]
    fn auto_target_on_empty_home_detects_nothing() {
        let home = fake_home(&[]);
        let targets = parse_targets("auto", home.path()).expect("parse auto");
        assert!(targets.is_empty());
        let out = sync(home.path(), true, &targets, "").expect("no-op sync");
        assert!(out.written.is_empty() && out.skipped.is_empty() && out.declined.is_empty());
    }
}
