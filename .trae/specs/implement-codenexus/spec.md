# CodeNexus 代码库索引工具实现 Spec

## Why

CodeNexus 需要对任意代码库（非仅 Git 仓库）建立可查询的代码知识图谱，支持多语言（C/Rust/Fortran/Python/TypeScript）解析、嵌套结构、变量数据流追踪、函数调用链追踪与跨语言 FFI 调用解析。现有工具（GitNexus、codebase-memory-mcp）均未同时支持"非 Git 代码库 + 跨语言 FFI 追踪 + 变量数据流"。本 Spec 严格依据 `docs/PRD.md`、`docs/TRD.md`、`docs/ADD.md`、`docs/DDD.md` 四份文档实现，禁止偏离文档要求。

## What Changes

- 新建 Rust 单 crate 多 mod 项目（ADR-001），模块划分：model / discover / parse / resolve / storage / index / embed / daemon / trace / cli
- 实现数据模型层：20 种节点类型 + 14 种边类型 + Language/FlowType 枚举（依据 DDD §4-5、ADD §3.4）
- 集成 LadybugDB 官方 lbug crate v0.17（ADR-002），实现图模式建表、CSV 批量加载、Cypher 查询
- 实现 tree-sitter 多语言解析（ADR-003），Extractor trait 适配器模式，每语言一个薄提取器
- 实现 ignore crate 文件发现（ADR-012），完整 gitignore 语义 + ALWAYS_SKIP_DIRS 硬编码跳过
- 实现 SHA-256 文件哈希增量索引（ADR-009），变更/新增/删除三类 diff
- 实现 rayon 并行解析（ADR-010），文件级并行 + 线程局部 parser 复用
- 实现符号解析自研通用编排器 + 薄语言适配器（ADR-011）：作用域链、符号表、调用关系、数据流、跨语言 FFI
- 实现跨语言 FFI 解析：名称匹配 + 签名匹配双策略，置信度 0.70-0.85 标注
- 实现追踪引擎：BFS 遍历 Calls/FfiCalls/DataFlows/Reads/Writes 边，深度限制
- 实现查询引擎：Cypher 查询 + 结构化搜索 + BM25 全文搜索 + 可选向量语义搜索（RRF 融合）
- 实现 CLI 工具（clap 4）：index/query/trace/impact/search/daemon/status/list/clean 九个子命令
- 实现守护模式（notify-debouncer-full，ADR-013）：文件监视 + 防抖（默认 2000ms）+ 自动增量更新
- 实现可选嵌入 feature（ADR-004）：外部 HTTP 嵌入服务 + LadybugDB VECTOR 存储
- 创建 Skill 文件指导 Agent 使用 CLI
- 遵守测试驱动开发（TDD）：先写测试再写实现，代码测试覆盖率 ≥ 95%
- 积极使用设计模式：门面模式（Facade）、适配器模式（Adapter）、策略模式、工厂模式、建造者模式、观察者模式、仓储模式

## Impact

- Affected specs: 无（全新项目）
- Affected code: 全新 Rust 项目，依据 ADD §3.3 组件图与 TRD §7.1 模块划分
  - `src/model/` - 数据模型（Node/Edge/Graph/NodeLabel/EdgeType/Language）
  - `src/discover/` - 文件发现（ignore crate 集成）
  - `src/parse/` - tree-sitter 解析（ParserPool + Extractor trait + 各语言适配器）
  - `src/resolve/` - 符号解析（scope/symbol_table/calls/dataflow/cross_lang）
  - `src/storage/` - LadybugDB 存储（lbug 封装/schema/connection/loader/query）
  - `src/index/` - 索引流水线（pipeline/hash/incremental/parallel）
  - `src/embed/` - 可选嵌入（feature gate）
  - `src/daemon/` - 守护模式（notify-debouncer-full）
  - `src/trace/` - 追踪引擎（call_graph/data_flow/impact）
  - `src/cli/` - CLI 命令（clap 4，九个子命令）
  - `src/query/` - 查询引擎（Cypher/结构化/全文/语义）
  - `tests/` - 集成测试与端到端测试
  - `skill/` - Skill 文件

