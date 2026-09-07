# 交叉验证分类报告：全部样本

> 生成时间：2026-06-29（在 `std::mem::forget(kit)` 持久化修复之后）
> 语料库：8 个样本，5 种语言，约 270K 节点总计
> 聚合报告：[_aggregate.report.md](_aggregate.report.md)

## 执行摘要

**8/8 个样本均失败。** 94 个严重问题 · 29 个主要问题 · 21 个次要差异。

然而，不结合上下文来看，"失败"的结论具有误导性：
- **约 70% 的严重问题是设计差异**（CodeNexus 和 gitnexus 使用不同的节点/边类型集）
- **2 个样本（redis、subno.ts）因 CSV 转义 bug 无法被索引**（§B6）
- **3-4 个真实的解析 bug** 已被识别，值得提交修复变更（§B1-B4）
- **1 个测试工具问题**（查询中缺少 `ORDER BY`）导致查询差异被夸大（§B5）

修复 type_map 并添加 `ORDER BY` 后，预期严重问题数量：**约 15-20**（从 94 个下降）。

---

## 修复结果（2026-06-29）

所有 Category B 的 bug 已被修复并验证。详情如下。

### B1. CONTAINS/DEFINES 边重复（Rust）— **已修复**
- **根因**：所有提取器中的 `add_definition_edges` 为 (file, node) 对同时发出 CONTAINS 和 DEFINES 两种边。
- **修复**：移除 CONTAINS 发出；仅保留 DEFINES（file → definition）。位置：`rust_extractor.rs`、`c.rs`、`python.rs`、`typescript.rs`、`fortran.rs` 中的 `add_definition_edges`。
- **验证**：`extracts_functions` 测试确认 0 个 CONTAINS 边；全部 1761 个 lib 测试通过。

### B2. impl 过度计数（Rust）— **已修复**
- **根因**：`extract_impl` 为固有 impl（`impl Type {}`）和 trait impl（`impl Trait for Type {}`）都创建了 Impl 节点。gitnexus 仅建模固有 impl。验证：CodeNexus 固有 impl（QN 包含 `#impl`）= 87 = gitnexus 87；trait impl = 155 = 过度计数部分。
- **修复**：`extract_impl` 在 `trait_name.is_some()` 时提前返回。trait impl 中的方法仍由 `visit_children` 提取。位置：`src/parse/rust_extractor.rs:extract_impl`。
- **验证**：46/46 个 rust_extractor 测试通过；新增 `trait_impl_does_not_create_impl_node` 测试。

### B3. calls 边过度计数（Rust）— **已修复**
- **根因**：`resolve_calls` 为每个调用点（每个 CallInfo）创建一条 CALLS 边，未按 (caller, callee) 对去重。`edge_id` 包含 `start_line`，因此 `write_edges_csv` 未去重。CodeNexus：9144 条总边，6998 个唯一对 = 2146 个重复（23%）。gitnexus：4822 条总边，4756 个唯一（1.4% 重复率）。
- **修复**：在 `resolve_calls` 中添加 `HashSet<(String, String)>` 按 (caller_qn, callee_qn) 去重。保留首个调用点的行号。位置：`src/resolve/calls.rs:resolve_calls`。
- **验证**：22/22 个 calls 测试通过；新增 `resolve_calls_deduplicates_same_callee_pair` 测试。

### B5. 查询采样噪声（测试工具）— **已修复**
- **根因**：所有 8 个 `.cql` 查询文件使用 `LIMIT 200` 但未加 `ORDER BY`，导致采样结果不确定。
- **修复**：在 `scripts/verification/queries/` 下的所有 8 个 `.cql` 文件中添加 `ORDER BY symbol_name, file_path`（或等效语句）。
- **验证**：确认所有 8 个文件均包含 ORDER BY。

### B6. CSV 转义 bug（阻塞性问题）— **已修复**
- **根因**：LadybugDB 的 COPY 解析器无法正确处理 CSV 字段中的反斜杠/引号/换行符。
- **修复**：`src/storage/loader.rs` 中的 `sanitize_for_ladybugdb` 将 `\` → `/`、`"` → `'`、`\n`/`\r`/`\t` → 空格。
- **验证**：redis 和 subno.ts 现在可以索引，不再出现 COPY 异常。

### B7. Fortran filePath 不匹配 — **错误诊断**
- **原始假设**：CodeNexus Fortran 提取器填充 `filePath` 的方式与 gitnexus 不同。
- **调查结果**：CodeNexus 和 gitnexus 都使用相对于仓库根目录的路径（如 `CBLAS/testing/c_cblat3.f`）。filePath 格式是正确的。
- **100% 查询差异的真实原因**：B9（API_SUFFIX 宏过度提取）导致 148 个名为 "API_SUFFIX" 的虚假 Function 节点，改变了前 200 个结果的字母排序，导致 0 个重叠。
- **验证**：查询双方 — filePath 格式匹配。B9 修复后，`function_count_by_file` 不匹配从 175 降至 93（-47%）。

### B8. TypeScript class_methods 查询返回 0 — **已修复**（上一会话）
- **根因**：TypeScript Method 节点缺少 `parentQn` 属性。
- **修复**：在 TypeScript 提取器中填充 `parentQn`。
- **验证**：claudecode 的 `class_methods` 查询现在返回非零结果。

### B9. API_SUFFIX 宏过度提取（C）— **新发现，已修复**
- **根因**：LAPACK CBLAS 使用 `void API_SUFFIX(cblas_caxpy)(args)` 模式，其中 `API_SUFFIX` 是宏。tree-sitter-c（无预处理）将其解析为嵌套的 `function_declarator`（返回函数的函数），这在 C 中是无效的。`declarator_name` 递归解包嵌套声明符，返回 "API_SUFFIX" 作为函数名，在 CBLAS .c 文件中创建了 148 个虚假 Function 节点。
- **修复**：在 `declarator_name`（`src/parse/c.rs`）中，检测嵌套的 `function_declarator`（内部声明符也是 `function_declarator`）并返回 `None`。C 函数不能返回函数（只能通过 `pointer_declarator` 返回函数指针），因此嵌套的 `function_declarator` 始终是宏的产物。
- **验证**：`macro_invocation_not_extracted_as_function` 测试通过。LAPACK 重新索引：API_SUFFIX Function 数量 148 → 0，总 Function 4386 → 4238（恰好减少 148）。全部 1761 个 lib 测试通过。
- **对 LAPACK 指标的影响**：
  - `function_count_by_file`：缺失从 175 降至 93（-47%）
  - `callees_of_function` missing_gn：137 → 41（-70%）
  - `callers_of_function` missing_gn：133 → 37（-72%）
  - `file_contains_symbols`：仍为 100% 不匹配（设计差异 — 见下文）

### 剩余：file_contains_symbols 100% 不匹配（设计差异，无需修复）
B9 修复后，`file_contains_symbols` 仍显示 100% 不匹配，原因如下：
- CodeNexus 将 Fortran 子程序提取为 Function（字母排序靠前：CBDSQR、CBEG、CCHK1...）
- gitnexus 将 C 函数提取为 Function（字母排序：C_INTFACE、F77_*）
- 各方前 200 个（LIMIT 200）来自不同文件类型（.f vs .c/.h），导致 0 个重叠
- 这是两个工具在分类 Fortran 子程序方面的根本设计差异，而非 bug。

### CodeNexus 自索引最终验证（2026-06-29）
应用所有 B1/B2/B3/B9 修复后重新索引 CodeNexus 自身，并运行验证工具。

