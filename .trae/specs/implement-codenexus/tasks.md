# Tasks

## Phase 1: 基础框架（数据模型 + LadybugDB 集成）

- [x] Task 1: 初始化 Rust 项目脚手架与 Cargo.toml
  - [x] SubTask 1.1: 创建 Cargo.toml，锁定依赖版本（Rust 1.81+，lbug 0.17，tree-sitter 0.22，rayon 1，ignore 0.4，notify 6 + notify-debouncer-full 0.7，clap 4，sha2，reqwest 0.12，thiserror，anyhow，tracing，serde，csv 1），定义 features（lsp/embed）
  - [x] SubTask 1.2: 创建 src/lib.rs 与 src/main.rs，模块声明（model/discover/parse/resolve/storage/index/embed/daemon/trace/cli/query）
  - [x] SubTask 1.3: 配置 rustfmt.toml 与 clippy（-D warnings），验证 `cargo build` 与 `cargo clippy` 通过
  - [x] SubTask 1.4: 编写脚手架测试验证框架可测试

- [x] Task 2: 实现数据模型层（model 模块）
  - [x] SubTask 2.1: 编写 NodeLabel 枚举测试（20 种节点类型，DDD §7.1），再实现枚举与 Display/FromStr
  - [x] SubTask 2.2: 编写 EdgeType 枚举测试（14 种边类型，DDD §7.2），再实现枚举与 Display/FromStr
  - [x] SubTask 2.3: 编写 Language 枚举测试（5 种语言 + 扩展名映射，DDD §7.3），再实现枚举与扩展名检测
  - [x] SubTask 2.4: 编写 FlowType 枚举测试（4 种数据流类型，DDD §7.4），再实现枚举
  - [x] SubTask 2.5: 编写 Node 结构测试（字段依据 ADD §3.4 与 DDD §5），再实现 Node 结构（建造者模式）
  - [x] SubTask 2.6: 编写 Edge 结构测试，再实现 Edge 结构
  - [x] SubTask 2.7: 编写 Graph 内存图测试（add_node/add_edge/get_node/neighbors），再实现 Graph 结构
  - [x] SubTask 2.8: 编写 NodeId/UUIDv7 生成测试，再实现 ID 生成

- [x] Task 3: 实现 LadybugDB 存储层（storage 模块）
  - [x] SubTask 3.1: 编写 storage/schema 测试（DDL 字符串生成，依据 DDD §12.1），再实现 schema 模块生成 20 节点表 + CodeRelation + Embedding DDL
  - [x] SubTask 3.2: 编写 storage/connection 测试（连接管理、初始化建表、索引创建 DDD §12.2），再实现 Connection 封装 lbug crate
  - [x] SubTask 3.3: 编写 storage/loader 测试（CSV 生成用 csv crate ADR-014、COPY 批量加载），再实现 Loader
  - [x] SubTask 3.4: 编写 storage/query 测试（Cypher 执行、结果解析），再实现 Query 模块
  - [x] SubTask 3.5: 编写 storage/repository 测试（仓储模式，CRUD 抽象），再实现 Repository
  - [x] SubTask 3.6: 编写多项目隔离测试（BR-INDEX-004），验证 project 属性过滤

## Phase 2: 文件发现 + tree-sitter 解析

- [x] Task 4: 实现文件发现（discover 模块）
  - [x] SubTask 4.1: 编写 discover 测试（ignore crate 集成 ADR-012、.gitignore/.codenexusignore 遵循、ALWAYS_SKIP_DIRS 硬编码 BR-INDEX-006），再实现 Walker
  - [x] SubTask 4.2: 编写语言检测测试（扩展名映射 DDD §7.3），再实现语言检测
  - [x] SubTask 4.3: 编写 AC-INDEX-004 测试（.gitignore target/ 跳过），验证通过

