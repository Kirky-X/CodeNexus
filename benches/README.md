# CodeNexus Performance Benchmarks

Criterion-based performance benchmarks covering all PRD §5.1 SLO indicators:
indexing throughput, incremental indexing, query latency, trace latency, memory
footprint, and daemon event-loop responsiveness.

## Overview

Six benchmark files exercise the full indexing/query/trace/daemon pipeline:

| File                 | Group        | Scenarios                                                        | PRD SLO dimension       |
|----------------------|--------------|------------------------------------------------------------------|-------------------------|
| `index_bench.rs`     | `index`      | `index_10_files`                                                 | Cold-start throughput   |
| `incremental_bench.rs` | `incremental` | `cold_start_1000`, `incremental_1_of_1000`, `incremental_500_of_1000` | Incremental throughput |
| `query_bench.rs`     | `query`      | `cypher_match_functions`, `cypher_count_functions`, `search_by_name`, `search_by_name_project` | Query latency |
| `trace_bench.rs`     | `trace`      | `trace_calls_depth_5`, `trace_calls_depth_10`, `trace_calls_depth_50`, `trace_all_depth_10` | Trace latency |
| `memory_bench.rs`    | `memory`     | `first_index_10k_files_peak`, `daemon_sustained_10_increments`, `ram_first_vs_default_comparison` | Peak RSS / sustained RSS |
| `daemon_bench.rs`    | `daemon`     | `debounce_response_latency`, `indexing_event_queue_throughput`   | Daemon responsiveness   |

`daemon_bench.rs` is feature-gated on `daemon` (Cargo.toml
`required-features = ["daemon"]`); the other five compile under all feature
presets.

## SLO threshold table

| Bench file         | Scenario                        | PRD SLO           | Baseline (measured) | Alert threshold |
|--------------------|---------------------------------|-------------------|---------------------|-----------------|
| `index_bench`      | `index_10_files`                | ≥ 100 files/s     | TBD                 | < 90 files/s    |
| `incremental_bench`| `cold_start_1000`               | ≥ 100 files/s     | 3929 files/s        | < 90 files/s    |
| `incremental_bench`| `incremental_1_of_1000`         | ≥ 500 files/s     | 4987 files/s        | < 450 files/s   |
| `incremental_bench`| `incremental_500_of_1000`       | ≥ 100 files/s     | 33.23 files/s ⚠️    | < 90 files/s    |
| `query_bench`      | `cypher_match_functions`        | P99 ≤ 200 ms      | TBD                 | > 250 ms        |
| `trace_bench`      | `trace_calls_depth_5`           | P99 ≤ 500 ms      | TBD                 | > 600 ms        |
| `memory_bench`     | `first_index_10k_files_peak`    | ≤ 1 GB peak RSS   | TBD (requires `BENCH_RUN_IGNORED=1`) | > 1.2 GB |
| `memory_bench`     | `daemon_sustained_10_increments`| ≤ 200 MB sustained| TBD (requires `BENCH_RUN_IGNORED=1`) | > 240 MB |
| `memory_bench`     | `ram_first_vs_default_comparison`| N/A (comparison) | default 450 MB / ram-first 568 MB | N/A |
| `daemon_bench`     | `debounce_response_latency`     | ≤ 3 s (debounce + index start) | 2.76 s/iter | > 4 s |
| `daemon_bench`     | `indexing_event_queue_throughput` | No events lost   | 9.18 elem/s (50/50 events) | Any event loss |

⚠️ `incremental_500_of_1000` measures 33.23 files/s, below the ≥ 100 files/s
SLO. This is a **src/ incremental-index performance issue** (delete + re-insert
path for 500 changed files), not a benchmark bug. The bench correctly surfaces
the regression; fixing it requires changes to `src/index/` which is out of scope
for this change.

⚠️ `ram_first_vs_default_comparison` shows ram-first (568 MB) using **more**
memory than default (450 MB) on a 1000-file fixture. This is expected for small
repos where LZ4 compression overhead exceeds the LadybugDB write-amplification
savings. ADR-024 recommends ram-first for repos ≥ 1 GB source; the 1000-file
fixture is well below that threshold.

## Running benchmarks

### Quick mode (CI-friendly)

`--quick` tells criterion to stop once statistical significance is reached
rather than running the full `measurement_time` window. It does **not** shorten
per-iteration time — slow benchmarks still need an adequate `measurement_time`
set inside the bench file.

```sh
# All benches (default features)
cargo bench -- --quick

# A single bench file
cargo bench --bench incremental_bench -- --quick
cargo bench --bench memory_bench -- --quick
cargo bench --bench daemon_bench --features daemon -- --quick

# Existing benches
cargo bench --bench index_bench -- --quick
cargo bench --bench query_bench -- --quick
cargo bench --bench trace_bench -- --quick
```

### Full mode

```sh
cargo bench --bench incremental_bench
cargo bench --bench daemon_bench --features daemon
```

### Saving a baseline for regression comparison

```sh
# Save baseline on main branch
cargo bench --bench incremental_bench -- --save-baseline main

# Compare on a feature branch
cargo bench --bench incremental_bench -- --baseline main
```

## Long-running scenarios (`BENCH_RUN_IGNORED=1`)

Two `memory_bench` scenarios are gated behind the `BENCH_RUN_IGNORED=1`
environment variable because they exercise 10 000-file fixtures or sustained
daemon loops that are too slow for CI:

- `first_index_10k_files_peak` — peak RSS over a 10 000-file first index.
- `daemon_sustained_10_increments` — RSS after each of 10 daemon-driven
  incremental re-indexes.

### Why not `cargo bench -- --ignored`?

design.md Decision 3 originally assumed criterion 0.5's `--ignored` flag would
opt into these long-running scenarios (mirroring libtest's `--ignored`).
**This assumption is incorrect.** criterion 0.5's `--ignored` flag "currently
means skip all benchmarks" (see `cargo bench -- --help`) — it does not behave
like libtest's `--ignored`.

To preserve the "default = quick, opt-in = slow" semantics without modifying
criterion, the two heavy scenarios check `BENCH_RUN_IGNORED=1` at runtime and
early-return (printing a skip message) when the variable is unset:

```sh
# Default: ram_first_vs_default_comparison runs; the two heavy scenarios skip
cargo bench --bench memory_bench -- --quick

# Opt-in: all three scenarios run
BENCH_RUN_IGNORED=1 cargo bench --bench memory_bench -- --quick
```

## Shared fixtures (`benches/common/mod.rs`)

Three helpers are shared across `incremental_bench`, `memory_bench`, and
`daemon_bench`:

- `generate_large_repo(file_count) -> TempDir` — builds a temp repo with N
  files spread across C/Rust/Fortran/Python/TypeScript, each containing a
  minimal parseable symbol definition.
- `open_test_db() -> (TempDir, StorageConnection)` — opens a fresh LadybugDB
  database in a temp dir.
- `measure_peak_rss(f) -> u64` — samples the current process's RSS every 100 ms
  while `f` runs and returns the peak (bytes), using the `sysinfo` crate.
