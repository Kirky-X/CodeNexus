// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! 代码质量分析三件套：复杂度 / 死代码 / 社区检测。
//!
//! 覆盖 CLI：`complexity` / `dead_code` / `community`。

use codenexus::analysis::community::CommunityDetector;
use codenexus::analysis::complexity::ComplexityAnalyzer;
use codenexus::analysis::dead_code::DeadCodeDetector;
use codenexus::storage::Repository;
use codenexus_examples::{index_sample_code, setup};

const SAMPLE_CODE: &str = r#"
pub fn main() {
    let tokens = parse("index query trace");
    let ok = validate(&tokens);
    let _ = grade(if ok { 95 } else { 40 });
}

pub fn parse(input: &str) -> Vec<String> {
    tokenize(input)
}

fn tokenize(input: &str) -> Vec<String> {
    input.split_whitespace().map(String::from).collect()
}

pub fn validate(tokens: &[String]) -> bool {
    !tokens.is_empty()
}

pub fn grade(score: u32) -> &'static str {
    if score >= 90 {
        "A"
    } else if score >= 80 {
        "B"
    } else if score >= 70 {
        "C"
    } else {
        "F"
    }
}

// Private + never called from any entry point -> dead code findings.
fn orphan_report() -> String {
    unreachable_leaf()
}

fn unreachable_leaf() -> String {
    "never called".to_string()
}
"#;

fn main() {
    println!("=== CodeNexus Example: Code Analysis (complexity/dead_code/community) ===\n");

    let ctx = setup(SAMPLE_CODE);
    let result = index_sample_code(&ctx, "demo");
    let repo = Repository::open(&ctx.db_path).expect("Repository::open");

    // --- 1. Complexity (CLI: `complexity`) -----------------------------
    let entries = ComplexityAnalyzer::new(&repo)
        .analyze(&result.project_id)
        .expect("complexity analyze");

    println!("=== Complexity ({} functions) ===", entries.len());
    let mut sorted = entries;
    sorted.sort_by_key(|e| std::cmp::Reverse(e.cyclomatic));
    for e in sorted.iter().take(5) {
        println!(
            "  {:<18} cyclomatic={:<3} severity={:?} maintainability={:.1}",
            e.name, e.cyclomatic, e.overall_severity, e.maintainability_index
        );
    }
    println!();

    // --- 2. Dead code (CLI: `dead_code`) -------------------------------
    let dead = DeadCodeDetector::new(&repo)
        .detect(&result.project_id, &["main"])
        .expect("dead_code detect");

    println!("=== Dead Code ({} findings) ===", dead.len());
    for e in &dead {
        println!(
            "  {:<18} confidence={:?} reason={}",
            e.name, e.confidence, e.reason
        );
    }
    println!();

    // --- 3. Community detection (CLI: `community`) ---------------------
    let communities = CommunityDetector::new(&repo, &result.project_id)
        .with_resolution(0.5)
        .detect_communities()
        .expect("community detect");

    println!("=== Communities ({} found) ===", communities.len());
    for c in &communities {
        println!(
            "  community #{} modularity={:.3} members={}",
            c.id, c.modularity, c.size
        );
        for m in &c.members {
            println!("     - {m}");
        }
    }
}