## ADDED Requirements

### Requirement: 项目脚手架与构建配置
系统 SHALL 使用 Rust 1.81+ 单 crate 多 mod 结构，Cargo.toml 锁定依赖版本，features 控制可选功能（lsp/embed），代码规范遵循 rustfmt + clippy（-D warnings）。

#### Scenario: 项目可编译
- **WHEN** 执行 `cargo build`
- **THEN** 项目成功编译，无警告

#### Scenario: clippy 通过
- **WHEN** 执行 `cargo clippy -- -D warnings`
- **THEN** 无 clippy 警告

### Requirement: 数据模型层
系统 SHALL 提供 20 种节点类型（Project/Folder/File/Module/Class/Struct/Enum/Trait/Impl/Function/Method/Variable/GlobalVar/Parameter/Const/Static/Macro/TypeAlias/Typedef/Namespace）与 14 种边类型（CONTAINS/DEFINES/MEMBER_OF/CALLS/FFI_CALLS/DATAFLOWS/READS/WRITES/IMPLEMENTS/EXTENDS/USES_TYPE/REFERENCES/IMPORTS/INCLUDES），字段定义严格遵循 DDD §5。

#### Scenario: 节点与边可构造
- **WHEN** 构造一个 Function 节点与一条 CALLS 边
- **THEN** 字段类型与 DDD §5.3/§5.8 一致，主键 id 为 UUIDv7 字符串

#### Scenario: Graph 内存图操作
- **WHEN** 向 Graph 添加节点与边并查询邻居
- **THEN** 返回正确邻居（按边类型过滤）

### Requirement: LadybugDB 存储层
系统 SHALL 通过 lbug crate 集成 LadybugDB，创建 20 张节点表 + 1 张 CodeRelation 关系表 + 1 张可选 Embedding 表（DDL 严格遵循 DDD §12.1），创建索引（DDD §12.2），支持 CSV 批量加载（COPY）与 Cypher 查询执行。

#### Scenario: 建表与索引
- **WHEN** 初始化数据库
- **THEN** 所有节点表、关系表、索引按 DDD §12 创建成功

#### Scenario: CSV 批量加载
- **WHEN** 加载一批节点与边
- **THEN** 通过 COPY FROM CSV 批量写入，使用 csv crate 生成 RFC 4180 合规 CSV（ADR-014）

#### Scenario: Cypher 查询
- **WHEN** 执行 `MATCH (f:Function) RETURN f.name LIMIT 10`
- **THEN** 返回函数名称列表

#### Scenario: 多项目隔离
- **WHEN** 同一数据库索引项目 A 与项目 B
- **THEN** 两项目数据通过 project 属性隔离，互不干扰（BR-INDEX-004）

### Requirement: 文件发现
系统 SHALL 使用 ignore crate（ripgrep 同源，ADR-012）实现文件发现，遵循 .gitignore 与 .codenexusignore 规则，硬编码跳过 ALWAYS_SKIP_DIRS（如 .git/target/node_modules 等）。

#### Scenario: gitignore 生效
- **GIVEN** .gitignore 包含 target/
- **WHEN** 索引代码库
- **THEN** target/ 目录被跳过（AC-INDEX-004）

#### Scenario: 硬编码跳过目录
- **WHEN** 遇到 .git/target/node_modules 等目录
- **THEN** 跳过该目录（BR-INDEX-006）

### Requirement: tree-sitter 多语言解析
系统 SHALL 使用 tree-sitter 0.22 解析 C/Rust/Fortran/Python/TypeScript 五种语言，采用适配器模式（Extractor trait），每语言一个薄提取器，提取函数/类/变量/调用/导入/赋值等符号。ParserPool 使用线程局部复用避免重复 Parser::new()。

#### Scenario: C 提取器
- **WHEN** 解析 C 文件
- **THEN** 提取函数定义、调用、#include、typedef、全局变量

#### Scenario: Rust 提取器
- **WHEN** 解析 Rust 文件
- **THEN** 提取 fn/struct/enum/trait/impl/extern "C" 块、调用、use

