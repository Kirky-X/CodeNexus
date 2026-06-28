// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Memory footprint benchmarks (Task 4, design.md Decision 3).
//!
//! Covers three PRD 5.1 memory scenarios:
//!
//! | Scenario                         | PRD SLO | Measured           | `BENCH_RUN_IGNORED` |
//! |----------------------------------|---------|--------------------|---------------------|
//! | `first_index_10k_files_peak`     | <=1 GB  | peak RSS over 10k-file first index | required |
//! | `daemon_sustained_10_increments` | <=200 MB| RSS after each of 10 incremental re-indexes (daemon-driven) | required |
//! | `ram_first_vs_default_comparison`| N/A     | peak RSS delta: `index_ram_first` vs `index` | not required |
//!
//! # `BENCH_RUN_IGNORED` instead of `--ignored`
//!
//! design.md Decision 3 assumed `cargo bench -- --ignored` would opt into the
//! long-running 10k scenarios. criterion 0.5's `--ignored` flag actually
//! **skips every benchmark** (see `cargo bench -- --help`: "currently means
//! skip all benchmarks") — it does not behave like libtest's `--ignored`. To
//! preserve the "default = quick, opt-in = slow" semantics without modifying
//! criterion, the two heavy scenarios gate on the `BENCH_RUN_IGNORED=1`
//! environment variable:
//!
//! ```sh
//! # default: only ram_first_vs_default runs
//! cargo bench --bench memory_bench -- --quick
//! # opt-in: all three scenarios run
//! BENCH_RUN_IGNORED=1 cargo bench --bench memory_bench -- --quick
//! ```
//!
//! # `daemon_sustained` feature gate
//!
//! `daemon_sustained_10_increments` exercises the real [`Daemon`] event loop
//! and is therefore compiled only when the `daemon` feature is on (matching
//! `daemon_bench.rs`'s `required-features`). Under `--no-default-features`
//! the scenario is absent from the group.
//!
//! [`Daemon`]: codenexus::daemon::Daemon

#[path = "common/mod.rs"]
mod common;

#[cfg(feature = "daemon")]
use std::path::PathBuf;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use codenexus::index::IndexFacade;

#[cfg(feature = "daemon")]
use codenexus::daemon::{Daemon, DaemonEvent, EventObserver};

use common::{generate_large_repo, measure_peak_rss};

/// Project name used for every indexing run in this bench file.
const PROJECT_NAME: &str = "memory_bench";

/// 1 GB in bytes — SLO upper bound for `first_index_10k_files_peak`
/// (design.md Decision 3).
const ONE_GB: u64 = 1_073_741_824;

/// 200 MB in bytes — SLO upper bound for each sample in
/// `daemon_sustained_10_increments` (design.md Decision 3).
const TWO_HUNDRED_MB: u64 = 209_715_200;

/// 50 MB in bytes — max allowed RSS growth across the 10 incremental
/// re-indexes, verifying there is no memory leak (design.md Decision 3).
const FIFTY_MB: u64 = 52_428_800;

/// Fixture size for the 10k-file scenario (design.md Decision 3).
const LARGE_REPO_FILE_COUNT: usize = 10_000;

/// Fixture size for `ram_first_vs_default_comparison`. 1000 files keeps the
/// bench fast enough to run by default while still exercising the LZ4
/// compress/decompress path meaningfully.
const COMPARISON_FILE_COUNT: usize = 1_000;

/// Number of incremental re-indexes the daemon-sustained scenario drives.
const SUSTAINED_INCREMENTS: usize = 10;

