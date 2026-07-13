// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! `hook` service: read a git hook payload from stdin and emit a JSON
//! decision.

use std::io::BufRead;

use serde::{Deserialize, Serialize};

use crate::service::error::CodeNexusError;
#[cfg(feature = "cli")]
use crate::service::error::to_api_error;
use crate::kit::{AsyncKit, AsyncReady, StorageModule};
#[cfg(feature = "cli")]
use crate::service::error::kit_not_initialized;
#[cfg(feature = "cli")]
use crate::service::runtime::kit;

#[cfg(feature = "cli")]
use sdforge::prelude::ApiError;
#[cfg(feature = "cli")]
use sdforge::forge;

/// JSON-serializable hook decision output.
///
/// `decision` is always `"pass"` — the hook never blocks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookDecision {
    pub decision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<HookSummary>,
}

/// Summary emitted for PostToolUse after a `codenexus rename`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookSummary {
    pub symbols_affected: usize,
    pub high_risk: usize,
    pub medium_risk: usize,
    pub low_risk: usize,
}

/// Parsed hook payload from the agent.
#[derive(Debug, Clone, Deserialize)]
struct HookPayload {
    #[serde(default)]
    tool_name: String,
    #[serde(default)]
    phase: String,
}

/// Builds the hook decision from the raw stdin payload.
fn build_decision(kit: &AsyncKit<AsyncReady>, raw: &str) -> HookDecision {
    let payload: HookPayload = match serde_json::from_str(raw) {
        Ok(p) => p,
        Err(_) => {
            return HookDecision {
                decision: "pass".to_string(),
                summary: None,
            };
        }
    };

    // Never intercept Read.
    if payload.tool_name == "Read" {
        return HookDecision {
            decision: "pass".to_string(),
            summary: None,
        };
    }

    // PostToolUse after a rename → emit a summary.
    if payload.phase == "PostToolUse" && payload.tool_name.contains("rename") {
        if let Ok(summary) = summarize_rename(kit) {
            return HookDecision {
                decision: "pass".to_string(),
                summary: Some(summary),
            };
        }
    }

    HookDecision {
        decision: "pass".to_string(),
        summary: None,
    }
}

