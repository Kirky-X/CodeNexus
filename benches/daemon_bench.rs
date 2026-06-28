// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Daemon mode benchmarks (Task 5, design.md Decision 4).
//!
//! Feature-gated: the file is compiled only when the `daemon` feature is on
//! (Cargo.toml `required-features = ["daemon"]`). The two scenarios exercise
//! the real [`Daemon`] event loop with a [`CountingIndexObserver`] that
//! records both the number of `on_events` batches and the total number of
//! [`DaemonEvent`]s seen, so the bench thread can synchronize on index
//! completion and assert no events were lost.
//!
//! | Scenario                           | Metric            | Verifies              |
//! |------------------------------------|-------------------|-----------------------|
//! | `debounce_response_latency`        | wall time per iter| debounce window + index start latency |
//! | `indexing_event_queue_throughput`  | events/second     | no events lost under load |
//!
//! Both scenarios spawn the daemon on a background thread (`Daemon::run`) and
//! drive file mutations from the bench thread. Each iteration's setup creates a
//! fresh fixture + daemon, and the routine stops + joins the daemon (via
//! `stop_handle`) before returning so no thread outlives the benchmark.
//!
//! [`Daemon`]: codenexus::daemon::Daemon

#![cfg(feature = "daemon")]

#[path = "common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{BatchSize, Criterion, Throughput, black_box, criterion_group, criterion_main};
use codenexus::daemon::{Daemon, DaemonEvent, EventObserver, DEFAULT_DEBOUNCE_MS};
use codenexus::index::IndexFacade;
use tempfile::TempDir;

use common::generate_large_repo;

/// Project name used for every indexing run in this bench file.
const PROJECT_NAME: &str = "daemon_bench";

/// Fixture size — 1000 files gives the daemon enough files to exercise the
/// code-file filter and the IndexObserver's incremental re-index path without
/// making setup prohibitively slow.
const FILE_COUNT: usize = 1_000;

/// Fixture languages, cycled by file index. Must match `common::mod.rs` so
/// `modify_file` targets the correct extension for each `file_{i}`.
const LANGUAGES: &[&str] = &["rs", "c", "f90", "py", "ts"];

/// Debounce window (ms) — matches BR-DAEMON-001 default. The latency scenario
/// expects per-iteration wall time to be at least this long.
const DEBOUNCE_MS: u64 = DEFAULT_DEBOUNCE_MS;

/// Number of file mutations the throughput scenario drives in a single
/// iteration. 50 is small enough to fit inside one debounce window (so
/// `on_events` receives them as one batch) but large enough to surface event
/// loss if the watcher drops any.
const THROUGHPUT_EVENT_COUNT: usize = 50;

/// Per-iteration ceiling for waiting on the observer. If the daemon does not
/// process an event within this window the bench fails loudly instead of
/// hanging.
const OBSERVER_TIMEOUT: Duration = Duration::from_secs(30);

/// Watcher initialization grace period. Matches the synchronization pattern
/// used by `daemon_run_for_duration_catches_code_file_change` in
/// `src/daemon/mod.rs` (400-500 ms sleep after spawn). Without this delay the
/// bench thread mutates files before `notify` has registered its watch, causing
/// the modify event to be missed and the bench to time out at 30 s.
const WATCHER_INIT_DELAY: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------------------
// Counting observer
// ---------------------------------------------------------------------------

/// Wraps [`IndexFacade`] with two shared counters so the bench thread can
/// synchronize on index completion (`index_count`) and verify no events were
/// lost (`event_count`).
///
/// Mirrors the production [`IndexObserver`] behavior (on_events →
/// `index_incremental`) but exposes the call counts via `Arc<AtomicUsize>`.
///
/// [`IndexObserver`]: codenexus::daemon::IndexObserver
struct CountingIndexObserver {
    facade: IndexFacade,
    project_name: String,
    watch_path: PathBuf,
    index_count: Arc<AtomicUsize>,
    event_count: Arc<AtomicUsize>,
}

