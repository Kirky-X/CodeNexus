// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

//! Shared benchmark fixtures.
//!
//! Provides two helpers reused across `incremental_bench`, `memory_bench`,
//! and `daemon_bench`:
//!
//! - [`generate_large_repo`] — builds a temp repo with N files spread across
//!   the 5 supported languages (C/Rust/Fortran/Python/TypeScript), each
//!   containing a minimal parseable symbol definition.
//! - [`measure_peak_rss`] — samples the current process's resident set size
//!   every 100 ms while `f` runs and returns the peak (bytes), using the
//!   `sysinfo` crate for cross-platform RSS access.
//!
//! `measure_peak_rss` is consumed by `memory_bench` only; every bench target
//! re-compiles this module, so `incremental_bench` / `daemon_bench` builds see
//! it as unused. The `allow(dead_code)` silences the per-target unused warning
//! so `cargo clippy -- -D warnings` stays green.
//!
//! [`IndexFacade`]: codenexus::index::IndexFacade
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sysinfo::{Pid, ProcessesToUpdate, System};
use tempfile::TempDir;

/// Supported fixture languages, in the order they are cycled through when
/// generating files. Matches the `full` feature preset
/// (C/Rust/Fortran/Python/TypeScript).
const LANGUAGES: &[&str] = &["rs", "c", "f90", "py", "ts"];

/// Generates a temp repository containing `file_count` source files spread
/// evenly across the 5 supported languages (see
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

/// Measures the peak resident set size (RSS, in bytes) of the current process
/// while `f` runs.
///
/// Spawns a background thread that polls `sysinfo` every 100 ms for the
/// current process's RSS and tracks the maximum observed value. After `f`
/// returns, the background thread is signalled to stop and joined; the peak
/// is then returned.
///
/// RSS is sampled at the process level, so it includes allocations made by
/// LadybugDB's C FFI as well as Rust allocations (this is why `sysinfo` was
/// chosen over a custom `GlobalAlloc`).
#[allow(dead_code)] // used by memory_bench; dead in the other bench targets
pub fn measure_peak_rss<F: FnOnce()>(f: F) -> u64 {
    let peak: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let stop: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

    let peak_worker = Arc::clone(&peak);
    let stop_worker = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let mut sys = System::new();
        let pid: Pid = Pid::from(std::process::id() as usize);
        while !stop_worker.load(Ordering::SeqCst) {
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), false);
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