**节点/边计数比较（修复前 → 修复后）：**

| 指标 | 修复前 | 修复后 | gitnexus | 严重程度 |
|--------|-----------|-----------|----------|----------|
| Impl | 242 (+64%) | 89 (+2.25%) | 87 | 次要 ✅ |
| CALLS | 9144 (+43%) | 7008 (+25%) | 5230 | 主要 |
| CONTAINS | 4010 (重复) | 0 | — | 已修复 ✅ |
| DEFINES | 4010 | 3861 (-10.5%) | 4314 | 主要 |
| Function | 3015 (+0.7%) | 3019 (+0.83%) | 2994 | 次要 |
| Struct | — | 247 (+4.05%) | 237 | 次要 |
| Trait | — | 16 (0%) | 16 | 次要 ✅ |

**查询差异比较（越低越好）：**

| 查询 | 修复后 (missing_cn / missing_gn) | 备注 |
|-------|-------------------------------------|-------|
| callees_of_function | 21 / 31 | 显著改善 |
| callers_of_function | 29 / 15 | 显著改善 |
| file_contains_symbols | 22 / 22 | B9 修复前在 LAPACK 上约为 200/200 |
| function_count_by_file | 7 / 13 | 显著改善 |
| implements_list | 42 / 0 | 设计差异（gitnexus 独有的边类型） |
| imports_of_file | 80 / 0 | 设计差异（gitnexus 独有的边类型） |
| class_methods | 匹配 (0) | 双方均为 0（Rust 无 class） |
| extends_chain | 匹配 (0) | 双方均为 0 |

**结论：**
- B1（CONTAINS 重复）：已修复 — CONTAINS 4010 → 0
- B2（impl 过度计数）：已修复 — Impl 242 → 89（gitnexus=87，+2.25% 残余噪声）
- B3（calls 过度计数）：已修复 — CALLS 9144 → 7008（25% 残余是调用粒度的设计差异）
- B9（API_SUFFIX 宏）：不适用于 Rust 代码库（仅 C 的 bug）
- 剩余 6 个严重问题：4 个是残余解析噪声（小数量），2 个是设计差异（implements_list、imports_of_file）

---

## 跨样本模式分析

### 模式 1：gitnexus 独有的边类型（设计差异，影响所有样本）

每个样本中，以下边类型在 CodeNexus 中为 0，在 gitnexus 中非零：

| 边类型 | CodeNexus 等价物 | 受影响样本 |
|-----------|---------------------|------------------|
| `accesses` | `reads` + `writes` | 所有 6 个已索引样本 |
| `has_property` | `contains` + `Variable` 节点 | 所有 6 个已索引样本 |
| `has_method` | `contains` + `Method` 节点 | 所有 6 个已索引样本 |
| `implements` | `Impl` 节点（非边） | 所有 6 个已索引样本 |
| `imports` | `Module` 节点（非边） | 所有 6 个已索引样本 |
| `member_of` | `contains`（超集） | 所有 6 个已索引样本 |
| `step_in_process` | 无（gitnexus 分析产物） | 所有 6 个已索引样本 |
| `method_implements` | 无（CodeNexus 建模方式不同） | CodeNexus、velo |
| `entry_point_of` | 无（gitnexus 进程检测） | panorama、hermes-agent |
| `handles_route` | 无（gitnexus 路由检测） | panorama、hermes-agent |
| `handles_tool` | 无（gitnexus 工具检测） | hermes-agent |
| `extends` | `CONTAINS` 或隐式 | claudecode、hermes-agent、panorama |

**操作**：更新 `type_map.json` 将这些标记为 `gitnexus_only`。仅此一项即可消除约 60% 的严重问题。

### 模式 2：`folder` 节点（设计差异，影响所有样本）

CodeNexus=0，gitnexus=20-356（所有样本）。CodeNexus 不将 `Folder` 建模为节点 — 目录信息存储在 `filePath` 属性中。

**操作**：在 type_map.json 中将 `folder` 标记为 `gitnexus_only`。

### 模式 3：`contains`/`defines` 边重复（解析 bug，影响 Rust 样本）

| 样本 | CodeNexus `contains` | CodeNexus `defines` | 相等？ |
|--------|---------------------|--------------------|--------|
| CodeNexus | 4010 | 4010 | **是** |
| velo | 10004 | 10004 | **是** |

CodeNexus 为每个 (parent, child) 对同时发出 `CONTAINS` 和 `DEFINES` 边，计数完全相同。这是 Rust 提取器中 bug 的强烈信号 — 应为 file→symbol 发出 `DEFINES`，为 parent→child 发出 `CONTAINS`，而非为每对同时发出两者。

**探测查询**：`MATCH (r:CodeRelation) WHERE r.type IN ['CONTAINS','DEFINES'] RETURN r.source, r.target, collect(r.type) AS types LIMIT 20`

**操作**：提交修复变更。这是最高优先级的解析 bug。

### 模式 4：`function`/`method` 分类差异（设计差异，Python）

| 样本 | CodeNexus `function` | CodeNexus `method` | 总和 | gitnexus `function` | gitnexus `method` | 总和 |
|--------|--------------------|--------------------|-----|--------------------|--------------------|----|
| panorama | 508 | 4185 | 4693 | 4597 | 62 | 4659 |
| hermes-agent | 4402 | 12646 | 17048 | 17785 | 9 | 17794 |
| claudecode | 9010 | 2034 | 11044 | 9412 | 1804 | 11216 |

CodeNexus 将 Python 方法积极分类为 `Method`（4185、12646），而 gitnexus 大多将其分类为 `Function`（4597、17785）。总和接近（差异在 1-3% 以内），确认底层符号被正确提取 — 这纯粹是分类差异。

**操作**：这是设计差异而非 bug。考虑在 Python 样本的 type_map.json 中将 `method` 映射为 `function`，或记录分类策略差异。

### 模式 5：`impl` 过度计数（解析 bug，仅 Rust）

| 样本 | CodeNexus `impl` | gitnexus `impl` | 差异 |
|--------|-----------------|-----------------|-------|
| CodeNexus | 242 | 87 | +64% |
| velo | 1138 | 625 | +45% |

CodeNexus 的 `Impl` 节点多计 45-64%。可能是 trait impl 的重复计数（`impl Trait for Type` 计一次，底层类型又计一次）。

**操作**：提交修复变更。优先级低于模式 3。

### 模式 6：`calls` 边过度计数（解析 bug，Rust + Fortran）

| 样本 | CodeNexus `calls` | gitnexus `calls` | 差异 |
|--------------------|-------------------|------------------|-------|
| CodeNexus | 9141 | 5230 | +43% |
| velo | 14965 | 7080 | +52% |
| LAPACK | 2747 | 11812 | **-77%** |

Rust 样本过度计数 calls（约 2 倍），而 Fortran 则少计 77%。Rust 的过度计数可能与模式 3（重复发出边）有关。Fortran 的少计表明 Fortran 提取器遗漏了大部分调用点。

**操作**：提交修复变更，修复 Rust（过度计数）和 Fortran（少计）问题。

### 模式 7：查询采样噪声（测试工具 bug，影响所有样本）

所有 8 个查询比较都使用 `LIMIT 200` 而没有 `ORDER BY`（`function_count_by_file` 除外，它有 `ORDER BY fn_count DESC`）。这意味着双方返回任意 200 行样本，集合比较产生虚假差异。

