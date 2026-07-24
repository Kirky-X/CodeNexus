# Changelog

All notable changes to CodeNexus are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.10] - 2026-07-25

大型仓库索引时 OOM 问题的完整 5 层防线，将峰值内存从 O(files) 降至 O(chunk_size)。

### Fixed

- **fix(storage): L1+L3+L4 memory-overflow — budget, graph views, streaming CSV** — L1: 新增 `MemoryBudget` + `Pressure` 枚举（sysinfo 探测可用内存，max_rss 设为 50%，分 Green/Yellow/Red 三级）。L3: `Graph::nodes_view/edges_view` 迭代器 + `for_each_node_with_label_mut` 就地修改，去除 `ScopeOutput/ResolveOutput` 中 all_nodes/all_edges 重复拷贝（消除 2 轮 node/edge 全量复制）。L4: `write_nodes_csv_stream<W:Write>` / `write_edges_csv_stream<W:Write>` 流式写入替代全量 String 拼接；`TempDir` 生命周期修正（移除 `std::mem::forget` 泄漏）。
- **fix(parse): L2 memory-overflow — mpsc channel + par_chunks streaming** — `parallel_parse_ram_first` 重构：rayon `par_chunks(8)` + `mpsc::sync_channel(4)` 限制并发 `ExtractResult` 数量上限为 4（原先 `par_iter().collect()` 同时驻留全部结果）；`std::thread::scope` 解耦生产者/消费者避免死锁；tracing subscriber 正确传播至 rayon worker。
- **fix(index): L5 adaptive degradation** — `CacheConfig::entry_max_bytes`（64 KiB 单条上限）防止超大 AST 驱逐小条目；`trace_upstream` 路径 `Vec<String>` 改为 `Rc<PathLink>` 链表（O(1) clone 代替 O(N*K) 拷贝）；`index_ram_first` 当仓库总大小超 `per_collection_soft_limit`（256 MiB）自动降级为 streaming disk-read 模式；`MemoryBudget` 移除死代码（`cache_entry_max_bytes` 字段等）。

### Documentation

- **docs(readme): document source-build for older libstdc++** — 新增「从源码编译」章节，指导 libstdc++ < 12 环境下的构建方式。

## [0.3.9] - 2026-07-21

### Fixed

- **fix(parse): DQ-002 duplicate FQN for struct field + impl method** — CalNexus indexing reported 4 DQ-002 Duplicate FQN warnings. Root cause: a struct field and an impl method with the same name produced identical FQNs (e.g. `Foo.bar#Foo` for both `struct Foo { bar: ... }` and `impl Foo { fn bar() {} }`), because both used the struct name as the disambiguator. Fix: prepend `field_` to the struct field disambiguator in `rust_extractor.rs::extract_struct_fields` — fields now use `#field_Foo` while methods keep `#Foo`, cleanly separating the two namespaces without changing Function FQNs. Eliminates 4 DQ-002 warnings; downstream `context`/`rename`/`impact` symbol resolution is no longer ambiguous on the 4 collision sites.
- **fix(parse): DQ-004 orphan edge from Python nested functions** — CalNexus indexing reported 2 DQ-004 Orphan edge warnings where the source node existed but the target (`_strip_blank_ends`) did not. Root cause: Python nested `def` (def inside another def) was previously skipped entirely to align with gitnexus function counts (170 vs 280), but outer functions calling inner ones still emitted CALLS edges targeting non-existent nodes. Fix in `python.rs::extract_function`: nested functions are now extracted as Function nodes but marked `is_global = false` so they don't pollute the global symbol table, while still providing a target for CALLS edges. Eliminates 2 DQ-004 warnings; the `flush_section`/`flush_req` → `_strip_blank_ends` call chain is no longer broken.

### Documentation

- **docs(appendix): document 3 known limitations from CalNexus 0.3.8 verification** — `references/appendix.md` Known Issues table extended with: (1) bash `dead_code` — `trap cleanup EXIT` (signal-triggered) and indirect calls via string concatenation / variable expansion are invisible to tree-sitter; results are correctly flagged Medium-confidence but require manual confirmation before deletion. (2) axum route extraction — updated existing entry to also cover `cross_service` returning empty `[]` for `Router::new().route(...)` macro registration (route strings are macro arguments, not standalone string literals). (3) LSP environment dependency — `lsp_goto_def`/`lsp_hover` may fail with exit 2 (`LSP communication error: server connection closed`) in environments where the language server exits immediately after startup; this is an environment/configuration issue, not a tool logic bug.

