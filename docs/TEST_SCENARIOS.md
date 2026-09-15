# 🧪 CodeNexus 测试场景矩阵

> 适用版本：CodeNexus **v0.3.12**（Cargo workspace，`Cargo.toml` MSRV 1.97.1 / edition 2021）
> 用途：穷举仓库现有测试套件覆盖的全部验收场景，作为回归对照与补充测试规划的基线。
> 编写依据（只读核对）：`tests/` 下 8 个集成测试文件、`src/` 内 147 个 `#[cfg(test)]` 模块、`tests/acceptance/run_acceptance.sh` 验收 harness、`benches/` 7 组 Criterion 基准、`.github/workflows/ci.yml`。
> 所有引用的测试名均经 `grep` 核实存在于当前代码；统计口径为 `#[test]` / `#[tokio::test]` 函数计数。

## 📋 目录

- [阅读约定](#-阅读约定)
- [测试分层总览](#-测试分层总览)
- [场景矩阵](#-场景矩阵)
  - [1. 索引与增量（IDX）](#1-索引与增量idx)
  - [2. Cypher 查询（QRY）](#2-cypher-查询qry)
  - [3. 搜索（SRCH）](#3-搜索srch)
  - [4. 追踪与影响分析（TRC / IMP）](#4-追踪与影响分析trc--imp)
  - [5. 符号上下文（CTX）](#5-符号上下文ctx)
  - [6. 分析工具包（ANA）](#6-分析工具包ana)
  - [7. 架构图与语义 Delta（DIA）](#7-架构图与语义-deltadia)
  - [8. MCP 集成（MCP）](#8-mcp-集成mcp)
  - [9. 团队制品（ART）](#9-团队制品art)
  - [10. CLI 契约（CLI）](#10-cli-契约cli)
  - [11. Kit 能力注册表（KIT）](#11-kit-能力注册表kit)
  - [12. 国际化与非 ASCII 路径（I18N）](#12-国际化与非-ascii-路径i18n)
  - [13. 错误处理与数据隔离（ERR）](#13-错误处理与数据隔离err)
  - [14. 多语言解析（MLT）](#14-多语言解析mlt)
- [验收测试（8 语言真实项目）](#-验收测试8-语言真实项目)
- [基准回归场景](#-基准回归场景)
- [执行命令（与 CI 一致）](#-执行命令与-ci-一致)
- [统计汇总](#-统计汇总)

---

## 📑 阅读约定

- **场景 ID**：按功能域前缀编号（如 `IDX-01`）；「对应测试」列给出真实的 `文件 :: 测试函数` 定位。
- **层级**：E2E = `tests/e2e_feature_suite.rs` / `tests/end_to_end.rs`；CLI = 子进程驱动的 `tests/cli_index_and_daemon.rs` / `tests/cli_integration.rs`；单元 = `src/` 内联 `#[cfg(test)]`。
- 单元测试约 4600+ 条（4588 个 `#[test]` + 29 个 `#[tokio::test]`），逐条穷举不具可读性，本矩阵在单元层只给出代表性锚点；集成层（118 条）逐条穷举。
- 变更代码后请对照本矩阵检查受影响场景，并保持「场景 ID ↔ 测试函数」的同步。

---

## 🗂️ 测试分层总览

| 层级 | 位置 | 数量（截至 v0.3.12） | 说明 |
|------|------|----------------------|------|
| 单元测试 | `src/` 内联 `#[cfg(test)]`（147 个文件） | 约 4600+ | 各模块核心逻辑；`cargo test --lib` 运行，CHANGELOG 记录的基线为 4740 passed / 24 ignored（RC 升级前） |
| 集成 E2E | `tests/e2e_feature_suite.rs` | 40 | 全功能端到端：索引→查询→追踪→分析工具包 |
| 集成 E2E | `tests/end_to_end.rs` | 20 | 多语言仓库索引→查询→FFI/数据流边验证 |
| CLI 子进程 | `tests/cli_index_and_daemon.rs` | 35 | 编译后的二进制子进程走完整 CLI 路径，覆盖 27 个子命令 |
| CLI 契约 | `tests/cli_integration.rs` | 7 | help / version / 空库 / 未知子命令 |
| 图集成 | `tests/diagram_integration.rs` | 2 | diagram 渲染与 showcase 质检门 |
| MCP | `tests/mcp_integration.rs` | 1 | MCP 服务器初始化与工具列举 |
| Kit 引导 | `tests/kit_bootstrap.rs` | 4 | 能力注册表可解析性 |
| 非_ASCII 路径 | `tests/non_ascii_path_test.rs` | 9 | CJK / Unicode 路径与项目名 |
| 验收测试 | `tests/acceptance/run_acceptance.sh` | 8 个真实项目 | 8 语言开源项目索引 + gitnexus 交叉验证 |
| 基准回归 | `benches/`（7 组 Criterion） | — | 吞吐/延迟/内存守护，见 [基准回归场景](#-基准回归场景) |
| 合计（集成层） | `tests/` | **118** | 8 个文件 |

---

## 🔢 场景矩阵

### 1. 索引与增量（IDX）

| ID | 场景 | 对应测试 |
|----|------|----------|
| IDX-01 | Rust 仓库索引成功 | `cli_index_and_daemon.rs :: index_builds_rust_repo_succeeds` |
| IDX-02 | 不存在的路径索引失败 | `cli_index_and_daemon.rs :: index_nonexistent_path_fails`；`end_to_end.rs :: index_nonexistent_path_returns_error` |
| IDX-03 | `--force` 全量重建退出码 0 | `cli_index_and_daemon.rs :: index_force_rebuild_exits_0`；`end_to_end.rs :: force_reindexes_all_files` |
| IDX-04 | 增量索引跳过未变更文件 | `cli_index_and_daemon.rs :: index_incremental_skips_unchanged`；`end_to_end.rs :: incremental_index_skips_unchanged_files` |
| IDX-05 | 增量索引检测到修改的文件 | `e2e_feature_suite.rs :: incremental_index_detects_modified_file`；`end_to_end.rs :: incremental_index_detects_new_file` |
| IDX-06 | 空仓库索引产出 0 文件 | `e2e_feature_suite.rs :: index_empty_repo_produces_zero_files` |
| IDX-07 | 无可解析符号的文件不产生符号 | `e2e_feature_suite.rs :: index_file_with_no_parseable_symbols` |
| IDX-08 | `.gitignore` 规则被遵守（target 目录跳过） | `e2e_feature_suite.rs :: gitignore_rules_are_respected`；`end_to_end.rs :: gitignore_target_dir_skipped` |
| IDX-09 | Unicode 路径索引 | `e2e_feature_suite.rs :: index_with_unicode_path` |
| IDX-10 | 索引结果字段完整（files/nodes/edges/project_id） | `e2e_feature_suite.rs :: index_result_fields_are_populated` |
| IDX-11 | 索引创建 Project 节点 | `end_to_end.rs :: index_creates_project_node` |
| IDX-12 | 多语言仓库整体索引成功 | `end_to_end.rs :: index_multilang_repo_succeeds` |
| IDX-13 | 索引过程发出完整日志事件 | `end_to_end.rs :: index_emits_all_log_events` |

### 2. Cypher 查询（QRY）

| ID | 场景 | 对应测试 |
|----|------|----------|
| QRY-01 | 查询返回 Function 节点 | `e2e_feature_suite.rs :: cypher_query_returns_function_nodes` |
| QRY-02 | 非法 Cypher 语法返回错误 | `e2e_feature_suite.rs :: cypher_invalid_syntax_returns_error`；`cli_index_and_daemon.rs :: query_invalid_cypher_exits_2` |
| QRY-03 | 空查询串返回错误 | `e2e_feature_suite.rs :: cypher_empty_string_returns_error` |
| QRY-04 | 索引后可执行 Cypher 查询 | `end_to_end.rs :: cypher_query_after_index`；`cli_integration.rs :: query_command_returns_result` |
| QRY-05 | 查询命中 Project 节点 | `e2e_feature_suite.rs :: cypher_query_project_node_exists` |
| QRY-06 | CLI `query` 返回节点名 | `cli_index_and_daemon.rs :: query_returns_node_names` |

### 3. 搜索（SRCH）

| ID | 场景 | 对应测试 |
|----|------|----------|
| SRCH-01 | 结构化搜索 exact 命中 | `e2e_feature_suite.rs :: structured_search_exact_match`；`cli_index_and_daemon.rs :: search_exact_finds_symbol` |
| SRCH-02 | 无匹配返回空结果 | `e2e_feature_suite.rs :: structured_search_no_match_returns_empty` |
| SRCH-03 | `limit` 参数生效 | `e2e_feature_suite.rs :: structured_search_respects_limit` |
| SRCH-04 | 按类型搜索返回正确 label | `e2e_feature_suite.rs :: search_by_type_returns_correct_label`；`end_to_end.rs :: structured_search_by_type` |
| SRCH-05 | BM25 全文搜索命中内容匹配 | `e2e_feature_suite.rs :: fulltext_search_finds_content_matches`；`end_to_end.rs :: fulltext_search_finds_matches` |
| SRCH-06 | 按文件搜索返回文件内符号 | `e2e_feature_suite.rs :: search_by_file_returns_symbols_in_file` |
| SRCH-07 | 全文搜索遵守 `--project` 过滤 | `e2e_feature_suite.rs :: fulltext_search_respects_project_filter` |
| SRCH-08 | 按名称结构化搜索 | `end_to_end.rs :: structured_search_by_name` |

### 4. 追踪与影响分析（TRC / IMP）

| ID | 场景 | 对应测试 |
|----|------|----------|
| TRC-01 | `trace calls` 返回调用路径 | `e2e_feature_suite.rs :: trace_calls_returns_call_paths`；`cli_index_and_daemon.rs :: trace_returns_call_paths` |
| TRC-02 | 未知符号返回空图 | `e2e_feature_suite.rs :: trace_unknown_symbol_returns_empty_graph` |
| TRC-03 | FFI 追踪返回跨语言路径 | `end_to_end.rs :: ffi_trace_returns_cross_language_path` |
| IMP-01 | 影响分析返回上游调用者 | `e2e_feature_suite.rs :: impact_analysis_returns_upstream_callers`；`cli_index_and_daemon.rs :: impact_returns_blast_radius` |
| TRC-04 | 高扇入子图影响分析受 5000 上限约束（基准回归） | `benches/trace_bench.rs :: impact_large_subgraph`（4971 节点，断言 `affected.len() <= MAX_NODES_LIMIT`） |

### 5. 符号上下文（CTX）

| ID | 场景 | 对应测试 |
|----|------|----------|
| CTX-01 | 360° 上下文收集入度与出度调用 | `e2e_feature_suite.rs :: context_collect_incoming_and_outgoing` |
| CTX-02 | CLI `context` 返回符号视图 | `cli_index_and_daemon.rs :: context_returns_symbol_view` |
| CTX-03 | 不存在的符号退出码 2 | `cli_index_and_daemon.rs :: context_nonexistent_symbol_exits_2` |

### 6. 分析工具包（ANA）

| ID | 场景 | 对应测试 |
|----|------|----------|
| ANA-01 | 死代码检测发现不可达函数 | `e2e_feature_suite.rs :: dead_code_detection_finds_unreachable_functions` |
| ANA-02 | 导出函数被判为 live | `e2e_feature_suite.rs :: dead_code_detection_exported_functions_are_live` |
| ANA-03 | CLI `dead_code` 退出码 0 | `cli_index_and_daemon.rs :: dead_code_exits_0` |
| ANA-04 | 架构概览返回语言统计 | `e2e_feature_suite.rs :: architecture_overview_returns_language_stats` |
| ANA-05 | CLI `architecture` 退出码 0 | `cli_index_and_daemon.rs :: architecture_exits_0` |
| ANA-06 | 复杂度分析返回指标 | `e2e_feature_suite.rs :: complexity_analysis_returns_metrics` |
| ANA-07 | CLI `complexity` 退出码 0 | `cli_index_and_daemon.rs :: complexity_exits_0` |
| ANA-08 | 社区检测返回结果 | `e2e_feature_suite.rs :: community_detection_returns_result` |
| ANA-09 | CLI `community` 退出码 0 | `cli_index_and_daemon.rs :: community_exits_0` |
| ANA-10 | 简单项目跨服务检测返回空 | `e2e_feature_suite.rs :: cross_service_detection_returns_empty_for_simple_project` |
| ANA-11 | CLI `cross_service` 退出码 0 | `cli_index_and_daemon.rs :: cross_service_exits_0` |
| ANA-12 | API 审查命令退出码 0（route_map / api_impact / shape_check / tool_map） | `cli_index_and_daemon.rs :: route_map_exits_0` / `api_impact_exits_0` / `shape_check_exits_0` / `tool_map_exits_0` |
| ANA-13 | git diff 影响检测在非 git 仓库优雅退出 | `cli_index_and_daemon.rs :: detect_changes_on_non_git_repo_exits_gracefully` |
| ANA-14 | rename dry-run 退出码 0 | `cli_index_and_daemon.rs :: rename_dry_run_exits_0` |

### 7. 架构图与语义 Delta（DIA）

| ID | 场景 | 对应测试 |
|----|------|----------|
| DIA-01 | diagram 从图事实渲染变体与类型 | `diagram_integration.rs :: diagram_int_renders_variants_and_types_from_graph_facts` |
| DIA-02 | showcase 档位阻止无效证据交付 | `diagram_integration.rs :: diagram_int_showcase_blocks_on_invalid_evidence` |

### 8. MCP 集成（MCP）

| ID | 场景 | 对应测试 |
|----|------|----------|
| MCP-01 | MCP 服务器初始化并列举全部工具 | `mcp_integration.rs :: mcp_server_initializes_and_lists_tools` |
| MCP-02 | CLI `hook` 读取 stdin 并响应 | `cli_index_and_daemon.rs :: hook_reads_stdin_and_responds` |
| MCP-03 | `setup --force false` 优雅退出 | `cli_index_and_daemon.rs :: setup_with_force_false_exits_gracefully` |

### 9. 团队制品（ART）

| ID | 场景 | 对应测试 |
|----|------|----------|
| ART-01 | `export` 生成 zstd 制品 | `cli_index_and_daemon.rs :: export_creates_artifact` |
| ART-02 | export → import 往返一致 | `cli_index_and_daemon.rs :: export_then_import_roundtrip` |

### 10. CLI 契约（CLI）

| ID | 场景 | 对应测试 |
|----|------|----------|
| CLI-01 | `--help` 列出全部命令 | `cli_integration.rs :: help_lists_all_commands` |
| CLI-02 | `--version` 打印版本 | `cli_integration.rs :: version_flag_prints_version`；库层 `e2e_feature_suite.rs :: version_api_returns_non_empty` |
| CLI-03 | 无子命令优雅退出 | `cli_integration.rs :: no_subcommand_exits_gracefully` |
| CLI-04 | 空库上 `list` / `status` 可用 | `cli_integration.rs :: list_command_works_with_empty_db` / `status_command_works_with_empty_db`；`cli_index_and_daemon.rs :: list_shows_indexed_project` / `status_shows_project` |
| CLI-05 | 未知子命令报错退出 | `cli_integration.rs :: unknown_subcommand_exits_with_error` |
| CLI-06 | daemon 检测新文件 / 修改文件 / 忽略非代码文件 | `cli_index_and_daemon.rs :: daemon_hot_update_detects_new_file` / `daemon_hot_update_detects_modified_file` / `daemon_hot_update_ignores_non_code_file` |
| CLI-07 | daemon 对不存在路径退出码 2 | `cli_index_and_daemon.rs :: daemon_nonexistent_path_exits_2` |
| CLI-08 | LSP 命令在无服务环境优雅退出 | `cli_index_and_daemon.rs :: lsp_goto_def_without_server_exits_gracefully` / `lsp_hover_without_server_exits_gracefully` |
| CLI-09 | `clean` 删除项目 | `cli_index_and_daemon.rs :: clean_removes_project` |

### 11. Kit 能力注册表（KIT）

| ID | 场景 | 对应测试 |
|----|------|----------|
| KIT-01 | 全部核心能力可经 Kit 解析 | `kit_bootstrap.rs :: all_core_capabilities_resolvable_through_kit`；`e2e_feature_suite.rs :: kit_all_core_modules_resolvable` |
| KIT-02 | `daemon` feature 开启时 daemon 能力可解析 | `kit_bootstrap.rs :: daemon_capability_resolvable_when_feature_on` |
| KIT-03 | `embed` feature 开启时 embed 能力可解析 | `kit_bootstrap.rs :: embed_capability_resolvable_when_feature_on` |
| KIT-04 | storage 能力功能正常 | `kit_bootstrap.rs :: storage_capability_is_functional` |

### 12. 国际化与非 ASCII 路径（I18N）

| ID | 场景 | 对应测试 |
|----|------|----------|
| I18N-01 | 中文目录名 | `non_ascii_path_test.rs :: test_chinese_directory_name` |
| I18N-02 | 中文文件名 | `non_ascii_path_test.rs :: test_chinese_file_name` |
| I18N-03 | 日文目录名 | `non_ascii_path_test.rs :: test_japanese_directory_name` |
| I18N-04 | 韩文目录名 | `non_ascii_path_test.rs :: test_korean_directory_name` |
| I18N-05 | ASCII 与非 ASCII 混合路径 | `non_ascii_path_test.rs :: test_mixed_ascii_and_non_ascii_paths` |
| I18N-06 | Unicode 项目名 | `non_ascii_path_test.rs :: test_unicode_project_name` |
| I18N-07 | 非 ASCII 数据库路径 | `non_ascii_path_test.rs :: test_non_ascii_db_path` |
| I18N-08 | walker 发现非 ASCII 路径 | `non_ascii_path_test.rs :: test_walker_discovers_non_ascii_paths` |
| I18N-09 | 非 ASCII 路径增量索引 | `non_ascii_path_test.rs :: test_incremental_index_non_ascii_path` |

### 13. 错误处理与数据隔离（ERR）

| ID | 场景 | 对应测试 |
|----|------|----------|
| ERR-01 | 损坏数据库返回退出码 4（DatabaseCorrupt） | `end_to_end.rs :: corrupt_db_returns_exit_code_4`；`e2e_feature_suite.rs :: corrupt_db_error_chain` |
| ERR-02 | 多项目数据隔离 | `e2e_feature_suite.rs :: multi_project_data_isolation`；`end_to_end.rs :: multi_project_isolation` |
| ERR-03 | EdgeType 模型往返序列化 | `e2e_feature_suite.rs :: model_edge_type_roundtrip` |

### 14. 多语言解析（MLT）

| ID | 场景 | 对应测试 |
|----|------|----------|
| MLT-01 | Go 文件索引 | `e2e_feature_suite.rs :: index_go_file` |
| MLT-02 | Java 文件索引 | `e2e_feature_suite.rs :: index_java_file` |
| MLT-03 | JavaScript 文件索引 | `e2e_feature_suite.rs :: index_javascript_file` |
| MLT-04 | Bash 文件索引 | `e2e_feature_suite.rs :: index_bash_file` |
| MLT-05 | HTML / CSS / JSON 文件索引 | `e2e_feature_suite.rs :: index_html_css_json_files` |
| MLT-06 | Python 文件索引 | `end_to_end.rs :: index_python_file` |
| MLT-07 | TypeScript 文件索引 | `end_to_end.rs :: index_typescript_file` |
| MLT-08 | Fortran 文件索引 | `end_to_end.rs :: index_fortran_file` |
| MLT-09 | 多语言 FFI 边进入图 | `e2e_feature_suite.rs :: multilang_ffi_edges_in_graph`；`end_to_end.rs :: ffi_edge_exists_after_multilang_index` |
| MLT-10 | Reads / Writes 数据流边存在 | `e2e_feature_suite.rs :: reads_writes_edges_after_multilang_index`；`end_to_end.rs :: reads_writes_edges_exist_after_multilang_index` |
| MLT-11 | 全管线串联：索引 → 查询 → 追踪 | `e2e_feature_suite.rs :: full_pipeline_index_then_query_then_trace` |

> 语言提取器的深层单元测试（各语言 extractor 的节点/边提取、FQN、作用域解析等）位于 `src/parse/`、`src/resolve/` 各模块的内联 `#[cfg(test)]` 中，随 `cargo test --lib` 运行。

---

## 🌐 验收测试（8 语言真实项目）

`tests/acceptance/run_acceptance.sh` 克隆 8 个开源项目（每种语言一个），用 CodeNexus 索引后与 gitnexus 交叉验证并产出 diff 报告：

| 项目 | 仓库 | 标签 | 语言 |
|------|------|------|------|
| serde | serde-rs/serde | v1.0.219 | Rust |
| requests | psf/requests | v2.32.3 | Python |
| vscode-uri | microsoft/vscode-uri | v3.0.2 | TypeScript |
| redis | redis/redis | 7.4.2 | C |
| gin | gin-gonic/gin | v1.10.0 | Go |
| jackson-databind | FasterXML/jackson-databind | jackson-databind-2.18.2 | Java |
| fmt | fmtlib/fmt | 11.0.2 | C++ |
| OpenBLAS | OpenMathLib/OpenBLAS | v0.3.28 | Fortran |

```bash
# 预演（仅回显命令，校验语法）
bash tests/acceptance/run_acceptance.sh --dry-run

# 完整验收（需要 git / jq / cargo）
bash tests/acceptance/run_acceptance.sh

# 清理 fixtures 与验收 DB
bash tests/acceptance/run_acceptance.sh --clean
```

---

## ⏱️ 基准回归场景

`benches/` 的 7 组 Criterion 基准承担性能回归守护（SLO 阈值表见 [`benches/README.md`](../benches/README.md)）：

| 基准 | 回归场景 |
|------|----------|
| `index_bench` | 10 文件冷启动吞吐 |
| `incremental_bench` | 1000 文件冷启动 / 单文件增量 / 500 文件批量增量 |
| `query_bench` | Cypher 匹配 / 计数 / 按名搜索 / 带项目过滤搜索 |
| `trace_bench` | 深度 5/10/50 调用链追踪、`trace_all` 深度 10、4971 节点高扇入影响分析上限守护 |
| `memory_bench` | 10k 文件首索峰值 RSS、daemon 10 轮增量持续 RSS、ram-first 与默认模式对比（长时场景需 `BENCH_RUN_IGNORED=1`） |
| `daemon_bench`（`daemon` feature） | 去抖响应延迟、事件队列吞吐（零丢失） |
| `graph_bench` | 图数据结构操作 |

---

## ▶️ 执行命令（与 CI 一致）

```bash
# 单元测试（CI 矩阵按 minimal / core / full / core,daemon,analysis,complexity / core,lsp,cache / full,embed 六档运行）
cargo test --lib --verbose
cargo test --lib --no-default-features --features "core" --verbose

# 集成测试（本地运行；需先编译二进制的 CLI 子进程测试由 cargo test 自动处理）
cargo test --test e2e_feature_suite
cargo test --test end_to_end
cargo test --test cli_index_and_daemon
cargo test --test non_ascii_path_test

# 格式与 lint 门禁
cargo +nightly fmt --all -- --check
cargo +1.95 clippy -- -D warnings
cargo +1.95 clippy --lib --no-default-features --features minimal -- -D warnings

# 覆盖率门禁（≥95% 行覆盖）
cargo llvm-cov --lib --fail-under-lines 95 --lcov --output-path lcov.info

# 验收测试（8 语言真实项目）
bash tests/acceptance/run_acceptance.sh --dry-run
```

> CI 的 test job 仅运行 `cargo test --lib`（六个 feature 档位）；`tests/` 集成测试与验收 harness 为本地/按需执行。

---

## 📊 统计汇总

| 类别 | 数量 |
|------|------|
| 单元测试函数（`src/` 内联） | 4588 `#[test]` + 29 `#[tokio::test]` ≈ 4617（147 个含 `#[cfg(test)]` 的源文件） |
| 集成测试函数（`tests/` 8 文件） | 118 |
| ├ `e2e_feature_suite.rs` | 40 |
| ├ `cli_index_and_daemon.rs` | 35 |
| ├ `end_to_end.rs` | 20 |
| ├ `non_ascii_path_test.rs` | 9 |
| ├ `cli_integration.rs` | 7 |
| ├ `kit_bootstrap.rs` | 4 |
| ├ `diagram_integration.rs` | 2 |
| └ `mcp_integration.rs` | 1 |
| 验收项目 | 8（每种语言一个真实开源仓库） |
| Criterion 基准组 | 7 |
| 覆盖率门禁 | 行覆盖 ≥ 95%（CI coverage job + pre-push 钩子） |
| 本矩阵穷举的集成层场景 | 100 条（IDX 13 + QRY 6 + SRCH 8 + TRC/IMP 4 + CTX 3 + ANA 14 + DIA 2 + MCP 3 + ART 2 + CLI 9 + KIT 4 + I18N 9 + ERR 3 + MLT 11 = 91 条编号场景，另有若干合并行） |

> 待补充：`src/` 内约 4600 条单元测试的逐条场景化（当前以模块锚点概括）；`fuzz/` 目录目前仅含 corpus、无已注册 fuzz target，暂无模糊测试场景。