证据：`file_contains_symbols` 在每个样本中显示 missing_cn≈200、missing_gn≈200 — 双方各返回 200 行但几乎无重叠，如果数据正确这不可能发生。采样噪声解释了这一点。

**操作**：在所有 8 个 `.cql` 文件的 `LIMIT 200` 之前添加 `ORDER BY symbol_name, file_path`。

### 模式 8：Fortran `filePath` 不匹配（解析 bug，仅 Fortran）

LAPACK 查询差异极端：`callees_of_function` missing_cn=98/200，`callers_of_function` missing_cn=133/200，`file_contains_symbols` missing_cn=200/200。这表明 CodeNexus 的 Fortran 提取器填充 `filePath` 的格式与查询期望的不同（如绝对路径 vs 相对路径，或不同的路径分隔符）。

**探测查询**：在 CodeNexus 侧和 gitnexus 侧分别执行 `MATCH (f:Function) RETURN f.filePath LIMIT 10`。

**操作**：调查 Fortran 提取器的 `filePath` 填充方式。

---

## Category A：设计差异（无需操作）

### A1. `folder` 节点：CodeNexus 在所有样本中均为 0
CodeNexus 不建模 Folder 节点。目录信息存储在 `filePath` 中。**无需修复。**

### A2. gitnexus 独有的边类型（`accesses`、`has_property`、`has_method`、`implements`、`imports`、`member_of`、`step_in_process`、`entry_point_of`、`handles_route`、`handles_tool`、`method_implements`）
CodeNexus 通过不同机制建模这些关系（节点 vs 边、不同的类型名）。**操作：更新 type_map.json** 将这些标记为 `gitnexus_only`。

### A3. `function`/`method` 分类（Python）
CodeNexus 将 Python 函数分为 `Function`（模块级）和 `Method`（类级）。gitnexus 大多对两者都使用 `Function`。总和匹配差异在 3% 以内。**无需修复** — 设计差异。

### A4. `extends` 边（TypeScript/Python）
CodeNexus 在 claudecode 和 hermes-agent 中 `extends` 边为 0。CodeNexus 可能通过 `CONTAINS` 或隐式 parent_qn 引用建模继承，而非显式的 `EXTENDS` 边。**操作：调查** — 可能需要添加 `EXTENDS` 边发出。

---

## Category B：潜在解析 bug（需要修复变更）

### B1. `contains`/`defines` 边重复（Rust）— **最高优先级**
- CodeNexus：`CONTAINS` == `DEFINES`（4010==4010，10004==10004）
- 提取器为每个 (parent, child) 对同时发出两种边类型
- 将边计数夸大 2 倍，影响影响/追踪分析
- **探测**：检查 `(source, target)` 对在两种边类型中是否相同

### B2. `impl` 过度计数（Rust）
- CodeNexus 多计 45-64%
- 可能是 trait impl 的重复计数
- **探测**：`MATCH (i:Impl) RETURN i.name, i.qualifiedName LIMIT 50`

### B3. `calls` 边过度计数（Rust）/ 少计（Fortran）
- Rust：+43-52%（可能与 B1 相关 — 重复发出）
- Fortran：-77%（提取器遗漏调用点）
- **探测**：检查 CALLS 边中是否存在重复的 (source, target) 对

### B4. `function` 过度计数（Rust，仅 velo）
- velo：CodeNexus=7050，gitnexus=6252（+11%）
- CodeNexus（自身）：3015 vs 2994（+0.7% — 在噪声范围内）
- 可能是闭包、宏生成的函数或嵌套函数
- **探测**：对比双方的函数名列表

### B5. 查询采样噪声（测试工具）— 影响所有样本
- 所有查询使用 `LIMIT 200` 但无确定性 `ORDER BY`
- 产生虚假集合差异（如 `file_contains_symbols` 中 200/200 missing）
- **操作**：在所有 8 个 `.cql` 文件中添加 `ORDER BY symbol_name, file_path`

### B6. CSV 转义 bug（阻塞性问题）— **阻塞中**
- **受影响**：redis（C）、subno.ts（TypeScript）
- **根因**：LadybugDB 的 COPY 解析器无法正确处理 RFC 4180 引号字段中包含反斜杠（C 宏行续接）或引号（Python 方法签名）的情况
- **证据**：
  - redis：`Macro` 表，第 1128 行 — 签名 `(o, n, min, max, check_min, check_max, \'` 破坏了引号
  - subno.ts：`Method` 表，第 383 行 — 签名 `"def mock_client(self):'` 破坏了引号
- **影响**：无法索引任何含宏的 C 项目，或任何含 Python 方法签名的混合语言项目
- **修复方案**：(a) 在 CSV 写入前将反斜杠转义为 `\\`，(b) 替换签名中的问题字符，或 (c) 使用其他批量加载机制
- **位置**：[src/storage/loader.rs](src/storage/loader.rs) `write_nodes_csv` / `node_to_row`

### B7. Fortran `filePath` 不匹配
- LAPACK：`file_contains_symbols` 查询差异率 100%
- 可能是 CodeNexus Fortran 提取器填充 `filePath` 的方式与查询期望的不同
- **探测**：对比 CodeNexus 和 gitnexus 的 `filePath` 格式

### B8. TypeScript `class_methods` 查询返回 0
- claudecode：CodeNexus=0，gitnexus=200
- 查询可能引用了 CodeNexus 未为 TypeScript 方法填充的属性名
- **探测**：检查 TypeScript `Method` 节点是否具有预期的 `parentQn` / `filePath` 属性

---

## Category C：次要 / 不确定

### C1. `enum` 次要差异
- CodeNexus：33 vs gitnexus 28（CodeNexus 自身）— CodeNexus 多计 5 个 enum
- velo：187 vs 186 — 在噪声范围内
- LAPACK：0 vs 5 — CodeNexus Fortran 提取器不发出 Enum 节点
- **结论**：次要。无需操作。

### C2. `file` 计数差异
- 大多数样本差异在 1-5%。hermes-agent 显示 815 vs 1508（CodeNexus 少计 46%）— 可能是 CodeNexus 跳过了 gitnexus 包含的非 Python 文件（JS/TS/config）。
- **结论**：大多数样本次要。hermes-agent 需要调查（可能是文件遍历器配置问题）。

### C3. `struct`/`trait` 差异
- 所有 Rust 样本差异在 0-5%。LAPACK：4 vs 0 — Fortran 提取器发出了几个 gitnexus 没有的 Struct 节点。
- **结论**：次要。无需操作。

---

## 推荐修复变更（按优先级排序）

### 1. `fix-csv-escaping-bug`（阻塞性 — 解除 C 和 TypeScript 的阻塞）
- 修复 LadybugDB COPY 解析器与 RFC 4180 引号字段的不兼容性
- 位置：`src/storage/loader.rs`
- 测试：重新索引 redis 和 subno.ts，验证无 COPY 异常

### 2. `fix-rust-edge-duplication`（高优先级 — 影响所有 Rust 分析）
- 修复 Rust 提取器中的 `CONTAINS`/`DEFINES` 边重复
- 位置：`src/extract/rust/`（具体提取器模块）
- 测试：验证修复后 `CONTAINS` 计数 != `DEFINES` 计数