## [0.3.8] - 2026-07-21

### Fixed

- **fix(analysis): bulwark testing 5 bugs + triage review fixes** — Phase 2 bulwark project testing (534 files / 19242 nodes / 94127 edges) revealed 5 real bugs, all fixed plus triage review (security/architecture/performance) findings addressed:
  - **P0: `query`/`list` without `--db` failed** even when `.codenexus/<project>.lbug` existed. Added `discover_single_indexed_db()` to scan `.codenexus/` and auto-select the single available index.
  - **P1: `dead_code` reported 99.2% false positives** (1376/1387) on Rust `tests.rs` — `#[test]`/`#[tokio::test]`/`#[rstest]` macro CALLS edges are invisible to tree-sitter. Added `TEST_ATTRIBUTE_MARKERS` const + `has_test_attribute_marker()` helper to treat `#[test]`-attributed functions as entry points.
  - **P2: `route_map`/`shape_check`/`cross_service`/`tool_map` returned 0 on axum projects** — axum uses programmatic `Router::new().route("/x", get(h))` instead of attribute macros. Added `extract_axum_routes()` in `rust_extractor.rs` emitting `NodeLabel::Route` + `EdgeType::HandlesRoute`. `api_review.rs` service layer now supports both legacy (Handler+HANDLES) and axum (Function+HANDLES_ROUTE) patterns via module constants `HANDLER_LIKE_EDGE_TYPES`/`ROUTE_LIKE_LABELS`/`HANDLER_LIKE_LABELS`/`CALLER_LIKE_LABELS`/`ENDPOINT_MATCH_FIELDS`.
  - **P1: `search --fulltext true` mislabeled `matchReason`** as `"prefix match"`/`"substring match"`. FTS path now returns `MATCH_REASON_FTS` (`"bm25 fts"`) and BM25F path returns `MATCH_REASON_BM25F_WEIGHTED` (`"bm25f weighted"`) — both extracted as module constants.
  - **P2: `impact` 1000-node cap too tight** for bulwark (270+ direct callers truncated on first BFS hop). Raised `MAX_SUBGRAPH_NODES` and `MAX_NODES_LIMIT` from 1000 to 5000 (aligned).
- **Triage review fixes** — Performance CRITICAL (C1): added `idx_rel_source`/`idx_rel_target` indexes on `CodeRelation` table (targeted WHERE queries were full-scanning 94k edges). Performance HIGH (H1): `load_caller_info` `Vec::contains` (O(N)) → `HashSet<&str>` (O(1)). Performance HIGH (H2): `shape_check` N+1 query pattern → single `load_calls_with_reason()` + `HashMap<target_id, Vec<reason>>` index. Performance MEDIUM (M1): `escape_identifier` `String` → `Cow<'_, str>` (Borrowed for common case). Architecture MEDIUM (M1): deleted `load_edges` (DRY with `load_edges_multi`). Architecture MEDIUM (M4): deleted `load_edge_reason` (replaced by `load_calls_with_reason`). Security LOW (LOW-1): `escape_identifier` security note added — only reserved-keyword escaping, NOT user-input sanitisation.

### Changed

- **refactor(trace): decouple impact tests from `ImpactConfig` defaults (M3)** — Performance review M3 (previously deferred): tests in `src/trace/impact.rs` asserted against hard-coded numeric literals (`5`, `10`) that mirrored production defaults, so any future change to `ImpactConfig::default()` or `MAX_DEPTH_LIMIT` would silently break tests or — worse — tests would pass while runtime behaviour diverged from the documented spec. Extracted `DEFAULT_MAX_DEPTH: u32 = 5` constant; `Default::default()` now references it; `impact_config_default_has_expected_values`/`new_uses_default_config`/`with_config_clamps_max_depth_to_limit` tests now reference `DEFAULT_MAX_DEPTH`/`MAX_DEPTH_LIMIT` instead of duplicating literals.

### Documentation