/// Queries the database for a rename summary.
fn summarize_rename(kit: &AsyncKit<AsyncReady>) -> std::result::Result<HookSummary, CodeNexusError> {
    let storage = kit.require::<StorageModule>()?;
    let rows = storage.query("MATCH (n:Function) RETURN count(n) AS total")?;
    let total = rows
        .first()
        .and_then(|row| row.first())
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    let rows = storage.query(
        "MATCH (n:Function) \
         OPTIONAL MATCH (r:CodeRelation) WHERE r.target = n.id \
         WITH n, count(r) AS incoming \
         RETURN incoming;",
    )?;
    let mut high_risk = 0;
    let mut medium_risk = 0;
    let mut low_risk = 0;
    for row in &rows {
        let incoming = row
            .first()
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

/// Reads stdin, builds the hook decision, and prints JSON.
#[cfg(any(feature = "cli", test))]
fn run(kit: &AsyncKit<AsyncReady>) -> Result<(), CodeNexusError> {
    let stdin = std::io::stdin();
    let mut input = String::new();
    stdin.lock().read_line(&mut input)?;
    let decision = build_decision(kit, &input);
    let json = serde_json::to_string(&decision)?;
    println!("{json}");
    Ok(())
}

/// CLI wrapper — reads stdin, prints the decision as JSON.
#[cfg(feature = "cli")]
#[forge(
    name = "hook",
    version = "0.3.2",
    description = "Read a git hook payload from stdin and emit a JSON decision.",
    cli = true
)]
async fn hook() -> Result<(), ApiError> {
    let kit = kit().ok_or_else(kit_not_initialized)?;
    run(&kit).map_err(|e| to_api_error(e, "hook_error"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::{build_kit, AsyncKit, AsyncReady, KitBootstrapConfig};

    fn fresh_kit() -> (tempfile::TempDir, AsyncKit<AsyncReady>) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hook_test.lbug");
        let kit = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(build_kit(&KitBootstrapConfig::new(path)))
            .expect("build_kit");
        (dir, kit)
    }

    // --- decision is always "pass" ---

    #[test]
    fn build_decision_pre_tool_use_returns_pass() {
        let (_dir, kit) = fresh_kit();
        let raw = r#"{"tool_name":"Bash","tool_input":{"command":"ls"},"phase":"PreToolUse"}"#;
        let decision = build_decision(&kit, raw);
        assert_eq!(decision.decision, "pass");
        assert!(decision.summary.is_none());
    }

    #[test]
    fn build_decision_post_tool_use_non_rename_returns_pass() {
        let (_dir, kit) = fresh_kit();
        let raw = r#"{"tool_name":"Bash","tool_input":{"command":"ls"},"phase":"PostToolUse"}"#;
        let decision = build_decision(&kit, raw);
        assert_eq!(decision.decision, "pass");
        assert!(decision.summary.is_none());
    }

    #[test]
    fn build_decision_read_tool_returns_noop_pass() {
        let (_dir, kit) = fresh_kit();
        let raw =
            r#"{"tool_name":"Read","tool_input":{"path":"/etc/passwd"},"phase":"PreToolUse"}"#;
        let decision = build_decision(&kit, raw);
        assert_eq!(decision.decision, "pass");
        assert!(
            decision.summary.is_none(),
            "Read must not produce a summary"
        );
    }

    #[test]
    fn build_decision_never_returns_block() {
        let (_dir, kit) = fresh_kit();
        let raw = r#"{"tool_name":"rm","tool_input":{"args":["-rf","/"]},"phase":"PreToolUse"}"#;
        let decision = build_decision(&kit, raw);
        assert_ne!(decision.decision, "block");
        assert_eq!(decision.decision, "pass");
    }

    #[test]
    fn build_decision_invalid_json_returns_noop_pass() {
        let (_dir, kit) = fresh_kit();
        let decision = build_decision(&kit, "not json");
        assert_eq!(decision.decision, "pass");
        assert!(decision.summary.is_none());
    }

    #[test]
    fn build_decision_empty_input_returns_noop_pass() {
        let (_dir, kit) = fresh_kit();
        let decision = build_decision(&kit, "");
        assert_eq!(decision.decision, "pass");
    }

    // --- PostToolUse rename → summary ---

    #[test]
    fn build_decision_post_tool_use_rename_emits_summary() {
        let (_dir, kit) = fresh_kit();
        let storage = kit.require::<StorageModule>().expect("require_storage");
        let node =
            crate::model::Node::builder(crate::model::NodeLabel::Function, "parse", "demo.parse")
                .id("f1")
                .project("demo")
                .file_path("/src/a.rs")
                .start_line(1)
                .end_line(5)
                .language(crate::model::Language::Rust)
                .build();
        storage
            .save_nodes(
                std::slice::from_ref(&node),
                crate::model::NodeLabel::Function,
            )
            .expect("save_nodes");
        let raw = r#"{"tool_name":"codenexus rename","tool_input":{"from":"parse","to":"parse_file"},"phase":"PostToolUse"}"#;
        let decision = build_decision(&kit, raw);
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
    fn run_with_empty_stdin_returns_ok() {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            eprintln!("skipping: stdin is a TTY (would block)");
            return;
        }
        let (_dir, kit) = fresh_kit();
        let result = run(&kit);
        assert!(
            result.is_ok(),
            "run with empty stdin should return Ok: {:?}",
            result.err()
        );
    }

    // --- summarize_rename risk classification ---

    fn fn_node(id: &str) -> crate::model::Node {
        crate::model::Node::builder(crate::model::NodeLabel::Function, id, format!("demo.{id}"))
            .id(id)
            .project("demo")
            .file_path("/src/demo.rs")
            .start_line(1)
            .end_line(10)
            .language(crate::model::Language::Rust)
            .build()
    }

    #[test]
    fn summarize_rename_classifies_low_medium_high_risk() {
        let (_dir, kit) = fresh_kit();
        let storage = kit.require::<StorageModule>().expect("require_storage");

        storage
            .save_nodes(
                &[fn_node("f_low"), fn_node("f_med"), fn_node("f_high")],
                crate::model::NodeLabel::Function,
            )
            .expect("save_nodes");

        let edges: Vec<crate::model::Edge> = [
            ("c1", "f_med"),
            ("c2", "f_med"),
            ("c3", "f_high"),
            ("c4", "f_high"),
            ("c5", "f_high"),
            ("c6", "f_high"),
            ("c7", "f_high"),
        ]
        .iter()
        .map(|(s, t)| {
            crate::model::Edge::builder(*s, *t, crate::model::EdgeType::Calls, "demo").build()
        })
        .collect();
        storage.save_edges(&edges).expect("save_edges");

        let summary = summarize_rename(&kit).expect("summarize_rename");
        assert_eq!(summary.symbols_affected, 3, "3 function nodes");
        assert_eq!(summary.low_risk, 1, "f_low has 0 incoming → low");
        assert_eq!(summary.medium_risk, 1, "f_med has 2 incoming → medium");
        assert_eq!(summary.high_risk, 1, "f_high has 5 incoming → high");
    }

    #[test]
    fn summarize_rename_empty_db_returns_zero_counts() {
        let (_dir, kit) = fresh_kit();
        let summary = summarize_rename(&kit).expect("summarize_rename on empty DB");
        assert_eq!(summary.symbols_affected, 0);
        assert_eq!(summary.high_risk, 0);
        assert_eq!(summary.medium_risk, 0);
        assert_eq!(summary.low_risk, 0);
    }

    #[test]
    fn build_decision_post_tool_use_rename_classifies_risk() {
        let (_dir, kit) = fresh_kit();
        let storage = kit.require::<StorageModule>().expect("require_storage");

        storage
            .save_nodes(&[fn_node("f_risky")], crate::model::NodeLabel::Function)
            .expect("save_nodes");
        let edges: Vec<crate::model::Edge> = (1..=5)
            .map(|i| {
                crate::model::Edge::builder(
                    format!("caller{i}"),
                    "f_risky",
                    crate::model::EdgeType::Calls,
                    "demo",
                )
                .build()
            })
            .collect();
        storage.save_edges(&edges).expect("save_edges");

        let raw = r#"{"tool_name":"codenexus rename","tool_input":{"from":"f_risky","to":"f_safe"},"phase":"PostToolUse"}"#;
        let decision = build_decision(&kit, raw);
        assert_eq!(decision.decision, "pass");
        let summary = decision.summary.expect("rename should produce summary");
        assert!(summary.symbols_affected >= 1);
        assert!(summary.high_risk >= 1, "5 incoming → high_risk >= 1");
    }

    // --- hook() CLI wrapper ---

    #[cfg(feature = "cli")]
    #[test]
    fn hook_returns_err_when_kit_not_initialized() {
        crate::service::runtime::reset_kit_for_testing();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(hook());
        assert!(
            result.is_err(),
            "hook should error when global kit is not initialized"
        );
        crate::service::runtime::reset_kit_for_testing();
    }

    #[cfg(feature = "cli")]
    #[test]
    fn hook_succeeds_when_kit_initialized_and_stdin_not_tty() {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            eprintln!("skipping: stdin is a TTY (would block)");
            return;
        }
        crate::service::runtime::reset_kit_for_testing();
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("hook_cli_test.lbug");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let kit = rt
            .block_on(build_kit(&KitBootstrapConfig::new(path)))
            .expect("build_kit");
        crate::service::runtime::init_kit(kit).expect("init_kit");
        let result = rt.block_on(hook());
        assert!(result.is_ok(), "hook should succeed: {:?}", result.err());
        crate::service::runtime::reset_kit_for_testing();
    }
}
