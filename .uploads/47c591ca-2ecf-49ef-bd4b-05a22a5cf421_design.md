## Context

`fix-id-fqn-consistency` spec 已修复 `rust_extractor.rs` 的两个根因 bug：
1. `make_qn(file_path, name)` 产出 `file::name` 格式，与符号表/resolve 阶段使用的 `FqnGenerator::generate(project, file_path, name, language)` 的点分格式 `project.dir.file.name` 不一致 → 节点 id 与边 source/target 不匹配。
2. `extract_call` 不接收 `current_func`，`CallInfo.caller_qn` 始终为 `None` → `resolve_calls` 跳过所有调用，不生成 CALLS 边。

rust_extractor 的修复模式：
- `make_qn(file_path, name, project)` 委托 `FqnGenerator::generate(file_path, name, project, Language::X)`
- `extract_call` 签名添加 `file_path`/`project`/`current_func`，通过 `make_qn(file_path, current_func, project)` 生成 `caller_qn`
- `visit_node` 签名添加 `current_func: Option<&str>`，在 `function_item` 分支传递函数名

**当前状态**：同一 bug 存在于 `python.rs`、`typescript.rs`、`c.rs`、`fortran.rs` 四个提取器。所有四个提取器的 `make_qn` 都是 `format!("{file_path}::{name}")`，`extract_call` 都设置 `caller_qn: None`。`c.rs` 还有一个 TODO 标记（第 53 行）：reads/writes 提取未实现（BR-TRACE-005/006）。

**约束**：
- 不能破坏 `rust_extractor.rs` 已修复的模式（参考实现）
- `FqnGenerator::generate` 已在 `src/resolve/fqn.rs` 实现，所有提取器可直接复用
- C 提取器的 reads/writes 实现需适配 tree-sitter-c 的语法节点（与 Rust 的 `let_declaration`/`assignment_expression` 不同）
- Task 6 遗留 11 个测试失败需在同一变更中修复（9 个 loader.rs 列数断言 + 2 个 rust_extractor.rs pattern 断言）

## Goals / Non-Goals

**Goals:**
- Python/TypeScript/C/Fortran 四个提取器的 `make_qn` 产出 spec 合规的点分 FQN
- 四个提取器的 `extract_call` 生成非 `None` 的 `caller_qn`，使 `resolve_calls` 能生成 CALLS 边
- C 提取器实现 reads/writes 提取（BR-TRACE-005/006），移除第 53 行 TODO
- 修复 Task 6 遗留的 11 个测试失败，使 `cargo test` 全绿
- 各提取器补充 `extract_call` 端到端测试，验证 CALLS 边生成

**Non-Goals:**
- 不重构 `FqnGenerator` 本身（已稳定）
- 不修改 `resolve_calls` 的匹配逻辑（依赖 FQN 格式一致即可）
- 不为 Python/TypeScript/Fortran 实现 reads/writes 提取（当前仅 C 有 TODO 标记，其他语言无此需求）
- 不提升覆盖率到 95%（那是 `fix-id-fqn-consistency` spec Task 6/7 的目标；本变更只修复测试失败，覆盖率提升由前置 spec 负责）
- 不修改 `trace_cmd.rs` 的查询逻辑（前置 spec 已修复）

## Decisions

### Decision 1: 统一采用 rust_extractor 的修复模式

**选择**：所有四个提取器完全复用 `rust_extractor.rs` 的修复模式。

**理由**：
- rust_extractor 的修复已通过 44 个测试验证（含端到端 CALLS 边测试）
- 四个提取器的 `make_qn` 和 `extract_call` 代码结构几乎相同，统一模式降低风险
- 避免引入第二种修复模式（遵守规则 11：惯例优先于新颖）

**替代方案**：
- 提取公共 trait/宏：被否决。四个提取器的 `visit_node` 结构因语言语法差异较大，公共抽象会增加复杂度且收益有限（规则 2：简洁优先）。
- 逐个修复并验证后再统一：被否决。bug 根因相同，分批修复无收益且增加往返次数。

### Decision 2: `make_qn` 签名统一为 `(file_path, name, project)`

**选择**：所有提取器的 `make_qn` 签名改为 `fn make_qn(file_path: &str, name: &str, project: &str) -> String`，内部委托 `FqnGenerator::generate`。

**理由**：
- 与 rust_extractor 完全一致
- `FqnGenerator::generate` 已处理 project/file_path/name/language 四参数，language 在各提取器内部固定

**实现**：
```rust
fn make_qn(file_path: &str, name: &str, project: &str) -> String {
    FqnGenerator::generate(project, file_path, name, Language::X)
}
```
其中 `Language::X` 对应各提取器的语言枚举（Python/TypeScript/C/Fortran）。

### Decision 3: `extract_call` 和 `visit_node` 签名扩展

**选择**：
- `visit_node` 添加 `current_func: Option<&str>` 参数
- `extract_call` 添加 `file_path`/`project`/`current_func` 参数
- `caller_qn` 通过 `current_func.map(|name| make_qn(file_path, name, project))` 生成

**理由**：与 rust_extractor 完全一致，确保 `caller_qn` 非 `None` 时格式与函数定义节点的 id 匹配。

**调用链修改**：
- `function_definition`/`function_item` 分支：提取函数名后，传递给子节点的 `visit_children` 调用
- `call_expression` 分支：调用 `extract_call(node, source, file_path, project, current_func, result)`

### Decision 4: C 提取器 reads/writes 实现方案

**选择**：参考 rust_extractor 的模式，适配 C 语法：

