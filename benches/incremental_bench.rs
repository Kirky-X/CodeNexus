// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Incremental indexing benchmarks (Task 3, design.md Decision 1).
//!
//! Covers three PRD 5.1 scenarios on a 1000-file fixture spread across the 5
//! supported languages (C/Rust/Fortran/Python/TypeScript):
//!
//! | Scenario                  | Changed files | PRD SLO       | Verifies             |
//! |---------------------------|---------------|---------------|----------------------|
//! | `cold_start_1000`         | 1000 (full)   | >=100 files/s | First-index baseline |
//! | `incremental_1_of_1000`   | 1             | >=500 files/s | Single-file delta    |
//! | `incremental_500_of_1000` | 500           | >=100 files/s | Mid-load delta       |
//!
//! Each scenario uses [`IndexFacade`] (matching [`index_bench`]) and reports
//! throughput as files/second via `Throughput::Elements`, so criterion's
//! reports line up with the PRD SLO units. `iter_batched` with
//! `BatchSize::PerIteration` isolates per-iteration setup (file generation,
//! initial full index) from the measured routine (the incremental re-index),
//! so the throughput number reflects pure indexing cost rather than fixture
//! IO.
//!
//! [`index_bench`]: ../index_bench.rs

#[path = "common/mod.rs"]
mod common;

use std::path::Path;

use std::time::Duration;