- **docs(skill): fix 2 doc-code inconsistencies** found in B-bulwark post-commit code-doc consistency review: `commands.md:158` `MAX_SUBGRAPH_NODES=1000` → `=5000`; `commands.md:131` `"bm25f"` → `"bm25f weighted"`.
- **docs(appendix): document axum route extraction limitation (diting M1)** — `references/appendix.md` Known Issues table now records that `extract_axum_routes` only recognizes bare-identifier handlers (`get(handler)`); path-qualified (`get(crate::module::handler)`) and closure (`get(|| { ... })`) handlers are skipped and require manual verification. Code is correct; this closes the doc-vs-code gap surfaced in the v0.3.8 diting review.

### Added

- **test(trace): `impact_5000_node_subgraph` benchmark (diting M2)** — new benchmark in `benches/trace_bench.rs` builds a 4971-node high-fanin graph (1 target + 70 direct callers + 4900 transitive callers) and runs `ImpactAnalyzer::analyze_impact` with both default config and `max_depth=10`. Asserts `affected.len() <= 5000` (`MAX_NODES_LIMIT`) so a regression that removes the cap surfaces here instead of in production. Latency is tracked under the `impact_large_subgraph` criterion group. Closes the "5000-node cap lacks memory regression test" gap from the v0.3.8 diting review.
- **test(cli): `discover_single_indexed_db` multi-`.lbug` coverage (diting M3)** — three new serial tests in `src/main.rs`: (1) `discover_single_indexed_db_returns_none_when_multiple_lbug_files` verifies the multi-file fallback returns `None`; (2) `discover_single_indexed_db_returns_path_when_single_lbug_file` verifies the single-file happy path returns the path; (3) `discover_single_indexed_db_returns_none_when_directory_missing` verifies the missing-directory path does not panic. Uses a local `CwdGuard` struct (Drop-restores cwd) + `serial_test::serial(db_discover)` to avoid cwd races. Closes the "multi-`.lbug` fallback untested" gap from the v0.3.8 diting review.

## [0.3.7] - 2026-07-20

### Fixed

- **fix(build): link `libgcc` statically to resolve `__cpu_model` linker failure** — `cargo install codenexus` failed on machines using the `mold` linker with `error: linking with gcc failed` / `mold: error: undefined symbol: __cpu_model` (referenced by `liblbug.a(base_csv_reader.cpp.o)`). `__cpu_model` is a GCC runtime symbol that lives in the static `libgcc.a`, not the dynamic `libgcc_s.so` that rustc links by default; `mold`'s strict symbol resolution rejects the undefined reference, while `ld` silently defers it to runtime — so the failure was environment-specific and invisible to default `cargo build`. `build.rs` now probes `libgcc.a`'s directory via `cc -print-file-name=libgcc.a` and links it statically (`cargo:rustc-link-lib=static=gcc`) on Linux/gcc targets, with a `target_os = "linux"` guard so macOS/Windows clang builds are unaffected. Verified: `RUSTFLAGS="-C link-arg=-Wl,--no-undefined"` strict link of `codenexus-verify` now succeeds.

## [0.3.6] - 2026-07-20

### Changed

- **perf(community): C3 — Louvain→Leiden algorithm upgrade** — `community` command now uses the Leiden algorithm (Traag et al., 2019) instead of plain Louvain. Leiden adds a refinement phase between the local-moving and aggregation phases that splits each community into its internally connected sub-communities, guaranteeing the connectivity invariant every community is internally connected. Plain Louvain could produce disconnected communities on graphs with weakly-linked cliques bridged via a hub. Shared `modularity_core` with a `RefineMode::{Plain, RefineConnected}` flag (M-9); `louvain()` retained as a test-only baseline for quality comparison. Additional performance fixes from 3-dimension review: `comm_tot` switched from per-node rebuilt `HashMap` to a `Vec<f64>` with O(1) incremental update on node move (HIGH-001); `refine_partition_connected` uses reusable `Vec<bool>` scratch buffers (MED-001) and move semantics for the partition (MED-004); `BTreeSet` replaces `HashSet + sort()` for deterministic node ordering (LOW-003). 7 new tests cover Leiden basic scenarios (M-6), public-API connectivity invariant (M-7), and mixed-input refinement (M-8).
- **chore(deps): major dependency upgrades + root-cause zstd-sys removal** — petgraph 0.6→0.8, reqwest 0.12→0.13 (feature `rustls-tls`→`rustls`), lsp-server 0.8→0.9 (ResponseKind enum), lsp-types 0.95→0.97 (Url→Uri), inklog 0.1.9→0.1.10 (gzip fallback via flate2 when `compression` feature disabled). Added `url` 2.x optional dep for `path_to_uri` conversion. zstd-sys completely eliminated from dependency tree. `cargo install --path . --locked` now succeeds without linker workarounds.

