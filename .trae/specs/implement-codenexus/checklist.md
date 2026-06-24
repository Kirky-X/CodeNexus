# CodeNexus 实现验证清单

## 项目脚手架与构建

- [x] Cargo.toml 锁定依赖版本，Rust 1.81+，features 定义 lsp/embed（ADR-001/ADR-004）
- [x] 单 crate 多 mod 结构，模块声明齐全（model/discover/parse/resolve/storage/index/embed/daemon/trace/cli/query）
- [x] `cargo build` 成功编译，无警告
- [x] `cargo clippy -- -D warnings` 无 clippy 警告（TRD §7.1）
- [x] rustfmt 配置就绪

## 数据模型层（DDD §4-5、ADD §3.4）

- [x] NodeLabel 枚举包含 20 种节点类型（Project/Folder/File/Module/Class/Struct/Enum/Trait/Impl/Function/Method/Variable/GlobalVar/Parameter/Const/Static/Macro/TypeAlias/Typedef/Namespace）
- [x] EdgeType 枚举包含 14 种边类型（CONTAINS/DEFINES/MEMBER_OF/CALLS/FFI_CALLS/DATAFLOWS/READS/WRITES/IMPLEMENTS/EXTENDS/USES_TYPE/REFERENCES/IMPORTS/INCLUDES）
- [x] Language 枚举支持 5 种语言及扩展名映射（c:.c/.h, rust:.rs, fortran:.f90/.f/.f95, python:.py, typescript:.ts/.tsx）
- [x] FlowType 枚举包含 4 种数据流类型（ArgPass/ReturnAssign/AssignFrom/AssignTo）
- [x] Node 结构字段与 DDD §5 一致（id/project/name/qualifiedName/filePath/startLine/endLine/language/signature/returnType/docstring/isExported/isGlobal/parentQn/properties）
- [x] Edge 结构字段与 DDD §5.8 一致（source/target/edge_type/confidence/reason/startLine）
- [x] Graph 内存图支持 add_node/add_edge/get_node/neighbors（按边类型过滤）
- [x] NodeId 使用 UUIDv7 字符串生成
- [x] 建造者模式用于 Node/Edge 构造

## LadybugDB 存储层（DDD §12、ADR-002/ADR-007/ADR-014）

- [x] schema 模块生成 20 张节点表 DDL（严格遵循 DDD §12.1）
- [x] schema 模块生成 CodeRelation 关系表 DDL（单一 REL TABLE，type 属性区分）
- [x] schema 模块生成可选 Embedding 表 DDL（FLOAT[384]）
- [x] Connection 封装 lbug crate，支持连接与初始化建表
- [x] 索引创建语句齐全（DDD §12.2：节点表索引、关系表索引、嵌入表索引、FTS 索引）
- [x] Loader 使用 csv crate 生成 RFC 4180 合规 CSV（ADR-014）
- [x] Loader 支持 COPY FROM CSV 批量加载
- [x] Query 模块支持 Cypher 执行与结果解析
- [x] Repository 仓储模式抽象数据访问
- [x] 多项目隔离通过 project 属性过滤（BR-INDEX-004）

## 文件发现（ADR-012、BR-INDEX-005/BR-INDEX-006）

- [x] 使用 ignore crate（ripgrep 同源）实现文件发现
- [x] 遵循 .gitignore 规则（AC-INDEX-004：target/ 被跳过）
- [x] 遵循 .codenexusignore 规则
- [x] ALWAYS_SKIP_DIRS 硬编码跳过（.git/target/node_modules 等，BR-INDEX-006）
- [x] 语言检测通过扩展名映射（DDD §7.3）

## tree-sitter 多语言解析（ADR-003/ADR-010、TC-003）

- [x] ParserPool 线程局部复用 parser（ADR-010）
- [x] ParserFactory 工厂模式按语言创建 Parser
- [x] Extractor trait 适配器模式定义统一接口
- [x] C 提取器：函数定义/调用/#include/typedef/全局变量
- [x] Rust 提取器：fn/struct/enum/trait/impl/extern "C"/use
- [x] Fortran 提取器：subroutine/function/module/ISO_C_BINDING/call
- [x] Python 提取器：def/class/import/__init__.py
- [x] TypeScript 提取器：function/class/import/export
- [x] rayon 文件级并行解析，无锁合并（ADR-010）
- [x] 新增语言仅实现 Extractor trait（TC-003 可扩展性）

## 符号解析与追踪（ADR-011、ADD §7.1/§7.4、BR-TRACE-001~008）

- [x] FQN 生成格式 project.dir.file.entity（ADD §7.1）
- [x] Python __init__.py 去末尾段特殊处理
- [x] Fortran 模块嵌套特殊处理
- [x] 作用域链（ScopeChain）实现
- [x] 文件级 + 项目级符号表（SymbolTable）实现
- [x] 调用解析通用 passes（receiver-bound-calls + free-call-fallback，ADR-011）
- [x] 同语言调用 CALLS 边（BR-TRACE-007）
- [x] 数据流 DATAFLOWS 边：参数传递（BR-TRACE-001）、返回赋值（BR-TRACE-002）、变量赋值（BR-TRACE-003）、函数赋值（BR-TRACE-004）
- [x] READS 边：函数读取变量（BR-TRACE-005）
- [x] WRITES 边：函数写入变量（BR-TRACE-006）
- [x] 跨语言 FFI_CALLS 边：名称匹配 + 签名匹配双策略（BR-TRACE-008、ADD §7.4）
- [x] FFI 置信度：签名匹配 0.85、仅名称匹配 0.70
- [x] AC-TRACE-001 通过（A 调用 B 返回 A→B 路径）
- [x] AC-TRACE-002 通过（变量 x 传递给 foo 参数返回数据流路径）
- [x] AC-TRACE-003 通过（Rust extern "C" 调 C 返回 FfiCalls 边路径）
- [x] AC-TRACE-004 通过（--depth 2 路径深度不超过 2）

