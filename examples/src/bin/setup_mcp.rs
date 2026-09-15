// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! MCP 接入配置：自动探测已安装的 AI coding agent 并写入 MCP server 配置。
//!
//! 覆盖 CLI：`setup`。通过 `run_with_home` 把"家目录"指到临时目录，
//! 演示不污染真实用户配置。

use std::io::Cursor;
use std::path::PathBuf;

use codenexus::service::setup::{run_with_home, Agent};

fn main() {
    println!("=== CodeNexus Example: Setup MCP Config (setup) ===\n");

    // Fake a home directory with Claude Code installed (marker dir `.claude/`).
    let temp = tempfile::TempDir::new().expect("temp dir");
    let fake_home: PathBuf = temp.path().to_path_buf();
    std::fs::create_dir_all(fake_home.join(".claude")).expect("create .claude marker dir");

    // 1. Detect installed agents (CLI: `setup` 的探测阶段).
    let detected = Agent::detect_all(&fake_home);
    println!("=== Detected Agents ===");
    for a in &detected {
        println!("  {a:?}");
    }
    println!();

    // 2. Write the MCP server config for each detected agent.
    //    stdin/stdout are injectable so the flow is testable end to end.
    let mut stdin = Cursor::new(Vec::new());
    let mut stdout = Vec::new();
    let output = run_with_home(&fake_home, true, &mut stdin, &mut stdout).expect("run_with_home");

    println!("=== Configured ===");
    for c in &output.configured {
        println!("  {:?} -> {}", c.agent, c.config_path);
    }
    println!("=== Skipped ===");
    for s in &output.skipped {
        println!("  {:?} -> {} ({})", s.agent, s.config_path, s.reason);
    }

    // 3. Show what was written into the agent's config.
    for c in &output.configured {
        let body = std::fs::read_to_string(&c.config_path).expect("read config");
        println!("\n--- {} ---\n{body}", c.config_path);
    }

    // 4. The server entry that agents will invoke (`codenexus mcp`).
    let entry = codenexus::service::setup::codenexus_mcp_entry();
    println!(
        "=== MCP Entry ===\n  command: {} {:?}",
        entry.command, entry.args
    );
}
