// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

use std::fs;
use std::path::PathBuf;

use codenexus::index::{IndexFacade, IndexResult};
use codenexus::query::QueryFacade;
use tempfile::TempDir;

pub struct ExampleContext {
    pub _temp_dir: TempDir,
    pub db_path: PathBuf,
    pub source_dir: PathBuf,
}

pub fn setup(sample_code: &str) -> ExampleContext {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let db_path = temp_dir.path().join("codenexus.lbug");
    let source_dir = temp_dir.path().join("src");
    fs::create_dir_all(&source_dir).expect("failed to create source dir");

    let sample_file = source_dir.join("lib.rs");
    fs::write(&sample_file, sample_code).expect("failed to write sample code");

    ExampleContext {
        _temp_dir: temp_dir,
        db_path,
        source_dir,
    }
}

pub fn index_sample_code(ctx: &ExampleContext, project_name: &str) -> IndexResult {
    let indexer = IndexFacade::new(&ctx.db_path).expect("IndexFacade::new");
    let result = indexer
        .index(&ctx.source_dir, project_name, true)
        .expect("indexing failed");

    println!("=== Indexing Complete ===");
    println!("  Files indexed:  {}", result.files_indexed);
    println!("  Nodes created:  {}", result.nodes_created);
    println!("  Edges created:  {}", result.edges_created);
    println!("  Project ID:     {}", result.project_id);
    println!();

    result
}

pub fn open_query(ctx: &ExampleContext) -> QueryFacade {
    QueryFacade::new(&ctx.db_path).expect("QueryFacade::new")
}
