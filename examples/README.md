# CodeNexus Examples

示例程序集合，演示 CodeNexus 的核心功能。每个示例都是独立可运行的二进制文件。

## 运行方式

```bash
# 运行单个示例
cargo run --manifest-path examples/Cargo.toml --bin basic_indexing

# 运行所有示例
for bin in basic_indexing cypher_query symbol_search call_tracing impact_analysis export_import; do
  cargo run --manifest-path examples/Cargo.toml --bin $bin
done
```

## 示例列表

| 示例 | 功能 | 说明 |
|------|------|------|
| `basic_indexing` | 基础索引 | 索引 Rust 源码到知识图谱，查询函数列表 |
| `cypher_query` | Cypher 查询 | 对图谱执行多种 Cypher 查询（按类型、按名称） |
| `symbol_search` | 符号搜索 | 按名称、类型搜索符号，处理空结果 |
| `call_tracing` | 调用链追踪 | 正向追踪函数调用路径，构建调用图 |
| `impact_analysis` | 影响分析 | 分析修改某符号的影响半径（反向 BFS） |
| `export_import` | 导出/导入 | 图谱数据库的导出与导入验证 |

## 前提条件

- Rust 1.81+
- CodeNexus 默认 feature（`full`：C/Rust/Fortran/Python/TypeScript + daemon）

## 每个示例的工作原理

1. 创建临时目录作为工作区
2. 将内嵌的 Rust 源码写入临时文件
3. 通过 `IndexFacade` 索引源码到 LadybugDB
4. 通过 `QueryFacade` 执行查询/搜索
5. 通过 `TraceFacade` / `ImpactAnalyzer` 执行追踪/影响分析
6. 退出时临时目录自动清理

## 作为库使用

这些示例展示了如何以编程方式使用 `codenexus` 库：

```rust
use codenexus::index::IndexFacade;
use codenexus::query::QueryFacade;

// 索引源码
let indexer = IndexFacade::new(db_path)?;
let result = indexer.index(&source_dir, "my-project", true)?;

// 查询图谱
let query = QueryFacade::new(db_path)?;
let functions = query.cypher("MATCH (f:Function) RETURN f.name")?;

// 搜索符号
let results = query.search("parse", Some(&result.project_id), 10)?;
```

## 注意事项

示例使用 `IndexFacade` 和 `QueryFacade` 直接操作数据库，而非通过 Kit 注册表。
这是因为 Kit 在启动时为每个子系统创建独立的数据库连接，可能导致文件数据库的数据可见性问题。
直接使用 Facade 是推荐的编程方式。
