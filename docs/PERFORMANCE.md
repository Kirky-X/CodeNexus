# ⚡ CodeNexus 性能指南

本指南汇总 CodeNexus 的性能 SLO、基准套件、实测基线与内存优化设计。所有数字均来自仓库内 [`benches/README.md`](../benches/README.md) 记录的 criterion 实测或 CHANGELOG 记录的修复历史，未实测的指标明确标注 TBD。

## 📋 目录

- [概述与 SLO](#-概述与-slo)
- [基准套件](#-基准套件)
- [实测基线](#-实测基线)
- [内存优化（L1–L7 防线）](#-内存优化l1l7-防线)
- [索引调优](#-索引调优)
- [查询与追踪调优](#-查询与追踪调优)
- [复现方法](#-复现方法)
- [相关文档](#-相关文档)

---

## 🎯 概述与 SLO

以下 SLO 来自 [`docs/PRD.md`](PRD.md) §5.1，是面向生产工作负载的验收参考：

| 维度 | SLO | 说明 |
|:-----|:----|:-----|
| 索引吞吐（冷启动） | ≥ 100 files/s | 首次全量索引 |
| 增量索引（单文件变更） | ≥ 500 files/s | 典型保存后重索引 |
| 增量索引（批量变更） | ≥ 100 files/s | 500/1000 文件变更 |
| 查询延迟 | P99 ≤ 200 ms | Cypher / 搜索 |
| 追踪延迟 | P99 ≤ 500 ms | 调用链追踪 |
| 内存（首次索引） | ≤ 1 GB peak RSS | 10k 文件仓库 |
| 内存（daemon 持续） | ≤ 200 MB sustained RSS | 10 轮增量后 |
| daemon 响应 | ≤ 3 s | 去抖 + 索引起动 |
| 事件队列 | 不允许事件丢失 | daemon 事件吞吐 |

---

## 📏 基准套件

7 组 Criterion 基准位于 `benches/`（`daemon_bench` 由 `daemon` feature 门控，其余五组在所有 feature 预设下编译；`graph_bench` 覆盖图数据结构）：

| 文件 | 组 | 场景 | SLO 维度 |
|------|-----|------|----------|
| `index_bench.rs` | `index` | `index_10_files` | 冷启动吞吐 |
| `incremental_bench.rs` | `incremental` | `cold_start_1000`、`incremental_1_of_1000`、`incremental_500_of_1000` | 增量吞吐 |
| `query_bench.rs` | `query` | `cypher_match_functions`、`cypher_count_functions`、`search_by_name`、`search_by_name_project` | 查询延迟 |
| `trace_bench.rs` | `trace` | `trace_calls_depth_5`、`trace_calls_depth_10`、`trace_calls_depth_50`、`trace_all_depth_10`、`impact_large_subgraph`（4971 节点高扇入回归） | 追踪延迟 |
| `memory_bench.rs` | `memory` | `first_index_10k_files_peak`、`daemon_sustained_10_increments`、`ram_first_vs_default_comparison` | 峰值 / 持续 RSS |
| `daemon_bench.rs`（`daemon`） | `daemon` | `debounce_response_latency`、`indexing_event_queue_throughput` | daemon 响应性 |

> 完整的 SLO 阈值表、告警阈值与共享 fixture 说明见 [`benches/README.md`](../benches/README.md)。

---

## 📊 实测基线

> 来自 `benches/README.md` 记录的实测值。实际性能取决于仓库规模与硬件，请用 `cargo bench` 在目标环境复现。

| 场景 | 实测 | SLO | 状态 |
|------|------|-----|------|
| `cold_start_1000`（1000 文件冷启动） | 约 3929 files/s | ≥ 100 files/s | ✅ |
| `incremental_1_of_1000`（单文件变更） | 约 4987 files/s | ≥ 500 files/s | ✅ |
| `incremental_500_of_1000`（500 文件变更） | 约 33.23 files/s | ≥ 100 files/s | ⚠️ 未达标 |
| `debounce_response_latency` | 约 2.76 s/iter | ≤ 3 s | ✅ |
| `indexing_event_queue_throughput` | 约 9.18 elem/s（50/50 事件），零丢失 | 不允许丢失 | ✅ |
| `ram_first_vs_default_comparison`（1000 文件） | 默认约 450 MB / ram-first 约 568 MB | 对比项（无 SLO） | ℹ️ |
| `first_index_10k_files_peak` | TBD（需 `BENCH_RUN_IGNORED=1`） | ≤ 1 GB | 待补充 |
| `daemon_sustained_10_increments` | TBD（需 `BENCH_RUN_IGNORED=1`） | ≤ 200 MB | 待补充 |
| `cypher_match_functions` | TBD | P99 ≤ 200 ms | 待补充 |
| `trace_calls_depth_5` | TBD | P99 ≤ 500 ms | 待补充 |

⚠️ `incremental_500_of_1000` 实测 33.23 files/s，低于 ≥ 100 files/s 的 SLO。这是 `src/` 增量索引在 500 文件批量删除+重插路径上的已知性能问题（基准正确地暴露了回归），修复需改动 `src/index/`。

ℹ️ `ram_first_vs_default_comparison` 在 1000 文件 fixture 上 ram-first（568 MB）比默认（450 MB）**更耗内存**——这是小仓库的预期行为：LZ4 压缩开销超过 LadybugDB 写放大的节省。ADR-024 建议 ram-first 用于 **≥ 1 GB 源码**的仓库。

**历史里程碑**：v0.3.11 的 L6+L7 内存优化将 70 GB 主机上的索引峰值内存从 **约 60 GB 降至约 4 GB**；v0.3.12 的 P 系列修复将 10 万符号仓库的 LSP hover 从约 10 万次 SQL 往返压缩到 200 次（批量 UNWIND，每批 500 条），节省 100–300 秒纯 SQL 等待。

---

## 🧠 内存优化（L1–L7 防线）

大仓库索引 OOM 的七层防线（v0.3.10 落地 L1–L5，v0.3.11 落地 L6–L7，v0.3.12 补充 P 系列优化）：

| 层 | 机制 | 说明 |
|----|------|------|
| L1 | `MemoryBudget` 三级内存压力 | sysinfo 探测可用内存，max_rss 设为 50%，Green/Yellow/Red 分级 |
| L2 | mpsc channel + par_chunks 流式解析 | rayon `par_chunks(8)` + `sync_channel(4)` 限制并发 `ExtractResult` 上限为 4 |
| L3 | `Graph::nodes_view/edges_view` 迭代器 | 去除 node/edge 全量重复拷贝 |
| L4 | 流式 CSV | `write_nodes_csv_stream` / `write_edges_csv_stream` 替代全量 String 拼接 |
| L5 | 自适应降级 | 仓库总大小超预算时 `index_ram_first` 自动降级为流式磁盘读取；`CacheConfig::entry_max_bytes` 防超大 AST 驱逐小条目 |
| L6 | 管线流式化 | `ctx.remove` 所有权转移取代 `Graph::clone`；LZ4 缓冲解析后立即 drop；迭代器 API 直写 |
| L7 | LadybugDB 资源封顶 | `buffer_pool_size` 封顶 4 GB（测试 256 MiB）、`max_num_threads = 8`；LSP 按需启动（纯 Rust 仓库只启动 rust-analyzer）；RAM-first 8× 放大因子预算（压缩缓冲/解压源码/IR/Graph/CSV 合计 8 倍） |

v0.3.12 追加的存储与 LSP 优化：动态 `buffer_pool_size`（25% 可用内存，clamp 到 [256 MiB, 4 GiB]）、CSV dedup key 改 tuple、BufWriter 64 KB → 256 KB、`PathInterner` PathBuf 去重（省约 15–30 MB）、LSP 扩展名路由优化（`.md` 等未知扩展名不再回退启动 rust-analyzer）、`std::thread::scope` 并行 LSP 启动。

---

## 🚀 索引调优

### RAM 优先模式

```bash
# LZ4 内存压缩 + 单次 COPY FROM 批量入库
codenexus index --path /path/to/project --name myproject --ram_first true
```

| 建议 | 说明 |
|------|------|
| 源码 ≥ 1 GB 时使用 | 小仓库收益为负（压缩开销 > 写放大节省） |
| 预算判定自动化 | `(total_bytes × 8) < (max_rss / 2)` 不满足时自动降级为流式磁盘读取 |
| 内存构成 | 压缩缓冲 1.0× + 解压源码 1.0× + IR 3.0× + Graph 2.0× + CSV 1.0× = 8× 放大 |

### 增量索引

- SHA-256 哈希比对天然增量：重复 `index` 只解析变更文件，`incremental_1_of_1000` 实测约 4987 files/s。
- `--force true` 全量重建；`--fresh true` 先删旧 DB 回收死空间（DB 膨胀时使用）。

### daemon

- `--debounce-ms`（默认 2000）控制去抖窗口；实测去抖响应约 2.76 s（SLO ≤ 3 s）。
- SIGTERM/SIGINT 优雅退出，不会打断进行中的增量索引。

---

## 🔍 查询与追踪调优

- **只读连接**：`QueryFacade::new_read_only` / `TraceConfig.read_only` 允许多进程并发读，且跳过 schema 初始化。
- **查询缓存**：`cache` feature（oxcache）缓存查询结果；`CacheConfig::entry_max_bytes`（64 KiB 单条上限）防止超大条目驱逐。
- **影响分析上限**：子图节点上限 `MAX_NODES_LIMIT = 5000`（v0.3.8 从 1000 提升，对齐 270+ 直接调用者的高扇入仓库）；`trace_bench` 的 `impact_large_subgraph` 场景做回归守护。
- **路径过滤**：`trace --path_filter "/src/api/**"` 在库层裁剪搜索空间，优先使用。
- **并行 LSP**：LSP 按需 + 并行启动（耗时 ≈ max(per-provider)）；纯 Rust 仓库只启动 rust-analyzer，节省 3–5 GB RSS。
- **批量 SQL**：LSP hover 语义类型更新走批量 `UNWIND`（每批 500 条符号），避免逐条往返。

---

## 🧪 复现方法

命令与 [`benches/README.md`](../benches/README.md) 一致：

```sh
# 快速模式（达到统计显著性即停止，CI 友好）
cargo bench -- --quick

# 单个基准文件
cargo bench --bench incremental_bench -- --quick
cargo bench --bench query_bench -- --quick
cargo bench --bench trace_bench -- --quick
cargo bench --bench memory_bench -- --quick
cargo bench --bench daemon_bench --features daemon -- --quick

# 完整模式
cargo bench --bench incremental_bench
cargo bench --bench daemon_bench --features daemon

# 基线保存与回归对比
cargo bench --bench incremental_bench -- --save-baseline main
cargo bench --bench incremental_bench -- --baseline main

# 长时场景（10k 文件 / daemon 持续增量）需要显式开启
BENCH_RUN_IGNORED=1 cargo bench --bench memory_bench -- --quick
```

> 说明：criterion 0.5 的 `--ignored` 语义是「跳过全部基准」而非 libtest 的「只跑 ignored」，因此两个长时场景用 `BENCH_RUN_IGNORED=1` 环境变量在运行时开关。共享 fixture（`benches/common/mod.rs`）提供 `generate_large_repo` / `open_test_db` / `measure_peak_rss`（每 100 ms 采样 RSS）。

---

## 📚 相关文档

| 文档 | 内容 |
|------|------|
| [benches/README.md](../benches/README.md) | SLO 阈值表、告警阈值、共享 fixture |
| [📖 用户指南](USER_GUIDE.md) | `--ram_first` / `--fresh` / daemon 的使用说明 |
| [🏗️ 架构文档](ARCHITECTURE.md) | 索引管线分层设计 |
| [📋 更新日志](CHANGELOG.md) | L1–L7 与 P 系列优化的完整记录（v0.3.10–v0.3.12） |
| [🎯 产品需求文档（PRD）](PRD.md) | §5.1 SLO 指标定义 |