/// Returns `true` when the caller has opted into the long-running ignored
/// scenarios via `BENCH_RUN_IGNORED=1`.
///
/// See the module docs for why we cannot use criterion's `--ignored` flag.
fn is_ignored_enabled() -> bool {
    std::env::var("BENCH_RUN_IGNORED")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Appends a comment to the file at `dir/file_{i}.rs` so its SHA-256 hash
/// changes and the incremental indexer picks it up.
fn modify_rust_file(dir: &Path, i: usize) {
    let path = dir.join(format!("file_{i}.rs"));
    let original = std::fs::read_to_string(&path).expect("read fixture for modify");
    std::fs::write(&path, format!("{original}// bench modify {i}\n"))
        .expect("write modified fixture");
}

// ---------------------------------------------------------------------------
// Scenario 1: first_index_10k_files_peak (ignored)
// ---------------------------------------------------------------------------

/// Peak RSS while indexing 10 000 files from a cold database (design.md
/// Decision 3, PRD SLO <= 1 GB).
///
/// Marked ignored because generating + indexing 10 000 files takes minutes;
/// run with `BENCH_RUN_IGNORED=1 cargo bench --bench memory_bench -- --quick`.
fn bench_first_index_10k_files_peak(c: &mut Criterion) {
    if !is_ignored_enabled() {
        return;
    }
    let mut group = c.benchmark_group("memory");
    // Each iteration indexes 10 000 files; keep the sample size small so the
    // bench finishes in a reasonable time. criterion's minimum is 10.
    group.sample_size(10);
    group.bench_function("first_index_10k_files_peak", |b| {
        b.iter_batched(
            || {
                let dir = generate_large_repo(LARGE_REPO_FILE_COUNT);
                let db_path = dir.path().join("bench.db");
                let facade = IndexFacade::new(&db_path).expect("IndexFacade::new");
                (dir, facade)
            },
            |(dir, facade)| {
                let peak = measure_peak_rss(|| {
                    facade
                        .index(dir.path(), PROJECT_NAME, false)
                        .expect("first index");
                });
                let peak_mb = peak / 1024 / 1024;
                eprintln!(
                    "first_index_10k_files_peak: peak RSS = {peak} bytes ({peak_mb} MB)"
                );
                assert!(
                    peak <= ONE_GB,
                    "peak RSS {peak} bytes ({peak_mb} MB) exceeds 1 GB SLO"
                );
                black_box(peak);
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

// ---------------------------------------------------------------------------
// Scenario 2: daemon_sustained_10_increments (ignored, daemon-gated)
// ---------------------------------------------------------------------------

/// Wraps [`IndexFacade`] with a shared counter so the bench thread can wait
/// for each incremental re-index to complete before sampling RSS.
///
/// Mirrors the production [`IndexObserver`] behavior (on_events →
/// index_incremental) but exposes the call count via an `Arc<AtomicUsize>`.
///
/// [`IndexObserver`]: codenexus::daemon::IndexObserver
#[cfg(feature = "daemon")]
struct CountingIndexObserver {
    facade: IndexFacade,
    project_name: String,
    watch_path: PathBuf,
    index_count: Arc<AtomicUsize>,
}

#[cfg(feature = "daemon")]
impl EventObserver for CountingIndexObserver {
    fn on_events(&mut self, _events: &[DaemonEvent]) {
        // Errors are logged inside IndexFacade; we only need the call count
        // to advance so the bench thread can synchronize.
        let _ = self
            .facade
            .index_incremental(&self.watch_path, &self.project_name, false);
        self.index_count.fetch_add(1, Ordering::SeqCst);
    }
}

/// State carried from setup into the measured routine for the daemon-sustained
/// scenario. The `JoinHandle` is joined at the end of each iteration so the
/// daemon thread does not outlive the benchmark.
#[cfg(feature = "daemon")]
struct DaemonSustainedState {
    dir: tempfile::TempDir,
    index_count: Arc<AtomicUsize>,
    stop_handle: Arc<std::sync::atomic::AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

/// Daemon-driven sustained-load RSS scenario (design.md Decision 3, PRD SLO
/// <=200 MB per sample, <=50 MB growth across 10 increments).
///
/// Spawns a real [`Daemon`] watching the fixture dir, mutates one file per
/// increment, waits for the [`CountingIndexObserver`] to register the
/// re-index, then samples RSS. Asserts every sample is within the 200 MB
/// ceiling and that total RSS growth across the 10 increments is within 50 MB
/// (catching leaks).
///
/// Marked ignored because it spawns a daemon thread and drives 10 incremental
/// re-indexes; run with `BENCH_RUN_IGNORED=1 cargo bench --bench memory_bench
/// -- --quick`.
#[cfg(feature = "daemon")]
fn bench_daemon_sustained_10_increments(c: &mut Criterion) {
    if !is_ignored_enabled() {
        return;
    }
    let mut group = c.benchmark_group("memory");
    group.sample_size(10);
    group.bench_function("daemon_sustained_10_increments", |b| {
        b.iter_batched(
            || {
                let dir = generate_large_repo(COMPARISON_FILE_COUNT);
                let db_path = dir.path().join("bench.db");
                let facade = IndexFacade::new(&db_path).expect("IndexFacade::new");
                // Warm-up: full first index so the database has hashes to
                // diff against on subsequent incremental runs.
                facade
                    .index(dir.path(), PROJECT_NAME, false)
                    .expect("warm-up index");

                let index_count = Arc::new(AtomicUsize::new(0));
                let observer = CountingIndexObserver {
                    facade,
                    project_name: PROJECT_NAME.to_string(),
                    watch_path: dir.path().to_path_buf(),
                    index_count: Arc::clone(&index_count),
                };

                // debounce_ms = 2000 (BR-DAEMON-001 default). The daemon
                // watches the fixture dir recursively.
                let mut daemon = Daemon::new(
                    dir.path(),
                    PROJECT_NAME,
                    codenexus::daemon::DEFAULT_DEBOUNCE_MS,
                    &db_path,
                );
                daemon.add_observer(Box::new(observer));
                let stop_handle = daemon.stop_handle();

                // Run the daemon event loop on a background thread so the
                // bench thread can drive file mutations and sample RSS.
                let handle = std::thread::spawn(move || {
                    // 120 s is plenty for 10 increments at 2 s debounce.
                    let _ = daemon.run_for_duration(Duration::from_secs(120));
                });

                DaemonSustainedState {
                    dir,
                    index_count,
                    stop_handle,
                    handle,
                }
            },
            |state| {
                let mut rss_samples: Vec<u64> = Vec::with_capacity(SUSTAINED_INCREMENTS);
                let mut sys = sysinfo::System::new();
                let pid = sysinfo::Pid::from(std::process::id() as usize);

                for i in 0..SUSTAINED_INCREMENTS {
                    modify_rust_file(state.dir.path(), i);
                    let expected = i + 1;
                    // Wait for the observer to register the re-index. Poll
                    // with a 30 s ceiling so a stuck daemon fails the bench
                    // instead of hanging forever.
                    let deadline = Instant::now() + Duration::from_secs(30);
                    while state.index_count.load(Ordering::SeqCst) < expected {
                        if Instant::now() > deadline {
                            panic!(
                                "daemon did not process increment {expected} within 30 s"
                            );
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }

                    sys.refresh_process(pid);
                    let rss = sys
                        .process(pid)
                        .map(|p| p.memory())
                        .unwrap_or(0);
                    let rss_mb = rss / 1024 / 1024;
                    eprintln!(
                        "daemon_sustained increment {expected}: RSS = {rss} bytes ({rss_mb} MB)"
                    );
                    assert!(
                        rss <= TWO_HUNDRED_MB,
                        "RSS {rss} bytes ({rss_mb} MB) exceeds 200 MB SLO at increment {expected}"
                    );
                    rss_samples.push(rss);
                }

                // Stop the daemon and join the background thread so it does
                // not outlive the iteration.
                state.stop_handle.store(true, Ordering::SeqCst);
                let _ = state.handle.join();

                // Assert RSS growth across all increments is within 50 MB,
                // catching leaks (design.md Decision 3).
                if let (Some(&first), Some(&last)) = (rss_samples.first(), rss_samples.last()) {
                    let growth = last.saturating_sub(first);
                    let growth_mb = growth / 1024 / 1024;
                    eprintln!(
                        "daemon_sustained: RSS growth = {growth} bytes ({growth_mb} MB)"
                    );
                    assert!(
                        growth <= FIFTY_MB,
                        "RSS growth {growth} bytes ({growth_mb} MB) exceeds 50 MB leak ceiling"
                    );
                }

                black_box(rss_samples);
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

// ---------------------------------------------------------------------------
// Scenario 3: ram_first_vs_default_comparison (default)
// ---------------------------------------------------------------------------

/// Compares peak RSS of [`IndexFacade::index`] vs [`IndexFacade::index_ram_first`]
/// on the same 1000-file fixture (design.md Decision 3, ADR-024).
///
/// Both runs go through [`measure_peak_rss`]; the routine reports the two
/// peaks and their delta so a regression in the LZ4 compression path is
/// visible. Not marked ignored — 1000 files is small enough to run by default.
///
/// [`IndexFacade::index_ram_first`]: codenexus::index::IndexFacade::index_ram_first
fn bench_ram_first_vs_default_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory");
    group.sample_size(10);
    // Each iteration runs `index` (force=false, first index ~250 ms) then
    // `index_ram_first` (force=true, which triggers delete+re-insert of all
    // 1000 records — observed ~60 s per iteration, a src/ performance
    // characteristic this bench surfaces). The default `measurement_time`
    // (5 s) would yield only one sample before tripping criterion's
    // `slice.len() > 1` stats assert; raise the window so at least two
    // samples can be collected. `--quick` short-circuits once significance
    // is reached, so this still finishes in ~2-3 iterations in practice.
    group.measurement_time(Duration::from_secs(300));
    group.bench_function("ram_first_vs_default_comparison", |b| {
        b.iter_batched(
            || {
                let dir = generate_large_repo(COMPARISON_FILE_COUNT);
                let db_path = dir.path().join("bench.db");
                let facade = IndexFacade::new(&db_path).expect("IndexFacade::new");
                (dir, facade)
            },
            |(dir, facade)| {
                // 1. Default streaming path.
                let default_peak = measure_peak_rss(|| {
                    facade
                        .index(dir.path(), PROJECT_NAME, false)
                        .expect("default index");
                });

                // 2. RAM-first path. `force=true` ensures the parse phase
                //    actually runs (otherwise the hash diff would skip every
                //    file since we just indexed them) and the LZ4
                //    compress/decompress path is exercised.
                let ram_peak = measure_peak_rss(|| {
                    facade
                        .index_ram_first(dir.path(), PROJECT_NAME, true)
                        .expect("ram-first index");
                });

                let delta = default_peak as i64 - ram_peak as i64;
                let default_mb = default_peak / 1024 / 1024;
                let ram_mb = ram_peak / 1024 / 1024;
                let delta_mb = delta / 1024 / 1024;
                eprintln!(
                    "ram_first_vs_default: default={default_peak} bytes ({default_mb} MB), \
                     ram_first={ram_peak} bytes ({ram_mb} MB), \
                     delta={delta} bytes ({delta_mb} MB)"
                );
                black_box((default_peak, ram_peak, delta));
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

// `criterion_group!` does not accept `#[cfg]` on individual targets, so we
// emit two group definitions gated on the `daemon` feature. Only one of them
// is compiled per build; both register the same group name `benches` so
// `criterion_main!` stays identical.
#[cfg(feature = "daemon")]
criterion_group!(
    benches,
    bench_first_index_10k_files_peak,
    bench_daemon_sustained_10_increments,
    bench_ram_first_vs_default_comparison,
);

#[cfg(not(feature = "daemon"))]
criterion_group!(
    benches,
    bench_first_index_10k_files_peak,
    bench_ram_first_vs_default_comparison,
);

criterion_main!(benches);
