// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! 项目生命周期：索引多个项目 → 列出 → 按名解析 → 删除。
//!
//! 覆盖 CLI：`index` / `list` / `status`（列表部分）/ `clean`。

use codenexus::service::project::resolve_project_id;
use codenexus::storage::Repository;
use codenexus_examples::{index_sample_code, setup};

const SAMPLE_CODE: &str = r#"
pub fn handler() -> u32 {
    compute(21) * 2
}

fn compute(n: u32) -> u32 {
    n + 1
}
"#;

fn main() {
    println!("=== CodeNexus Example: Project Lifecycle (index/list/clean) ===\n");

    let ctx = setup(SAMPLE_CODE);

    // 1. Index two projects into the same database.
    let first = index_sample_code(&ctx, "service-a");
    let _second = index_sample_code(&ctx, "service-b");

    // 2. List all indexed projects (CLI: `list` / `status`).
    let repo = Repository::open(&ctx.db_path).expect("Repository::open");
    let projects = repo.list_projects().expect("list_projects");
    println!("=== Indexed Projects ({} total) ===", projects.len());
    for p in &projects {
        println!(
            "  {} | files={} | last_commit={}",
            p.name, p.file_count, p.last_commit
        );
    }
    println!();

    // 3. Resolve a project by name (name takes precedence over id).
    let project_id = resolve_project_id(&repo, "service-b").expect("resolve service-b");
    println!("=== Resolved ===");
    println!("  'service-b' -> {project_id}");
    println!();

    // 4. Remove a project and all of its nodes (CLI: `clean`).
    repo.delete_project(&project_id).expect("delete_project");
    println!("=== Cleaned ===");
    println!("  service-a kept id: {}", first.project_id);
    println!("  service-b removed (project + all related nodes)");

    let remaining = repo.list_projects().expect("list_projects after clean");
    println!("  remaining projects: {}", remaining.len());
}