impl EventObserver for CountingIndexObserver {
    fn on_events(&mut self, events: &[DaemonEvent]) {
        // Record events seen *before* the re-index so the bench thread can
        // observe the delta as soon as `on_events` returns.
        self.event_count.fetch_add(events.len(), Ordering::SeqCst);
        let _ = self
            .facade
            .index_incremental(&self.watch_path, &self.project_name, false);
        self.index_count.fetch_add(1, Ordering::SeqCst);
    }
}

/// State carried from setup into the measured routine. The `JoinHandle` is
/// joined at the end of each iteration so the daemon thread does not outlive
/// the benchmark.
struct DaemonBenchState {
    dir: TempDir,
    index_count: Arc<AtomicUsize>,
    event_count: Arc<AtomicUsize>,
    stop_handle: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

/// Builds a fresh fixture, runs the first index, then starts a daemon watching
/// the fixture directory on a background thread.
fn setup_daemon() -> DaemonBenchState {
    let dir = generate_large_repo(FILE_COUNT);
    let db_path = dir.path().join("bench.db");
    let facade = IndexFacade::new(&db_path).expect("IndexFacade::new");
    // Warm-up: full first index so the database has hashes to diff against.
    facade
        .index(dir.path(), PROJECT_NAME, false)
        .expect("warm-up index");

    let index_count = Arc::new(AtomicUsize::new(0));
    let event_count = Arc::new(AtomicUsize::new(0));
    let observer = CountingIndexObserver {
        facade,
        project_name: PROJECT_NAME.to_string(),
        watch_path: dir.path().to_path_buf(),
        index_count: Arc::clone(&index_count),
        event_count: Arc::clone(&event_count),
    };

    let mut daemon = Daemon::new(dir.path(), PROJECT_NAME, DEBOUNCE_MS, &db_path);
    daemon.add_observer(Box::new(observer));
    let stop_handle = daemon.stop_handle();

    // Use `run()` (not `run_for_duration`) so `stop_handle` can halt the
    // daemon from the bench thread. `run_for_duration` only checks its
    // deadline and ignores `stop`, which would force each iteration to run
    // the full duration instead of stopping after the first event.
    let handle = std::thread::spawn(move || {
        let _ = daemon.run();
    });

    // Give the watcher time to initialize before the routine mutates files.
    // Without this, notify may not have registered its watch when the first
    // modify happens, causing the event to be missed (race condition observed
    // during task 5.5 verification). Mirrors the unit-test pattern in
    // `src/daemon/mod.rs::daemon_run_for_duration_catches_code_file_change`.
    std::thread::sleep(WATCHER_INIT_DELAY);

    DaemonBenchState {
        dir,
        index_count,
        event_count,
        stop_handle,
        handle,
    }
}

/// Stops the daemon background thread and joins it so no thread outlives the
/// iteration. Called at the end of every routine.
fn teardown_daemon(state: DaemonBenchState) {
    state.stop_handle.store(true, Ordering::SeqCst);
    let _ = state.handle.join();
}

/// Appends a language-appropriate comment to `dir/file_{i}.{ext}` so its
/// SHA-256 hash changes and the watcher emits a Modify event.
///
/// `i` determines both the file extension (cycled through `LANGUAGES`) and the
/// comment suffix. Fortran uses `!` for line comments; all other supported
/// languages use `//`.
fn modify_file(dir: &Path, i: usize) {
    let ext = LANGUAGES[i % LANGUAGES.len()];
    let path = dir.join(format!("file_{i}.{ext}"));
    let original = std::fs::read_to_string(&path).expect("read fixture for modify");
    let suffix = match ext {
        "f90" => format!("\n! bench modify {i}\n"),
        _ => format!("\n// bench modify {i}\n"),
    };
    std::fs::write(&path, format!("{original}{suffix}")).expect("write modified fixture");
}

// ---------------------------------------------------------------------------
// Scenario 1: debounce_response_latency
// ---------------------------------------------------------------------------

/// Measures the wall-time latency from a single file modification to the
/// observer's `on_events` callback firing (design.md Decision 4).
///
/// Each iteration mutates one file, waits for `index_count` to advance, then
/// stops the daemon. criterion's per-iteration time therefore captures the
/// debounce window (2 s) plus the index-start latency, and criterion's
/// statistics report the P50/P99 distribution.
fn bench_debounce_response_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("daemon");
    group.sample_size(10);
    // Each iteration waits out the 2 s debounce window plus the incremental
    // index (~200 ms), so ~2.2 s/iter. The default 5 s measurement window
    // would only fit ~2 samples before tripping criterion's `slice.len() > 1`
    // assert; raise the window so at least 10 samples can be collected.
    // `--quick` short-circuits once significance is reached.
    group.measurement_time(Duration::from_secs(300));
    group.bench_function("debounce_response_latency", |b| {
        b.iter_batched(
            setup_daemon,
            |state| {
                let initial = state.index_count.load(Ordering::SeqCst);
                modify_file(state.dir.path(), 0);

                // Wait for the observer to fire. The per-iteration wall time
                // captured by criterion is debounce_ms + index-start latency.
                let deadline = Instant::now() + OBSERVER_TIMEOUT;
                while state.index_count.load(Ordering::SeqCst) <= initial {
                    if Instant::now() > deadline {
                        panic!("daemon did not process modify event within 30 s");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }

                teardown_daemon(state);
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

// ---------------------------------------------------------------------------
// Scenario 2: indexing_event_queue_throughput
// ---------------------------------------------------------------------------

/// Measures event-queue throughput while the daemon is under load (design.md
/// Decision 4 — "indexing期间不丢事件").
///
/// Each iteration rapidly mutates `THROUGHPUT_EVENT_COUNT` files (all within
/// one debounce window), waits for the observer to process them, then asserts
/// no events were lost. `Throughput::Elements` reports events/second.
fn bench_indexing_event_queue_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("daemon");
    group.throughput(Throughput::Elements(THROUGHPUT_EVENT_COUNT as u64));
    group.sample_size(10);
    // Same rationale as `debounce_response_latency`: ~2.2 s/iter means the
    // default 5 s window is too tight.
    group.measurement_time(Duration::from_secs(300));
    group.bench_function("indexing_event_queue_throughput", |b| {
        b.iter_batched(
            setup_daemon,
            |state| {
                let initial_events = state.event_count.load(Ordering::SeqCst);

                // Fire a burst of file modifications. All within one debounce
                // window → the observer should see them as a single batch.
                for i in 0..THROUGHPUT_EVENT_COUNT {
                    modify_file(state.dir.path(), i);
                }

                // Wait for the observer to process the batch. We synchronize on
                // `event_count` (not `index_count`) so the assertion below sees
                // the full set of processed events even if the debouncer splits
                // them across multiple `on_events` calls.
                let deadline = Instant::now() + OBSERVER_TIMEOUT;
                while state.event_count.load(Ordering::SeqCst) - initial_events
                    < THROUGHPUT_EVENT_COUNT
                {
                    if Instant::now() > deadline {
                        panic!(
                            "daemon did not process {THROUGHPUT_EVENT_COUNT} events within 30 s"
                        );
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }

                let processed_events =
                    state.event_count.load(Ordering::SeqCst) - initial_events;
                // design.md Decision 4: assert no events lost. notify may
                // coalesce identical paths but should not drop distinct file
                // modifications within a debounce window.
                assert!(
                    processed_events >= THROUGHPUT_EVENT_COUNT,
                    "event loss: expected >={THROUGHPUT_EVENT_COUNT} events, got {processed_events}"
                );

                teardown_daemon(state);
                black_box(processed_events);
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_debounce_response_latency,
    bench_indexing_event_queue_throughput,
);
criterion_main!(benches);
