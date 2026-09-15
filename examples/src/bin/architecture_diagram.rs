// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! 架构总览 + 自包含交互式架构图 HTML + 双项目架构 diff。
//!
//! 覆盖 CLI：`architecture` / `diagram` / `arch_diff`。

use codenexus::analysis::architecture::ArchitectureAnalyzer;
use codenexus::diagram::{self, QualityProfile};
use codenexus::service::project::resolve_project_id;
use codenexus::storage::Repository;
use codenexus_examples::{setup, ExampleContext};
use std::path::PathBuf;

const SAMPLE_CODE: &str = r#"
pub mod api {
    pub fn list_projects() -> Vec<&'static str> {
        vec!["alpha"]
    }
}

pub mod core {
    pub fn index_dir() -> u32 {
        42
    }
}
"#;

/// 构造单个项目的架构 DiagramDocument（arch_diff 的 Before/After 输入）。
fn project_document(repo: &Repository, project: &str) -> diagram::DiagramDocument {
    let analyzer = ArchitectureAnalyzer::new(repo);
    let overview = analyzer.overview(project).expect("overview");
    let layer_map = analyzer.module_layer_map(project).expect("layer map");
    let mut doc = diagram::from_overview(&overview, &layer_map, project);
    doc.meta.locale = "en".into();
    doc
}

fn index_project(ctx: &ExampleContext, name: &str, extra_file: Option<(&str, &str)>) {
    if let Some((file, content)) = extra_file {
        std::fs::write(ctx.source_dir.join(file), content).expect("write extra file");
    }
    index_silent(ctx, name);
}

fn index_silent(ctx: &ExampleContext, name: &str) {
    use codenexus::index::IndexFacade;
    let indexer = IndexFacade::new(&ctx.db_path).expect("IndexFacade::new");
    indexer
        .index(&ctx.source_dir, name, true)
        .expect("indexing failed");
}

fn main() {
    println!("=== CodeNexus Example: Architecture Overview + Diagram + Arch Diff ===\n");

    let ctx = setup(SAMPLE_CODE);

    // Index "main" and a "feature" variant (one extra module) in the same DB.
    index_project(&ctx, "main", None);
    index_project(
        &ctx,
        "feature",
        Some(("billing.rs", "pub fn charge() -> u32 { 1 }\n")),
    );

    let repo = Repository::open(&ctx.db_path).expect("Repository::open");

    // --- 1. Architecture overview (CLI: `architecture`) -----------------
    let analyzer = ArchitectureAnalyzer::new(&repo);
    let pid = resolve_project_id(&repo, "main").expect("resolve main");
    let overview = analyzer.overview(&pid).expect("overview");

    println!("=== Architecture Overview (main) ===");
    println!("  languages:      {}", overview.languages.len());
    println!("  packages:       {}", overview.packages.len());
    println!("  entry_points:   {}", overview.entry_points.len());
    println!("  layers:         {}", overview.layers.len());
    println!("  hotspots:       {}", overview.hotspots.len());
    let layer_map = analyzer.module_layer_map(&pid).expect("layer map");
    println!("  layer map:      {} modules", layer_map.len());
    println!();

    // --- 2. Render a self-contained interactive HTML (CLI: `diagram`) ----
    let base_doc = project_document(&repo, "main");
    let rendered =
        diagram::render(&base_doc, QualityProfile::Standard, None).expect("diagram render");
    let out_path = PathBuf::from(ctx.db_path.parent().unwrap()).join("architecture.html");
    diagram::write_atomically(&out_path, rendered.html.as_bytes()).expect("write diagram");
    println!("=== Diagram ===");
    println!("  written: {}", out_path.display());
    println!("  diagnostics: {}", rendered.diagnostics.len());
    println!();

    // --- 3. Before/Delta/After between two projects (CLI: `arch_diff`) ---
    let head_doc = project_document(&repo, "feature");
    let delta =
        diagram::render_delta(&base_doc, &head_doc, "main -> feature").expect("render delta");
    let diff_path = PathBuf::from(ctx.db_path.parent().unwrap()).join("arch_diff.html");
    diagram::write_atomically(&diff_path, delta.html.as_bytes()).expect("write diff");
    let receipt_path = diff_path.with_extension("html.receipt.json");
    diagram::write_atomically(&receipt_path, delta.receipt_json.as_bytes()).expect("write receipt");
    println!("=== Arch Diff ===");
    println!("  written: {}", diff_path.display());
    println!("  receipt: {}", receipt_path.display());
}