#### Scenario: Fortran 提取器
- **WHEN** 解析 Fortran 文件
- **THEN** 提取 subroutine/function/module、ISO_C_BINDING、call

#### Scenario: 并行解析
- **WHEN** 解析多文件
- **THEN** rayon 文件级并行，线程局部 parser 复用，无锁合并（ADR-010）

### Requirement: 符号解析与追踪
系统 SHALL 实现自研通用编排器 + 薄语言适配器（ADR-011）：作用域链、文件级与项目级符号表、调用关系解析（receiver-bound-calls + free-call-fallback 通用 passes）、数据流追踪（参数传递/返回赋值/变量赋值/读取/写入）、跨语言 FFI 解析（名称+签名双匹配，置信度 0.70-0.85）。

#### Scenario: 同语言调用追踪
- **GIVEN** 函数 A 调用函数 B
- **WHEN** trace A --type calls
- **THEN** 返回 A→B 调用路径（AC-TRACE-001）

#### Scenario: 变量数据流追踪
- **GIVEN** 变量 x 传递给函数 foo 的参数
- **WHEN** trace x --type dataflow
- **THEN** 返回 x→foo.param 数据流路径（AC-TRACE-002）

#### Scenario: 跨语言 FFI 追踪
- **GIVEN** Rust 函数通过 extern "C" 调用 C 函数
- **WHEN** trace rust_func --type calls
- **THEN** 返回含 FfiCalls 边的路径（AC-TRACE-003）

#### Scenario: 深度限制
- **GIVEN** --depth 2
- **WHEN** trace symbol
- **THEN** 返回路径深度不超过 2（AC-TRACE-004）

### Requirement: 增量索引
系统 SHALL 使用 SHA-256 文件哈希（ADR-009）实现增量索引：哈希一致跳过（BR-INDEX-001）、磁盘无文件则删除节点与边（BR-INDEX-002）、--force 忽略哈希全量重解析（BR-INDEX-003）。

#### Scenario: 增量跳过未变更文件
- **GIVEN** 已索引的代码库
- **WHEN** 修改一个文件后再次索引
- **THEN** 仅解析变更文件，跳过未变更文件（AC-INDEX-002）

#### Scenario: 强制重索引
- **GIVEN** --force 标志
- **WHEN** 执行索引
- **THEN** 忽略哈希，全量重解析（AC-INDEX-005）

#### Scenario: 文件删除检测
- **GIVEN** 数据库有该文件但磁盘无
- **WHEN** 索引
- **THEN** 删除节点与关联边（BR-INDEX-002）

### Requirement: 查询与搜索
系统 SHALL 支持 Cypher 查询（query 命令）、结构化搜索（search 命令，按名称/类型/文件）、BM25 全文搜索（LadybugDB FTS 扩展）、可选向量语义搜索（外部 HTTP 嵌入服务 + RRF 融合）。

#### Scenario: Cypher 查询
- **GIVEN** 已索引的代码库
- **WHEN** query "MATCH (f:Function) RETURN f.name LIMIT 10"
- **THEN** 返回函数名称列表（AC-QUERY-001）

#### Scenario: 结构化搜索
- **GIVEN** 已索引的代码库
- **WHEN** search "parse"
- **THEN** 返回含 parse 关键词的符号（AC-SEARCH-001）

#### Scenario: 语义搜索
- **GIVEN** 启用嵌入功能
- **WHEN** search "解析函数" --semantic
- **THEN** 返回语义相关的符号（AC-SEARCH-002）

### Requirement: 守护模式
系统 SHALL 使用 notify-debouncer-full（ADR-013）实现守护模式：监视项目根目录、防抖窗口（默认 2000ms，可配置 --debounce-ms）、代码文件过滤、索引期间暂停事件处理、防抖结束后触发增量索引。

#### Scenario: 防抖触发增量索引
- **GIVEN** 守护模式运行中
- **WHEN** 修改一个代码文件
- **THEN** 2s 后自动触发增量索引（AC-DAEMON-001）

#### Scenario: 连续修改合并
- **GIVEN** 连续修改多个文件
- **WHEN** 最后一次修改后 2s
- **THEN** 仅触发一次增量索引（AC-DAEMON-002）

