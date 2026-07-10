// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Indexing pipeline benchmarks (Task 15).
//!
//! Measures [`IndexFacade::index`] throughput on a fixture of small Rust
//! files. The target SLO is >= 100 files/second; criterion's `index` group
//! surfaces the per-run wall time so the regression can be tracked over time.
//!
//! Each benchmark iteration creates a fresh temp directory, writes `count`
//! Rust files, and indexes them into a fresh on-disk LadybugDB database. The
//! `IndexFacade` opens and closes the database connection inside `index`, so
//! dropping the `TempDir` at the end of each iteration is safe (no lingering
//! open handles).

use std::path::Path;

use codenexus::index::IndexFacade;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use tempfile::TempDir;

/// Writes `count` trivial Rust files (`file_0.rs` .. `file_{n-1}.rs`) into
/// `dir`, each defining a single function.
fn write_rust_files(dir: &Path, count: usize) {
    for i in 0..count {
        let path = dir.join(format!("file_{i}.rs"));
        std::fs::write(&path, format!("fn func_{i}() {{}}\n")).unwrap();
    }
}

fn bench_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("index");
    // Indexing involves database I/O; a smaller sample size keeps the bench
    // runtime reasonable while still producing stable measurements.
    group.sample_size(10);
    group.bench_function("index_10_files", |b| {
        b.iter(|| {
            let tmp = TempDir::new().unwrap();
            write_rust_files(tmp.path(), 10);
            let db = tmp.path().join("bench.db");
            let facade = IndexFacade::new(&db).unwrap();
            let result = facade.index(tmp.path(), "bench", false).unwrap();
            black_box(result);
        });
    });
    group.finish();
}

criterion_group!(benches, bench_index);
criterion_main!(benches);