## 增量索引（ADR-009、BR-INDEX-001~003）

- [x] SHA-256 文件哈希计算（ADR-009）
- [x] 哈希一致跳过（BR-INDEX-001）
- [x] 文件删除检测：删除节点与关联边（BR-INDEX-002）
- [x] --force 忽略哈希全量重解析（BR-INDEX-003）
- [x] AC-INDEX-001 通过（C/Rust/Fortran 代码库端到端索引）
- [x] AC-INDEX-002 通过（增量仅解析变更文件）
- [x] AC-INDEX-003 通过（多项目共存互不干扰）
- [x] AC-INDEX-004 通过（.gitignore target/ 跳过）
- [x] AC-INDEX-005 通过（--force 全量重解析）
- [x] 异常处理：路径不存在退出码 1、数据库锁定重试 3 次退出码 2、解析失败跳过继续、内存不足退出码 3、数据库损坏退出码 4

## 索引流水线（门面模式）

- [x] IndexFacade 门面模式封装 discover→parse→resolve→storage 编排
- [x] Pipeline 输出 project_id/files_indexed/files_skipped/nodes_created/edges_created/duration_ms（PRD §4.1.3）

## 追踪引擎

- [x] 调用图 BFS 遍历 Calls/FfiCalls 边，深度限制
- [x] 数据流 BFS 遍历 DataFlows/Reads/Writes 边
- [x] 影响分析（变更符号爆炸半径，P1）
- [x] TraceFacade 门面模式（--type calls/dataflow/all）
- [x] trace 输出 paths[].nodes/paths[].edges/paths[].depth（PRD §4.2.3）

## 查询引擎

- [x] Cypher 查询（AC-QUERY-001 通过）
- [x] 结构化搜索（按名称/类型/文件，AC-SEARCH-001 通过）
- [x] BM25 全文搜索（LadybugDB FTS 扩展）
- [x] QueryFacade 门面模式

## CLI 工具（clap 4）

- [x] index 命令（输入输出 PRD §4.1.3）
- [x] query 命令（Cypher 查询）
- [x] trace 命令（输入输出 PRD §4.2.3）
- [x] impact 命令（影响分析）
- [x] search 命令（--semantic/--limit）
- [x] daemon 命令（守护模式）
- [x] status 命令（索引状态）
- [x] list 命令（列出已索引项目）
- [x] clean 命令（清理项目索引）
- [x] 退出码：0 成功、1 输入异常、2 数据库锁定、3 系统异常、4 数据库损坏

## 守护模式（ADR-013、BR-DAEMON-001~004、观察者模式）

- [x] notify-debouncer-full 文件监视 + 防抖（ADR-013）
- [x] 防抖默认 2000ms（BR-DAEMON-001）
- [x] 可配置 --debounce-ms（BR-DAEMON-004）
- [x] 代码文件过滤（BR-DAEMON-002）
- [x] 索引期间暂停事件处理（BR-DAEMON-003）
- [x] AC-DAEMON-001 通过（修改代码文件 2s 后触发增量索引）
- [x] AC-DAEMON-002 通过（连续修改合并为一次索引）
- [x] AC-DAEMON-003 通过（非代码文件不触发）
- [x] 观察者模式用于文件变更事件订阅

## 可选嵌入（ADR-004、策略模式）

- [x] embed feature gate（ADR-004）
- [x] reqwest HTTP 调用外部嵌入服务（OpenAI 兼容 API）
- [x] API Key 环境变量传入不持久化（TRD §6.1）
- [x] 向量存储 LadybugDB Embedding 表 FLOAT[384]
- [x] 语义搜索向量 + RRF 融合（AC-SEARCH-002 通过）
- [x] Windows 降级仅 BM25（R-003/TR-005）
- [x] 嵌入服务不可用降级跳过
- [x] 策略模式用于搜索策略切换（BM25/Semantic/Hybrid）

## 设计模式应用

- [x] 门面模式（Facade）：IndexFacade/QueryFacade/TraceFacade
- [x] 适配器模式（Adapter）：Extractor trait 各语言适配
- [x] 策略模式（Strategy）：搜索策略切换
- [x] 工厂模式（Factory）：ParserFactory
- [x] 建造者模式（Builder）：Node/Edge/Graph 构造
- [x] 观察者模式（Observer）：守护模式文件变更
- [x] 仓储模式（Repository）：StorageRepository

## 测试驱动开发与覆盖率（用户要求 ≥ 95%）

- [x] TDD 流程：每个任务先写测试再写实现
- [x] 单元测试覆盖所有模块
- [x] 集成测试覆盖端到端流程（tests/ 目录）
- [x] IO 层使用 tempfile（TR-006）
- [x] `cargo tarpaulin --fail-under 95` 覆盖率 ≥ 95%
- [x] `cargo test` 全部测试通过（1119 测试：1102 单元 + 15 集成 + 2 文档）
- [x] `cargo clippy -- -D warnings` 无警告

## Skill 文件

- [x] skill/SKILL.md 创建，指导 Agent 使用 CLI 九个命令
- [x] Agent 可按 Skill 文件正确执行索引、查询、追踪操作

## 文档合规性（禁止偏离）

- [x] 实现严格遵循 PRD.md 功能清单与验收标准
- [x] 实现严格遵循 TRD.md 技术选型与性能指标
- [x] 实现严格遵循 ADD.md 架构与 ADR 决策
- [x] 实现严格遵循 DDD.md 图模式与 DDL
- [x] 所有 ADR（001-014）决策已落实