#### Scenario: 非代码文件忽略
- **GIVEN** 修改非代码文件
- **WHEN** 文件变更
- **THEN** 不触发索引（AC-DAEMON-003）

### Requirement: CLI 工具
系统 SHALL 提供 clap 4 CLI，包含九个子命令：index（索引）、query（Cypher 查询）、trace（追踪）、impact（影响分析）、search（搜索）、daemon（守护）、status（状态）、list（列出项目）、clean（清理）。退出码：0 成功、1 输入异常、2 数据库锁定、3 系统异常、4 数据库损坏。

#### Scenario: index 命令
- **WHEN** 执行 `codenexus index <path> --name <project>`
- **THEN** 输出 project_id/files_indexed/files_skipped/nodes_created/edges_created/duration_ms

#### Scenario: 异常退出码
- **WHEN** 路径不存在
- **THEN** 提示"路径不存在"，退出码 1

### Requirement: 可选嵌入 Feature
系统 SHALL 将嵌入功能作为可选 feature（ADR-004，`[feature=embed]`），通过 reqwest 调用外部 HTTP 嵌入服务（OpenAI 兼容 API），API Key 通过环境变量传入不持久化，向量存储于 LadybugDB Embedding 表（FLOAT[384]），Windows 降级为仅 BM25。

#### Scenario: 嵌入生成
- **GIVEN** 启用 embed feature 且配置嵌入服务
- **WHEN** 索引代码库
- **THEN** 生成嵌入向量并存储

#### Scenario: 嵌入服务不可用降级
- **WHEN** 嵌入服务不可用
- **THEN** 跳过嵌入生成，继续索引

### Requirement: Skill 文件
系统 SHALL 创建 Skill 文件指导 Agent 使用 CLI 命令（index/query/trace/impact/search/daemon/status/list/clean）。

#### Scenario: Agent 可按 Skill 使用
- **WHEN** Agent 读取 Skill 文件
- **THEN** 能正确执行索引、查询、追踪等操作

### Requirement: 测试驱动开发与覆盖率
系统 SHALL 遵循 TDD 流程（先写测试再写实现），代码测试覆盖率 ≥ 95%（高于文档 90% 基线，依据用户要求）。IO 层使用 tempfile，核心逻辑优先覆盖。

#### Scenario: 覆盖率达标
- **WHEN** 执行 `cargo tarpaulin --fail-under 95`
- **THEN** 覆盖率 ≥ 95%，构建通过

### Requirement: 设计模式应用
系统 SHALL 积极使用设计模式：
- 门面模式（Facade）：IndexFacade/QueryFacade/TraceFacade 封装子系统复杂交互，简化 CLI 调用
- 适配器模式（Adapter）：Extractor trait 适配各语言 tree-sitter 解析到统一接口
- 策略模式（Strategy）：搜索策略（BM25/Semantic/Hybrid RRF）可切换
- 工厂模式（Factory）：ParserFactory 按语言创建 tree-sitter Parser
- 建造者模式（Builder）：Node/Edge/Graph 构造
- 观察者模式（Observer）：守护模式文件变更事件订阅
- 仓储模式（Repository）：StorageRepository 抽象 LadybugDB 数据访问

#### Scenario: 门面模式简化调用
- **WHEN** CLI 调用 IndexFacade::index()
- **THEN** 内部编排 discover/parse/resolve/storage，CLI 无需感知子系统

#### Scenario: 适配器模式扩展语言
- **WHEN** 新增语言提取器
- **THEN** 仅实现 Extractor trait，无需修改解析编排（TC-003 可扩展性）

## MODIFIED Requirements

### Requirement: 实施阶段顺序
依据 ADD §11 实施路线图，阶段顺序为：Phase 1 基础框架 → Phase 2 文件发现+解析 → Phase 3 符号解析+追踪+跨语言 → Phase 4 索引流水线+存储 → Phase 5 查询追踪+CLI → Phase 6 守护模式+嵌入 → Phase 7 测试套件+覆盖率 → Phase 9 Skill 创建（Phase 8 文档已完成）。TDD 要求每个阶段先写测试。
