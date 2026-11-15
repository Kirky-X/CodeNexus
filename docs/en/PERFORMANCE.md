# ⚡ CodeNexus Performance Guide

This guide summarizes CodeNexus's performance SLOs, benchmark suite, measured baselines, and memory-optimization design. All numbers come from criterion measurements recorded in [`benches/README.md`](../../benches/README.md) or from fix histories recorded in the CHANGELOG; unmeasured metrics are explicitly marked TBD.

## 📋 Table of Contents

- [Overview & SLOs](#-overview--slos)
- [Benchmark suite](#-benchmark-suite)
- [Measured baselines](#-measured-baselines)
- [Memory optimization (L1–L7 defenses)](#-memory-optimization-l1l7-defenses)
- [Indexing tuning](#-indexing-tuning)
- [Query & trace tuning](#-query--trace-tuning)
- [Reproducing results](#-reproducing-results)
- [Related docs](#-related-docs)

---

## 🎯 Overview & SLOs

The SLOs below are acceptance references for production workloads; all thresholds come from the SLO threshold table in [`benches/README.md`](../../benches/README.md) (index throughput, single-file incremental, query latency, trace latency, first-index memory, and daemon sustained memory are baseline gate items; incremental bulk changes ≥ 100 files/s, daemon response ≤ 3 s, and zero event-queue loss are additional acceptance items from the same table):

| Dimension | SLO | Notes |
|:----------|:----|:------|
| Index throughput (cold start) | ≥ 100 files/s | First full index |
| Incremental indexing (single file change) | ≥ 500 files/s | Typical re-index after save |
| Incremental indexing (bulk changes) | ≥ 100 files/s | 500/1000 files changed |
| Query latency | P99 ≤ 200 ms | Cypher / search |
| Trace latency | P99 ≤ 500 ms | Call-chain tracing |
| Memory (first index) | ≤ 1 GB peak RSS | 10k-file repo |
| Memory (daemon sustained) | ≤ 200 MB sustained RSS | After 10 incremental rounds |
| daemon response | ≤ 3 s | Debounce + index startup |
| Event queue | No event loss allowed | daemon event throughput |

---

## 📏 Benchmark suite

7 Criterion benchmark groups live in `benches/` (`daemon_bench` is gated by the `daemon` feature; the other six compile under every feature preset; `graph_bench` covers the graph data structures):

| File | Group | Scenarios | SLO dimension |
|------|-------|-----------|---------------|
| `index_bench.rs` | `index` | `index_10_files` | Cold-start throughput |
| `incremental_bench.rs` | `incremental` | `cold_start_1000`, `incremental_1_of_1000`, `incremental_500_of_1000` | Incremental throughput |
| `query_bench.rs` | `query` | `cypher_match_functions`, `cypher_count_functions`, `search_by_name`, `search_by_name_project` | Query latency |
| `trace_bench.rs` | `trace` | `trace_calls_depth_5`, `trace_calls_depth_10`, `trace_calls_depth_50`, `trace_all_depth_10`, `impact_large_subgraph` (4971-node high-fan-in regression) | Trace latency |
| `memory_bench.rs` | `memory` | `first_index_10k_files_peak`, `daemon_sustained_10_increments`, `ram_first_vs_default_comparison` | Peak / sustained RSS |
| `daemon_bench.rs` (`daemon`) | `daemon` | `debounce_response_latency`, `indexing_event_queue_throughput` | daemon responsiveness |

> The full SLO threshold table, alert thresholds, and shared fixture notes are in [`benches/README.md`](../../benches/README.md).

---

## 📊 Measured baselines

> Measured values recorded in `benches/README.md`. Real performance depends on repo size and hardware; reproduce with `cargo bench` on the target environment.

| Scenario | Measured | SLO | Status |
|----------|----------|-----|--------|
| `cold_start_1000` (1000-file cold start) | ~3929 files/s | ≥ 100 files/s | ✅ |
| `incremental_1_of_1000` (single-file change) | ~4987 files/s | ≥ 500 files/s | ✅ |
| `incremental_500_of_1000` (500-file change) | ~33.23 files/s | ≥ 100 files/s | ⚠️ Below SLO |
| `debounce_response_latency` | ~2.76 s/iter | ≤ 3 s | ✅ |
| `indexing_event_queue_throughput` | ~9.18 elem/s (50/50 events), zero loss | No loss allowed | ✅ |
| `ram_first_vs_default_comparison` (1000 files) | default ~450 MB / ram-first ~568 MB | Comparison item (no SLO) | ℹ️ |
| `first_index_10k_files_peak` | TBD (requires `BENCH_RUN_IGNORED=1`) | ≤ 1 GB | To be filled |
| `daemon_sustained_10_increments` | TBD (requires `BENCH_RUN_IGNORED=1`) | ≤ 200 MB | To be filled |
| `cypher_match_functions` | TBD | P99 ≤ 200 ms | To be filled |
| `trace_calls_depth_5` | TBD | P99 ≤ 500 ms | To be filled |

⚠️ `incremental_500_of_1000` measures 33.23 files/s, below the ≥ 100 files/s SLO. This is a known performance issue in the `src/` incremental indexer on the 500-file bulk delete+reinsert path (the benchmark is correctly exposing the regression); fixing it requires changes to `src/index/`.

ℹ️ On the 1000-file fixture, `ram_first_vs_default_comparison` shows ram-first (568 MB) using **more** memory than default (450 MB) — expected behavior for small repos: the LZ4 compression cost outweighs the savings from reduced LadybugDB write amplification. ADR-024 recommends ram-first for repos with **≥ 1 GB of source**.

**Historical milestones**: the L6+L7 memory work in v0.3.11 cut index peak memory on a 70 GB host from **~60 GB to ~4 GB**; the P-series fixes in v0.3.12 compressed LSP hover on a 100k-symbol repo from ~100k SQL round trips to 200 (batched UNWIND, 500 symbols per batch), saving 100–300 seconds of pure SQL wait.

---

## 🧠 Memory optimization (L1–L7 defenses)

The seven-layer defense against OOM when indexing large repos (L1–L5 shipped in v0.3.10, L6–L7 in v0.3.11, plus the P-series optimizations in v0.3.12):

| Layer | Mechanism | Notes |
|-------|-----------|-------|
| L1 | `MemoryBudget` three-level memory pressure | sysinfo probes available memory, max_rss set to 50%, Green/Yellow/Red tiers |
| L2 | mpsc channel + par_chunks streaming parse | rayon `par_chunks(8)` + `sync_channel(4)` caps concurrent `ExtractResult` at 4 |
| L3 | `Graph::nodes_view/edges_view` iterators | Removes full node/edge duplicate copies |
| L4 | Streaming CSV | `write_nodes_csv_stream` / `write_edges_csv_stream` replace whole-buffer String concatenation |
| L5 | Adaptive degradation | When total repo size exceeds budget, `index_ram_first` automatically falls back to streaming disk reads; `CacheConfig::entry_max_bytes` evicts small entries before oversized ASTs |
| L6 | Pipeline streaming | `ctx.remove` ownership transfer replaces `Graph::clone`; LZ4 buffers drop immediately after parsing; iterator APIs write through |
| L7 | LadybugDB resource caps | `buffer_pool_size` capped at 4 GB (256 MiB in tests), `max_num_threads = 8`; LSP starts on demand (pure-Rust repos only start rust-analyzer); RAM-first 8× amplification budget (compressed buffer / decompressed source / IR / Graph / CSV total 8×) |

Storage and LSP optimizations added in v0.3.12: dynamic `buffer_pool_size` (25% of available memory, clamped to [256 MiB, 4 GiB]), CSV dedup key switched to a tuple, BufWriter 64 KB → 256 KB, `PathInterner` PathBuf dedup (saves ~15–30 MB), LSP extension routing optimization (unknown extensions like `.md` no longer fall back to starting rust-analyzer), and `std::thread::scope` parallel LSP startup.

---

## 🚀 Indexing tuning

### RAM-first mode

```bash
# LZ4 in-memory compression + single COPY FROM bulk load
codenexus index --path /path/to/project --name myproject --ram_first true
```

| Guidance | Notes |
|----------|-------|
| Use when source ≥ 1 GB | Negative payoff for small repos (compression cost > write-amplification savings) |
| Automated budget check | When `(total_bytes × 8) < (max_rss / 2)` is not satisfied, automatically falls back to streaming disk reads |
| Memory composition | Compressed buffer 1.0× + decompressed source 1.0× + IR 3.0× + Graph 2.0× + CSV 1.0× = 8× amplification |

### Incremental indexing

- SHA-256 hash comparison is natively incremental: repeated `index` runs parse only changed files; `incremental_1_of_1000` measures ~4987 files/s.
- `--force true` rebuilds everything; `--fresh true` deletes the old DB first to reclaim dead space (use when the DB bloats).

### daemon

- `--debounce-ms` (default 2000) controls the debounce window; measured debounce response ~2.76 s (SLO ≤ 3 s).
- SIGTERM/SIGINT shut down gracefully without interrupting an in-flight incremental index.

---

## 🔍 Query & trace tuning

- **Read-only connections**: `QueryFacade::new_read_only` / `TraceConfig.read_only` allow concurrent multi-process reads and skip schema initialization.
- **Query cache**: the `cache` feature (oxcache) caches query results; `CacheConfig::entry_max_bytes` (64 KiB per entry) prevents oversized entries from evicting others.
- **Impact analysis cap**: subgraph node cap `MAX_NODES_LIMIT = 5000` (raised from 1000 in v0.3.8 to match repos with 270+ direct callers); the `impact_large_subgraph` scenario in `trace_bench` guards against regressions.
- **Path filtering**: `trace --path_filter "/src/api/**"` prunes the search space at the library layer; prefer it.
- **Parallel LSP**: LSP starts on demand and in parallel (cost ≈ max(per-provider)); pure-Rust repos only start rust-analyzer, saving 3–5 GB RSS.
- **Batched SQL**: LSP hover semantic-type updates use batched `UNWIND` (500 symbols per batch) instead of per-symbol round trips.

---

## 🧪 Reproducing results

Commands match [`benches/README.md`](../../benches/README.md):

```sh
# Quick mode (stops once statistical significance is reached; CI-friendly)
cargo bench -- --quick

# Single benchmark file
cargo bench --bench incremental_bench -- --quick
cargo bench --bench query_bench -- --quick
cargo bench --bench trace_bench -- --quick
cargo bench --bench memory_bench -- --quick
cargo bench --bench daemon_bench --features daemon -- --quick

# Full mode
cargo bench --bench incremental_bench
cargo bench --bench daemon_bench --features daemon

# Baseline saving and regression comparison
cargo bench --bench incremental_bench -- --save-baseline main
cargo bench --bench incremental_bench -- --baseline main

# Long-running scenarios (10k files / daemon sustained increments) need an explicit opt-in
BENCH_RUN_IGNORED=1 cargo bench --bench memory_bench -- --quick
```

> Note: in criterion 0.5, `--ignored` means "skip all benchmarks" rather than libtest's "run only ignored", so the two long-running scenarios are toggled at runtime via the `BENCH_RUN_IGNORED=1` environment variable. The shared fixture (`benches/common/mod.rs`) provides `generate_large_repo` / `measure_peak_rss` (RSS sampled every 100 ms).

---

## 📚 Related docs

| Document | Content |
|----------|---------|
| [benches/README.md](../../benches/README.md) | SLO threshold table, alert thresholds, shared fixtures |
| [📖 User Guide](USER_GUIDE.md) | Using `--ram_first` / `--fresh` / daemon |
| [📋 Changelog](CHANGELOG.md) | Full record of the L1–L7 and P-series optimizations (v0.3.10–v0.3.12) |