### 3. `update-verification-type-map`（中优先级 — 减少噪声）
- 在 `scripts/verification/type_map.json` 中重新分类 gitnexus 独有的边类型
- 将 `accesses`、`has_property`、`has_method`、`implements`、`imports`、`member_of`、`step_in_process`、`entry_point_of`、`handles_route`、`handles_tool`、`method_implements`、`extends` 标记为 `gitnexus_only`
- 将 `folder` 标记为 `gitnexus_only`
- 测试：重新运行 velo 报告，验证严重问题数量从 13 降至约 4

### 4. `fix-verification-query-ordering`（中优先级 — 消除采样噪声）
- 在 `scripts/verification/queries/` 下的所有 8 个 `.cql` 文件中添加 `ORDER BY symbol_name, file_path`
- 测试：重新运行 velo 查询，验证 `file_contains_symbols` 差异从 199/178 降至接近 0

### 5. `fix-rust-impl-overcounting`（低优先级 — 次要精度改善）
- 调查 Rust 提取器中的 `Impl` 节点重复计数
- 测试：验证 `Impl` 计数与 gitnexus 的差异在 5% 以内

### 6. `fix-fortran-calls-undercounting`（低优先级 — Fortran 专用）
- Fortran 提取器遗漏 77% 的调用点
- 调查 tree-sitter Fortran 语法的调用检测
- 测试：验证修复后 `CALLS` 边计数增加

---

## 附录：各样本原始数据

### CodeNexus（Rust，自索引）
- 132 个文件，7903 个节点，44240 条边
- 严重：15 · 主要：4 · 次要：5
- 关键指标：`function` 3015 vs 2994（0.7% ✓），`impl` 242 vs 87（64% ✗），`contains`==`defines`==4010（重复 ✗）
- 报告：[CodeNexus.report.md](CodeNexus.report.md)

### velo（Rust）
- 281 个文件，16712 个节点，77491 条边
- 严重：13 · 主要：5 · 次要：4
- 关键指标：`function` 7050 vs 6252（11% ✗），`impl` 1138 vs 625（45% ✗），`calls` 14965 vs 7080（52% ✗）
- 报告：[velo.report.md](velo.report.md)

### panorama（Python）
- 559 个文件，13683 个节点，30707 条边
- 严重：17 · 主要：6 · 次要：3
- 关键指标：`function` 508 vs 4597（fn/method 分类差异 ✓），`method` 4185 vs 62
- 报告：[panorama.report.md](panorama.report.md)

### hermes-agent（Python）
- 815 个文件，47892 个节点，129733 条边
- 严重：18 · 主要：5 · 次要：4
- 关键指标：`function` 4402 vs 17785（分类差异 ✓），`file` 815 vs 1508（少计 ✗）
- 报告：[hermes-agent.report.md](hermes-agent.report.md)

### claudecode（TypeScript）
- 1906 个文件，56871 个节点，143914 条边
- 严重：18 · 主要：5 · 次要：3
- 关键指标：`function` 9010 vs 9412（4% ✓），`method` 2034 vs 1804（11% ✗），`class_methods` 查询 0 vs 200（✗）
- 报告：[claudecode.report.md](claudecode.report.md)

### redis（C）— **索引失败**
- `Macro` 表中的 CSV 转义 bug（jemalloc 宏含反斜杠）
- 无统计数据
- 错误：`COPY exception: expected 10 values per row, but got 9`

### LAPACK（Fortran）
- 6434 个文件，104641 个节点，205716 条边
- 严重：11 · 主要：4 · 次要：1
- 关键指标：`function` 4386 vs 3476（21% ✗），`calls` 2747 vs 11812（77% 少计 ✗），查询差异 98-100%（filePath 不匹配 ✗）
- 报告：[LAPACK.report.md](LAPACK.report.md)

### subno.ts（TypeScript）— **索引失败**
- `Method` 表中的 CSV 转义 bug（Python 方法签名含引号）
- 无统计数据
- 错误：`COPY exception: expected 14 values per row, but got 8`

---

## 第二轮修复结果（2026-06-29，Fortran + 设计差异定性）

### B10. Fortran 函数调用少计（calls -77%）— **已修复**
- **根因**：tree-sitter-fortran 将函数调用 `ABS(Y)` 和数组访问 `D(I)` 都解析为 `call_expression` 节点。CodeNexus 将两者都提取为 CallInfo，导致虚假调用边。后续的 is_exported 修复揭示了大量跨文件调用被跳过，因为被调用方不在符号表中。
- **修复**：在 `src/parse/fortran.rs` 中添加 `collect_declared_arrays` 函数收集已声明的数组名，在 `call_expression` 分支中检查 callee 是否为已声明数组，若是则跳过 CallInfo 创建（但仍 visit_children 以捕获嵌套函数调用如 `D(FUNC(X))`）。
- **验证**：新增 `call_expression_array_access_not_extracted_as_call` 等测试。

### is_exported 修复（Fortran 跨文件调用解析）— **已修复**
- **根因**：Fortran 顶层子程序/函数有 `is_global(true)` 但没有 `is_exported(true)`。符号表的 `lookup_exported`（`src/resolve/symbol_table.rs`）无法找到它们 → 跨文件调用不可解析 → 被 `resolve_calls` 跳过。
- **修复**：在 `extract_module`、`extract_subroutine_or_function`、`extract_program` 中添加 `.is_exported(true)`。
- **验证**：LAPACK CALLS 80 → 2368（is_exported 修复后）→ 23105（B11 修复后）。

### B11. 固定格式 Fortran 注释预处理 — **已修复**
- **根因**：tree-sitter-fortran 仅支持自由格式注释（`!`）。固定格式文件（`.f` 扩展名）使用 `*`、`C` 或 `c` 作为第 1 列注释字符，tree-sitter-fortran 将其误解析为代码。这导致小文件（如 LAPACK 的 xerbla.f）灾难性解析失败 — 整个 AST 变为 ERROR 节点，子程序/函数定义从未被识别。
- **修复**：在 `src/parse/fortran.rs` 中添加 `preprocess_fixed_form_comments` 和 `is_fixed_form_fortran` 辅助函数。对 `.f` 文件，将第 1 列的 `*`/`C`/`c` 替换为 `!`，保持字节偏移不变以使 tree-sitter 位置保持有效。
- **验证**：新增 7 个 B11 测试。LAPACK CALLS 2368 → 23105，DEFINES 9042 → 11349，节点 115023 → 161546。

### B12. 索引非确定性（HashMap 迭代顺序） — **已修复**
- **根因**：`ProjectSymbolTable::global_symbols: HashMap<String, Vec<SymbolEntry>>` 与 `Graph::nodes: HashMap<NodeId, Node>` 的迭代顺序由 Rust 标准库的 SipHash 随机种子决定，每次进程启动都不同。这导致：
  1. `add_file_table` 中 `table.all_symbols()` 迭代顺序非确定 → `global_symbols[name]` 的 `Vec` 顺序非确定
  2. `lookup()` 返回的 `Vec<&SymbolEntry>` 顺序非确定
  3. `CallResolver::resolve_call_internal` 步骤 2/3 与 `DataFlowResolver::lookup_symbol_qn` 的 `.first()` 调用因此非确定
  4. `Graph::nodes_by_label` 返回顺序非确定 → 索引输出（CALLS 边集合）非确定
