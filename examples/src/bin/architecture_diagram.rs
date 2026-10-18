// Copyright (c) 2026 Kirky.X🌠
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

// Module boundaries are derived from the directory layout of the indexed
// files: `api/mod.rs` belongs to module `api`, `core/mod.rs` to `core`.
// The `crate::core::index_dir()` call creates the CALLS edge that becomes a
// module dependency, and the axum-style route registers `list_projects` as a
// Controller while `index_dir` is classified as a Service.
const SAMPLE_CODE: &str = r#"
pub mod api;
pub mod core;
"#;

const API_CODE: &str = r#"
pub fn list_projects() -> u32 {
    crate::core::index_dir()
}

pub fn router() {
    Router::new().route("/projects", get(list_projects));
}
"#;

const CORE_CODE: &str = r#"
pub fn index_dir() -> u32 {
    42
}
"#;

const FEATURE_CODE: &str = r#"
pub mod api;
pub mod billing;
pub mod core;
"#;

const BILLING_CODE: &str = r#"
pub fn charge() -> u32 {
    1
}
"#;

/// 构造单个项目的架构 DiagramDocument（arch_diff 的 Before/After 输入）。
///
/// `project_id` 必须是索引后图中的项目 id；用户输入的 name 需先用
/// [`resolve_project_id`] 解析，`ArchitectureAnalyzer` 不会自行解析。
fn project_document(repo: &Repository, project_id: &str, title: &str) -> diagram::DiagramDocument {
    let analyzer = ArchitectureAnalyzer::new(repo);
    let overview = analyzer.overview(project_id).expect("overview");
    let layer_map = analyzer.module_layer_map(project_id).expect("layer map");
    let mut doc = diagram::from_overview(&overview, &layer_map, title);
    doc.meta.locale = "en".into();
    assert!(
        !doc.components.is_empty()
            && doc
                .components
                .iter()
                .all(|component| !component.id.is_empty() && !component.label.is_empty()),
        "architecture must yield named module components"
    );
    doc
}

/// 写入示例源码；路径相对于被索引的根目录，子目录决定模块边界。
fn write_files(ctx: &ExampleContext, files: &[(&str, &str)]) {
    for (relative, content) in files {
        let path = ctx.source_dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create source subdir");
        }
        std::fs::write(path, content).expect("write source file");
    }
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
    write_files(
        &ctx,
        &[
            ("lib.rs", SAMPLE_CODE),
            ("api/mod.rs", API_CODE),
            ("core/mod.rs", CORE_CODE),
        ],
    );
    index_silent(&ctx, "main");
    write_files(
        &ctx,
        &[("lib.rs", FEATURE_CODE), ("billing/mod.rs", BILLING_CODE)],
    );
    index_silent(&ctx, "feature");

    let repo = Repository::open(&ctx.db_path).expect("Repository::open");

    // Project names are user input; analyzers query by canonical project id.
    let main_id = resolve_project_id(&repo, "main").expect("resolve main");
    let feature_id = resolve_project_id(&repo, "feature").expect("resolve feature");

    // --- 1. Architecture overview (CLI: `architecture`) -----------------
    let analyzer = ArchitectureAnalyzer::new(&repo);
    let overview = analyzer.overview(&main_id).expect("overview");

    println!("=== Architecture Overview (main) ===");
    println!("  project id:     {main_id}");
    println!("  languages:      {}", overview.languages.len());
    println!("  packages:       {}", overview.packages.len());
    println!("  entry_points:   {}", overview.entry_points.len());
    println!("  modules:        {}", overview.module_boundaries.len());
    println!("  layers:         {}", overview.layers.len());
    println!("  hotspots:       {}", overview.hotspots.len());
    let layer_map = analyzer.module_layer_map(&main_id).expect("layer map");
    println!("  layer map:      {} modules", layer_map.len());
    println!();

    // --- 2. Render a self-contained interactive HTML (CLI: `diagram`) ----
    let base_doc = project_document(&repo, &main_id, "main");
    let rendered =
        diagram::render(&base_doc, QualityProfile::Standard, None).expect("diagram render");
    let out_path = PathBuf::from(ctx.db_path.parent().unwrap()).join("architecture.html");
    diagram::write_atomically(&out_path, rendered.html.as_bytes()).expect("write diagram");
    println!("=== Diagram ===");
    println!("  components:     {}", base_doc.components.len());
    println!("  written: {}", out_path.display());
    println!("  diagnostics: {}", rendered.diagnostics.len());
    println!();

    // --- 3. Before/Delta/After between two projects (CLI: `arch_diff`) ---
    let head_doc = project_document(&repo, &feature_id, "feature");
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
