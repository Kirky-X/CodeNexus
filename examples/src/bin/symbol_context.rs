// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! 符号 360° 视图：调用方 / 被调方 / 执行流，以及子图加载与符号消歧。
//!
//! 覆盖 CLI：`context`。

use codenexus::storage::Repository;
use codenexus::trace::context::{
    collect_incoming, collect_outgoing, collect_processes, resolve_start_id, ContextCollector,
};
use codenexus::trace::graph_loader::{load_graph_for_symbol, MAX_SUBGRAPH_NODES};
use codenexus_examples::{index_sample_code, setup};

const SAMPLE_CODE: &str = r#"
pub fn main() {
    let report = build_report("sales");
    let _ = summarize(&report);
}

pub fn build_report(region: &str) -> Vec<String> {
    let rows = fetch_rows(region);
    rows.into_iter().map(format_row).collect()
}

fn fetch_rows(region: &str) -> Vec<String> {
    vec![format!("{region}:r1"), format!("{region}:r2")]
}

fn format_row(row: String) -> String {
    row.to_uppercase()
}

pub fn summarize(report: &[String]) -> usize {
    report.len()
}
"#;

fn main() {
    println!("=== CodeNexus Example: Symbol Context (context) ===\n");

    let ctx = setup(SAMPLE_CODE);
    let result = index_sample_code(&ctx, "demo");

    // --- 1. Collector view: direct DB-backed 360° context ---------------
    let repo = Repository::open(&ctx.db_path).expect("Repository::open");
    let context = ContextCollector::new(&repo)
        .collect(&result.project_id, "build_report")
        .expect("collect context");

    println!("=== ContextCollector: build_report ===");
    println!("  symbol:   {}", context.symbol.name);
    println!("  callers:  {}", context.callers.len());
    println!("  callees:  {}", context.callees.len());
    println!("  tests:    {}", context.test_context.len());
    println!();

    // --- 2. Subgraph view: load + BFS collections ------------------------
    let (graph, truncated) =
        load_graph_for_symbol(&ctx.db_path, "build_report", 1, MAX_SUBGRAPH_NODES, true)
            .expect("load subgraph");

    let start_id = resolve_start_id(&graph, "build_report")
        .expect("resolve")
        .unwrap_or_else(|| panic!("symbol build_report not found in graph"));

    let incoming = collect_incoming(&graph, &start_id);
    let outgoing = collect_outgoing(&graph, &start_id);
    let processes = collect_processes(&graph, &start_id);

    println!("=== Subgraph 360° View (truncated={truncated}) ===");
    println!("  incoming callers:  {}", incoming.len());
    for n in &incoming {
        println!("     <- {}", n.name);
    }
    println!("  outgoing callees:  {}", outgoing.len());
    for n in &outgoing {
        println!("     -> {}", n.name);
    }
    println!("  execution flows:   {}", processes.len());
}
