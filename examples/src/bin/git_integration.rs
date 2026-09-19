// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Git 集成：把 `git diff` 的变更行映射到受影响符号并做风险分级。
//!
//! 覆盖 CLI：`detect_changes` / `hook`（决策摘要部分）。
//!
//! `detect_changes` 的核心 = 「unified diff 解析 → 变更行区间 → 与
//! Function 节点的 [startLine, endLine] 求交 → 按入边数分级
//! (0=low, 1..4=medium, >=5=high)」。本示例用 `Repository::query`
//! 复现该管线（`parse_unified_diff` 在 crate 内是私有的）。

use std::process::Command;

use codenexus::index::IndexFacade;
use codenexus::query::QueryFacade;
use tempfile::TempDir;

const SAMPLE_CODE: &str = r#"pub fn kept() -> u32 {
    1
}

pub fn edited() -> u32 {
    2
}

pub fn runner() -> u32 {
    edited() + kept()
}
"#;

const EDITED_CODE: &str = r#"pub fn kept() -> u32 {
    1
}

pub fn edited() -> u32 {
    3
}

pub fn added() -> u32 {
    4
}

pub fn runner() -> u32 {
    edited() + kept() + added()
}

pub fn extra() -> u32 {
    added()
}
"#;

/// `git diff --unified=0` 的 `@@ -a,b +c,d @@` → 新文件变更行区间 (start, end)。
fn changed_line_ranges(diff: &str) -> Vec<(u32, u32)> {
    let mut ranges = Vec::new();
    for line in diff.lines() {
        let Some(hunk) = line.strip_prefix("@@ ") else {
            continue;
        };
        let Some((_, new_side)) = hunk.split_once(" +") else {
            continue;
        };
        let spec = new_side.split(" @@").next().unwrap_or("");
        let mut parts = spec.split(',');
        let start: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
        let count: u32 = parts.next().unwrap_or("1").parse().unwrap_or(1);
        if start > 0 {
            ranges.push((start, start + count.saturating_sub(1)));
        }
    }
    ranges
}

fn git(repo: &TempDir, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-C", repo.path().to_str().expect("utf8 path")])
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8 git output")
}

fn main() {
    println!("=== CodeNexus Example: Git Integration (detect_changes/hook) ===\n");

    let repo_dir = TempDir::new().expect("temp dir");
    let src = repo_dir.path().join("lib.rs");
    std::fs::write(&src, SAMPLE_CODE).expect("write sample");

    // git init + baseline commit.
    git(&repo_dir, &["init", "-q"]);
    git(
        &repo_dir,
        &[
            "-c",
            "user.name=demo",
            "-c",
            "user.email=demo@local",
            "add",
            ".",
        ],
    );
    git(
        &repo_dir,
        &[
            "-c",
            "user.name=demo",
            "-c",
            "user.email=demo@local",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );

    // Introduce an edit + an addition (unstaged working-tree changes).
    std::fs::write(&src, EDITED_CODE).expect("write edited sample");

    // Index the CURRENT (edited) state so line numbers match the diff's new side.
    let db_path = repo_dir.path().join("codenexus.lbug");
    let indexer = IndexFacade::new(&db_path).expect("IndexFacade::new");
    indexer
        .index(repo_dir.path(), "git-demo", true)
        .expect("indexing failed");

    // --- detect_changes pipeline: diff -> ranges -> affected symbols -----
    let diff = git(&repo_dir, &["diff", "--unified=0", "--", "lib.rs"]);
    let ranges = changed_line_ranges(&diff);
    println!("=== Changed Line Ranges ({} hunks) ===", ranges.len());
    for (s, e) in &ranges {
        println!("  lib.rs:{s}-{e}");
    }
    println!();

    let query = QueryFacade::new(&db_path).expect("QueryFacade::new");
    let mut classified: Vec<(String, usize)> = Vec::new();
    println!("=== Affected Symbols ===");
    for (start, end) in &ranges {
        let cypher = format!(
            "MATCH (f:Function) WHERE f.filePath ENDS WITH 'lib.rs' \
             AND f.startLine <= {end} AND f.endLine >= {start} \
             RETURN DISTINCT f.name, f.id"
        );
        let qr = query.cypher(&cypher).expect("symbol query");
        for row in &qr.rows {
            let name = row[0].as_str().unwrap_or("?").to_string();
            let id = row[1].as_str().unwrap_or("?").replace('\'', "\\'");
            // Edges live in the `CodeRelation` node table (source/target/type
            // properties) — count incoming CALLS by target id, exactly like
            // the `detect_changes` service does.
            let count_cypher = format!(
                "MATCH (r:CodeRelation) WHERE r.target = '{id}' \
                 AND r.type = 'CALLS' RETURN count(r)"
            );
            let callers = query
                .cypher(&count_cypher)
                .expect("caller count")
                .rows
                .first()
                .and_then(|r| r[0].as_i64())
                .unwrap_or(0) as usize;
            let risk = match callers {
                0 => "low",
                1..=4 => "medium",
                _ => "high",
            };
            println!("  {name:<10} incoming_calls={callers:<3} risk={risk}");
            classified.push((name, callers));
        }
    }
    println!();

    // --- hook decision summary (CLI: `hook` PostToolUse payload) ---------
    let high = classified.iter().filter(|(_, c)| *c >= 5).count();
    let medium = classified
        .iter()
        .filter(|(_, c)| (1..=4).contains(c))
        .count();
    let low = classified.iter().filter(|(_, c)| *c == 0).count();
    println!("=== Hook Summary (would be attached to the JSON decision) ===");
    println!(
        "  {{ \"decision\": \"pass\", \"summary\": {{ \"symbols_affected\": {}, \"high_risk\": {}, \"medium_risk\": {}, \"low_risk\": {} }} }}",
        classified.len(), high, medium, low
    );
}