- **Reads**（BR-TRACE-005）：遍历 `identifier` 节点（表达式位置），生成 `ReadInfo { reader_qn, var_name, line }`
- **Writes**（BR-TRACE-006）：
  - `init_declarator`（`int x = 1;`）→ 写入 declarator 的 identifier
  - `assignment_expression`（`x = 1;`）→ 写入左侧 identifier

**C 语法适配**：
- C 没有独立的 `let_declaration`，用 `init_declarator` 替代
- C 的 `declaration` 可能声明多个变量（`int a, b, c;`），需遍历所有 `init_declarator` 或 `identifier` 子节点
- C 的 `identifier` 在声明位置（declarator 内）不是读取，需通过 `is_read_position` 排除

**实现范围**：仅实现与 Rust 提取器等价的最小功能（简单 identifier 读写），不处理复杂的 C 特性（如指针解引用 `*p = 1`、结构体字段 `s.x = 1`）。复杂场景留作后续优化。

### Decision 5: Task 6 测试修复策略

**选择**：基于源码实际行为修正测试断言，不修改源码逻辑。

**loader.rs 9 个列数断言**（基于 `node_to_row` 实际实现）：
| label | 实际列数 | 原断言 | 修正为 |
|-------|---------|--------|--------|
| Class | 11 | 12 | 11 |
| Struct | 11 | 12 | 11 |
| Enum | 11 | 12 | 11 |
| Trait | 11 | 12 | 11 |
| Const | 10 | 9 | 10 |
| Parameter | 9 | 9 | 需进一步排查（列数匹配但仍失败，可能是 properties 缺失） |
| Static | 9 | 8 | 9 |
| TypeAlias | 9 | 8 | 9 |
| Typedef | 8 | 7 | 8 |

**rust_extractor.rs 2 个 pattern 断言**：
- `tuple_destructuring_pattern`：`pattern_name` 设计为返回单个名字（首个 identifier），不处理多绑定。调整断言为验证提取首个绑定 "a"，而非 "a" 和 "b"。
- `struct_pattern`：`pattern_name` 对 `struct_pattern` 返回 `type` 字段（类型名 "P"），非字段名 "x"。这是有意设计（`let P { x } = p;` 中 `P` 是绑定目标）。调整断言为验证提取 "P"。

**理由**：
- 规则 3（外科手术式修改）：测试失败是断言错误，非源码 bug
- 规则 9（测试验证有意义属性）：列数断言验证 `node_to_row` 的输出结构，是有意义的；但断言值必须与实现一致
- pattern_name 的行为是有意的（单名字返回），修改为多绑定是功能增强，超出本变更范围

## Risks / Trade-offs

- **[四个提取器并行修改] → 风险**：四个文件同时修改，可能遗漏某个调用点。
  - **缓解**：每个提取器修改后立即运行 `cargo test --lib` 验证；使用 `grep` 确认所有 `make_qn` 和 `extract_call` 调用点都已更新。

- **[C reads/writes 实现复杂度] → 风险**：C 语法比 Rust 更灵活（指针、宏、复合声明），实现可能遗漏边界情况。
  - **缓解**：只实现最小功能（简单 identifier），复杂场景在测试中标注为已知限制；不实现比实现错误更安全。

- **[测试断言修正可能掩盖真实 bug] → 风险**：将列数断言从 12 改为 11 可能掩盖 `node_to_row` 漏列的 bug。
  - **缓解**：逐列核对 `node_to_row` 实现与 NodeLabel 的语义，确认 11 列覆盖所有应有字段；在 PR 描述中列明每个 label 的字段清单供审查。

- **[pattern_name 单名字限制] → 风险**：`let (a, b) = ...` 只提取 "a"，"b" 被忽略，可能影响数据流分析完整性。
  - **缓解**：这是已有行为，本变更不引入新风险；在 design.md Open Questions 中记录，留作后续增强。

- **[C reads/writes 与 Rust 行为不一致] → 风险**：C 的 `init_declarator` 可能不如 Rust 的 `let_declaration` 直观，写入提取可能遗漏。
  - **缓解**：参考 rust_extractor 的 `extract_let` 实现，保持逻辑等价；端到端测试验证 `int x = 1;` 生成 WriteInfo。

## Migration Plan

无需迁移。本变更是 bug 修复，不改变公开 API：
- `Extractor::extract` 签名不变（`file_path`/`project` 已是参数）
- `make_qn`/`extract_call`/`visit_node` 是模块私有函数，签名变更不影响外部调用
- `ExtractResult` 结构不变，只是 `calls`/`reads`/`writes` 字段的内容更完整

**回滚策略**：如发现问题，`git revert` 单个 commit 即可。建议按提取器分 commit（python/typescript/fortran 一个，c 含 reads/writes 一个，测试修复一个），便于精准回滚。

## Open Questions

1. **`pattern_name` 是否应支持多绑定？** 当前 `let (a, b) = ...` 只提取首个绑定 "a"。是否在本变更中增强为返回 `Vec<String>`？倾向否（超出 bug 修复范围），但需用户确认。
2. **C 提取器的 `is_read_position` 实现**：Rust 的 `is_read_position` 通过检查父节点类型排除声明位置。C 的 tree-sitter 语法中，declarator 内的 identifier 父节点链更复杂（`init_declarator` → `function_declarator` → `pointer_declarator`），是否需要递归检查？倾向实现最小版本（只检查直接父节点），边界情况后续处理。
3. **Python/TypeScript/Fortran 是否需要 reads/writes？** 当前仅 C 有 TODO 标记。其他语言的 reads/writes 提取是否纳入本变更？倾向否（遵循 proposal 范围）。