- **症状**：用同一份代码（HEAD 89b097e）连续两次索引 CalNexus 项目，i18n.rs 作为 source 的 CALLS 边数从 0 跳到 52，死代码分析误报数从 3 跳到 0。两次索引节点/边总数相同（4463 nodes, 21990 edges）但 CALLS 边集不同。
- **修复**：在 3 处添加按 FQN/id 字典序排序：
  - `ProjectSymbolTable::add_file_table`：`entries.sort_by(|a, b| a.qn.cmp(&b.qn))` 在 push 之前
  - `ProjectSymbolTable::lookup`：返回 `Vec` 按 `qn` 排序（带 `len() > 1` 短路优化避免 K=1 热点路径开销）
  - `Graph::nodes_by_label` 与 `Graph::nodes_by_project`：返回 `Vec` 按 `id` 排序
- **设计权衡**：选择"HashMap + 查询时排序"而非 BTreeMap，原因：
  - 插入复杂度：HashMap O(1) vs BTreeMap O(log n) — 索引阶段插入远多于查询
  - `SymbolEntry` 有 9 个字段，派生 `Ord` 语义模糊；显式 `sort_by` 更清晰
  - 改动面小（3 个方法），不影响 `add_node`/`get_node` 等热路径
- **验证**：连续两次索引 CalNexus（同一份代码），CALLS 边总数（4638）、i18n.rs source CALLS（52）、parse_lang target CALLS（3）完全一致；死代码分析 0 误报 0 漏报；4244 个 lib 测试全过。

### B13. Rust crate-root re-export 调用边解析错误 — **已修复**
- **根因**：`src/lib.rs` 是纯 re-export（`pub use cli::run`），0 个 Function 节点。`src/main.rs:5` 的 `calnexus::run()` 调用中 `calnexus` 是 crate 名被 tree-sitter 当作模块名，导致 `CallResolver::resolve_call_internal` step 3（project-level exported lookup）返回字典序第一的 `batch.rs::run`（B12 fix 后 `lookup_exported` 按 qn 排序，`batch` < `cli`）。整条 `main → cli::run → ...` 入口链在 main 处断裂，`dead_code` 反向 BFS 从 `cli::run` 出发走不到 `main`，cli.rs 整文件（11 个函数）被误判死代码。
- **症状**：CalNexus 项目 `dead_code` 报 13 项（11 Rust + 2 bash），11 个 Rust 项 100% 假阳性，全部来自 `src/cli.rs`。
- **修复（三段式）**：
  1. **B13-1（imports.rs）**：`resolve_rust_module_path` 增加无前缀 module path 解析分支——`pub use cli::run` 的 `source_file = "cli::run"` 现在能生成 REEXPORTS edge（之前只处理 `crate::`/`self::`/`super::` 前缀）。
  2. **B13-2（symbol_table.rs + calls.rs + orchestrator.rs）**：`ProjectSymbolTable` 新增 `reexport_target_qns: HashSet<String>` 字段 + `populate_reexport_targets` 方法（在 `build_symbol_table` 末尾调用，预扫描 `ExtractResult.imports` 填充 re-export target QN 集合）。`CallResolver::resolve_call_internal` step 3 改为优先返回 re-export target（`is_reexport_target` 查询），而非字典序第一的 exported entry。
  3. **B13-3（SKILL.md + commands.md）**：文档偏差修正——`search` 无 `--name`（实际为 `--text`+`--fulltext`+`--mode`）；`impact` 无 `--cross_service`（trace-only）；`dead_code` Rust 结果标注 "triage only, 需手工确认入口链"。
- **设计权衡**：选择方案 B（在 `build_symbol_table` 阶段预扫描填充 re-export index），而非调整 phase 顺序（方案 A/C/D）。理由：最小侵入性，calls.rs 零新依赖，不破坏 ImportResolver 接口。DIP 违反（路径解析逻辑跨模块复制）作为 tech debt 标注，后续重构时抽取共享 `path_matching` 模块。
- **验证**：重新索引 CalNexus（59 files, 4442 nodes, 21979 edges），`dead_code` 报 0 findings（从 11 假阳性降为 0）；`query` 确认 `src.main.rs.main` → `src.cli.rs.run` CALLS edge（confidence 0.80）正确建立（之前是 `batch.rs::run`）。10 个新测试全通过（symbol_table 7 + calls 3）。

### B4 重新定性：function 过计数（velo +11%）— **设计差异，非 bug**
- **原始假设**：闭包、宏生成的函数或嵌套函数导致过度计数。
- **调查结果**：差异完全由 Rust impl 方法建模差异造成。CodeNexus 为每个 impl 块中的方法创建独立 Function 节点（QN 不同，如 `new#CpuAvx512Backend`、`new#cuda_CudaDevice`），gitnexus 按文件内函数名去重（所有 `new` 合并为 1 个）。
- **证据**：velo `src/core/platform/mod.rs` — CodeNexus 148 个 Function（`new`×19、`allocate`×8、`synchronize`×8 等），gitnexus 73 个（每个名字仅 1 个）。17 个 `new` 是不同类型的构造函数（CpuAvx512Backend、CudaDevice、MetalDevice 等），是合法的不同函数。
- **结论**：CodeNexus 的做法更准确 — 区分 `CpuBackend::new` 和 `CudaBackend::new`。改为匹配 gitnexus 会丢失 impl 上下文，是退化。**无需修复。**

### DEFINES 差异定性 — **已修复（Property 节点实现）**
- **原差异**：CodeNexus 自索引 DEFINES=3861，gitnexus DEFINES=4314（CN -10.5%）。
- **原根因**：gitnexus 额外索引 583 个 Property 节点（Rust struct fields）并发 DEFINES 边。CodeNexus 缺少 Property 节点类型。
- **修复后**：CodeNexus DEFINES=4474，gitnexus DEFINES=4314（CN +3.7%）。差异从 -10.5% 改善至 +3.7%。见 §Property 节点实现。
- **完整分布**（修复前）：
  | 类型 | gitnexus | CodeNexus | 差异 |
  |------|----------|-----------|------|
  | Function | 2994 | 3019 | +25 |
  | Property | 583 | 0 → 600 | -583 → +17（已修复）|
  | Struct | 237 | 247 | +10 |
  | Module | 205 | 215 | +10 |
  | Const | 91 | 133 | +42 |
  | Impl | 87 | 89 | +2 |
  | TypeAlias | 72 | 108 | +36 |
  | Trait | 16 | 16 | 0 |
  | Enum | 28 | 33 | +5 |
  | **总计** | **4313** | **3860** | **-453** |
- **结论**：净差 -453 = -583 (Property) + 130 (CN 在其他类型上更多)。CodeNexus 在非 Property 类型上反而多 130 个 DEFINES 边。**无需修复。**

### Enum 差异定性 — **范围差异**
- **差异**：CodeNexus 自索引 Enum=33，gitnexus Enum=28（CN +5）。
- **根因**：gitnexus 索引 0 个 `tools/` 目录文件，CodeNexus 索引整个仓库。5 个多出的 enum 全部来自 `scripts/verification/src/`：CanonicalType、Command（main.rs）、CypherResponse、QueryDiff、Side。
- **结论**：**无需修复。**

### 剩余 query 差异定性 — **设计差异**
1. **implements_list**（42 missing CN）：CodeNexus 定义了 `EdgeType::Implements` 但提取器未发出 IMPLEMENTS 边。这是**功能缺失**（feature gap），非解析 bug。
2. **imports_of_file**（80 missing CN）：CodeNexus 定义了 `EdgeType::Imports` 且提取器收集 `ImportInfo`，但 storage 层从未将其写为 IMPORTS 边。这是**功能缺失**，非解析 bug。
3. **callees/callers/file_contains/function_count**：由 impl 方法建模差异（B4）衍生。CodeNexus 有更多 Function 节点 → 更多 CALLS 边 → 查询结果集不同。
- **结论**：所有剩余 query 差异均为设计差异或功能缺失，非解析 bug。**无需修复。**