- [x] Task 5: 实现 tree-sitter 解析基础（parse 模块）
  - [x] SubTask 5.1: 编写 ParserPool 测试（线程局部复用 ADR-010），再实现 ParserPool
  - [x] SubTask 5.2: 编写 ParserFactory 测试（工厂模式，按语言创建 Parser），再实现 ParserFactory
  - [x] SubTask 5.3: 编写 Extractor trait 测试（适配器模式，定义统一接口），再实现 Extractor trait 与 ExtractResult 结构

- [x] Task 6: 实现各语言提取器（适配器模式）
  - [x] SubTask 6.1: 编写 C 提取器测试（函数定义/调用/#include/typedef/全局变量），再实现 parse/c 提取器
  - [x] SubTask 6.2: 编写 Rust 提取器测试（fn/struct/enum/trait/impl/extern "C"/use），再实现 parse/rust 提取器
  - [x] SubTask 6.3: 编写 Fortran 提取器测试（subroutine/function/module/ISO_C_BINDING/call），再实现 parse/fortran 提取器
  - [x] SubTask 6.4: 编写 Python 提取器测试（def/class/import/__init__.py），再实现 parse/python 提取器
  - [x] SubTask 6.5: 编写 TypeScript 提取器测试（function/class/import/export），再实现 parse/typescript 提取器

- [ ] Task 7: 实现并行解析（parse 模块）
  - [ ] SubTask 7.1: 编写并行解析测试（rayon 文件级并行、无锁合并 ADR-010），再实现 parallel_parse

## Phase 3: 符号解析 + 变量追踪 + 跨语言

- [ ] Task 8: 实现符号解析基础（resolve 模块）
  - [ ] SubTask 8.1: 编写 FQN 生成测试（project.dir.file.entity 格式 ADD §7.1、Python __init__.py 特殊处理、Fortran 模块嵌套），再实现 FQN 生成
  - [ ] SubTask 8.2: 编写作用域链测试（resolve/scope），再实现 ScopeChain
  - [ ] SubTask 8.3: 编写符号表测试（文件级 + 项目级 resolve/symbol_table），再实现 SymbolTable

- [ ] Task 9: 实现调用关系与数据流解析（resolve 模块）
  - [ ] SubTask 9.1: 编写调用解析测试（receiver-bound-calls + free-call-fallback 通用 passes ADR-011、BR-TRACE-007 同语言调用），再实现 resolve/calls
  - [ ] SubTask 9.2: 编写数据流测试（BR-TRACE-001 参数传递、BR-TRACE-002 返回赋值、BR-TRACE-003 变量赋值、BR-TRACE-005 读取、BR-TRACE-006 写入），再实现 resolve/dataflow
  - [ ] SubTask 9.3: 编写 AC-TRACE-001 测试（A 调用 B 返回 A→B 路径），验证通过
  - [ ] SubTask 9.4: 编写 AC-TRACE-002 测试（变量 x 传递给 foo 参数返回数据流路径），验证通过

- [ ] Task 10: 实现跨语言 FFI 解析（resolve/cross_lang 模块）
  - [ ] SubTask 10.1: 编写 FFI 解析测试（Rust extern "C" 调 C、C↔Fortran ISO_C_BINDING、名称匹配 + 签名匹配双策略 ADD §7.4、置信度 0.70-0.85 BR-TRACE-008），再实现 cross_lang
  - [ ] SubTask 10.2: 编写 AC-TRACE-003 测试（Rust extern "C" 调 C 返回 FfiCalls 边路径），验证通过

## Phase 4: 索引流水线 + 存储