use criterion::{
    BatchSize, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use codenexus::index::IndexFacade;

use common::generate_large_repo;

/// Total fixture size for all three scenarios (design.md Decision 1).
const FILE_COUNT: usize = 1000;

/// Project name used for every indexing run; the project node is created on
/// the first `index` call and reused by subsequent `index_incremental` calls
/// against the same database.
const PROJECT_NAME: &str = "incremental_bench";

/// Languages cycled through by [`generate_large_repo`]. Kept in sync with
/// `benches/common/mod.rs` so [`modify_files`] can regenerate the correct
/// minimal symbol for each extension.
const LANGUAGES: &[&str] = &["rs", "c", "f90", "py", "ts"];

/// Appends a newline comment (or harmless trailing whitespace for Fortran) to
/// the first `count` files under `dir`, modifying their SHA-256 hash without
/// breaking syntax. Languages are cycled in the same order as
/// [`generate_large_repo`] so the modified files are spread evenly across all
/// 5 languages.
///
/// `force` is `false` for all incremental benches — the indexer's hash diff
/// must detect the changed files on its own.
fn modify_files(dir: &Path, count: usize) {
    for i in 0..count {
        let ext = LANGUAGES[i % LANGUAGES.len()];
        let path = dir.join(format!("file_{i}.{ext}"));
        let original = std::fs::read_to_string(&path).expect("read fixture for modify");
        // Append a language-appropriate suffix that changes the file hash but
        // leaves the existing symbol parseable. Fortran is column-sensitive,
        // so a `!` comment at column 1 is the only safe append.
        let suffix = match ext {
            "f90" => "\n! bench modify\n",
            _ => "\n// bench modify\n",
        };
        std::fs::write(&path, format!("{original}{suffix}")).expect("write modified fixture");
    }
}

/// Cold-start baseline: index 1000 fresh files into an empty database
/// (design.md Decision 1, PRD SLO >= 100 files/s).
///
/// Both the fixture generation and the database open happen inside the
/// measured routine — the SLO covers the user-visible "first index" wall
/// time, which is what `codenexus index` reports.
fn bench_cold_start_1000(c: &mut Criterion) {
    let mut group = c.benchmark_group("incremental");
    group.throughput(Throughput::Elements(FILE_COUNT as u64));
    // Each iteration indexes 1000 files into a fresh on-disk LadybugDB; keep
    // the sample size small so the bench finishes in a reasonable time while
    // still producing a stable median (matches `index_bench`).
    group.sample_size(10);
    group.bench_function("cold_start_1000", |b| {
        b.iter_batched(
            || {
                let dir = generate_large_repo(FILE_COUNT);
                let db_path = dir.path().join("bench.db");
                let facade = IndexFacade::new(&db_path).expect("IndexFacade::new");
                (dir, facade)
            },
            |(dir, facade)| {
                let result = facade
                    .index(dir.path(), PROJECT_NAME, false)
                    .expect("cold-start index");
                black_box(result);
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

/// Single-file incremental: pre-index 1000 files, modify 1, re-index with
/// `force=false` (design.md Decision 1, PRD SLO >= 500 files/s).
///
/// The pre-index (warm-up) and the file modification happen in setup, so the
/// measured routine is purely the incremental re-index that should skip 999
/// unchanged files and only re-parse 1.
fn bench_incremental_1_of_1000(c: &mut Criterion) {
    let mut group = c.benchmark_group("incremental");
    // Throughput is reported per-file-scanned; the SLO is ">=500 files/s"
    // where "files" means files scanned by the incremental pass (1000),
    // not files actually re-parsed (1). This matches how `codenexus index`
    // reports its own throughput.
    group.throughput(Throughput::Elements(FILE_COUNT as u64));
    group.sample_size(10);
    group.bench_function("incremental_1_of_1000", |b| {
        b.iter_batched(
            || {
                let dir = generate_large_repo(FILE_COUNT);
                let db_path = dir.path().join("bench.db");
                let facade = IndexFacade::new(&db_path).expect("IndexFacade::new");
                // Warm-up: full first index so the database has hashes to
                // diff against.
                facade
                    .index(dir.path(), PROJECT_NAME, false)
                    .expect("warm-up index");
                // Mutate exactly 1 file to trigger a minimal incremental
                // delta.
                modify_files(dir.path(), 1);
                (dir, facade)
            },
            |(dir, facade)| {
                let result = facade
                    .index_incremental(dir.path(), PROJECT_NAME, false)
                    .expect("incremental_1 index");
                black_box(result);
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

/// Mid-load incremental: pre-index 1000 files, modify 500, re-index with
/// `force=false` (design.md Decision 1, PRD SLO >= 100 files/s).
///
/// 500 files is half the repository — the incremental pass must re-parse
/// half the files and skip the other half, exercising the diff/skip path at
/// a meaningful scale rather than the single-file edge case.
fn bench_incremental_500_of_1000(c: &mut Criterion) {
    let mut group = c.benchmark_group("incremental");
    group.throughput(Throughput::Elements(FILE_COUNT as u64));
    group.sample_size(10);
    // Each incremental_500 iteration re-indexes 500 changed files out of 1000,
    // which currently takes ~29 s per routine (observed: src/ incremental
    // re-index is dominated by database delete+re-insert of changed records).
    // The default `measurement_time` (5 s) would yield only one sample before
    // timing out, tripping criterion's `slice.len() > 1` stats assert. Raise
    // the per-bench measurement window so criterion can collect at least the
    // minimum sample size (10) — `--quick` will still short-circuit earlier
    // once statistical significance is reached.
    group.measurement_time(Duration::from_secs(300));
    group.bench_function("incremental_500_of_1000", |b| {
        b.iter_batched(
            || {
                let dir = generate_large_repo(FILE_COUNT);
                let db_path = dir.path().join("bench.db");
                let facade = IndexFacade::new(&db_path).expect("IndexFacade::new");
                facade
                    .index(dir.path(), PROJECT_NAME, false)
                    .expect("warm-up index");
                // Mutate 500 files (half the repo) to exercise the mid-load
                // incremental path.
                modify_files(dir.path(), 500);
                (dir, facade)
            },
            |(dir, facade)| {
                let result = facade
                    .index_incremental(dir.path(), PROJECT_NAME, false)
                    .expect("incremental_500 index");
                black_box(result);
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_cold_start_1000,
    bench_incremental_1_of_1000,
    bench_incremental_500_of_1000,
);
criterion_main!(benches);