## [0.3.5] - 2026-07-17

### Added

- **test(cli): comprehensive CLI integration tests** — `tests/cli_index_and_daemon.rs` adds 35 tests covering: (1) `index` CLI (build, force rebuild, incremental, nonexistent path), (2) `daemon` CLI hot update (new file, modified file, SIGTERM graceful shutdown, exit code), (3) all 27 post-index subcommands exercised via the binary child process (route_map, shape_check, api_impact, tool_map, architecture, community, complexity, dead_code, cross_service, context, trace, impact, search, query, detect_changes, rename, export/import, setup, hook, mcp, list, status, clean, lsp_goto_def, lsp_hover). Tests spawn the compiled `codenexus` binary as a child process to verify the full CLI path: argument parsing → service dispatch → storage → output formatting.

### Fixed

- **fix(error): correct exit code classification + resolve_start_id ambiguity detection** — `resolve_start_id` now fails fast on ambiguous symbols instead of silently resolving to one match; exit code mapping corrected (0=success, 1=internal error, 2=invalid input/ProjectNotFound/Query, 4=NotFound/DatabaseCorrupt). Verified: `context --symbol new --enhanced true --project CodeNexus` returns `ambiguous symbol 'new': 99 candidates` in ~0.3s (exit 1) instead of timing out.
- **fix(daemon): SIGTERM/SIGINT signal handlers for graceful shutdown (BUG-002)** — daemon now installs `signal_hook` handlers for SIGTERM and SIGINT, draining the event loop cleanly before exit (exit 0). Previously, signals could interrupt in-flight incremental indexing, leaving the DB in an inconsistent state.
- **fix(cli): gate all read commands against missing DB + validate `context --project`** — read commands (`query`/`list`/`search`/`impact`/`context`/`trace`) now check DB existence before opening, exiting 4 (NotFound) with a clear message instead of a stack-traced `StorageError`. `context --enhanced true --project <name|id>` resolves name→id via `resolve_project_id` (previously matched the raw value against `Function.project`, which stores the id, so name lookups failed).
- **fix(test): test hardening from 3-dimension review** — `/bin/kill` absolute path (security MEDIUM, avoids PATH lookup); `index_args_with` builder unifies arg construction (architecture MEDIUM); `wait_for_exit` timeout branch now reaps the zombie via `wait()` (LOW); `let _tmp = tmp` extends DB lifetime for assertions (LOW); post-index read tests assert non-empty stdout + exit 0 (LOW).

### Changed

- **perf(index): parallelize hash diffing + fix quality test regression** — `src/index/phases.rs` adds `parallel_parse` and `parallel_parse_ram_first` functions using rayon to parallelize SHA-256 file hash diffing across cores during incremental indexing. Hash diff is now a parallel pre-pass before the (already-parallel) parse phase, eliminating the sequential single-threaded hash stage that dominated incremental reindex on multi-core machines.

## [0.3.4] - 2026-07-15

### Fixed

- **fix(docs): resolve docs.rs build failure** — `.cargo/config.toml` now provides fallback values for `LBUG_PRECOMPILED_SOURCE` and `LBUG_PRECOMPILED_LIBRARY_DIR`. When docs.rs sets `DOCS_RS=1`, lbug's build.rs skips C++ compilation without emitting `cargo:rustc-env`, causing `env!()` macros to fail. The fallback values are overridden by `cargo:rustc-env` in normal builds (higher precedence), so this change is transparent to local development.
- **fix(parse): clippy 1.95 `collapsible_match` in fortran extractor** — collapsed nested `if` into match guard for the `"identifier"` arm in `extract_call`.
- **fix(bench): adapt sysinfo 0.39.5 API** — `refresh_process(pid)` → `refresh_processes(ProcessesToUpdate::Some(&[pid]), false)` in `benches/common/mod.rs` and `benches/memory_bench.rs`.