- [ ] Task 11: 实现索引流水线（index 模块，门面模式）
  - [ ] SubTask 11.1: 编写 hash 测试（SHA-256 文件哈希 ADR-009），再实现 index/hash
  - [ ] SubTask 11.2: 编写增量索引测试（哈希 diff：changed/added/deleted、BR-INDEX-001 跳过、BR-INDEX-002 删除检测、BR-INDEX-003 --force），再实现 index/incremental
  - [ ] SubTask 11.3: 编写 AC-INDEX-002 测试（增量仅解析变更文件），验证通过
  - [ ] SubTask 11.4: 编写 AC-INDEX-005 测试（--force 全量重解析），验证通过
  - [ ] SubTask 11.5: 编写 Pipeline 测试（门面模式 IndexFacade，编排 discover→parse→resolve→storage），再实现 index/pipeline
  - [ ] SubTask 11.6: 编写 AC-INDEX-001 测试（C/Rust/Fortran 代码库端到端索引），验证通过
  - [ ] SubTask 11.7: 编写 AC-INDEX-003 测试（多项目共存互不干扰），验证通过
  - [ ] SubTask 11.8: 编写异常处理测试（路径不存在退出码 1、数据库锁定重试 3 次退出码 2、解析失败跳过继续、内存不足退出码 3、数据库损坏退出码 4），再实现异常处理

## Phase 5: 查询追踪 + CLI

- [ ] Task 12: 实现追踪引擎（trace 模块）
  - [ ] SubTask 12.1: 编写调用图遍历测试（BFS Calls/FfiCalls 边、深度限制 AC-TRACE-004），再实现 trace/call_graph
  - [ ] SubTask 12.2: 编写数据流遍历测试（BFS DataFlows/Reads/Writes 边），再实现 trace/data_flow
  - [ ] SubTask 12.3: 编写影响分析测试（变更符号爆炸半径 P1），再实现 trace/impact
  - [ ] SubTask 12.4: 编写 TraceFacade 测试（门面模式，--type calls/dataflow/all），再实现 TraceFacade

- [ ] Task 13: 实现查询引擎（query 模块）
  - [ ] SubTask 13.1: 编写 Cypher 查询测试（AC-QUERY-001），再实现 query/cypher
  - [ ] SubTask 13.2: 编写结构化搜索测试（按名称/类型/文件 AC-SEARCH-001），再实现 query/structured
  - [ ] SubTask 13.3: 编写全文搜索测试（BM25 LadybugDB FTS），再实现 query/fulltext
  - [ ] SubTask 13.4: 编写 QueryFacade 测试（门面模式），再实现 QueryFacade

- [ ] Task 14: 实现 CLI 工具（cli 模块）
  - [ ] SubTask 14.1: 编写 index 命令测试（输入输出 PRD §4.1.3、退出码），再实现 cli/index_cmd
  - [ ] SubTask 14.2: 编写 query 命令测试，再实现 cli/query_cmd
  - [ ] SubTask 14.3: 编写 trace 命令测试（输入输出 PRD §4.2.3），再实现 cli/trace_cmd
  - [ ] SubTask 14.4: 编写 impact 命令测试，再实现 cli/impact_cmd
  - [ ] SubTask 14.5: 编写 search 命令测试（--semantic/--limit），再实现 cli/search_cmd
  - [ ] SubTask 14.6: 编写 status 命令测试，再实现 cli/status_cmd
  - [ ] SubTask 14.7: 编写 list 命令测试，再实现 cli/list_cmd
  - [ ] SubTask 14.8: 编写 clean 命令测试，再实现 cli/clean_cmd
  - [ ] SubTask 14.9: 编写 CLI 入口测试（clap 4 子命令路由、退出码 0/1/2/3/4），再实现 cli/main

## Phase 6: 守护模式 + 可选嵌入

- [ ] Task 15: 实现守护模式（daemon 模块，观察者模式）
  - [ ] SubTask 15.1: 编写守护模式测试（notify-debouncer-full ADR-013、防抖默认 2000ms BR-DAEMON-001、可配置 --debounce-ms BR-DAEMON-004、代码文件过滤 BR-DAEMON-002、索引期间暂停 BR-DAEMON-003），再实现 daemon
  - [ ] SubTask 15.2: 编写 AC-DAEMON-001 测试（修改代码文件 2s 后触发增量索引），验证通过
  - [ ] SubTask 15.3: 编写 AC-DAEMON-002 测试（连续修改合并为一次索引），验证通过
  - [ ] SubTask 15.4: 编写 AC-DAEMON-003 测试（非代码文件不触发），验证通过
  - [ ] SubTask 15.5: 编写 daemon 命令测试，再实现 cli/daemon_cmd