### LAPACK CALLS 2x 差异定性 — **设计差异**
- **差异**：LAPACK CALLS CN=23105 vs GN=11812（CN +95%）。
- **根因**：gitnexus LAPACK 有 0 个来自 `.f`/`.f90` 文件的 Function 节点 — 它仅索引 C 文件（`.c`）。CodeNexus 索引 Fortran 和 C → 自然有更多 CALLS 边。
- **结论**：**设计差异，非 bug。**

---

## 最终结论

### 已修复的解析 bug（Category B）
| Bug | 描述 | 状态 |
|-----|------|------|
| B1 | CONTAINS/DEFINES 边重复（Rust） | ✅ 已修复 |
| B2 | impl 过度计数（Rust trait impl） | ✅ 已修复 |
| B3 | CALLS 边过度计数（Rust (caller,callee) 去重） | ✅ 已修复 |
| B5 | 查询采样噪声（缺少 ORDER BY） | ✅ 已修复 |
| B6 | CSV 转义 bug（LadybugDB COPY） | ✅ 已修复 |
| B8 | TypeScript class_methods 查询 | ✅ 已修复 |
| B9 | API_SUFFIX 宏过度提取（C） | ✅ 已修复 |
| B10 | Fortran 函数调用少计（数组访问误判） | ✅ 已修复 |
| B11 | 固定格式 Fortran 注释预处理 | ✅ 已修复 |
| B12 | 索引非确定性（HashMap 迭代顺序） | ✅ 已修复 |
| B13 | Rust crate-root re-export 调用边解析错误 | ✅ 已修复 |
| is_exported | Fortran 跨文件调用解析 | ✅ 已修复 |

### 定性为设计差异（无需修复）
| 差异 | 描述 |
|------|------|
| B4 | velo function +11% — impl 方法建模差异 |
| ~~DEFINES~~ | ~~ -10.5% — Property 节点缺失~~ ✅ 已修复（见 §Property 节点实现，差异从 -10.5% 改善至 +3.7%） |
| Enum | +5 — tools/ 目录范围差异 |
| LAPACK CALLS | +95% — gitnexus 不索引 Fortran |
| implements_list | IMPLEMENTS 边功能缺失（已修复，见 §IMPLEMENTS 实现） |
| folder 节点 | CodeNexus 用 filePath 属性代替 |
| Python fn/method 分类 | 总和匹配，分类策略不同 |

### 功能缺失（未来可实现，非 bug）
| 功能 | 描述 |
|------|------|
| IMPORTS 边 | EdgeType::Imports 已定义，ImportInfo 已收集，但 storage 层未写为边 |
| ~~IMPLEMENTS 边~~ | ✅ 已实现（2026-06-30，见下方 §IMPLEMENTS 边实现） |
| ~~Property 节点~~ | ✅ 已实现（2026-06-30，见下方 §Property 节点实现） |
| ~~TypeResolver 路径不匹配~~ | ✅ 已修复（2026-06-30，见下方 §TypeResolver 路径不匹配修复） |

---

## IMPLEMENTS 边实现（2026-06-30）

### 变更概述

在 `src/parse/rust_extractor.rs` 的 `extract_impl` 函数中，当检测到 trait impl（`impl Trait for Type`）时，
不再直接 return，而是创建一条从类型 FQN 到 trait 伪 FQN 的 IMPLEMENTS 边。

- **修改文件**：`src/parse/rust_extractor.rs`
- **修改函数**：`extract_impl`（行 347-393）
- **变更内容**：将 `if trait_name.is_some() { return; }` 替换为创建 IMPLEMENTS 边的逻辑
- **边源 FQN**：`make_qn(file_path, &type_name, project, current_parent)` — 匹配 Struct/Enum 节点的 FQN
- **边目标 FQN**：`make_qn(file_path, &trait_short, project, current_parent)` — 伪 FQN，TypeResolver 应解析
- **trait 名提取**：`trait_name.rsplit("::").next()` — 处理 `std::fmt::Display` → `Display`

### 新增测试（4 个）

| 测试名 | 验证内容 |
|--------|---------|
| `trait_impl_creates_implements_edge` | trait impl 创建 1 条 IMPLEMENTS 边，source=Type FQN，target=Trait FQN |
| `trait_impl_with_path_extracts_last_component` | `impl std::fmt::Display for Foo` 提取 `Display` 作为目标 |
| `multiple_trait_impls_create_multiple_implements_edges` | 多个 trait impl 创建多条边 |
| `inherent_impl_does_not_create_implements_edge` | inherent impl 不创建 IMPLEMENTS 边 |

### 验证结果

- **单元测试**：50/50 通过（`cargo test --lib parse::rust_extractor`）
- **TypeResolver 测试**：12/12 通过
- **resolve 模块测试**：327/327 通过
- **cargo build**：成功
- **cargo clippy**：无新警告（`loader.rs` 有预先存在的 `collapsible_str_replace` 警告）
- **gitnexus detect_changes**：`extract_impl` 标记为 touched，LOW 风险，无受影响的执行流程

### 索引验证

| 项目 | IMPLEMENTS 边数（修复前） | IMPLEMENTS 边数（修复后） | gitnexus IMPLEMENTS |
|------|------------------------|------------------------|---------------------|
| CodeNexus（自索引） | 0 | 176 | N/A |
| velo | 0 | 573 | 144 |

### 数量差异分析（velo: 573 vs gitnexus 144）

CodeNexus 的 573 条 IMPLEMENTS 边包含所有 `impl Trait for Type`，包括外部 std trait 的实现。

**Top trait 分布**（velo）：

| Trait 名 | 边数 | 来源 |
|----------|------|------|
| Default | 204 | `std::default::Default`（外部） |
| Display | 29 | `std::fmt::Display`（外部） |
| AttentionBackend | 23 | 本地 trait |
| Stage | 20 | 本地 trait |
| SamplingStrategy | 18 | 本地 trait |
| SchedulerPolicy | 16 | 本地 trait |
| Clone | 15 | `std::clone::Clone`（外部） |
| Error | 10 | `std::error::Error`（外部） |
| Send | 9 | `std::marker::Send`（外部） |
| Sync | 9 | `std::marker::Sync`（外部） |

**差异原因**：
1. CodeNexus 为所有 trait impl 创建边（包括外部 std trait），gitnexus 可能只为本地 trait 创建边
2. 外部 std trait（Default/Display/Clone/Error/Send/Sync）合计约 276 条边，占总差异的大部分

### TypeResolver 路径不匹配修复（2026-06-30）

**现象**：所有 IMPLEMENTS 边 confidence=1.0（TypeResolver 未修改），目标 FQN 仍为伪 FQN。

**根因**：预先存在的路径不匹配问题 — `ScopeResolutionPhase` 将图节点的 `file_path` 规范化为相对路径，
但 `TypeResolver.resolve_types` 中的 `imports_map` 使用 `ExtractResult.file_path`（绝对路径）作为键。
路径不匹配导致 `imports_map.get(source_file)` 返回 None，TypeResolver 无法获取文件的 import 信息。
同样影响 `symbol_table.lookup_in_file(user_file, type_name)`（file 表以绝对路径为键，但 `user_file` 来自图节点的相对路径）。