### Changed

- **chore(deps): upgrade CI Rust 1.94→1.95** — required by sysinfo 0.39.5 MSRV. Updated `ci.yml`, `release.yml`, `Cargo.toml`, `clippy.toml`, `README.md`, `README_EN.md`, `docs/CONTRIBUTING.md`.
- **chore(deps): notify 6→8, notify-debouncer-full 0.7→0.6** — notify 8.x requires notify-debouncer-full 0.6 (0.7 only compatible with notify 6.x). daemon code accesses notify API via `notify_debouncer_full::notify::*` re-export, no code changes needed.
- **chore(deps): sysinfo 0.30→0.39** — dependabot PR #24.
- **chore(deps): lbug 0.17.1→0.18.0** — dependabot PR #23.
- **chore(deps): lsp-server 0.7→0.8** — dependabot PR #22.
- **chore(deps): tree-sitter-go 0.23→0.25** — dependabot PR #26.
- **chore(deps): action-gh-release 2.6.2→3.0.1** — dependabot PR #27.

## [0.3.3] - 2026-07-15

### Added

- **feat(i18n): ICU4X-based Unicode case folding + NFC normalization** — new `src/model/i18n.rs` module (`i18n` feature, included in `full` preset) providing `fold_case` (Unicode-aware, e.g. German ß→ss, Turkish İ→i̇), `normalize_nfc` (NFC composition), and `is_cjk` (CJK script detection). Tokenizer now uses case folding + CJK script boundary detection so CJK identifiers are not split on ASCII boundaries. 15 unit tests.
- **feat(trace): `TaintPathTracer` for cross-language multi-hop taint tracking** — new `src/trace/taint.rs` module with BFS traversal over DataFlows/Reads/Writes/FfiCalls edges, supporting source-to-sink taint path queries and all-reachable-path queries. 30 unit tests covering basic paths, cycle detection, FFI edge following, depth limits, and boundary conditions.
- **feat(embed): enable semantic search by default in `full` feature** — `embed` feature (vector embedding via ONNX local inference + OpenAI HTTP) is now included in `full`, making semantic search available out of the box. Automatic fallback to BM25 on model absence or unsupported platforms.
- **refactor(model): FromStr/Display for 4 high-impact enums** — `SearchMode`, `TraceType`, `ServiceProtocol`, `DiffMode` now implement `FromStr` + `Display`, replacing ad-hoc string parsing with standard trait-based conversion.

### Changed

- **chore(harness): CI modernization** — `.github/workflows/ci.yml` upgraded to Rust 1.91, split into 4 jobs (lint/test/coverage/security) with a 6-combination feature matrix (minimal/core/full/mixed). Added `.github/dependabot.yml`, `.github/codeql.yml`, `clippy.toml` (msrv=1.91). `release.yml` adds conditional crates.io publish. `.pre-commit-config.yaml` adds coverage gate (≥95% lines). `rustfmt.toml` fixes deprecated `fn_args_layout` → `fn_params_layout`.
- **chore(deps): sdforge 0.4.1 → 0.4.2** — pulls `cli::GlobalArg`, `CliBuilder::with_global_arg()`, `mcp::serve_stdio()`, and `pub use clap/rmcp` re-exports from sdforge. Adds `rmcp` as optional dependency (gated behind `mcp` feature) for sdforge trait path resolution.

### Fixed

- **fix(test): feature-gate Unicode-specific i18n tests** — `fold_case_german_sharp_s`, `fold_case_turkish_i_with_dot`, and `normalize_nfc_decomposed_to_composed` now require `#[cfg(feature = "i18n")]` since they test ICU4X behavior unavailable in ASCII-only fallback mode.
- **fix(test): feature-gate `ac_index_001_indexes_c_rust_fortran_files`** — test requires `lang-c` and `lang-fortran` parsers; now gated with `#[cfg(all(feature = "lang-c", feature = "lang-fortran"))]`.

## [0.3.2] - 2026-07-11

### Changed

