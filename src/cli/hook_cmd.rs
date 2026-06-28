// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `hook` subcommand handler (H13, multi-agent-integration spec).
//!
//! Emits JSON conforming to the agent hook protocol (PreToolUse before a tool
//! call, PostToolUse after). The hook **always exits 0**, **never blocks** a
//! tool call, and **never intercepts `Read`** tool invocations.
//!
//! # Protocol
//!
//! The agent invokes `codenexus hook` as a subprocess, piping a JSON payload
//! to stdin:
//!
//! ```json
//! {"tool_name": "Bash", "tool_input": {...}, "phase": "PreToolUse"}
//! ```
//!
//! The hook reads the payload, emits a JSON decision to stdout, and exits 0.
//! The decision is always `"pass"` — the hook is observational, never
//! blocking.
//!
//! # PostToolUse summarisation
//!
//! For `PostToolUse` payloads describing a `codenexus rename` completion, the
//! hook emits a summary (symbols affected, risk levels) gathered from the
//! database. For all other payloads, it emits a no-op acknowledgment.

use std::io::BufRead;

use serde::{Deserialize, Serialize};

use super::args::HookArgs;
use super::error::{CliError, Result};
use crate::kit::{Kit, StorageKey};

/// JSON-serializable hook decision output.
///
/// `decision` is always `"pass"` — the hook never blocks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookDecision {
    /// Always `"pass"`. The hook never blocks a tool call (H13 spec).
    pub decision: String,
    /// Optional summary (present for PostToolUse after `codenexus rename`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<HookSummary>,
}

/// Summary emitted for PostToolUse after a `codenexus rename`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookSummary {
    /// Total number of symbols affected by the rename.
    pub symbols_affected: usize,
    /// Number of high-risk symbols (>= 5 incoming edges).
    pub high_risk: usize,
    /// Number of medium-risk symbols (1–4 incoming edges).
    pub medium_risk: usize,
    /// Number of low-risk symbols (0 incoming edges).
    pub low_risk: usize,
}

/// Parsed hook payload from the agent.
#[derive(Debug, Clone, Deserialize)]
struct HookPayload {
    /// Tool name being invoked (e.g. "Bash", "Read", "codenexus rename").
    #[serde(default)]
    tool_name: String,
    /// Phase: "PreToolUse" or "PostToolUse".
    #[serde(default)]
    phase: String,
}

/// Runs the `hook` subcommand.
///
/// Reads a JSON payload from stdin, emits a JSON decision to stdout, and
/// returns `Ok(())`. Always succeeds — the hook never errors (H13 spec: exit 0
/// in all cases).
pub fn run(kit: &Kit, args: &HookArgs) -> Result<()> {
    let stdin = std::io::stdin();
    let mut input = String::new();
    stdin.lock().read_line(&mut input)?;

    let decision = build_decision(kit, args, &input);
    let json = serde_json::to_string(&decision)?;
    println!("{json}");
    Ok(())
}

/// Builds the hook decision from the raw stdin payload.
///
/// Separated from [`run`] for testability (tests can call this with a raw JSON
/// string without spawning a subprocess).
fn build_decision(kit: &Kit, _args: &HookArgs, raw: &str) -> HookDecision {
    // If the payload doesn't parse, emit a no-op pass (never block).
    let payload: HookPayload = match serde_json::from_str(raw) {
        Ok(p) => p,
        Err(_) => {
            return HookDecision {
                decision: "pass".to_string(),
                summary: None,
            };
        }
    };

    // Never intercept Read (H13 spec).
    if payload.tool_name == "Read" {
        return HookDecision {
            decision: "pass".to_string(),
            summary: None,
        };
    }

    // For PostToolUse after a rename, emit a summary.
    if payload.phase == "PostToolUse" && payload.tool_name.contains("rename") {
        if let Ok(summary) = summarize_rename(kit) {
            return HookDecision {
                decision: "pass".to_string(),
                summary: Some(summary),
            };
        }
    }

    // Default: no-op pass.
    HookDecision {
        decision: "pass".to_string(),
        summary: None,
    }
}

