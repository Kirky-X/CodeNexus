# Changelog

All notable changes to CodeNexus are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/Kirky-X/codenexus/compare/v0.3.3...HEAD
[0.3.3]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.3
[0.3.2]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.2
[0.3.1]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.1
[0.3.0]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.0
[0.1.0]: https://github.com/Kirky-X/codenexus/releases/tag/v0.1.0
