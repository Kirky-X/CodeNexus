// Copyright (c) 2026 Kirky.X. All rights reserved.
// SPDX-License-Identifier: MIT

//! Shared benchmark fixtures (design.md Decision 6).
//!
//! Provides three helpers reused across `incremental_bench`, `memory_bench`,
//! and `daemon_bench`:
//!
//! - [`generate_large_repo`] — builds a temp repo with N files spread across
//!   the 5 supported languages (C/Rust/Fortran/Python/TypeScript), each
//!   containing a minimal parseable symbol definition.
//! - [`open_test_db`] — opens a fresh LadybugDB database in a temp dir.
//! - [`measure_peak_rss`] — samples the current process's resident set size
//!   every 100 ms while `f` runs and returns the peak (bytes), using the
//!   `sysinfo` crate for cross-platform RSS access (design.md Decision 2).
//!
//! `open_test_db` is part of the shared fixture API required by the
//! performance-benchmark-coverage-spec but is not directly consumed by the
//! three benches added in this change (they use [`IndexFacade`] which opens
//! its own repository handle); it is kept `pub` for future benches that need
//! direct [`StorageConnection`] access. The module-level `allow(dead_code)`
//! silences the resulting unused-warning so `cargo clippy -- -D warnings`
//! stays green.
//!
//! [`IndexFacade`]: codenexus::index::IndexFacade
//! [`StorageConnection`]: codenexus::storage::StorageConnection

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use codenexus::storage::StorageConnection;
use sysinfo::{Pid, System};
use tempfile::TempDir;

/// Supported fixture languages, in the order they are cycled through when
/// generating files. Matches the `full` feature preset
/// (C/Rust/Fortran/Python/TypeScript).
const LANGUAGES: &[&str] = &["rs", "c", "f90", "py", "ts"];

/// Generates a temp repository containing `file_count` source files spread
/// evenly across the 5 supported languages (design.md Decision 1 / spec
/// `generate_large_repo`).
///
/// Each file carries a minimal parseable symbol definition (`fn`/`int`/
/// `subroutine`/`def`/`function`) so the indexing pipeline exercises real
/// tree-sitter parsing rather than skipping empty files. The caller owns the
/// returned [`TempDir`]; dropping it cleans up the fixture.
#[must_use]
pub fn generate_large_repo(file_count: usize) -> TempDir {
    let dir = TempDir::new().expect("tempdir for large repo");
    for i in 0..file_count {
        let ext = LANGUAGES[i % LANGUAGES.len()];
        let path = dir.path().join(format!("file_{i}.{ext}"));
        std::fs::write(&path, minimal_symbol(ext, i)).expect("write fixture file");
    }
    dir
}

/// Returns the minimal parseable symbol definition for a given language
/// extension. The integer `i` ensures each file is unique so hash-based
/// incremental indexing does not deduplicate them.
fn minimal_symbol(ext: &str, i: usize) -> String {
    match ext {
        "rs" => format!("fn func_{i}() {{}}\n"),
        "c" => format!("int func_{i}(void) {{ return {i}; }}\n"),
        "f90" => format!("      subroutine sub_{i}()\n      end subroutine sub_{i}\n"),
        "py" => format!("def func_{i}():\n    return {i}\n"),
        "ts" => format!("function func_{i}(): number {{ return {i}; }}\n"),
        other => panic!("unsupported fixture extension: {other}"),
    }
}

/// Opens a fresh LadybugDB database inside a temp directory (design.md
/// Decision 6 / spec `open_test_db`).
///
/// Returns the [`TempDir`] (caller owns it so the database files survive for
/// the benchmark's lifetime) and the open [`StorageConnection`]. Schema
/// initialization is deliberately left to the caller — benches that go
/// through [`IndexFacade`] do not need a pre-initialized schema, and
/// `IndexFacade` will run `init_schema` itself on the first `index` call.
///
/// [`IndexFacade`]: codenexus::index::IndexFacade
pub fn open_test_db() -> (TempDir, StorageConnection) {
    let dir = TempDir::new().expect("tempdir for test db");
    let db_path = dir.path().join("bench.db");
    let conn = StorageConnection::open(&db_path).expect("open test db");
    (dir, conn)
}

/// Measures the peak resident set size (RSS, in bytes) of the current process
/// while `f` runs (design.md Decision 2 / spec `measure_peak_rss`).
///
/// Spawns a background thread that polls `sysinfo` every 100 ms for the
/// current process's RSS and tracks the maximum observed value. After `f`
/// returns, the background thread is signalled to stop and joined; the peak
/// is then returned.
///
/// RSS is sampled at the process level, so it includes allocations made by
/// LadybugDB's C FFI as well as Rust allocations (this is why `sysinfo` was
/// chosen over a custom `GlobalAlloc` — see design.md Decision 2).
pub fn measure_peak_rss<F: FnOnce()>(f: F) -> u64 {
    let peak: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let stop: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

    let peak_worker = Arc::clone(&peak);
    let stop_worker = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let mut sys = System::new();
        let pid: Pid = Pid::from(std::process::id() as usize);
        while !stop_worker.load(Ordering::SeqCst) {
            sys.refresh_process(pid);
            if let Some(proc_info) = sys.process(pid) {
                // `Process::memory` returns the resident set size in bytes
                // (physical RAM held by the process).
                let rss = proc_info.memory();
                peak_worker.fetch_max(rss, Ordering::SeqCst);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });

    f();

    stop.store(true, Ordering::SeqCst);
    let _ = handle.join();

    peak.load(Ordering::SeqCst)
}