- [ ] Task 16: 实现可选嵌入（embed 模块，feature gate，策略模式）
  - [ ] SubTask 16.1: 编写嵌入服务客户端测试（reqwest HTTP 调用 OpenAI 兼容 API、API Key 环境变量不持久化），再实现 embed/client
  - [ ] SubTask 16.2: 编写向量存储测试（LadybugDB Embedding 表 FLOAT[384]），再实现 embed/storage
  - [ ] SubTask 16.3: 编写语义搜索测试（向量搜索 + RRF 融合 AC-SEARCH-002、Windows 降级仅 BM25），再实现 embed/search（策略模式）
  - [ ] SubTask 16.4: 编写嵌入服务不可用降级测试，验证跳过嵌入继续索引

## Phase 7: 测试套件 + 覆盖率

- [ ] Task 17: 完善测试套件与覆盖率
  - [ ] SubTask 17.1: 编写集成测试（tests/ 目录，端到端索引真实代码库、查询、追踪全流程）
  - [ ] SubTask 17.2: 补充单元测试覆盖率达 95%（IO 层用 tempfile，核心逻辑优先覆盖 TR-006）
  - [ ] SubTask 17.3: 运行 `cargo tarpaulin --fail-under 95` 验证覆盖率达标
  - [ ] SubTask 17.4: 运行 `cargo clippy -- -D warnings` 验证无警告
  - [ ] SubTask 17.5: 运行 `cargo test` 验证全部测试通过

## Phase 9: Skill 创建

- [ ] Task 18: 创建 Skill 文件
  - [ ] SubTask 18.1: 创建 skill/SKILL.md，指导 Agent 使用 CLI 命令（index/query/trace/impact/search/daemon/status/list/clean）
  - [ ] SubTask 18.2: 验证 Agent 可按 Skill 文件正确执行索引、查询、追踪操作

# Task Dependencies

- Task 2（数据模型）依赖 Task 1（脚手架）
- Task 3（存储层）依赖 Task 2（数据模型）
- Task 4（文件发现）依赖 Task 2（数据模型）
- Task 5（解析基础）依赖 Task 2（数据模型）
- Task 6（语言提取器）依赖 Task 5（解析基础）
- Task 7（并行解析）依赖 Task 6（语言提取器）
- Task 8（符号解析基础）依赖 Task 7（并行解析）
- Task 9（调用与数据流）依赖 Task 8（符号解析基础）
- Task 10（跨语言 FFI）依赖 Task 9（调用与数据流）
- Task 11（索引流水线）依赖 Task 3（存储层）、Task 4（文件发现）、Task 10（跨语言 FFI）
- Task 12（追踪引擎）依赖 Task 11（索引流水线）
- Task 13（查询引擎）依赖 Task 3（存储层）
- Task 14（CLI）依赖 Task 11、Task 12、Task 13
- Task 15（守护模式）依赖 Task 11（索引流水线）、Task 14（CLI）
- Task 16（嵌入）依赖 Task 3（存储层）、Task 13（查询引擎）
- Task 17（测试套件）依赖 Task 14、Task 15、Task 16
- Task 18（Skill）依赖 Task 14（CLI）

# 可并行任务

- Task 4（文件发现）与 Task 5（解析基础）可与 Task 3（存储层）并行（均仅依赖 Task 2）
- Task 13（查询引擎）可与 Task 12（追踪引擎）并行（均依赖 Task 11/Task 3）
- Task 16（嵌入）可与 Task 15（守护模式）并行（分别依赖 Task 13/Task 11）