- **refactor(arch): unified CLI/MCP service layer via sdforge `#[service_api]`** — all 27 CLI commands and 6 MCP tools now share a single service layer in `src/service/`. Each command defines a core function + CLI wrapper (`cli = true`, no `tool_name`) + MCP wrapper (`tool_name`, no `cli = true`). This replaces the previous split between `src/cli/*_cmd.rs` (CLI handlers) and `src/mcp/mod.rs` (MCP handlers). Key discovery: sdforge macro uses `name` for CLI command name and `tool_name` for MCP tool name; omitting `tool_name` suppresses MCP registration, omitting `cli = true` suppresses CLI registration.
- **refactor(cli): simplify `src/cli/mod.rs`** — now only exports `error` module (CliError + From<ApiError>). All `*_cmd.rs` files, `args.rs`, and `disambiguation.rs` deleted; argument parsing is now generated by sdforge macros.
- **refactor(mcp): delete `src/mcp/`** — MCP server construction moved to `src/main.rs` via `sdforge::mcp::build()`. Kit injection uses global `OnceLock<Arc<Kit>>` in `src/service/runtime.rs`.
- **refactor(mod): harden mod/crate boundaries** — `mod.rs` files now contain only `pub mod` / `pub use` / trait definitions. Implementation moved to dedicated files: `src/daemon/{error,event,index_observer,daemon}.rs`, `src/ir/{types,extract_result}.rs`, `src/parse/helpers.rs`, `src/trace/{context,types}.rs`.

### Added

- **test(cli): `tests/cli_integration.rs`** — 7 CLI integration tests covering help, version, no-subcommand, list, status, query, and unknown-subcommand exit codes.

### Fixed

- **fix(cli): clap "command name `codenexus` is duplicated"** — all `#[service_api]` declarations previously used `name = "codenexus"`, causing 32 duplicate CLI subcommands. Fixed by setting `name` to the per-command tool name.
- **fix(mcp): MCP integration test 32 tools instead of 5** — CLI wrappers incorrectly carried `tool_name`, generating unwanted MCP tool registrations. Fixed by removing `tool_name` from all 27 CLI wrappers.

## [0.3.1] - 2026-07-10

### Fixed

- **fix(mcp): rename search `semantic`→`fulltext` + add cypher validation in `query`** — the MCP `search` tool's `semantic` parameter was renamed to `fulltext` to accurately reflect its behavior (BM25 full-text search vs structured name search). The `query` MCP tool now validates input via `validate_cypher_subset` (ADR-021) before execution, rejecting destructive Cypher clauses.
- **fix(mcp): use `AtomicU64` counter for unique `error_id`** — `mcp_error` now generates unique `error_id` values via a process-level `AtomicU64` counter instead of timestamps, guaranteeing uniqueness under concurrent calls.
- **fix(test): implement `Drop` for `McpClient`** — the test helper `McpClient` in `tests/mcp_integration.rs` now implements `Drop` to kill the MCP subprocess on drop, preventing orphaned processes from accumulating across test runs.

### Changed

- **perf(resolve): add `fill_reachable_from`** — `IncludesGraph` gains a `fill_reachable_from` method that writes into a caller-provided `HashSet` buffer, avoiding per-call allocation when the caller already has a reusable set.
- **ci(release): source-only GitHub Release** — `.github/workflows/release.yml` now creates a GitHub Release (auto-generated notes + prerelease detection on `-rc`/`-beta`/`-alpha` suffixes) when a `v*` tag is pushed. GitHub auto-attaches the tag's source archives (zip/tar.gz); no binary compilation or `crates.io` publish.

## [0.3.0] - 2026-07-10

### Added

