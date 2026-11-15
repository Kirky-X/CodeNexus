# 🧪 CodeNexus Test Scenario Matrix

> Applies to CodeNexus **v0.4.0-rc.2** (Cargo workspace, `Cargo.toml` MSRV 1.97.1 / edition 2021)
> Purpose: exhaustively enumerate every acceptance scenario covered by the repo's existing test suites, as a baseline for regression comparison and additional test planning.
> Written from (read-only verification): 11 integration test files under `tests/`, 172 `#[cfg(test)]` modules in `src/`, the `tests/acceptance/run_acceptance.sh` acceptance harness, 7 Criterion benchmark groups in `benches/`, and `.github/workflows/ci.yml`.
> Every referenced test name was verified with `grep` to exist in the current code; statistics count `#[test]` / `#[tokio::test]` functions (fixed-string grep).

## 📋 Table of Contents

- [Reading conventions](#-reading-conventions)
- [Test layer overview](#-test-layer-overview)
- [Scenario matrix](#-scenario-matrix)
  - [1. Indexing & incremental (IDX)](#1-indexing--incremental-idx)
  - [2. Cypher queries (QRY)](#2-cypher-queries-qry)
  - [3. Search (SRCH)](#3-search-srch)
  - [4. Tracing & impact analysis (TRC / IMP)](#4-tracing--impact-analysis-trc--imp)
  - [5. Symbol context (CTX)](#5-symbol-context-ctx)
  - [6. Analysis toolkit (ANA)](#6-analysis-toolkit-ana)
  - [7. Architecture diagrams & semantic Delta (DIA)](#7-architecture-diagrams--semantic-delta-dia)
  - [8. MCP integration (MCP)](#8-mcp-integration-mcp)
  - [9. Team artifacts (ART)](#9-team-artifacts-art)
  - [10. CLI contract (CLI)](#10-cli-contract-cli)
  - [11. Kit capability registry (KIT)](#11-kit-capability-registry-kit)
  - [12. i18n & non-ASCII paths (I18N)](#12-i18n--non-ascii-paths-i18n)
  - [13. Error handling & data isolation (ERR)](#13-error-handling--data-isolation-err)
  - [14. Multi-language parsing (MLT)](#14-multi-language-parsing-mlt)
- [Acceptance tests (8 real-language projects)](#-acceptance-tests-8-real-language-projects)
- [Benchmark regression scenarios](#-benchmark-regression-scenarios)
- [Execution commands (identical to CI)](#-execution-commands-identical-to-ci)
- [Statistics summary](#-statistics-summary)

---

## 📑 Reading conventions

- **Scenario IDs**: numbered by functional-domain prefix (e.g. `IDX-01`); the "Corresponding test" column gives the real `file :: test function` location.
- **Layers**: E2E = `tests/e2e_feature_suite.rs` / `tests/end_to_end.rs`; CLI = subprocess-driven `tests/cli_index_and_daemon.rs` / `tests/cli_integration.rs`; unit = inline `#[cfg(test)]` in `src/`.
- There are 4832 unit tests in total (4789 `#[test]` + 43 `#[tokio::test]`); enumerating each is not readable, so at the unit layer this matrix only gives representative anchors; the integration layer (140 tests) is enumerated one by one.
- After changing code, check affected scenarios against this matrix and keep the "scenario ID ↔ test function" mapping in sync.

---

## 🗂️ Test layer overview

| Layer | Location | Count (as of v0.4.0-rc.2) | Notes |
|-------|----------|---------------------------|-------|
| Unit tests | inline `#[cfg(test)]` in `src/` (172 files) | 4832 | Core logic of each module; run by `cargo test --lib` (4789 `#[test]` + 43 `#[tokio::test]`) |
| Integration E2E | `tests/e2e_feature_suite.rs` | 42 | Full-feature end to end: index → query → trace → analysis toolkit |
| Integration E2E | `tests/end_to_end.rs` | 20 | Multi-language repo index → query → FFI/data-flow edge verification |
| CLI subprocess | `tests/cli_index_and_daemon.rs` | 41 | Compiled binary subprocess walks the full CLI path, covering 27 subcommands |
| CLI contract | `tests/cli_integration.rs` | 15 | help (full subcommand list) / version / empty DB / unknown subcommand |
| Diagram integration | `tests/diagram_integration.rs` | 2 | diagram rendering and the showcase quality gate |
| MCP | `tests/mcp_integration.rs` | 2 | MCP server initialization and tool listing |
| hub artifacts | `tests/hub_integration.rs` | 2 | push/pull/list full chain on an in-process HTTP registry (receipts / request headers / BLAKE3 checks / tamper rejection) |
| Kit bootstrap | `tests/kit_bootstrap.rs` | 4 | Capability registry resolvability |
| Non-ASCII paths | `tests/non_ascii_path_test.rs` | 9 | CJK / Unicode paths and project names |
| Log attributes | `tests/log_attr_integration.rs` | 2 | Log field masking and attribute assertions |
| Viewer | `tests/viewer_integration.rs` | 1 | Embedded viewer assets and route self-check |
| Acceptance tests | `tests/acceptance/run_acceptance.sh` | 8 real projects | 8-language open-source project indexing + gitnexus cross-validation |
| Benchmark regression | `benches/` (7 Criterion groups) | — | Throughput/latency/memory guards, see [Benchmark regression scenarios](#-benchmark-regression-scenarios) |
| Total (integration layer) | `tests/` | **140** | 11 files |

---

## 🔢 Scenario matrix

### 1. Indexing & incremental (IDX)

| ID | Scenario | Corresponding test |
|----|----------|--------------------|
| IDX-01 | Rust repo indexes successfully | `cli_index_and_daemon.rs :: index_builds_rust_repo_succeeds` |
| IDX-02 | Indexing a nonexistent path fails | `cli_index_and_daemon.rs :: index_nonexistent_path_fails`; `end_to_end.rs :: index_nonexistent_path_returns_error` |
| IDX-03 | `--force` full rebuild exits 0 | `cli_index_and_daemon.rs :: index_force_rebuild_exits_0`; `end_to_end.rs :: force_reindexes_all_files` |
| IDX-04 | Incremental indexing skips unchanged files | `cli_index_and_daemon.rs :: index_incremental_skips_unchanged`; `end_to_end.rs :: incremental_index_skips_unchanged_files` |
| IDX-05 | Incremental indexing detects modified files | `e2e_feature_suite.rs :: incremental_index_detects_modified_file`; `end_to_end.rs :: incremental_index_detects_new_file` |
| IDX-06 | Indexing an empty repo produces 0 files | `e2e_feature_suite.rs :: index_empty_repo_produces_zero_files` |
| IDX-07 | Files with no parseable symbols produce no symbols | `e2e_feature_suite.rs :: index_file_with_no_parseable_symbols` |
| IDX-08 | `.gitignore` rules are respected (target dir skipped) | `e2e_feature_suite.rs :: gitignore_rules_are_respected`; `end_to_end.rs :: gitignore_target_dir_skipped` |
| IDX-09 | Unicode path indexing | `e2e_feature_suite.rs :: index_with_unicode_path` |
| IDX-10 | Index result fields complete (files/nodes/edges/project_id) | `e2e_feature_suite.rs :: index_result_fields_are_populated` |
| IDX-11 | Indexing creates a Project node | `end_to_end.rs :: index_creates_project_node` |
| IDX-12 | Multi-language repo indexes as a whole | `end_to_end.rs :: index_multilang_repo_succeeds` |
| IDX-13 | Indexing emits the complete set of log events | `end_to_end.rs :: index_emits_all_log_events` |

### 2. Cypher queries (QRY)

| ID | Scenario | Corresponding test |
|----|----------|--------------------|
| QRY-01 | Query returns Function nodes | `e2e_feature_suite.rs :: cypher_query_returns_function_nodes` |
| QRY-02 | Invalid Cypher syntax returns an error | `e2e_feature_suite.rs :: cypher_invalid_syntax_returns_error`; `cli_index_and_daemon.rs :: query_invalid_cypher_exits_2` |
| QRY-03 | Empty query string returns an error | `e2e_feature_suite.rs :: cypher_empty_string_returns_error` |
| QRY-04 | Cypher queries run after indexing | `end_to_end.rs :: cypher_query_after_index`; `cli_integration.rs :: query_command_returns_result` |
| QRY-05 | Query hits the Project node | `e2e_feature_suite.rs :: cypher_query_project_node_exists` |
| QRY-06 | CLI `query` returns node names | `cli_index_and_daemon.rs :: query_returns_node_names` |

### 3. Search (SRCH)

| ID | Scenario | Corresponding test |
|----|----------|--------------------|
| SRCH-01 | Structured exact search hits | `e2e_feature_suite.rs :: structured_search_exact_match`; `cli_index_and_daemon.rs :: search_exact_finds_symbol` |
| SRCH-02 | No match returns an empty result | `e2e_feature_suite.rs :: structured_search_no_match_returns_empty` |
| SRCH-03 | The `limit` parameter takes effect | `e2e_feature_suite.rs :: structured_search_respects_limit` |
| SRCH-04 | Search by type returns the correct label | `e2e_feature_suite.rs :: search_by_type_returns_correct_label`; `end_to_end.rs :: structured_search_by_type` |
| SRCH-05 | BM25 full-text search finds content matches | `e2e_feature_suite.rs :: fulltext_search_finds_content_matches`; `end_to_end.rs :: fulltext_search_finds_matches` |
| SRCH-06 | Search by file returns symbols within the file | `e2e_feature_suite.rs :: search_by_file_returns_symbols_in_file` |
| SRCH-07 | Full-text search respects the `--project` filter | `e2e_feature_suite.rs :: fulltext_search_respects_project_filter` |
| SRCH-08 | Structured search by name | `end_to_end.rs :: structured_search_by_name` |

### 4. Tracing & impact analysis (TRC / IMP)

| ID | Scenario | Corresponding test |
|----|----------|--------------------|
| TRC-01 | `trace calls` returns call paths | `e2e_feature_suite.rs :: trace_calls_returns_call_paths`; `cli_index_and_daemon.rs :: trace_returns_call_paths` |
| TRC-02 | Unknown symbol returns an empty graph | `e2e_feature_suite.rs :: trace_unknown_symbol_returns_empty_graph` |
| TRC-03 | FFI trace returns a cross-language path | `end_to_end.rs :: ffi_trace_returns_cross_language_path` |
| IMP-01 | Impact analysis returns upstream callers | `e2e_feature_suite.rs :: impact_analysis_returns_upstream_callers`; `cli_index_and_daemon.rs :: impact_returns_blast_radius` |
| TRC-04 | High-fan-in subgraph impact analysis respects the 5000 cap (benchmark regression) | `benches/trace_bench.rs :: impact_large_subgraph` (4971 nodes, asserts `affected.len() <= MAX_NODES_LIMIT`) |

### 5. Symbol context (CTX)

| ID | Scenario | Corresponding test |
|----|----------|--------------------|
| CTX-01 | 360° context collects incoming and outgoing calls | `e2e_feature_suite.rs :: context_collect_incoming_and_outgoing` |
| CTX-02 | CLI `context` returns the symbol view | `cli_index_and_daemon.rs :: context_returns_symbol_view` |
| CTX-03 | Nonexistent symbol exits 2 | `cli_index_and_daemon.rs :: context_nonexistent_symbol_exits_2` |

### 6. Analysis toolkit (ANA)

| ID | Scenario | Corresponding test |
|----|----------|--------------------|
| ANA-01 | Dead-code detection finds unreachable functions | `e2e_feature_suite.rs :: dead_code_detection_finds_unreachable_functions` |
| ANA-02 | Exported functions are judged live | `e2e_feature_suite.rs :: dead_code_detection_exported_functions_are_live` |
| ANA-03 | CLI `dead_code` exits 0 | `cli_index_and_daemon.rs :: dead_code_exits_0` |
| ANA-04 | Architecture overview returns language statistics | `e2e_feature_suite.rs :: architecture_overview_returns_language_stats` |
| ANA-05 | CLI `architecture` exits 0 | `cli_index_and_daemon.rs :: architecture_exits_0` |
| ANA-06 | Complexity analysis returns metrics | `e2e_feature_suite.rs :: complexity_analysis_returns_metrics` |
| ANA-07 | CLI `complexity` exits 0 | `cli_index_and_daemon.rs :: complexity_exits_0` |
| ANA-08 | Community detection returns a result | `e2e_feature_suite.rs :: community_detection_returns_result` |
| ANA-09 | CLI `community` exits 0 | `cli_index_and_daemon.rs :: community_exits_0` |
| ANA-10 | Cross-service detection on a simple project returns empty | `e2e_feature_suite.rs :: cross_service_detection_returns_empty_for_simple_project` |
| ANA-11 | CLI `cross_service` exits 0 | `cli_index_and_daemon.rs :: cross_service_exits_0` |
| ANA-12 | API review commands exit 0 (route_map / api_impact / shape_check / tool_map) | `cli_index_and_daemon.rs :: route_map_exits_0` / `api_impact_exits_0` / `shape_check_exits_0` / `tool_map_exits_0` |
| ANA-13 | git diff change detection exits gracefully on a non-git repo | `cli_index_and_daemon.rs :: detect_changes_on_non_git_repo_exits_gracefully` |
| ANA-14 | rename dry run exits 0 | `cli_index_and_daemon.rs :: rename_dry_run_exits_0` |

### 7. Architecture diagrams & semantic Delta (DIA)

| ID | Scenario | Corresponding test |
|----|----------|--------------------|
| DIA-01 | diagram renders variants and types from graph facts | `diagram_integration.rs :: diagram_int_renders_variants_and_types_from_graph_facts` |
| DIA-02 | showcase tier blocks delivery on invalid evidence | `diagram_integration.rs :: diagram_int_showcase_blocks_on_invalid_evidence` |

### 8. MCP integration (MCP)

| ID | Scenario | Corresponding test |
|----|----------|--------------------|
| MCP-01 | MCP server initializes and lists all tools | `mcp_integration.rs :: mcp_server_initializes_and_lists_tools` |
| MCP-02 | CLI `hook` reads stdin and responds | `cli_index_and_daemon.rs :: hook_reads_stdin_and_responds` |
| MCP-03 | `setup --force false` exits gracefully | `cli_index_and_daemon.rs :: setup_with_force_false_exits_gracefully` |

### 9. Team artifacts (ART)

| ID | Scenario | Corresponding test |
|----|----------|--------------------|
| ART-01 | `export` produces a zstd artifact | `cli_index_and_daemon.rs :: export_creates_artifact` |
| ART-02 | export → import round trip is consistent | `cli_index_and_daemon.rs :: export_then_import_roundtrip` |

### 10. CLI contract (CLI)

| ID | Scenario | Corresponding test |
|----|----------|--------------------|
| CLI-01 | `--help` lists all commands | `cli_integration.rs :: help_lists_all_commands` |
| CLI-02 | `--version` prints the version | `cli_integration.rs :: version_flag_prints_version`; library layer `e2e_feature_suite.rs :: version_api_returns_non_empty` |
| CLI-03 | No subcommand exits gracefully | `cli_integration.rs :: no_subcommand_exits_gracefully` |
| CLI-04 | `list` / `status` work on an empty DB | `cli_integration.rs :: list_command_works_with_empty_db` / `status_command_works_with_empty_db`; `cli_index_and_daemon.rs :: list_shows_indexed_project` / `status_shows_project` |
| CLI-05 | Unknown subcommand errors out | `cli_integration.rs :: unknown_subcommand_exits_with_error` |
| CLI-06 | daemon detects new files / modified files / ignores non-code files | `cli_index_and_daemon.rs :: daemon_hot_update_detects_new_file` / `daemon_hot_update_detects_modified_file` / `daemon_hot_update_ignores_non_code_file` |
| CLI-07 | daemon exits 2 on a nonexistent path | `cli_index_and_daemon.rs :: daemon_nonexistent_path_exits_2` |
| CLI-08 | LSP commands exit gracefully without a server | `cli_index_and_daemon.rs :: lsp_goto_def_without_server_exits_gracefully` / `lsp_hover_without_server_exits_gracefully` |
| CLI-09 | `clean` removes a project | `cli_index_and_daemon.rs :: clean_removes_project` |

### 11. Kit capability registry (KIT)

| ID | Scenario | Corresponding test |
|----|----------|--------------------|
| KIT-01 | All core capabilities resolve through the Kit | `kit_bootstrap.rs :: all_core_capabilities_resolvable_through_kit`; `e2e_feature_suite.rs :: kit_all_core_modules_resolvable` |
| KIT-02 | daemon capability resolves when the `daemon` feature is on | `kit_bootstrap.rs :: daemon_capability_resolvable_when_feature_on` |
| KIT-03 | embed capability resolves when the `embeddings` feature is on | `kit_bootstrap.rs :: embed_capability_resolvable_when_feature_on` |
| KIT-04 | storage capability is functional | `kit_bootstrap.rs :: storage_capability_is_functional` |

### 12. i18n & non-ASCII paths (I18N)

| ID | Scenario | Corresponding test |
|----|----------|--------------------|
| I18N-01 | Chinese directory name | `non_ascii_path_test.rs :: test_chinese_directory_name` |
| I18N-02 | Chinese file name | `non_ascii_path_test.rs :: test_chinese_file_name` |
| I18N-03 | Japanese directory name | `non_ascii_path_test.rs :: test_japanese_directory_name` |
| I18N-04 | Korean directory name | `non_ascii_path_test.rs :: test_korean_directory_name` |
| I18N-05 | Mixed ASCII and non-ASCII paths | `non_ascii_path_test.rs :: test_mixed_ascii_and_non_ascii_paths` |
| I18N-06 | Unicode project name | `non_ascii_path_test.rs :: test_unicode_project_name` |
| I18N-07 | Non-ASCII database path | `non_ascii_path_test.rs :: test_non_ascii_db_path` |
| I18N-08 | Walker discovers non-ASCII paths | `non_ascii_path_test.rs :: test_walker_discovers_non_ascii_paths` |
| I18N-09 | Incremental indexing on non-ASCII paths | `non_ascii_path_test.rs :: test_incremental_index_non_ascii_path` |

### 13. Error handling & data isolation (ERR)

| ID | Scenario | Corresponding test |
|----|----------|--------------------|
| ERR-01 | Corrupt database returns exit code 4 (DatabaseCorrupt) | `end_to_end.rs :: corrupt_db_returns_exit_code_4`; `e2e_feature_suite.rs :: corrupt_db_error_chain` |
| ERR-02 | Multi-project data isolation | `e2e_feature_suite.rs :: multi_project_data_isolation`; `end_to_end.rs :: multi_project_isolation` |
| ERR-03 | EdgeType model round-trip serialization | `e2e_feature_suite.rs :: model_edge_type_roundtrip` |

### 14. Multi-language parsing (MLT)

| ID | Scenario | Corresponding test |
|----|----------|--------------------|
| MLT-01 | Go file indexing | `e2e_feature_suite.rs :: index_go_file` |
| MLT-02 | Java file indexing | `e2e_feature_suite.rs :: index_java_file` |
| MLT-03 | JavaScript file indexing | `e2e_feature_suite.rs :: index_javascript_file` |
| MLT-04 | Bash file indexing | `e2e_feature_suite.rs :: index_bash_file` |
| MLT-05 | HTML / CSS / JSON file indexing | `e2e_feature_suite.rs :: index_html_css_json_files` |
| MLT-06 | Python file indexing | `end_to_end.rs :: index_python_file` |
| MLT-07 | TypeScript file indexing | `end_to_end.rs :: index_typescript_file` |
| MLT-08 | Fortran file indexing | `end_to_end.rs :: index_fortran_file` |
| MLT-09 | Multi-language FFI edges enter the graph | `e2e_feature_suite.rs :: multilang_ffi_edges_in_graph`; `end_to_end.rs :: ffi_edge_exists_after_multilang_index` |
| MLT-10 | Reads / Writes data-flow edges exist | `e2e_feature_suite.rs :: reads_writes_edges_after_multilang_index`; `end_to_end.rs :: reads_writes_edges_exist_after_multilang_index` |
| MLT-11 | Full pipeline chained: index → query → trace | `e2e_feature_suite.rs :: full_pipeline_index_then_query_then_trace` |

> Deeper unit tests of the language extractors (per-language node/edge extraction, FQNs, scope resolution, etc.) live in the inline `#[cfg(test)]` blocks of `src/parse/` and `src/resolve/` modules, run by `cargo test --lib`.

---

## 🌐 Acceptance tests (8 real-language projects)

`tests/acceptance/run_acceptance.sh` clones 8 open-source projects (one per language), indexes them with CodeNexus, cross-validates against gitnexus, and produces a diff report:

| Project | Repository | Tag | Language |
|---------|------------|-----|----------|
| serde | serde-rs/serde | v1.0.219 | Rust |
| requests | psf/requests | v2.32.3 | Python |
| vscode-uri | microsoft/vscode-uri | v3.0.2 | TypeScript |
| redis | redis/redis | 7.4.2 | C |
| gin | gin-gonic/gin | v1.10.0 | Go |
| jackson-databind | FasterXML/jackson-databind | jackson-databind-2.18.2 | Java |
| fmt | fmtlib/fmt | 11.0.2 | C++ |
| OpenBLAS | OpenMathLib/OpenBLAS | v0.3.28 | Fortran |

```bash
# Dry run (echoes commands only, validates syntax)
bash tests/acceptance/run_acceptance.sh --dry-run

# Full acceptance (requires git / jq / cargo)
bash tests/acceptance/run_acceptance.sh

# Clean fixtures and acceptance DBs
bash tests/acceptance/run_acceptance.sh --clean
```

---

## ⏱️ Benchmark regression scenarios

The 7 Criterion benchmark groups in `benches/` guard performance regressions (SLO threshold table in [`benches/README.md`](../../benches/README.md)):

| Benchmark | Regression scenarios |
|-----------|----------------------|
| `index_bench` | 10-file cold-start throughput |
| `incremental_bench` | 1000-file cold start / single-file incremental / 500-file bulk incremental |
| `query_bench` | Cypher match / count / search by name / search with project filter |
| `trace_bench` | Call-chain tracing at depth 5/10/50, `trace_all` depth 10, 4971-node high-fan-in impact-analysis cap guard |
| `memory_bench` | 10k-file first-index peak RSS, daemon sustained RSS over 10 incremental rounds, ram-first vs default comparison (long-running scenarios need `BENCH_RUN_IGNORED=1`) |
| `daemon_bench` (`daemon` feature) | Debounce response latency, event-queue throughput (zero loss) |
| `graph_bench` | Graph data structure operations |

---

## ▶️ Execution commands (identical to CI)

```bash
# Unit tests (the CI matrix runs seven tiers: minimal / core / full / core,daemon,analysis,complexity / core,lsp,cache / full,embeddings / full,webui)
cargo test --lib --verbose
cargo test --lib --no-default-features --features "core" --verbose

# Integration tests (local; CLI subprocess tests that need the compiled binary are handled by cargo test automatically)
cargo test --test e2e_feature_suite
cargo test --test end_to_end
cargo test --test cli_index_and_daemon
cargo test --test non_ascii_path_test

# Format and lint gates
cargo +nightly fmt --all -- --check
cargo +1.97.1 clippy -- -D warnings
cargo +1.97.1 clippy --lib --no-default-features --features minimal -- -D warnings

# Coverage gate (≥95% line coverage)
cargo llvm-cov --lib --fail-under-lines 95 --lcov --output-path lcov.info

# Acceptance tests (8 real-language projects)
bash tests/acceptance/run_acceptance.sh --dry-run
```

> The CI test job only runs `cargo test --lib` (seven feature tiers); the `tests/` integration tests and the acceptance harness run locally / on demand.

---

## 📊 Statistics summary

| Category | Count |
|----------|-------|
| Unit test functions (inline in `src/`) | 4789 `#[test]` + 43 `#[tokio::test]` = 4832 (172 source files containing `#[cfg(test)]`) |
| Integration test functions (11 files in `tests/`) | 140 |
| ├ `e2e_feature_suite.rs` | 42 |
| ├ `cli_index_and_daemon.rs` | 41 |
| ├ `end_to_end.rs` | 20 |
| ├ `non_ascii_path_test.rs` | 9 |
| ├ `cli_integration.rs` | 15 |
| ├ `hub_integration.rs` | 2 |
| ├ `kit_bootstrap.rs` | 4 |
| ├ `diagram_integration.rs` | 2 |
| ├ `log_attr_integration.rs` | 2 |
| ├ `mcp_integration.rs` | 2 |
| └ `viewer_integration.rs` | 1 |
| Acceptance projects | 8 (one real open-source repo per language) |
| Criterion benchmark groups | 7 |
| Coverage gate | Line coverage ≥ 95% (CI coverage job + pre-push hook) |
| Integration-layer scenarios enumerated by this matrix | 100 rows (IDX 13 + QRY 6 + SRCH 8 + TRC/IMP 4 + CTX 3 + ANA 14 + DIA 2 + MCP 3 + ART 2 + CLI 9 + KIT 4 + I18N 9 + ERR 3 + MLT 11 = 91 numbered scenarios, plus several merged rows) |

> To be added: per-scenario enumeration of the 4832 unit tests in `src/` (currently summarized by module anchors); the `fuzz/` directory registers 4 fuzz targets (`escape_cypher_string`, `escape_identifier`, `cypher_subset_parse`, `cnxp_header`); 0.3.13-rc.1 ran 300–500 smoke iterations per target, and when folding them into regression, set a per-target run-count cap.