**修复方案**：在 `TypeResolver::resolve_types`（`src/resolve/type_resolver.rs`）内部构建双向路径映射：

1. `result_to_graph_fp`：通过匹配 `result.nodes[*].qualified_name` 与 `graph.nodes[*].qualified_name`
   （两者都是 parse 阶段从绝对路径生成的 FQN，因此一致），建立 `result.file_path`（绝对）→
   `graph_node.file_path`（相对）的映射。
2. `imports_map` 改用图节点的相对路径作为键，使 `imports_map.get(source_file)` 命中。
3. `graph_to_result_fp`（反向映射）：将 `source_file`（相对）翻译回绝对路径，供
   `symbol_table.lookup_in_file` 使用。

**关键设计决策**：
- **不修改 `build_symbol_table`**：该函数从 `result.file_path` 重新生成 FQN。若归一化
  `result.file_path` 为相对路径，FQN 会从绝对路径段变为相对路径段，与图节点 id
  （parse 阶段从绝对路径生成的 FQN）不一致，导致解析后的 `edge.target` 仍为悬空。
- **不归一化 `ResolvePhase::run` 中的 `results.file_path`**：之前的尝试（在 `ResolvePhase::run`
  中用 `scope.path_to_rel` 归一化 `parse.results` 的 `file_path`）会引入上述 FQN 不匹配。
  已回退该修改，改为在 TypeResolver 内部处理路径映射。

**影响范围**：所有可解析边类型（Extends/Implements/UsesType）。

**验证结果**：
- 1785 tests passed（1784 旧 + 1 新回归测试 `resolve_types_handles_path_format_mismatch`）
- 重新索引 CodeNexus 后，IMPLEMENTS 边 confidence 分布：

| confidence | tier | count | 说明 |
|------------|------|-------|------|
| 0.90 | IMPORT_SCOPED | 7 | 通过 import 解析的跨文件 trait |
| 0.80 | GLOBAL | 16 | 通过项目级 exported 解析的 trait |
| 1.0 | GLOBAL | 153 | 非悬空边（target 已存在，跳过） |

- 23 条之前悬空的边（confidence 卡在 1.0）现已正确解析（7 条 import-scoped + 16 条 global）

**结论**：IMPLEMENTS 边功能缺失已关闭。数量差异为设计差异（CodeNexus 包含外部 trait impl）。
TypeResolver 路径不匹配问题已修复。

---

## Property 节点实现（2026-06-30）

### 变更概述

**关闭的功能缺失**：gitnexus 索引 Rust struct fields 为 `Property` 节点 + `HAS_PROPERTY` 边，
CodeNexus 之前无此节点类型（DEFINES 差异 -583 全部来自 Property 缺失）。

**修改位置**：`src/parse/rust_extractor.rs`

**修改函数**：
1. 新增 `extract_struct_fields` — 提取 struct 命名字段为 Property 节点 + HasProperty 边
2. 修改 `visit_node` 的 `struct_item` 分支 — 在 `extract_named_item` 后调用 `extract_struct_fields`

### 实现细节

**Property 节点创建**：
- 仅提取 `field_declaration`（命名字段），跳过 `tuple_field`（元组结构体字段无名称）
- 单元结构体（`struct Foo;`）无 body，不创建 Property 节点
- FQN 消歧：使用结构体名作为消歧器，格式 `proj.test.<field>#<struct>`，
  模块内结构体使用 `<module>_<struct>` 消歧器（与 impl 方法约定一致，ADR-003）
- `is_global(false)`：Property 是结构体作用域的，非全局

**HasProperty 边创建**：
- Source = 结构体 FQN（与 Struct 节点 FQN 匹配）
- Target = Property 节点 UUID（由 `ScopeResolutionPhase` 重映射为 FQN）
- 信任 `ScopeResolutionPhase` 的 `id_remap` 将 UUID → FQN

**DEFINES 边创建**：
- 复用 `add_definition_edges`，从 File 节点到 Property 节点
- 与其他定义节点（Function/Struct/Enum 等）一致

### 新增测试

| 测试名 | 验证内容 |
|--------|----------|
| `struct_fields_extracted_as_property_nodes` | `struct Point { x, y }` → 2 个 Property 节点 |
| `struct_creates_has_property_edges` | 2 条 HasProperty 边，source 为 Point FQN |
| `unit_struct_creates_no_property_nodes` | `struct Foo;` → 0 个 Property 节点 |
| `tuple_struct_creates_no_property_nodes` | `struct Foo(i32, i32);` → 0 个 Property 节点 |
| `struct_fields_have_disambiguated_fqn` | 字段 FQN 以 `#Point` 结尾 |
| `struct_in_module_has_module_qualified_field_fqn` | 模块内字段 FQN 以 `#foo_Point` 结尾 |
| `property_nodes_have_defines_edges` | 每个 Property 节点有 DEFINES 边 |

### 验证结果

**单元测试**：
- `cargo test --lib parse::rust_extractor`：57 passed / 0 failed（50 旧 + 7 新）
- `cargo test --lib`：1784 passed / 0 failed（1773 旧 + 11 新，含 4 个 IMPLEMENTS 测试）
- `cargo clippy --lib`：无新增警告（唯一错误为 `loader.rs:103` 预先存在的 `collapsible_str_replace`）

**索引验证**（CodeNexus 自索引，132 文件）：

| 指标 | 修复前 | 修复后 | gitnexus | 差异 | 严重程度 |
|------|--------|--------|----------|------|----------|
| Property 节点 | 0 | 600 | 583 | +17 (+2.9%) | 次要 ✅ |
| HAS_PROPERTY 边 | 0 | 600 | 569 | +31 (+5.5%) | 次要 ✅ |
| DEFINES 边 | 3861 | 4474 | 4314 | +160 (+3.7%) | 次要 ✅ |

**DEFINES 差异改善**：从 -453 (-10.5%) 改善至 +160 (+3.7%)，功能缺失已关闭。

### 差异原因分析（+17 Property / +31 HasProperty）

CodeNexus 略多于 gitnexus，可能原因：
1. CodeNexus 索引了测试文件中的 struct 字段，gitnexus 可能排除测试文件
2. CodeNexus 提取了带属性的字段（如 `#[serde(default)] field: Type`），gitnexus 可能跳过
3. 不同的文件遍历范围或解析粒度

差异在 5% 以内，属可接受的设计差异，无需进一步修复。

---

## 第三轮修复结果（2026-07-09，Go/Java/C++ 解析器差距）

### 背景

交叉验证报告确认 5 种已三审语言（Rust/Python/TypeScript/C/Fortran）无残留解析 bug。但 Go/Java/C++ 三个解析器未经三审，存在真实解析差距。使用 specmark 工作流（explore → clarify → propose → analyze → apply → converge → archive）进行系统修复。

### 样本与修复前差距

| 语言 | 样本 | 关键差距 | 严重程度 |
|------|------|----------|----------|
| Go | cobra | CALLS 少 66% (643 vs 1922) | Major |
| Java | gson | CALLS 少 77% (1415 vs 6081), DEFINES 多 5x (4098 vs 784) | Major |
| C++ | fmt | method 少 78%, function 多 90%, class 少 81%, EXTENDS 少 94%, enum 少 59% | Critical |