- **sdforge MCP framework integration** — replaced hand-written JSON-RPC in `src/cli/mcp_cmd.rs` with sdforge's declarative `#[forge]` macro + sdforge `mcp` stdio transport. 6 MCP tools exposed: `query`, `trace`, `impact`, `search`, `context`, `architecture`. New `mcp` feature flag gates sdforge/tokio dependencies.
- **C++ #include tracking** — `INCLUDES` edge type for C++ `#include` directives (separate from `IMPORTS` used by other languages). `IncludesGraph` data structure + `resolve_include` basename matching + `lookup_exported_in_scope` for #include-scoped cross-file call resolution. Fixes BUG-C4 (C++ free functions now correctly `is_exported=true`).
- **Complexity analysis** (v0.2.1) — cyclomatic, cognitive, nesting depth, and function-length metrics with 4-level severity classification (Green/Yellow/Red/Critical). `complexity` feature flag.
- **Dead-code detection** (`dead-code` command, `analysis` feature) — identifies unreachable functions.
- **Architecture overview** (`architecture` command, `analysis` feature) — graph-based architecture summary.
- **Community detection** (`community` command, `community` feature) — Louvain modularity optimization on the CALLS graph.
- **Cross-service link detection** (`cross-service` command, `cross-service` feature) — matches HTTP route patterns against caller string literals.
- **API review toolkit** (`route_map`, `shape_check`, `api_impact`, `tool_map` commands, `api-review` feature) — route maps, shape checks, API impact analysis, tool mappings.
- **LSP semantic type resolution** (`lsp_goto_def`, `lsp_hover` commands, `lsp` feature, v0.2.0) — subprocess integration with rust-analyzer for IDE-grade definition/hover queries.
- **Go, Java, C++ language support** — tree-sitter grammars for Go (`lang-go`), Java (`lang-java`), C++ (`lang-cpp`). Total supported languages: 8.

### Changed

- **lib.rs / main.rs boundary clarified** — `src/lib.rs` exposes the Rust SDK interface; `src/main.rs` wraps it for CLI + MCP via sdforge. The `mcp` subcommand is handled by the binary's `mcp` module (`src/mcp/mod.rs`), not a `*_cmd` module in the library.
- **CLI dispatch refactored** — `Command::Mcp` variant is feature-gated and dispatched to `mcp::run(kit, args)` in the binary, not through the library's `cli::dispatch`.
- **Feature presets updated** — `core` now includes C+Rust+Python (was C+Rust+Fortran). `full` includes all 21 languages + daemon + analysis + complexity + api-review + community + cross-service + lsp + mcp + cli + cache.

### Fixed

- **BUG-C4: C++ cross-file call resolution** — C++ free functions were not marked `is_exported=true`, causing cross-file CALLS edges to fail resolution. Fixed by enabling `is_exported` for C++ free functions (non-methods).

## [0.1.0] - 2026-06-29

Initial public release. CodeNexus indexes source code into a queryable knowledge graph using tree-sitter for parsing and LadybugDB for graph storage, with a Cypher subset query interface, symbol tracing, impact analysis, and a Model Context Protocol (MCP) server for AI agent integration.

### Added

- **Multi-language parsing** for C, Rust, Fortran, Python, and TypeScript via tree-sitter grammars, with tiered feature presets (`minimal` < `core` < `full`).
- **Unified graph schema** with 44 node types and 30 edge types, each edge carrying a confidence score (0.0-1.0) and a confidence tier (`SameFile` / `ImportScoped` / `Global`).
- **CLI commands**: `index`, `query`, `trace`, `impact`, `search`, `context`, `detect-changes`, `rename`, `export`, `import`, `setup`, `hook`, `mcp`, `daemon`, `status`, `list`, `clean`.
- **Incremental indexing** with SHA-256 file hash diffing — re-parses only changed files.
- **RAM-first indexing** (`--ram-first`) — LZ4-compress source into memory and emit a single `COPY FROM` dump.
- **Parallel parsing** with Rayon + a thread-local tree-sitter parser pool.
- **Symbol tracing** (`trace`) — bidirectional `Calls` and `DataFlows` paths with `--uid` / `--file` / `--kind` disambiguation narrowing.
- **Impact analysis** (`impact`) — change blast radius layered by depth, with `--min-confidence` filtering.
- **360° symbol context** (`context`) — incoming calls/imports, outgoing calls, and participating processes.
- **Detect-changes** — git diff → affected symbols with `risk_level`.
- **Rename** (`rename`) — graph-edits for high-confidence matches plus text-search edits, with `--dry-run`.
- **Cross-language FFI resolution** — C–Fortran `bind(C)` and Rust `extern` FFI calls recorded as `FfiCalls` edges.
- **Team artifacts** — `export` / `import` of compressed `.graph.zst` indexes for sharing across machines.
- **MCP server** (`mcp`) — stdio JSON-RPC 2.0 server implementing Model Context Protocol version 2024-11-05, plus `setup` auto-detection of Claude Code / Cursor / Codex and `hook` for `PreToolUse` / `PostToolUse` JSON events.
- **Daemon mode** (`daemon` feature) — file watching with debounced auto-incremental reindexing.
- **Vector embedding** (`embed` feature) — semantic search via remote OpenAI-compatible HTTP API or local ONNX inference (`ort` + `tokenizers`).
- **Cypher subset validation** (ADR-021) — PEG-based validation at the CLI boundary using `pest`, surfacing query errors before they reach the database.
- **Database corruption detection** with exit code 4 mapped through Kit errors.
- **Batch `delete_file_nodes`** for incremental reindex — avoids N×1 round trips during reindex.
- **Benchmark suite** (`criterion`) covering `index`, `query`, `trace`, `incremental`, `memory`, and `daemon` paths.
- **End-to-end integration tests** for corruption handling and batch deletion.
- **Library crate** — CodeNexus is published as a Rust library in addition to the CLI binary, with runnable examples under `examples/`.

