# CodeNexus 数据库压缩：实现与实测

> 针对"数据库文件体积过大"的压缩能力说明。含 gzip / zstd / lz4 三种算法在同一真实库文件上的压缩率与耗时实测、现有 export/import pipeline 的压缩效果、以及原地 CHECKPOINT 的收益分析。

## 摘要

| 能力 | 机制 | 实测效果（191.8 MiB 库） |
|---|---|---|
| 离线压缩（已内置） | `export` → zstd-19 制品 | → 24.4 MiB（省 **87.3%**） |
| 内存压缩（已内置） | `--ram_first` → lz4 索引 | 内存占用显著降低 |
| 列级压缩（已内置） | LadybugDB `enable_compression`（默认开） | 新列写入即压缩 |
| 原地 `CHECKPOINT` | flush WAL | 收益 ≈ **0**（见 §5，不新增命令） |

**核心结论**：体积压缩已由 `export` 充分覆盖（192 MB → 24 MB）；原地 `CHECKPOINT` 对已索引库无实质收益，故不新增 `compact` 命令（避免为不存在的需求写代码）。算法层面有一个反直觉但重要的发现：**gzip-6 在压缩率与速度上均优于当前 zstd-19**（§4）。

## 1. 数据库类型与文件布局

CodeNexus 的知识图谱存储于 **LadybugDB**（基于 DuckDB 列存引擎，文件魔数 `LBUG*`）。`.gitnexus/` 目录典型布局：

```text
.gitnexus/
├── lbug                 # 主库（列存，已含内部压缩）
├── lbug.wal             # 预写日志（增量写入缓冲）
├── meta.json            # GitNexus 索引元数据
├── parse-cache/         # GitNexus (Node) 的解析缓存 — 非 codenexus 产物
└── parsedfile-cache/    # GitNexus (Node) 的解析缓存 — 非 codenexus 产物
```

> **口径说明（数据来源）**：下文实测样本为 `.gitnexus/lbug`（191.8 MiB，9232 符号 / 33188 关系）。该库由 **GitNexus（Node 工具）** 索引产生，非 codenexus Rust 代码产物。但**压缩率是文件字节内容的属性**，与生产者无关——同一份 LadybugDB 列存字节流，无论谁产生，各算法的压缩率特征一致。故本数据对 codenexus 同类库同样成立。`parse-cache` / `parsedfile-cache` 属 GitNexus，不在 codenexus 压缩职责范围内。

## 2. 现有压缩能力：export / import pipeline

`export`（`src/service/export.rs`）把整库读出，用纯 Rust `oxiarc-zstd`（level 19）压缩成团队共享制品；`import` 反向还原。制品格式：

```text
[magic "CNXP" 4B][manifest_len 4B LE][manifest JSON][zstd-compressed DB bytes]
```

- `manifest` 携带 `format_version` / `original_size` / `exported_at`，便于校验；
- 压缩内核 = `oxiarc_zstd::compress_with_level(bytes, 19)`，与系统 zstd CLI 解耦（无需外部二进制）；
- `import` 做魔数 + 版本校验、清理 WAL、可选重索引，并提供**往返测试**保证完整性。

用法：

```bash
codenexus export --db .gitnexus/lbug --output repo.cnxp --project <name>
codenexus import --db .gitnexus/lbug --input repo.cnxp
```

## 3. 三种算法实测对比

样本：`.gitnexus/lbug`，**201 146 368 字节（191.8 MiB）**。release 构建，单线程，所有 round-trip 经 SHA-256 校验通过（完整性 OK）。

| 算法 | level | 压缩后（字节） | 压缩后（MiB） | 压缩率 | 节省 | 编码耗时 | 解码耗时 |
|---|---|---|---|---|---|---|---|
| gzip | 6 | 19 052 359 | 18.17 | 9.47% | 90.53% | 1.89s | 0.25s |
| gzip | 9 | 18 880 000 | 18.00 | 9.39% | 90.61% | 5.14s | 0.27s |
| zstd | 3 | 26 378 347 | 25.16 | 13.11% | 86.89% | 1.23s | 0.40s |
| zstd | 9 | 25 664 246 | 24.47 | 12.76% | 87.24% | 2.12s | 0.47s |
| **zstd** | **19（=export）** | 25 619 492 | **24.43** | 12.74% | **87.26%** | 4.46s | 0.46s |
| lz4 | 1（fast） | 31 558 944 | 30.10 | 15.69% | 84.31% | 0.10s | 0.09s |

### 解读