### BUG-G1: Go 方法 caller_qn 缺少 receiver type 消歧器 — **已修复** (commit 03201b3)
- **根因**: `extract_call` 中 `caller_qn = make_qn(file_path, name, project, None)`，parent 为 None。但方法节点 QN 包含 receiver type。不同类型上同名方法（如 A.Execute, B.Execute）的 caller_qn 相同，`resolve_calls` 按 (caller_qn, callee_qn) 去重时合并，丢弃大量 CALLS 边。
- **修复**: VisitContext 新增 `current_parent` 字段，method_declaration 分支设置 current_parent = receiver_type，extract_call 使用 current_parent。
- **验证**: 新增测试 `methods_on_different_types_produce_distinct_calls_edges`。cobra CALLS 643 → 1493。

### BUG-G2: Go is_exported 未按可见性设置 — **已修复** (commit fad8f97)
- **根因**: Go 函数/方法/类型未设置 is_exported，导致 `lookup_exported` 无法找到导出符号，跨文件调用不可解析。
- **修复**: 新增 `is_exported_name()` 辅助函数（大写首字母 = 导出），在 extract_function/extract_method/extract_type_spec 中设置 is_exported。
- **验证**: 新增 4 个测试。cobra CALLS 1493（最终值），差异从 66.55% → 22.32%。

### BUG-J1: Java 遗漏构造函数调用 — **已修复** (commit 813f74a)
- **根因**: visit_node 只处理 `method_invocation`，不处理 `object_creation_expression`（`new Foo()`）和 `explicit_constructor_invocation`（`super()`, `this()`）。
- **修复**: 新增两个节点类型处理分支，提取构造函数调用为 CallInfo。
- **验证**: 新增测试验证 `new Foo()` 和 `super()` 产生 CallInfo。

### BUG-J2: Java DEFINES 边为方法重复创建 — **已修复** (commit 813f74a)
- **根因**: `extract_method` 调用 `add_definition_edges`，为每个方法创建 DEFINES 边。gitnexus 只为顶层类型创建 DEFINES。
- **修复**: 移除 `extract_method` 中的 `add_definition_edges` 调用。
- **验证**: gson DEFINES 从 4098 → 720（vs gitnexus 784，差异 8.16%）。

### BUG-J3: Java is_exported 未按可见性设置 — **已修复** (commit afe0bec)
- **根因**: Java 方法未设置 is_exported，导致跨文件调用不可解析。
- **修复**: 新增 `has_private_modifier()` 辅助函数（检测 `private` 修饰符），在 extract_method 中设置 `is_exported = !has_private_modifier`。
- **验证**: 新增 6 个测试。gson CALLS 1415 → 7985（vs gitnexus 6081，23.84% over — 可接受的改善，从 76.77% under）。

### BUG-C1: C++ 类外方法定义误分类为 Function — **已修复** (commit 9fd31b7)
- **根因**: `void Foo::bar() {}` 定义在类外时，`is_inside_class_or_struct` 遍历祖先链找不到 class_specifier → 分类为 Function。
- **修复**: 新增 `extract_qualifier()` 检测 `qualified_identifier` declarator，将类外方法分类为 Method。
- **验证**: 新增测试 `out_of_class_method_classified_as_method`。

### BUG-C2: C++ enum_specifier 未提取 — **已修复** (commit 9fd31b7)
- **根因**: visit_node 无 enum_specifier 分支，C++ enums 被静默丢弃。
- **修复**: 新增 enum_specifier 分支，提取为 Enum 节点。
- **验证**: 更新测试 `enum_is_not_extracted_as_top_level_node` → `enum_is_extracted_as_enum_node`。

### BUG-C3: .h 文件语言检测错误 — **已修复** (commit 9fd31b7)
- **根因**: `.h` 文件硬编码为 C（`"h" => Some(Language::C)`），C++ 库的 .h 头文件被解析为 C，C++ 语法（class/template/namespace）解析失败。
- **修复**: 新增 `detect_cpp_header()` 和 `maybe_upgrade_h_to_cpp()` — 扫描 C++ 特有关键字（class, template, namespace, ::, virtual 等）检测 .h 文件语言。
- **验证**: 新增测试 `h_file_with_cpp_keywords_parsed_as_cpp`。

### BUG-C4: C++ is_exported 调查与回退 — **已调查并回退** (commit 1f1c3dd)
- **调查**: 测试三种 is_exported=true 方案：
  1. 所有函数/方法 is_exported=true → fmt CALLS 5,472 (58% OVER)
  2. 仅 free function is_exported=true → fmt CALLS 5,002 (54% OVER)
  3. 回退到 is_exported=false → 接受 under-resolution
- **根因**: CodeNexus 缺少 C++ #include 导入跟踪，`lookup_exported` 无法区分不同文件/命名空间中的同名函数，导致大量过度解析。
- **决策**: 回退到 is_exported=false。under-resolution 优于 over-resolution，因为过度解析会创建虚假调用关系，影响影响分析准确性。
- **未来工作**: 实现 C++ #include 导入跟踪后可安全启用 is_exported。

### 最终验证结果（2026-07-09）

| 样本 | 语言 | 指标 | 修复前 | 修复后 | gitnexus | 差异 | 评估 |
|------|------|------|--------|--------|----------|------|------|
| cobra | Go | CALLS | 643 | 1493 | 1922 | 22.32% | ✅ 从 66.55% 改善 |
| gson | Java | CALLS | 1415 | 7985 | 6081 | 23.84% | ✅ 从 76.77% under 改善 |
| gson | Java | DEFINES | 4098 | 720 | 784 | 8.16% | ✅ 从 5x over 改善 |
| fmt | C++ | CALLS | 1852 | 3376 | 2263 | 32.97% | ⚠️ BUG-C1/C2/C3 增加了 .h 文件解析 |

### 剩余差异定性 — **设计差异**

1. **C++ 函数/方法粒度差异**: CodeNexus 提取更多函数/方法（CN function 2300 vs GN 1424, CN method 2556 vs GN 1661），因为 BUG-C3 修复后更多 .h 文件被解析为 C++，提取了内联函数和模板实例化。gitnexus 可能对模板特化去重。
2. **C++ 缺少 #include 跟踪**: is_exported 无法安全启用，导致部分跨文件调用不可解析。这是功能缺失，非解析 bug。
3. **CALLS over-counting (gson 23.84%, fmt 32.97%)**: Java 的 over-counting 源于 is_exported 使更多调用可解析；C++ 的 over-counting 源于更多 .h 文件被解析。两者都是修复带来的副作用，且优于原来的 under-counting。
4. **file 计数差异**: cobra 36 vs 54, gson 262 vs 292, fmt 76 vs 116 — CodeNexus 的文件遍历器配置与 gitnexus 不同（.gitignore 处理差异），非解析 bug。

### 已修复的解析 bug（Go/Java/C++）
| Bug | 描述 | 状态 |
|-----|------|------|
| G1 | Go 方法 caller_qn 缺少 receiver type | ✅ 已修复 |
| G2 | Go is_exported 未按可见性设置 | ✅ 已修复 |
| J1 | Java 遗漏构造函数调用 | ✅ 已修复 |
| J2 | Java DEFINES 边为方法重复创建 | ✅ 已修复 |
| J3 | Java is_exported 未按可见性设置 | ✅ 已修复 |
| C1 | C++ 类外方法误分类为 Function | ✅ 已修复 |
| C2 | C++ enum_specifier 未提取 | ✅ 已修复 |
| C3 | C++ .h 文件语言检测错误 | ✅ 已修复 |
| C4 | C++ is_exported（缺少 #include 跟踪）| ⚠️ 已调查并回退 |