### Changed

- **Migrated all components to the `trait-kit` unified registry** (T6 / unified-architecture Phase 2). The in-tree `src/kit/shim.rs` fallback was removed once every module used `build_kit`. `trait-kit` is now a hard dependency.
- **Broke the `parse` ↔ `resolve` circular dependency** by introducing a `src/ir/` module for shared intermediate representations (R-3).
- **Reduced `visit_node` parameter count** by introducing a `VisitContext` carried across the 5 extractors (R-1).
- **Calls edge confidence range** adjusted to 0.80–0.95 to better reflect extraction certainty.
- **File path normalization** and `Parameter` node persistence added to the index pipeline.
- **Release profile** tuned for maximum performance and smallest binary: `opt-level = 3`, fat LTO, single codegen unit, `panic = abort`, `strip = true`.

### Fixed

- **FQN collision (P0)** — fully qualified names now retain the full file name and use a disambiguator suffix to avoid cross-file collisions.
- **Architectural orphan edges (P0-1)** — parser edge endpoint IDs are now synced so edges never reference missing nodes.
- **CSV header ghost nodes (P2-1 / DQ-005)** — `COPY FROM` now uses the `HEADER` option so CSV header rows are not ingested as phantom nodes.
- **C `#define` macro missed extraction (P1)** — `preproc_def` / `preproc_function_def` nodes are now extracted.
- **C anonymous `typedef struct` and header-file function declarations** are now extracted (P1-1 / P1-2).
- **Rust `Module` missed extraction (P2-1)** — module nodes are now correctly emitted.
- **Python `Function` over-extraction (P2-5)** — false-positive function nodes are suppressed.
- **TypeScript `Const` / `Interface` / `Function` extraction bugs (P2)** — three correctness bugs in the TS extractor.
- **TypeScript anonymous `export default` function** missed extraction (P2-4).
- **`dedupe_qn` O(N²) → O(1)** — switched to a `HashSet` lookup (MED-002), eliminating a quadratic bottleneck during qualified-name deduplication.
- **CSV injection safety** — string escaping consolidated into the schema module and applied consistently across Python and TypeScript extractors.
- **Daemon debounce and index phase error handling** — file-watching events no longer crash the daemon on transient parse errors.
- **Exit code 4 for corrupt DB** now surfaces correctly through the Kit error layer rather than being masked as a generic failure.
- **`pattern_name` fallback in the Rust extractor** now only accepts valid identifiers, preventing garbage names from malformed patterns.

### Security

- **CSV injection hardening** — `escape_cypher_string` consolidated into the storage schema module; all string fields routed through it before `COPY FROM`.
- **Database corruption detection** — corrupt LadybugDB files are detected at startup and reported with a distinct exit code (4) instead of being loaded into a half-valid state.
- **`.env` files ignored by default** in `.gitignore`, with an explicit `!.env.example` allow-list so the template is tracked but real secrets never are.

[Unreleased]: https://github.com/Kirky-X/codenexus/compare/v0.3.8...HEAD
[0.3.8]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.8
[0.3.7]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.7
[0.3.6]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.6
[0.3.5]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.5
[0.3.4]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.4
[0.3.3]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.3
[0.3.2]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.2
[0.3.1]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.1
[0.3.0]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.0
[0.1.0]: https://github.com/Kirky-X/codenexus/releases/tag/v0.1.0
