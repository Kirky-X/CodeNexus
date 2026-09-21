# 📋 CodeNexus 更新日志

All notable changes to CodeNexus are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> 发布前请对照 [RELEASING.md](RELEASING.md) 的对账清单核对文档数字（命令数 / MCP 工具数 / 语言数 / 示例数 / advisory 忽略清单）。

## 📋 目录

- [Unreleased](#unreleased)
- [0.3.13-rc.1 — 2026-09-21](#0313-rc1---2026-09-21)
- [0.3.12 — 2026-07-30](#0312---2026-07-30)
- [0.3.11 — 2026-07-26](#0311---2026-07-26)
- [0.3.10 — 2026-07-25](#0310---2026-07-25)
- [0.3.9 — 2026-07-21](#039---2026-07-21)
- [0.3.8 — 2026-07-21](#038---2026-07-21)
- [0.3.7 — 2026-07-20](#037---2026-07-20)
- [0.3.6 — 2026-07-20](#036---2026-07-20)
- [0.3.5 — 2026-07-17](#035---2026-07-17)
- [0.3.4 — 2026-07-15](#034---2026-07-15)
- [0.3.3 — 2026-07-15](#033---2026-07-15)
- [0.3.2 — 2026-07-11](#032---2026-07-11)
- [0.3.1 — 2026-07-10](#031---2026-07-10)
- [0.3.0 — 2026-07-10](#030---2026-07-10)
- [0.1.0 — 2026-06-29](#010---2026-06-29)

---

## [Unreleased]

## [0.3.13-rc.1] - 2026-09-21

冻结改动收编 + 跨仓解耦：并行会话冻结的 14 文件成品改动（注释任务标记清理 + graph-viewer server 日志迁移 inklog）收编入库；同批移除 `[patch.crates-io]` 与全部跨仓 path 依赖，基库一律改走 crates.io 发布版（trait-kit `0.5.0-rc.6`、sdforge/oxcache `0.5.0-rc.5`、inklog `0.3.0-rc.5`），仓库从此自包含克隆可构建。CI test 矩阵 6 条腿此前因 cfg 门错挂无法编译的三处一并前向修复。

### Fixed（冻结改动收编：cfg 门修复）
### Fixed（架构审查修复批次二：遗留项收尾）

- **fix(security): fuzz harness 恢复 + 两个新目标** — 从丢失的提交 fef636b 找回 `fuzz/Cargo.toml` 与 `escape_cypher_string`/`escape_identifier` 目标（含转义不变量断言）；新增 `cypher_subset_parse`（MCP 只读语法门 pest 解析器永不负 panic + 拒绝信息非空）与 `cnxp_header`（`.cnxp` 工件头解析永不 panic + Ok 蕴含格式版本正确）两个目标；CI 新增 `fuzz-smoke` job（nightly + rust-src + cargo-fuzz，构建全部目标并各跑 2000 次冒烟）。注：`fuzz/Cargo.lock` 未恢复（需联网由 cargo-fuzz 重新生成）。
- **fix(index): LoadPhase 删除阶段事务化 + 删除失败中止** — 增量/全量索引的批量删除包进显式事务（`BEGIN TRANSACTION`…`COMMIT`，经新增的 `StorageConnection::in_write_transaction`/`WriteTx` 专用连接——事务状态绑定单连接，旧的 statement-per-connection `execute` 无法组成事务），失败即 ROLLBACK 并中止索引；旧的 warn-and-continue 会把未删除的旧节点与新节点并存成重复符号。存储原语（`save_nodes_stream`/`save_edges_stream`/`delete_file_nodes_batch`/`save_project`/`load_from_csv`）泛化出 `_on` 变体以在事务连接上运行；新增 5 个事务回归测试。**引擎限制（如实记录）**：lbug 0.20 拒绝在同一事务内删除后重插相同主键（Project id 与变更文件的 FQN 节点 id 正是如此），故"删+插"无法整体原子化——事务只覆盖删除阶段；插入阶段保持逐语句自动提交 + `with_retry`。残余崩溃窗口是自愈的：缺失哈希的文件在下次 diff 中被归类为 `added` 重新索引。另修复 `show_tables()` 列序误读（name 在第二列）与保留字导致 `Database` 表未建（`Database` 与引擎保留字冲突被 init_schema 跳过）两个连带的静默跳过问题；`delete_file_nodes_batch_on` 在事务内 fail-loud（吞错只会把失败推迟到 COMMIT 并掩盖真因）。
- **fix(cache): 命名空间隔离失效** — `CacheStore` 新增 `invalidate_namespace(ns)`（`OxcacheStore` 经每命名空间代数计数器实现，无需后端键枚举），索引/写查询只失效 `cypher:` 命名空间；内容寻址的 `ast:`（AST sexp）与 `embed:`（向量）条目跨索引运行存活——此前 daemon 每次增量索引后的全量 `invalidate_all` 会把语义搜索的向量缓存一并清空。`cypher` 失效联动清空 `#[cached]` 查询宏注册表。
- **fix(storage): schema 版本号 + `clean --rebuild` 自愈** — 新增 `SchemaMeta` 单行表（`SCHEMA_VERSION="1"`，`init_schema` 时盖章；无版本行的旧库视为 legacy 直接收编不破坏），`Repository::open`/`open_read_only` 校验版本，不匹配即报 `Schema` 错误并携带恢复指引（替代下游莫名其妙的 binder 错误）；`clean --rebuild true` 在 Kit 构建前删除 DB 文件（拒绝非 `.lbug`、连带 `.wal`），供版本不匹配/损坏场景一键重置。新增 3 个版本检查测试。
- **fix(embed): 向量缓存接线** — `CachedEmbedClient`（BLAKE3 内容寻址，`embed:<hash>` 键）此前只有测试引用，现接入生产路径：`EmbedModule::build` 从 CacheModule 取 `Arc<dyn CacheStore>` 注入 `EmbedCapability`，embed 调用经缓存包装（命中免 ONNX 推理/HTTP 往返）；`local_client` 改持 `Arc` 以在缓存包装与回退路径间共享懒加载实例。配合命名空间隔离，向量不再被索引运行清掉。
- **refactor(lsp): 六适配器收敛为单实现** — gopls/pyright/clangd/typescript-language-server/jdtls/fortls 六个 86–90% 相同的适配器（各 ~355 行）收敛为 `lsp::server_spec::ServerClient<S>` 单实现 + `LspServerSpec` 关联常量（默认二进制名），各适配器文件退化为 spec ZST + 历史类型别名（`GoplsClient` 等），公共名与每语言测试模块保持不变；生产代码去重 ~800 行。
- **fix(tests): LSP 六适配器测试去重** — 12 个逐字节相同的机制测试（mock 会话、超时、shutdown 语义等）移入 `server_spec.rs` 对代表性 `GoplsSpec` 运行；各适配器仅保留默认路径 wiring 与 `#[ignore]` 真服务器集成测试。适配器文件 355 → ~75 行。
- **perf(embed): ONNX 批量推理** — `LocalEmbedClient::embed` 从逐文本 `[1, seq_len]` 推理改为按 chunk（≤32）pad 到 `[batch, max_len]` 单次 `session.run`（ONNX 批并行），均值池化按各自 attention mask 逐文本执行；运行时拒绝批量形状（模型导出固定 batch 轴）时自动回退逐文本路径，正确性不依赖导出轴。
- **fix(docs): ADR-022/PRD MCP 协议代次修正** — rmcp 3.4 实际按客户端协商 `2024-11-05`/`2025-03-26`/`2025-06-18`（原文档误写死 2024-11-05）。
- **fix(fuzz): 可离线构建与冒烟运行** — fuzz crate 声明独立 `[workspace]`（避免被根 workspace 吸收），`Cargo.lock` 由本地 registry 离线生成（317 包）；四个目标在 dev profile 实际编译并各运行 300–500 次冒烟（0 crash）。

- **refactor(lib): 内部模块 `#[doc(hidden)]`** — `cache/daemon/diagnostics/discover/ir/parse/resolve/storage/trace/analysis/diagram/lsp` 标注 `#[doc(hidden)]`（docs 不再呈现内部件；`kit`/`model`/`service`/`index`/`query` 为受支持公共面）。完全 `pub(crate)` 收口属 semver breaking，留作 0.4 专用 PR。

### Fixed（架构审查修复批次：P0/P1/P2 落地）

- **fix(index): 增量索引跨文件边丢失（正确性）** — 增量运行时 `resolve_all` 只对重解析文件构建符号表，而 LoadPhase 已删除变更文件旧节点的全部出边，导致每次增量索引净丢失"变更文件 → 未变更文件"的 CALLS/DataFlows/Type/IMPORTS 边，图质量随增量次数衰减直到 `--force` 重建。新增 `Repository::get_symbol_rows`/`get_file_id_rows`（按提取器实际发出的 20 个 label 逐表加载，`node_table_columns` 驱动动态投影）与 `resolve::ResolveEnrichment`：ResolvePhase 从 DB 补录未变更文件的符号表条目、File id 与函数 id（排除 changed/added/deleted 的过期行），ImportResolver 经 `with_enrichment` 合并外部文件/函数索引。富化数据不进入 graph（LoadPhase 写入集保持为重解析文件）。新增回归测试 `incremental_reindex_preserves_cross_file_calls_edges`。
- **fix(security): graph-viewer server 加固** — 移除 `lbug_path` 任意路径参数（任意 DB 读取原语，叠加 DNS rebinding 可被动窃取索引中的源码）；DB 只从启动扫描白名单解析；新增启动时随机 token（`x-graph-token` 头校验，`/dev/urandom` 熵源）与 Host 回环校验；删除硬编码个人绝对路径。前端走浏览器内 WASM 查询，不消费被移除的参数。
- **fix(security): import 完整性与解压上限** — `zstd_decompress` 改用 `oxiarc_zstd::decompress_with_limit`（上限 4 GiB），解压炸弹映射为 `InvalidInput`（exit 2）；`ArtifactManifest` 新增 `payload_blake3` 摘要（`#[serde(default)]`，旧工件可导入），export 计算写入、import 校验，外加 `original_size` 一致性检查；新增篡改检测测试。
- **fix(service): rename 写路径加固** — `apply_text_edits` 原按 CWD 解析编辑路径（`root != CWD` 时写错位置，且 DB 投毒的 `../` 相对路径可越界写盘）；新增 `resolve_under_root`（拒绝 `..` 组件 + root 与候选路径均 canonicalize 后前缀校验），`collect_candidate_files` 与 `apply_text_edits` 双重设防。
- **fix(mcp,cli): MCP server 只读打开 DB** — MCP 工具面（query/trace/impact/search/context/architecture/diagram/arch_diff）本为纯读，但 Kit 以读写模式独占 DB 写锁：daemon/index 运行时 MCP 启动直接 `DatabaseLocked`，反之 MCP 存活期间任何写命令失败。改为 `with_read_only(true)`（缺失 DB 明确报错 exit 4），解锁 IDE + CLI + MCP 并存；MCP 模式复用 `.codenexus/` 单库发现，`--db` 同时接受空格与 `=` 形式，未知参数拒绝（exit 2）而非静默回退默认库。
- **fix(cli): INFO 日志移出 stdout** — inklog console sink 默认 `stderr_levels=["error","warn"]`，索引管线的 info! 事件混入重定向的 JSON 输出，破坏"每命令单 JSON"契约；`init_inklog` 显式 `console_stderr_levels` 覆盖全部级别。
- **fix(cli): SENTINEL_DEFAULTS 补 trace/search/impact/context** — 四个高频命令的全部可选参数纳入哨兵默认值表（depth/limit 按引擎拒绝 0 值与 DEFAULT_LIMIT 语义取 5/50/3/1），用户不再需要逐个传 flag。
- **fix(archive): 发布流程 `cargo publish` 移除 `--no-verify`** — 多 feature crate 发布前验证不可跳过（坏包无法撤回）。

### Changed（架构审查修复批次：性能与维护性）

- **perf(resolve): O(F²) 路径匹配消除** — 新增 `FilePathIndex`（键归一化一次 + 查询路径 `/` 边界后缀枚举，O(路径深度) 次 HashMap 探测，语义与旧全表扫描等价），`resolve_imports` 主循环与 `build_function_index` 改用之；`ScanPhase` 的 `path_to_file_id` 嵌套扫描改为先建 rel→id 映射。10k 文件仓库消除 ~10⁸ 量级比较/分配。
- **perf(index): 每文件 4 次磁盘读 → 3 次** — `FileDiff` 新增 `hashes`（diff 阶段已算的 BLAKE3 随分类产出传递），`build_file_nodes` 复用而非重读重哈希；`diff_files_with_hints` 新增 daemon 快路径（(size,mtime) 命中即复用上次哈希，每文件一次 stat 替代全读 + BLAKE3，IndexFacade 跨增量批次持有 hint map），daemon 常驻场景的增量 diff 从 O(N) 次全文件读降为 O(N) 次 stat。
- **perf(parse): ParserPool 接线（兑现 ADR-010）** — 新增 `with_pooled_parser`（thread-local 池，Parser 借用不出闭包），21 个语言提取器的 `extract` 从逐文件 `Parser::new()+set_language` 改为池化复用。
- **refactor(parse): node_text 去重** — 20 份逐字节相同的私有 `node_text` 拷贝提升到 `parse::helpers`（同 `dedupe_qn` 模式）；顺带清理全工作区存量 clippy lint（unused imports/variables、assert_eq! 布尔字面量、clamp、collapse-if 等），`--all-targets -D warnings` 现为零告警。
- **chore(ci): 门禁补强** — CI 增加 `cargo test --tests --features full`（513 个集成测试此前完全在门禁外）；工具链 1.95 → 1.97.1 对齐声明的 MSRV（三处工作流）；audit-check 的 ignore 改为从 deny.toml 动态生成（单一事实源，附复核日期；移除已不在 lock 中的 rkyv RUSTSEC-2026-0235）；coverage/test 工作流同步。
- **docs: 发布对账清单** — 新增 `docs/RELEASING.md`（命令数 29/MCP 工具数 8/语言数 21/示例数 14/advisory 清单五项对账 + 机械校验命令），CHANGELOG 头部链接；修正 skill（28 命令→29、8 语言→21，补 arch_diff/diagram 条目与附录语言表 13 种）、README/README_EN（示例 6→14 全清单）、USER_GUIDE（30 子命令→29+mcp 口径）。


### Added

- **feat(cli): DX 修复批次一——配置文件层、MCP 扩容、索引进度与首跑引导** — ① 新增 `.codenexus/config.json` per-project 配置文件（`src/main.rs` `ProjectConfig`）：`general.db`/`general.debounce_ms`/`general.verbose` + `command_defaults.<命令>.<参数>`（值字符串化后仅在用户未显式传 flag 时生效，经 `extract_args` 叠加，优先级 CLI flag > 配置文件 > 内置默认；`general.db` 仅在 name/path 推导与单 `.lbug` 自动发现都无结果时兜底，形状错误只 `[warn]` 不中断），README/USER_GUIDE/FAQ/API_REFERENCE 同步文档；② MCP 工具面 8→10：`dead_code`、`detect_changes` 以 `#[forge]` MCP wrapper 加入（`detect_changes` 抽取 `run_detect_changes` 共享核心，CLI wrapper 改走该核心，行为不变）；③ 索引管线逐阶段进度：`DagPipeline::run` 每阶段完成向 stderr 输出 `[codenexus] [i/n] <phase> ok (<elapsed>)`（stdout 仍是纯 JSON 通道）；④ `codenexus` 无参数运行改为输出三行快速开始（index → query → setup）+ `--help` 指引，仍 exit 0；⑤ 新增哨兵默认值：`index --force/--lsp/--embed/--ram_first` 默认 `false`、`detect_changes --mode` 默认 `unstaged`（叠加并行批次已有的 trace/search/impact/context 默认值），`build_cli_command` 抽出供测试断言，新增 13 个 bin 单测（哨兵默认值、配置叠加优先级、ProjectConfig 解析/告警/加载）。验证：`cargo check --all-targets --all-features` 0 error、`cargo test --all-features --bin codenexus` 38 passed / 0 failed、`cargo test --all-features --lib service::` 761 passed / 0 failed、`pipeline_dag` 31 passed。

### Changed

- **chore(deps): 自研四库启用 `[patch.crates-io]` 本地资源库直连（替换 crates.io 远端发布版）** — `Cargo.toml` 启用原注释模板并改为相对路径：`trait-kit`/`sdforge`/`oxcache`/`inklog` 分别 patch 到 `../base/*` 本地工作树（宏子 crate `trait-kit-macros`/`sdforge-macros`/`oxcache_macros` 经父 crate 自带 `path = "./…"` 依赖自动本地化，无需单独 patch；`confers` 不在依赖树内，跳过）。背景：base 各库 HEAD 领先 crates.io 发布版（sdforge `v0.5.0-rc.4-8-g4d8e3ff`、trait-kit `v0.5.0-rc.5-7-gd849d42`，截至 2026-09-16，且含未提交修改），远端发布版属"旧版本"；patch 后构建直接消费本地最新代码。配套：① `ci.yml` 四个 job（lint/test/coverage/security）新增 base 仓库浅克隆步骤到 `../base`（`cargo-deny`/`cargo metadata` 同样需解析 patched manifest；克隆的是 origin/main，领先提交推送后 CI 才能验证到）；② 发布约定保留：`cargo publish` 前必须整段注释 `[patch.crates-io]`（见 `Cargo.toml` 内注释 ⚠️ 标记）。验证：`cargo tree -i sdforge` 显示来源 `(/home/kirky/projects/base/sdforge)`、`cargo check --all-features --lib --bins` 0 error、`cargo test --all-features --bin codenexus` 38 passed / `--lib` 4578 passed / 0 failed。

- **refactor(cli,mcp): DX 修复批次二——stdout JSON 纯净性、日志位置、CLI 文案与 MCP 工具描述** — ① inklog console sink 显式 `console_stderr_levels(["error","warn","info","debug","trace","fatal"])`：inklog 默认 `stderr_levels=["error","warn"]` 会把 info 级日志（`index_started`/`index_completed`/`performance` 等）写进 stdout，污染 `codenexus index > result.json` 重定向输出；修复后所有日志走 stderr，stdout 永远只有命令 JSON；② 日志文件 `logs/codenexus.log` → `.codenexus/logs/codenexus.log`（副作用目录收敛到用户已 gitignore 的单一 `.codenexus/`，inklog file sink 自建父目录），TRD/FAQ/USER_GUIDE/skill 同步；③ CLI 用户面文案统一英文：DB 锁冲突三行引导（原中文）与 `--verbose` help（原中文）改为英文，与其余 CLI 输出一致；④ 10 个 MCP 工具（原 8 个）的 `description` 全部扩写为含参数语义与默认值的说明（`impact` 明示 `max_depth=0` legacy 与 `>0` enhanced 双形态输出；`query` 明示写子句拒绝与 UNION/多标签 OR 不支持；`context` 明示 enhanced 返回不同 JSON 形状）；⑤ `context_mcp` 不再静默忽略 `project`/`enhanced` 参数（原 `#[allow(unused_variables)]`），与 CLI 同分支：enhanced 走 `run_context_enhanced`、非空 project 先校验存在性，返回 `serde_json::Value` 兼容两种输出形状。

### Fixed

- **fix(mcp): `codenexus mcp --db=<path>`（等号形式）被静默忽略** — 原手写参数解析只识别空格分隔形式，`--db=/x/y.lbug` 无提示回退默认库导致查询恒空。现 `parse_mcp_db_arg` 同时支持 `--db <path>` 与 `--db=<path>`，未知参数/缺值以 exit 2 拒绝并给出 usage，新增 `-h/--help`；MCP 服务器同时改为**只读**打开数据库（工具面本就是查询-only），可与 `index`/`daemon` 写入进程并存；启动时 DB 文件不存在则按只读命令同款文案 exit 4，省略 `--db` 时复用 `.codenexus/` 单 `.lbug` 自动发现。

- **feat(analysis,cache,daemon): 吸收 base 生态四件套（RICE Top-3 落地）** — ① oxcache `#[cached]` 查询缓存：oxcache 开启 `macros` feature，`CacheModule::build_cap` 注册 `codenexus-query` 宏服务缓存（`run_query_cached`，键 = `build_kit` 写入的 DB 身份 + Cypher 文本，TTL 300s），`OxcacheStore::invalidate_all` 联动清空宏注册表（index/clean 后缓存自动失效），CLI/MCP `query` 走缓存路径；② trait-kit `lifecycle`/`health`/`shutdown` feature 启用：`CacheModule`/`StorageModule` 实现 `AsyncHealthCheck`（moka 探针往返 / lbug `RETURN 1`，与 dbnexus `LadybugConnection::health_check` 同探针），`CacheModule` 实现 `AsyncLifecycle`（bootstrap `register_lifecycle` 注册，`on_ready` build 期 / `on_shutdown` 经 `AsyncKit::shutdown_async` 逆拓扑执行，CLI handler 成功后调用），daemon 停机接入 `ShutdownCoordinator` 三阶段（StopRequests→DrainQueue→CloseConnections）；`status` 输出新增 `kit_health` 字段（仅非健康模块，`skip_serializing_if` 空省略）；③ CLI 新增布尔 `--verbose` 旗标（`ArgAction::SetTrue`），经 `KitBootstrapConfig.verbose` → `DaemonConfig.verbose_events` → daemon 每批事件 `debug!` 诊断（trait-kit `toggle` 仅同步 `Kit` 提供、`AsyncKit` 无对应 API，故走配置流而非开关后端）。inklog 文件 PII 脱敏（`pii_masking_enabled` 默认开启）与轮转/压缩/保留沿用纯配置 builder；文件级 `SamplingSink` 采样暂缓——上游缺口：builder `add_sink` 触发的 `build_with_deps` 路径不安装全局 tracing/log 前端（仅 `with_config_and_sinks` 安装），接入即全局静默，待上游修复后回归。验证：`cargo check --all-targets --all-features` 0 error、`cargo test --lib --all-features` 4570 passed / 0 failed、`query`/`daemon` 二进制冒烟（SIGTERM 分阶段停机日志齐全）。

- **chore(deps): 全量依赖升级至最新版并统一 minor 级写法** — 跨版本：`lbug` 0.18→0.20.4（自研，graph-viewer server 联动）、`tree-sitter` 0.26→0.27（`Node::kind` 改借用节点生命周期：`analysis/complexity.rs` Halstead 收集去 `&'static` 化；`LanguageError` 新增 `NotParseable` 变体：`parse/error.rs` 测试补分支）、`criterion` 0.5→0.8（7 个 bench `criterion::black_box`→`std::hint::black_box`）、`oxiarc-zstd` 0.3→0.4、`pest`/`pest_derive` 2.7→2.9、`serial_test` 3→4.0、`tree-sitter-ocaml` 0.25→0.26、`rust_decimal` 1.37→1.43、`uuid` 1.23→1.26、自研四库 rc.2→最新（trait-kit `0.5.0-rc.5`、sdforge/oxcache `0.5.0-rc.4`、inklog `0.3.0-rc.4`）；写法统一 minor 级（`"1"`→`"1.53"`、`"2"`→`"2.3"` 等，禁 major/patch 级）。例外说明：`ort` 维持 rc（2.0.0-rc.13，crates.io max_stable_version=null、1.16.3 已 yanked，rc 即最新）；自研 rc 写法必须三段式（cargo 不接受 `0.5-rc.4`）。验证：复跑 `cargo outdated` 直接依赖 0 行待更新。

- **chore(deps): 自研基础库升级至 RC 版本** — trait-kit `0.3.0` → `0.5.0-rc.2`、sdforge `0.4.7` → `0.5.0-rc.2`、oxcache `0.3.9` → `0.5.0-rc.2`、inklog `0.1.12` → `0.3.0-rc.2`（四库依赖链锁定，联动升级，规则 25 升级前基线 4740 测试全绿）。MSRV `1.95` → `1.97.1`（`clippy.toml` `msrv` 同步）。唯一 API 适配：`KitError::BuildFailed.context` / `MissingCapability.key` 字段 `&'static str` → `String`（`src/service/error.rs` 测试代码 6 处 `.to_string()`），运行时行为不变；`load_config_or_default` / inklog builder / oxcache sync API / `#[forge]` 宏均向后兼容，零改动。新特性开启：`mcp`/`cli` feature 追加 `sdforge/inklog`（sdforge 内部日志接入 inklog 0.3.0-rc.2，同一版本已在依赖树，无树外新 crate）；评估后暂不开启：trait-kit `lifecycle/health/shutdown`（需 9+ Kit 模块实现对应 trait，列为后续独立变更）、oxcache 分布式后端（redis/dragonfly/aerospike）与 compression/macros/batch/lock/bloom（无对应场景或收益边际）、inklog `compression`（zstd-sys 与 lbug bundled zstd 符号冲突风险，现有 LZ4 `file_compress` 已满足）。传递依赖变化：+confers `0.6.0-rc.2`（trait-kit 必选）、+ICU4X i18n 栈（sys-locale/unic-langid/zerovec）、+stacker/psm，-opentelemetry 全家桶 / -secrecy / -tracing-opentelemetry；rmcp `2.2`→`3.2`、tokio `1.52`→`1.53`。验证：`cargo test` 4740 passed / 0 failed / 24 ignored（与基线一致）、`cargo clippy --all-targets` 0 error、release 二进制 111,783,896 → 112,313,304 bytes（+0.47%）、`cargo tree --duplicates` 零版本分叉。另：`deny.toml` 补 6 条 `[[licenses.clarify]]`（MIT，带版本限定）——RC manifest 漏写 `license` 字段导致 `cargo deny` licenses 失败，base 工作区 LICENSE 实证均为 MIT，正式版补字段后可移除。完整决策表见 specmark change `upgrade-base-libs-rc`（design.md D3）。

- **fix(daemon): 无 hub 组合编译失败（E0308）** — `DaemonRunner::start` 在返回元组的元素上挂 `#[cfg(feature = "hub")]`，无 hub 组合下元组元数 (3) 与解构 (4) 不匹配，`core,daemon,…` 组合 lib test 无法编译（i18n 整改批次引入，CI 未及验证）。改为 hub 无关三元组 + `notify_webhook` 独立 cfg 读取，hub 路径语义不变。
- **fix(service): 三处 cfg 门错挂导致无 cli 组合 lib test 编译失败（E0425）** — ① `evolve.rs` 的 `index_core` import、② `taint.rs` 的 `resolve_project_id` import 均被 `#[cfg(feature = "cli")]` 门控，但其调用方 `run_evolve`/`run_taint`（无门控的可测试核心）无条件使用；③ `query.rs` 的 `runtime::kit` import 未覆盖 `run_query_cached` 的 `test + cache` 组合。三处均放开为随调用方编译。至此 CI test 矩阵 6 条腿全部可编译可测试。

### Changed（冻结改动收编与跨仓解耦）

- **chore(comments): 清理注释中的任务体系内部标记** — 全仓注释移除 `M2:`/`L3:`/`L6`/`M3` 等任务代号前缀（resolve 五模块、index/phases、analysis/api_review、main tests、trace_bench、storage/quality），注释语义不变、代码零改动。
- **refactor(graph-viewer): graph-server 生产日志迁移 inklog** — `tracing-subscriber` 移除，改用 `inklog::LoggerManager`（`{timestamp} [{level}] {target} - {message}` 控制台格式）；原 per-target filter 不再支持（单用途 dev server 影响可忽略）。已实测：启动日志为 inklog 格式、敏感地址经 inklog 脱敏、401 鉴权正常。
- **chore(deps): 移除 `[patch.crates-io]` 与跨仓 path 依赖，基库走 crates.io** — 根 `Cargo.toml` 与 `graph-viewer/server/Cargo.toml` 删除 `path = "../base/*"` 字段，version req 升至已发布最新：trait-kit `0.5.0-rc.5 → 0.5.0-rc.6`、sdforge `0.5.0-rc.4 → 0.5.0-rc.5`、oxcache `0.5.0-rc.4 → 0.5.0-rc.5`、inklog `0.3.0-rc.4 → 0.3.0-rc.5`（无依赖项指向未发布版本）。配套：`ci.yml` 四个 job（lint/test/coverage/security）删除 base 仓库浅克隆步骤（不再需要）；`release.yml` 删除「发布前剥离 patch 段」步骤（段已不存在，`str.index` 会直接抛错）；`RELEASING.md` 对账项 7 改写为「基库一律 crates.io version req、禁止重新引入跨仓 path/patch」。验证：`cargo deny check`（advisories/bans/licenses/sources）全绿、graph-server 冒烟通过。

## [0.3.12] - 2026-07-30

P-DB + P-DB-fix：动态 `max_db_size` + `--fresh` 标志解决 DuckDB 16 GiB 硬编码上限 + DELETE 死空间累积导致的 DB 膨胀问题（6.6 MB 源码 → 35 GB DB）。read-only 连接 max_db_size 从 16 GiB 改为 4 TiB cap，修复 >16 GiB DB 查询崩溃的 CRITICAL bug。

### Fixed

- **fix(service): P-01 — UNWIND batch UPDATE for LSP hover `semantic_type`** — `enhance_with_lsp` hover 循环改用批量 Cypher `UNWIND [... ] AS row MATCH (n {id: row.id, project: '...'}) SET n.semantic_type = row.sem;`，每批 500 条 symbol 一条语句，替代原先每 symbol 一条 `execute(&build_semantic_type_update(...))`。100k symbols 仓库：100k 次同步 SQL 往返 → 200 次（500× 减少），节省 100–300 s 纯 SQL 等待。新增 `build_batch_semantic_type_update`（构造 UNWIND 语句，`escape_cypher_string` 转义 id/sem/project，`write!` 直接写入预分配 String 避免临时 format! 分配，动态 `with_capacity` 估算避免 reallocation）+ `flush_semantic_type_batch`（先尝试批量 execute，失败 emit `[warn]` 后回退到逐条 execute 以保证前向兼容，Rule 12 失败可见性）+ `LSP_HOVER_BATCH_SIZE=500` 常量。`flush_semantic_type_batch` 返回 `()`（非 usize），调用方依赖 enhanced/skipped 计数即可。11 个新单元测试覆盖空 batch / 单条 / 500 条 / 转义 / 批量成功 / 批量失败回退 / 全失败 / 计数正确性。
- **fix(storage): P-03 — eliminate `Vec<String>` clone-drain in `write_*_csv_stream`** — `write_edges_csv_stream` 和 `write_nodes_csv_stream` 消除中间 `Vec<String>` 分配：`edge_to_row(edge).into_iter().map(sanitize_for_ladybugdb)` iterator 直接传给 `csv::Writer::write_record`（csv crate 接受 `impl IntoIterator<Item = impl AsRef<[u8]>>`，`String: AsRef<[u8]>`）。1M-edge 仓库省 ~216 MB 峰值分配（1M × 9 String structs × 24 B）；100k-node 仓库省 ~31 MB。`edge_to_row`/`node_to_row` 签名不变（仍返回 `Vec<String>`，但调用方 `into_iter()` 消费，不 collect）。5 个新回归测试断言字节级输出与 P-03 前 `Vec<String>` 参考实现相同（含 dedup / 空输入 / sanitization 场景）；参考实现添加 SYNC 注释提醒与生产 dedup 逻辑同步。
- **fix(service): P-04 — `std::thread::scope` parallel LSP startup** — `enhance_with_lsp` 启动循环改用 `std::thread::scope` 并行 spawn 每个 active provider 的 `start(workspace)`。Pre-P-04 串行启动 8 个 LSP server 最坏 ~20–40 s（rust-analyzer ~3-8 s + clangd ~1-3 s + gopls ~2-5 s + …）；并行后总耗时 ≈ max(per-provider)，8-core 机器一核一个。新增 `MAX_LSP_PROVIDERS=8` const + `build_lsp_providers` assert 守护 semaphore-free 并发上界（防未来新增 provider 未调 const）。新增 `start_active_providers_parallel` 私有函数：`std::panic::catch_unwind(AssertUnwindSafe)` 捕获 panic provider 转 `LspError::ServerStart`（防 buggy provider 中断整个 scope），失败语义不变（degraded continue + warning）。文档化 panic-then-shutdown 契约：`LspProvider::shutdown` 实现必须 safe to call after panicked `start()`（当前所有 client 用 `Mutex<Option<Session>>` 初始化为 `None`，shutdown 短路 `None`，safe by construction）。6 个新单元测试覆盖并行加速 / 全启动 / 跳过 inactive / panic 隔离 / failure 隔离 / 空集合短路。
- **fix(service): P-08 — `PathInterner` for `entries` PathBuf dedup** — `enhance_with_lsp` 新增 `PathInterner`（`HashMap<PathBuf, Arc<PathBuf>>` + `intern(PathBuf) -> Arc<PathBuf>`），`entries` 类型从 `Vec<(String, PathBuf, u32)>` 改为 `Vec<(String, Arc<PathBuf>, u32)>`。10k 文件 × 30 symbol = 300k entries 中 ~270k 是重复 PathBuf，现在共享同一 `Arc<PathBuf>`（`Arc::ptr_eq` 指针相等），省 ~15–30 MB 堆分配。`intern` 用 `get + insert` 模式（hit 路径零 PathBuf 分配，仅 Arc clone；miss 路径 1 PathBuf clone + 1 Arc 分配），避免 `entry()` API 在 hit 路径的 `to_owned()` 分配 churn（perf-review H-01: 290k hits × 80 B = 23.2 MB 节省）。key 用 `PathBuf` 而非 `String`，避免 `to_string_lossy()` 在非 UTF-8 路径上的字节精度回归（arch-review MEDIUM-1，Linux 文件系统支持任意字节序列）。`Arc<PathBuf>` 自动 deref 到 `&Path`，`LspProvider::hover(&self, file: &Path, ...)` trait 签名不变。不引入 `string-interner` crate。移除 `len()`/`is_empty()`/`Default`（YAGNI，dead code）。6 个新单元测试覆盖去重 / 不同路径不共享 / strong_count / Send+Sync / 空/Unicode 路径。
- **fix(security): tiangang LOW-1 — `escape_cypher_string` 转义 NUL 字节** — `escape_cypher_string` 新增 `\0` → `\\0` 转义，防止嵌入 NUL 提前终止 C-string 视图（防御纵深，无已知利用向量；LSP hover 输出极少含 NUL，但 `id`/`sem`/`project` 仍可能是任意字符串）。新增 3 个测试覆盖 NUL 在开头/中间/结尾的转义。
- **fix(storage): tiangang/diting review fixes — `flush_semantic_type_batch` 返回 `()` + `as u32` 替代 `try_from`** — `flush_semantic_type_batch` 返回类型从 `usize` 改为 `()`（arch-review LOW-1: 返回值无调用方使用，dead code）。`*enhanced += processed as u32` 替代 `u32::try_from(processed).unwrap_or(u32::MAX)`（perf-review L-02: `processed` 上界为 `LSP_HOVER_BATCH_SIZE=500`，`as u32` 不溢出且无死防御）。`debug_assert!` 改为 `if stmt.is_empty() { return; }` 防御性 return（arch-review LOW-5: `debug_assert` 在 release 编译消失，显式 `if` 是生产守护）。
- **fix(resolve): H1 — drop master `Vec<Edge>` from sub-resolvers** — 5 个子解析器（`CallResolver::resolve_calls` / `DataFlowResolver::resolve_dataflows` / `FfiResolver::resolve_ffi` / `ImportResolver::resolve_imports` / `TypeResolver::resolve_types`）返回类型从 `Vec<Edge>` 改为 `()`，edge 直接 `graph.add_edge()`，不再收集到 master Vec 后立即丢弃。大型仓库（1M+ edges）省 ~100 MB 峰值 RSS。同步移除 `prune_dangling_type_edges_vec` 死代码（L6 后已无 `all_edges` 调用方）。
- **fix(ir): L3 — encapsulate `ExtractResult.edges`** — `edges` 字段从 `pub` 改为 `pub(crate)`，新增 `edges()`/`edges_mut()` 访问器。L7-2 在 `ScopeResolutionPhase::run` 后 `clear()` + `shrink_to_fit()` 该字段，`pub(crate)` 防止 crate 外调用方读到清空后的 Vec。同步新增 `Phase` trait 数据流契约文档（single-source-of-truth + bounded-lifetime + borrow-order rules）。
- **fix(lsp): L1 + LOW-2/3/4 — kill_and_warn helper + D-state 注释** — 抽取 `kill_and_warn(child, timeout_ms, context)` helper 消除 `kill_session` / `force_kill_and_wait` 两处重复的 `child.kill() + wait_with_timeout + eprintln!` 序列；LOW-2/LOW-3：两条 kill 路径（`provider.shutdown()` 失败回退 + `child.kill()` 直接调用）现在都通过 helper 发 stderr 警告（无 kill 路径静默）；LOW-4：注释从 "zombie" 修正为 "D-state stall or zombie reaped by init"（D-state 是因，zombie 是可能结果）。
- **fix(tests): LOW-1 — std::env::temp_dir() → tempfile::TempDir** — 18 处测试中 `std::env::temp_dir()` 替换为 `tempfile::tempdir()`（或 `tempfile::TempDir`），避免并发测试冲突 + 自动清理临时文件。
- **fix(service): M2/P-07 + P-02 + P-10 — LSP 扩展名路由优化** — `collect_active_extensions` 对未知扩展名（`.md`/`.toml`/`.json`）跳过而非回退到 `default_ext`（原先为 `.md` 启动 rust-analyzer 浪费 300 MB–2 GB RSS）；`select_provider_for_ext` 用 `HashMap<&str, &dyn LspProvider>` O(1) 查找替代 8-provider 线性扫描（100k+ symbols → 100k+ scans）；`collect_active_extensions` 内部用 `HashSet<&str>` O(1) 查找替代 `&[&str]` 线性扫描。
- **fix(index): M3 — remove dead `per_collection_soft_limit`** — `MemoryBudget` 移除 `per_collection_soft_limit` 字段 / `DEFAULT_SOFT_LIMIT` 常量 / `with_soft_limit` / `collection_exceeds_limit`（L7-4 RAM-first 放大因子已替代该软限制）。
- **fix(storage): L4 — rename inherent `save_nodes`/`save_edges` to `_stream`** — `Repository` inherent 方法 `save_nodes`/`save_edges` 重命名为 `save_nodes_stream`/`save_edges_stream`，消除与 `Storage` trait 同名方法的命名冲突。切片调用方（`repo.save_nodes(&[..])`）现在通过 trait 方法路由（trait 委托到 `_stream`）；迭代器调用方（`save_nodes_by_label` / `LoadPhase`）必须直接调用 `_stream`（trait 的 `&[Node]` 签名不接受任意 `impl Iterator`）。
- **fix(storage): P-06 — `write_edges_csv_stream` dedup key 改为 tuple** — `HashSet<String>` 改为 `HashSet<(&str, &str, EdgeType, u32)>`，每条 edge 不再分配 ~80 字节的 `format!("{}_{}_{}_{}")` String。1M-edge 仓库省 ~80 MB 纯 dedup-key 分配。`EdgeType` 已 `derive(Hash + Eq + Copy)`，tuple 形式直接借用 `edge.source`/`edge.target`。
- **fix(storage): P-11 — BufWriter 64 KB → 256 KB** — `save_nodes_stream`/`save_edges_stream` 的 `BufWriter` 容量从 64 KB 提升到 256 KB，1M-row CSV dump 的 syscall 从 ~16/MB 降至 ~4/MB。内存开销有界：每次调用至多一个 256 KB BufWriter 存活。
- **fix(storage): P-05 — dynamic DuckDB `buffer_pool_size`** — `compute_buffer_pool_size()` 新函数：25% 可用内存，clamp 到 `[256 MiB, 4 GiB]`。70 GB 主机（~60 GB 可用）→ 4 GiB（cap）；16 GB 主机（~12 GB 可用）→ 3 GiB；4 GB 主机（~2 GB 可用）→ 512 MiB。`sysinfo` 返回 0 时回退到 L7-5 的 4 GiB 固定值。测试构建保持 256 MiB 固定 cap（并行测试需要）。
- **fix(service): P-09 — document why UNION is not used** — `build_symbol_queries` 添加文档注释说明为何保持 2 个独立查询：LadybugDB 的 Cypher 子集不支持 `UNION`/`UNION ALL`（参见 `analysis/architecture.rs`），也不支持多标签 `WHERE (n:A OR n:B)` 表达式。两个查询顺序执行，结果在 Rust 中通过 `rows.extend(r)` 合并。
- **fix(storage): P-DB — dynamic `max_db_size` + `--fresh` flag for DB space reclamation** — DuckDB `max_db_size` 从硬编码 16 GiB 改为动态计算（`compute_max_db_size`：80% 可用磁盘空间，`next_power_of_two` 对齐 DuckDB 2 的幂要求，clamp 到 `[16 GiB, 4 TiB]`，`OnceLock` 进程级缓存避免重复探测）。新增 `--fresh` 标志（仅 `index` 命令）：索引前删除旧 DB 文件，回收 DuckDB DELETE 不回收的死空间（6.6 MB 源码经多次 `--force` 累积至 35 GB DB 的问题）。`delete_db_for_fresh` 含 `.lbug` 扩展名校验（防误删非 DB 文件）+ TOCTOU 修复（直接 `remove_file` 匹配 `NotFound`，消除 `exists()` + `remove_file()` 竞态窗口）+ `exit(5/6)` 错误处理（规则 12 失败显性化）。sysinfo 添加 `disk` feature 支持磁盘空间探测。
- **fix(storage): P-DB-fix — read-only 连接 `max_db_size` 改为 4 TiB cap + 审查修复** — kueiku 审查发现 C1 CRITICAL：read-only 连接固定 16 GiB `max_db_size`，但 DB 文件可能 >16 GiB（P-DB 修复动机），read-only 查询（query/impact/trace 等）触及 >16 GiB 页面会触发 `BufferManagerException`。lbug `VMRegion` 用 `max_db_size` 决定 mmap 区域大小，read-only 连接也需要覆盖整个 DB 文件。改为 4 TiB CAP（虚拟地址空间预留，非实际内存）。同步修复：H1 — `--fresh` 添加到 `SENTINEL_DEFAULTS` 默认 `false`（原先 sdforge 标记 `required=true`，用户必须传 `--fresh false`）；M1 — `handle_fresh_flag` 用 `parse::<bool>()` 替代 `s == "true"`（与 handler 的 `bool::from_str` 保持一致，`--fresh True` 不再静默忽略）；MEDIUM-2 — exit code 4 → 6（InvalidInput 语义与 NotFound 的 exit 4 冲突）；LOW-2 — `Ok(false)` 添加日志（用户可见 `--fresh` 生效状态）；L-2 — `next_power_of_two` 改为 `checked_next_power_of_two().unwrap_or(CAP)`（消除理论 panic 风险）。

## [0.3.11] - 2026-07-26

L6+L7 memory-overflow fix：L6 管线流式化 + LSP kill 超时 + iterator API 将峰值内存从 O(graph) 降至 O(1)（ResolvePhase 不再 clone Graph）；L7 在 L6 基础上进一步定位并修复 6 个剩余瓶颈（LZ4 缓冲生命周期 / 解析 edge 清理 / ctx drain / RAM 预算放大因子 / LadybugDB buffer_pool 封顶 / LSP 按需启动），70 GB 主机峰值内存从 60 GB 降至 ~4 GB。

### Fixed

- **fix(index): L6 memory-overflow — pipeline streaming + ctx.take()** — `ResolvePhase` 改用 `ctx.remove::<ScopeOutput>("scope")` 取得 Graph 所有权（替代 `scope.graph.clone()`，消除大型仓库多 GB 的 deep-clone）；`ctx.remove::<ParseOutput>("parse")` 在 ResolvePhase 结束后释放 `ExtractResult` AST 快照；`build_includes_edges` 返回 `IncludesGraph`（INCLUDES edge 直接 move 进 graph，不再 clone）；`resolve_all` 返回 `()` 替代 `Vec<Edge>`（master Vec 立即被丢弃，~100 MB 浪费）；`prune_dangling_type_edges` 用 `std::mem::take` + `retain` 替代 `HashSet<String>` 全量节点 id 拷贝（百 MB 级 HashSet 消除）；`Phase::run` 签名从 `&PipelineCtx` 改为 `&mut PipelineCtx` 以支持 `ctx.remove` 独占借用。
- **fix(storage): L6-3 iterator API — save_nodes/save_edges/write_nodes_csv_stream/write_edges_csv_stream** — 4 个函数签名从 `&[Node]`/`&[Edge]` 改为 `impl IntoIterator<Item = &Node/&Edge>`，调用方可直接从 `Graph::nodes_view()`/`edges_view()` 流式写入，消除 `Vec<Node>`/`Vec<Edge>` 中间集合（大型仓库 ~10 MB 峰值 RSS）；新增 `NodeCsvStats` 结构体（与 `EdgeCsvStats` 对称），dedup 内移到 `write_nodes_csv_stream`（HashSet by node id，first-wins 语义）；`save_nodes_by_label` 移除 `deduped: Vec<Node>` per-label clone；`EdgeCsvStats`/`NodeCsvStats` 新增 `total` 字段（恢复 `save_edges` 切到 iterator 后丢失的可观测性）；`save_nodes`/`save_edges` 内部包 `BufWriter`（64 KB）减少 write syscall。`Storage` trait 方法保留 `&[Node]`/`&[Edge]` 签名以维持 object safety（dyn Storage 在 89+ callsites 使用）。
- **fix(lsp): L6-3 LSP kill timeout — bound force_kill_and_wait with KILL_TIMEOUT_MS** — 新增 `kill_session`（initialize 失败路径的 force-kill，避免 orphan LSP server）；`shutdown_session` 统一 graceful shutdown（shutdown request → exit notification → wait_with_timeout → force_kill_and_wait）；`force_kill_and_wait` 用 `wait_with_timeout` 替代 `child.wait()`（防止 D-state/zombie 永久阻塞）；新增 `KILL_TIMEOUT_MS=3000`/`SHUTDOWN_TIMEOUT_MS=5000` 常量；`send_raw_request` 在 channel disconnected 时 emit stderr warning（fail-loud，Rule 12）。`client.rs::shutdown` 改用 `shutdown_session`；`client.rs::initialize` 失败时调用 `kill_session` 防 orphan。
- **fix(index): L7-1 memory-overflow — drop LZ4 buffers after parse phase** — `ParsePhase` 移除 `RamFirstSources` 字段，LZ4 压缩缓冲存入 `PipelineCtx`（键值 `"ram_first_compressed"`，由 `Pipeline::run_inner` 调用 `ctx.insert(ParsePhase::RAM_FIRST_KEY, compressed)` 转交），`ParsePhase::run` 起始处通过 `ctx.remove::<Option<RamFirstSources>>(Self::RAM_FIRST_KEY).flatten()` 取得并在函数返回时立即 drop（原先压缩缓冲在 `ParsePhase` 结构体字段中存活至 pipeline 结束）。10k 文件仓库省 ~3 GB 峰值 RSS。`RamFirstSources` 类型从 `ParsePhase` 字段移至 `PipelineCtx` 类型擦除存储。
- **fix(index): L7-2 memory-overflow — clear parse edges after scope resolution** — `ScopeResolutionPhase::run` 在把 `result.edges` clone 进 `graph.edges` 后，对每个 `ExtractResult` 执行 `result.edges.clear()` + `result.edges.shrink_to_fit()` + `result.seen_qns.clear()` + `result.seen_qns.shrink_to_fit()`，然后 `ctx.insert("parse", parse)` 回写。`ExtractResult.edges` 字段添加文档注释标注 L7-2 不变式（"After `ScopeResolutionPhase::run`, this Vec is `clear()` + `shrink_to_fit()`'d... Resolvers MUST read edges from `graph.edges`, NOT from this field"）。大型仓库省 ~1 GB 重复 edge 数据（5M edges × 200 B = 1 GB for 200 MiB repo）。
- **fix(index): L7-3 memory-overflow — drain ScanOutput/ResolveOutput from PipelineCtx in LoadPhase** — `LoadPhase::run` 改用 `ctx.remove::<ScanOutput>("scan")` 和 `ctx.remove::<ResolveOutput>("resolve")` 取得所有权（替代 `ctx.get` 共享借用），LoadPhase 返回时 `ScanOutput`（含 `FileInfo` 列表）和 `ResolveOutput`（含 `Graph`）立即 drop，不再存活至 pipeline ctx 整体 drop。省 ~1–2 GB 后置管线内存。
- **fix(index): L7-4 memory-overflow — RAM-first amplification factor** — 新增 `RAM_FIRST_AMPLIFICATION_FACTOR: u64 = 8` 常量；`evaluate_ram_first_budget` 改为 `(total_bytes × 8) < (max_rss_bytes / 2)` 判定（替代原 `total_bytes < per_collection_soft_limit`）。修正预算失真：RAM-first 模式同时驻留 LZ4 缓冲（1.0×）+ 解压源码（1.0×）+ IR ExtractResult（3.0×）+ Graph（2.0×）+ CSV 流（1.0×）= 8× 放大；原判定用 raw bytes 对比 256 MiB 软限制，200 MiB 仓库会通过检查（200 < 256）但实际峰值 ~1.6 GB。70 GB 主机（max_rss=35 GB）阈值 17.5 GB，允许 ~2.2 GB 仓库走 RAM-first；4 GB 笔记本（max_rss=2 GB）阈值 1 GB，仅允许 ~125 MB 仓库——内存受限主机的正确防御行为。
- **fix(storage): L7-5 memory-overflow — cap LadybugDB buffer_pool_size to 4 GB** — `open_with` 在非 `cfg!(test)` 分支显式设置 `SystemConfig::default().buffer_pool_size(4 GiB).max_db_size(16 GiB).max_num_threads(8)`（生产环境），测试分支保持 `256 MiB / 1 GiB / 8`。DuckDB 默认 `buffer_pool_size = 80% 物理内存`（70 GB 主机 ~56 GB），是 L6 修复后剩余的主要内存瓶颈；显式封顶省 ~52 GB。`max_db_size=16 GiB` 防止数据库文件无限增长吞噬磁盘；`max_num_threads=8` 限制 DuckDB 内部并行度避免与 rayon 线程池竞争。
- **fix(lsp): L7-6 memory-overflow — on-demand LSP startup** — `enhance_with_lsp` 重构为「先查询 Function/Method 行 → 收集实际出现的文件扩展名集合 → 只 `start()` 需要的 LSP provider」（原先无条件启动全部 8 个 LSP server：rust-analyzer / pyright / clangd / gopls / tsserver / fortls / jdtls，每个 300 MB–2 GB RSS）。新增 `collect_active_extensions` 纯函数（无 I/O / 无 LSP / 无 DB，可单元测试）+ 8 个单元测试覆盖空仓库 / 纯 Rust / 纯 Python / 混合 / 未知扩展名回退 / C+CPP 双激活 / 去重 / 8 语言 polyglot 全激活场景。纯 Rust 仓库现在只启动 rust-analyzer，省 ~3–5 GB；混合仓库启动 union 子集。`shutdown()` 仍对所有 8 个 provider 调用（未启动的 provider 短路 `Ok(())`，符合 `LspProvider::shutdown` 契约）。

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

- **fix(analysis): bulwark testing 5 bugs + triage review fixes** — bulwark project testing (534 files / 19242 nodes / 94127 edges) revealed 5 real bugs, all fixed plus triage review (security/architecture/performance) findings addressed:
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

- **Migrated all components to the `trait-kit` unified registry**. The in-tree `src/kit/shim.rs` fallback was removed once every module used `build_kit`. `trait-kit` is now a hard dependency.
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

[Unreleased]: https://github.com/Kirky-X/codenexus/compare/v0.3.11...HEAD
[0.3.11]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.11
[0.3.10]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.10
[0.3.9]: https://github.com/Kirky-X/codenexus/releases/tag/v0.3.9
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
