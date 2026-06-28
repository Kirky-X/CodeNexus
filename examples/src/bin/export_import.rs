// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

use std::fs;

use codenexus_examples::{index_sample_code, open_query, setup};
use tempfile::TempDir;

const SAMPLE_CODE: &str = r#"
pub fn parse(input: &str) -> Vec<String> {
    input.split_whitespace().map(String::from).collect()
}

pub fn validate(tokens: &[String]) -> bool {
    !tokens.is_empty()
}
"#;

fn main() {
    println!("=== CodeNexus Example: Export / Import ===\n");

    let ctx = setup(SAMPLE_CODE);
    let _result = index_sample_code(&ctx, "export-demo");

    let query = open_query(&ctx);

    let qr = query
        .cypher("MATCH (f:Function) RETURN f.name")
        .expect("cypher failed");
    println!("--- Before export ---");
    println!("  Functions in DB: {}", qr.rows.len());
    for row in &qr.rows {
        println!("    {:?}", row);
    }
    println!();

    let export_path = ctx._temp_dir.path().join("exported.lbug");
    fs::copy(&ctx.db_path, &export_path).expect("export (copy) failed");
    let export_size = fs::metadata(&export_path).map(|m| m.len()).unwrap_or(0);
    println!("--- Export ---");
    println!("  Exported DB to: {}", export_path.display());
    println!("  Size: {} bytes", export_size);
    println!();

    let import_dir = TempDir::new().expect("import temp dir");
    let import_path = import_dir.path().join("imported.lbug");
    fs::copy(&export_path, &import_path).expect("import (copy) failed");
    println!("--- Import ---");
    println!("  Imported DB to: {}", import_path.display());
    println!();

    let import_query = codenexus::query::QueryFacade::new(&import_path).expect("imported QueryFacade");
    let qr = import_query
        .cypher("MATCH (f:Function) RETURN f.name")
        .expect("cypher failed");
    println!("--- Verification ---");
    println!("  Source functions: {}", qr.rows.len());
    println!("  Export size:      {} bytes", export_size);
    println!("  Import successful: DB copied to new location");
    println!();
    println!("Note: In production, use `codenexus export` / `codenexus import`");
    println!("which compress with zstd and embed a version manifest header.");
}