- **压缩率排序**（越小越狠）：gzip-9 (9.39%) < gzip-6 (9.47%) < zstd-19 (12.74%) < zstd-3 (13.11%) < lz4 (15.69%)。
- **速度排序**：lz4（0.1s 编 / 0.09s 解）远快于其余；gzip-6 编码（1.89s）快于 zstd-19（4.46s）；解码端 zstd / gzip 都在 0.25–0.47s。
- **当前 export 选 zstd-19**：压缩率 12.74%，编码 4.46s，解码 0.46s。

## 4. 关键发现：gzip-6 在本数据集上反超 zstd-19

通用认知里 zstd 通常压过 gzip。本实测却出现 **gzip-6 压缩率（9.47%）优于 zstd-19（12.74%），且编码更快（1.89s vs 4.46s）**。

**原因**：LadybugDB 列存内部已用 zstd 压缩（`enable_compression` 默认开）。文件字节流本质是"已被 zstd 压缩过的数据"。再次压缩时：
- **zstd 对自身输出**的二次压缩增益小（zstd 不擅长压 zstd 流）；
- **deflate（gzip）的滑动窗口 + Huffman** 对这类已压缩流 + DuckDB page 结构有不同的抓取，反而更狠。

含义（可选优化方向，**当前未改**）：
- 若追求极致离线制品体积，export 改 gzip-6 可从 24.4 MiB 降到 18.2 MiB（**再省 26%**），且编码更快；
- 但 zstd 的优势在解码一致性、与 `--ram_first`（lz4）生态对称、以及未来字典压缩可演进性。

> 维持 zstd-19 的理由：已在生产、有往返测试、生态一致。改 gzip 是"再省 26%"的可选项，非必须。遵循最小变更原则，未动 export。

## 5. 原地 CHECKPOINT：为何不新增 `compact` 命令

考虑过新增 `compact` 命令（打开库 → `CHECKPOINT` → 报告前后体积）。经分析**收益≈0，不值得写**：

1. **CHECKPOINT 只 flush WAL**，不回收已删数据的物理空间（后者是 `VACUUM` 的职责，LadybugDB 未暴露）；
2. **WAL 本就极小**：实测 `lbug.wal = 12 KB`，对 192 MB 主库，CHECKPOINT 的收益上界 = 12 KB / 192 MB ≈ **0.006%**；
3. **已索引库已 CHECKPOINT 过**：`src/service/index.rs:265` 在索引完成后即执行 `connection().execute("CHECKPOINT;")`，活跃库的 WAL 日常就是空的。

结论：新增一个收益 0.006% 的命令是 YAGNI 反例。若未来出现"频繁删除导致主库膨胀"的真实场景，再评估 LadybugDB 是否暴露 VACUUM 或走 export→import 重塑路径。

## 6. 完整性保证

压缩必须可逆。三层保障：

- **round-trip SHA-256**：§3 每个算法压缩后立即解压并比对哈希，全部 OK；
- **export/import 往返测试**：`src/service/import.rs` 内置端到端测试，校验魔数 / 版本 / 数据一致；
- **WAL 清理**：import 时清理残留 WAL，避免新老数据混杂。

## 7. 推荐与权衡

```mermaid
flowchart LR
    Need{"压缩目标?"} -->|离线归档/迁移| Exp["export (zstd-19)"]
    Need -->|极致体积(可选)| Gz["可改 gzip-6: 再省26%"]
    Need -->|内存索引| Ram["--ram_first (lz4)"]
    Need -->|原地瘦身| Skip["CHECKPOINT 收益≈0, 跳过"]
    Exp & Gz & Ram --> Verify["SHA-256 round-trip 校验"]
```

| 场景 | 推荐 | 理由 |
|---|---|---|
| 团队共享 / 迁移 / 归档 | `export`（zstd-19，已内置） | 87% 压缩，完整往返保证 |
| 追求最小制品（可选优化） | 评估 export 改 gzip-6 | 再省 26%，编码更快 |
| 降低内存占用 | `--ram_first`（lz4，已内置） | 极速编解码 |
| 想原地"瘦身"活跃库 | 不做 | CHECKPOINT 收益 0.006%，不值得 |

## 复现指引

```bash
# 复现 §3 算法实测（含完整性校验）
cd /tmp && cargo new compress_bench
# 依赖: oxiarc-zstd(0.3,default-features=false) + lz4_flex(0.14) + flate2(1) + sha2(0.10)
cargo run --release -- <path-to-lbug>
```