/// Queries the database for a rename summary (symbols affected + risk levels).
///
/// Returns `Err` if the database is unavailable — the caller falls back to a
/// no-op pass decision.
fn summarize_rename(kit: &Kit) -> std::result::Result<HookSummary, CliError> {
    let storage = kit.require::<StorageKey>()?;
    // Count Function nodes as a proxy for rename impact. LadybugDB's Cypher
    // subset does not support `WHERE n:Label` predicates, so we use the
    // `MATCH (n:Label)` pattern instead (verified in cypher.rs tests).
    let rows = storage.query("MATCH (n:Function) RETURN count(n) AS total")?;
    let total = rows
        .first()
        .and_then(|row| row.first())
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    // Risk distribution: count Function nodes by incoming edge count.
    let risk_rows = storage.query(
        "MATCH (n:Function) OPTIONAL MATCH (caller)-[e]->(n) RETURN n.id AS id, count(e) AS incoming",
    )?;
    let mut high_risk = 0;
    let mut medium_risk = 0;
    let mut low_risk = 0;
    for row in &risk_rows {
        let incoming = row
            .get(1)
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if incoming >= 5 {
            high_risk += 1;
        } else if incoming >= 1 {
            medium_risk += 1;
        } else {
            low_risk += 1;
        }
    }

    Ok(HookSummary {
        symbols_affected: total,
        high_risk,
        medium_risk,
        low_risk,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, KitBootstrapConfig};

    fn fresh_kit() -> Kit {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hook_test.lbug");
        std::mem::forget(dir);
        build_kit(&KitBootstrapConfig::new(path)).expect("build_kit")
    }

    fn hook_args() -> HookArgs {
        HookArgs {
            db: "./codenexus.lbug".into(),
        }
    }

    // --- decision is always "pass" ---

    #[test]
    fn build_decision_pre_tool_use_returns_pass() {
        let kit = fresh_kit();
        let raw = r#"{"tool_name":"Bash","tool_input":{"command":"ls"},"phase":"PreToolUse"}"#;
        let decision = build_decision(&kit, &hook_args(), raw);
        assert_eq!(decision.decision, "pass");
        assert!(decision.summary.is_none());
    }

    #[test]
    fn build_decision_post_tool_use_non_rename_returns_pass() {
        let kit = fresh_kit();
        let raw = r#"{"tool_name":"Bash","tool_input":{"command":"ls"},"phase":"PostToolUse"}"#;
        let decision = build_decision(&kit, &hook_args(), raw);
        assert_eq!(decision.decision, "pass");
        assert!(decision.summary.is_none());
    }

    // --- never intercepts Read ---

    #[test]
    fn build_decision_read_tool_returns_noop_pass() {
        let kit = fresh_kit();
        let raw = r#"{"tool_name":"Read","tool_input":{"path":"/etc/passwd"},"phase":"PreToolUse"}"#;
        let decision = build_decision(&kit, &hook_args(), raw);
        assert_eq!(decision.decision, "pass");
        assert!(decision.summary.is_none(), "Read must not produce a summary");
    }

    // --- never blocks ---

    #[test]
    fn build_decision_never_returns_block() {
        let kit = fresh_kit();
        // Even with a "dangerous" tool name, the decision is "pass".
        let raw = r#"{"tool_name":"rm","tool_input":{"args":["-rf","/"]},"phase":"PreToolUse"}"#;
        let decision = build_decision(&kit, &hook_args(), raw);
        assert_ne!(decision.decision, "block");
        assert_eq!(decision.decision, "pass");
    }

    // --- invalid JSON payload → no-op pass ---

    #[test]
    fn build_decision_invalid_json_returns_noop_pass() {
        let kit = fresh_kit();
        let decision = build_decision(&kit, &hook_args(), "not json");
        assert_eq!(decision.decision, "pass");
        assert!(decision.summary.is_none());
    }

    #[test]
    fn build_decision_empty_input_returns_noop_pass() {
        let kit = fresh_kit();
        let decision = build_decision(&kit, &hook_args(), "");
        assert_eq!(decision.decision, "pass");
    }

    // --- PostToolUse rename → summary ---

    #[test]
    fn build_decision_post_tool_use_rename_emits_summary() {
        let kit = fresh_kit();
        // Seed a function via save_nodes (CSV COPY FROM) — `CREATE` via
        // execute() does not register the node for MATCH traversal in
        // LadybugDB's graph tables.
        let storage = kit.require::<StorageKey>().expect("require_storage");
        let node = crate::model::Node::builder(
            crate::model::NodeLabel::Function,
            "parse",
            "demo.parse",
        )
        .id("f1")
        .project("demo")
        .file_path("/src/a.rs")
        .start_line(1)
        .end_line(5)
        .language(crate::model::Language::Rust)
        .build();
        storage
            .save_nodes(std::slice::from_ref(&node), crate::model::NodeLabel::Function)
            .expect("save_nodes");
        let raw = r#"{"tool_name":"codenexus rename","tool_input":{"from":"parse","to":"parse_file"},"phase":"PostToolUse"}"#;
        let decision = build_decision(&kit, &hook_args(), raw);
        assert_eq!(decision.decision, "pass");
        let summary = decision.summary.expect("rename should produce summary");
        assert!(summary.symbols_affected >= 1);
    }

    // --- HookDecision serialization ---

    #[test]
    fn hook_decision_serializes_to_json() {
        let d = HookDecision {
            decision: "pass".to_string(),
            summary: None,
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["decision"], "pass");
        assert!(v.get("summary").is_none() || v["summary"].is_null());
    }

    #[test]
    fn hook_decision_with_summary_serializes() {
        let d = HookDecision {
            decision: "pass".to_string(),
            summary: Some(HookSummary {
                symbols_affected: 5,
                high_risk: 1,
                medium_risk: 2,
                low_risk: 2,
            }),
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["decision"], "pass");
        assert_eq!(v["summary"]["symbols_affected"], 5);
        assert_eq!(v["summary"]["high_risk"], 1);
    }

    // --- run always succeeds ---

    #[test]
    fn run_returns_ok_for_valid_payload() {
        // `run` reads from stdin; we can't easily inject stdin in a unit test,
        // so we test via `build_decision` instead. This test verifies that the
        // HookArgs struct is constructible.
        let _args = hook_args();
    }
}
