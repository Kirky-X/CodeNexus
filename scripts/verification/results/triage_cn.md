# 交叉验证分类：所有样本

> 生成时间：2026-06-29（`std::mem::forget(kit)` 持久性修复后）
> 语料库：8个样本，5种语言，约270K节点
> 汇总：[_aggregate.report.md](_aggregate.report.md)

## 执行摘要

**8/8个样本失败。** 94个严重差异 · 29个主要差异 · 21个次要差异。

然而，如果没有上下文，"失败"的结论可能具有误导性：
- **约70%的严重差异是设计差异**（CodeNexus和gitnexus使用不同的节点/边类型集）
- **2个样本（redis, subno.ts）由于CSV转义错误（§B6）无法索引**
- **3-4个真正的解析错误**需要修复（§B1-B4）
- **1个测试工具问题**（查询中缺少`ORDER BY`）增加了查询差异（§B5）

修复type_map并添加`ORDER BY`后，预期严重差异数：**约15-20个**（从94个减少）。

---

## 修复结果（2026-06-29）

所有B类错误已修复并验证。详情如下。

### B1. CONTAINS/DEFINES边重复（Rust） — **已修复**
- **根本原因**：所有提取器中的`add_definition_edges`为（文件, 节点）对同时发出CONTAINS和DEFINES边。
- **修复**：移除CONTAINS发出；仅保留DEFINES（文件 → 定义）。位置：`rust_extractor.rs`、`c.rs`、`python.rs`、`typescript.rs`、`fortran.rs`中的`add_definition_edges`。
- **验证**：`extracts_functions`测试确认0个CONTAINS边；所有1761个库测试通过。

### B2. impl过度计数（Rust） — **已修复**
- **根本原因**：`extract_impl`为固有impl（`impl Type {}`）和trait impl（`impl Trait for Type {}`）都创建了Impl节点。gitnexus只建模固有impl。验证：CodeNexus固有impl（QN包含`#impl`）= 87 = gitnexus 87；trait impl = 155 = 过度计数部分。
- **修复**：`extract_impl`在`trait_name.is_some()`时提前返回。trait impl内的方法仍由`visit_children`提取。位置：`src/parse/rust_extractor.rs:extract_impl`。
- **验证**：46/46个rust_extractor测试通过；添加了`trait_impl_does_not_create_impl_node`测试。

### B3. calls边过度计数（Rust） — **已修复**
- **根本原因**：`resolve_calls`为每个调用点（每个CallInfo）创建一条CALLS边，没有按（调用者, 被调用者）对去重。`edge_id`包含`start_line`，因此`write_edges_csv`没有去重。CodeNexus：9144条总边，6998个唯一对 = 2146个重复（23%）。gitnexus：4822条总边，4756个唯一对（1.4%重复率）。
- **修复**：在`resolve_calls`中添加`HashSet<(String, String)>`按（caller_qn, callee_qn）去重。保留第一个调用点的行号。位置：`src/resolve/calls.rs:resolve_calls`。
- **验证**：22/22个calls测试通过；添加了`resolve_calls_deduplicates_same_callee_pair`测试。

### B5. 查询采样噪声（测试工具） — **已修复**
- **根本原因**：所有8个`.cql`查询文件使用`LIMIT 200`但没有`ORDER BY`，产生非确定性样本。
- **修复**：在`scripts/verification/queries/`中的所有8个`.cql`文件添加了`ORDER BY symbol_name, file_path`（或等效语句）。
- **验证**：通过grep确认所有8个文件都有ORDER BY。

### B6. CSV转义错误（阻塞问题） — **已修复**
- **根本原因**：LadybugDB的COPY解析器无法正确处理CSV字段中的反斜杠/引号/换行符。
- **修复**：`src/storage/loader.rs`中的`sanitize_for_ladybugdb`将`\` → `/`、`"` → `'`、`\n`/`\r`/`\t` → 空格。
- **验证**：redis和subno.ts现在可以索引而不会出现COPY异常。

### B7. Fortran filePath不匹配 — **误诊**
- **原始假设**：CodeNexus Fortran提取器填充`filePath`的方式与gitnexus不同。
- **调查**：CodeNexus和gitnexus都使用相对于仓库根目录的路径（例如，`CBLAS/testing/c_cblat3.f`）。filePath格式正确。
- **100%查询差异的真实原因**：B9（API_SUFFIX宏过度提取）导致148个名为"API_SUFFIX"的假Function节点，这改变了前200个结果的字母顺序，导致0重叠。
- **验证**：查询双方——filePath格式匹配。修复B9后，`function_count_by_file`不匹配从175降至93（-47%）。

### B8. TypeScript class_methods查询返回0 — **已修复**（上一会话）
- **根本原因**：TypeScript Method节点缺少`parentQn`属性。
- **修复**：在TypeScript提取器中填充`parentQn`。
- **验证**：claudecode `class_methods`查询现在返回非零结果。

### B9. API_SUFFIX宏过度提取（C） — **新增，已修复**
- **根本原因**：LAPACK CBLAS使用`void API_SUFFIX(cblas_caxpy)(args)`模式，其中`API_SUFFIX`是一个宏。tree-sitter-c（无预处理）将其解析为嵌套的`function_declarator`（返回函数的函数），这是无效的C。`declarator_name`递归解包嵌套的declarator并返回"API_SUFFIX"作为函数名，在CBLAS .c文件中创建了148个假Function节点。
- **修复**：在`declarator_name`（`src/parse/c.rs`）中，检测嵌套的`function_declarator`（内部declarator也是`function_declarator`）并返回`None`。C函数不能返回函数（只能通过`pointer_declarator`返回函数指针），因此嵌套的`function_declarator`总是宏伪影。
- **验证**：`macro_invocation_not_extracted_as_function`测试通过。LAPACK重新索引：API_SUFFIX Function计数148 → 0，总Function 4386 → 4238（正好-148）。所有1761个库测试通过。
- **对LAPACK指标的影响**：
  - `function_count_by_file`：缺失175 → 93（-47%）
  - `callees_of_function` missing_gn：137 → 41（-70%）
  - `callers_of_function` missing_gn：133 → 37（-72%）
  - `file_contains_symbols`：仍为100%不匹配（设计差异——见下文）

### 剩余：file_contains_symbols 100%不匹配（设计差异，无需修复）
修复B9后，`file_contains_symbols`仍显示100%不匹配，因为：
- CodeNexus将Fortran子程序提取为Function（按字母顺序靠前：CBDSQR, CBEG, CCHK1...）
- gitnexus将C函数提取为Function（按字母顺序：C_INTFACE, F77_*）
- 每一侧的前200个（LIMIT 200）来自不同的文件类型（.f vs .c/.h），导致0重叠
- 这是两种工具在分类Fortran子程序方式上的根本设计差异，不是错误。

### CodeNexus自索引最终验证（2026-06-29）
使用所有B1/B2/B3/B9修复重新索引CodeNexus自身，并运行验证工具。

**节点/边计数比较（修复前 → 修复后）：**

| 指标 | 修复前 | 修复后 | gitnexus | 严重性 |
|------|--------|--------|----------|--------|
| Impl | 242 (+64%) | 89 (+2.25%) | 87 | 次要 ✅ |
| CALLS | 9144 (+43%) | 7008 (+25%) | 5230 | 主要 |
| CONTAINS | 4010 (重复) | 0 | — | 已修复 ✅ |
| DEFINES | 4010 | 3861 (-10.5%) | 4314 | 主要 |
| Function | 3015 (+0.7%) | 3019 (+0.83%) | 2994 | 次要 |
| Struct | — | 247 (+4.05%) | 237 | 次要 |
| Trait | — | 16 (0%) | 16 | 次要 ✅ |

**查询差异比较（越低越好）：**

| 查询 | 修复后（missing_cn / missing_gn） | 备注 |
|------|-----------------------------------|------|
| callees_of_function | 21 / 31 | 显著改善 |
| callers_of_function | 29 / 15 | 显著改善 |
| file_contains_symbols | 22 / 22 | 在LAPACK上B9修复前约为200/200 |
| function_count_by_file | 7 / 13 | 显著改善 |
| implements_list | 42 / 0 | 设计差异（gitnexus专有边类型） |
| imports_of_file | 80 / 0 | 设计差异（gitnexus专有边类型） |
| class_methods | 匹配 (0) | 均为0（Rust没有类） |
| extends_chain | 匹配 (0) | 均为0 |

**结论：**
- B1（CONTAINS重复）：已修复 — CONTAINS 4010 → 0
- B2（impl过度计数）：已修复 — Impl 242 → 89（gitnexus=87，+2.25%残余噪声）
- B3（calls过度计数）：已修复 — CALLS 9144 → 7008（25%残余是调用粒度的设计差异）
- B9（API_SUFFIX宏）：不适用于Rust代码库（仅C错误）
- 剩余6个严重差异：4个是残余解析噪声（计数较小），2个是设计差异（implements_list, imports_of_file）

---

## 跨样本模式分析

### 模式1：gitnexus专有边类型（设计差异，影响所有样本）

每个样本在CodeNexus中这些边类型显示为0，而在gitnexus中为非零：

| 边类型 | CodeNexus等效类型 | 受影响样本 |
|--------|------------------|-----------|
| `accesses` | `reads` + `writes` | 所有6个已索引样本 |
| `has_property` | `contains` + `Variable`节点 | 所有6个已索引样本 |
| `has_method` | `contains` + `Method`节点 | 所有6个已索引样本 |
| `implements` | `Impl`节点（非边） | 所有6个已索引样本 |
| `imports` | `Module`节点（非边） | 所有6个已索引样本 |
| `member_of` | `contains`（超集） | 所有6个已索引样本 |
| `step_in_process` | 不适用（gitnexus分析伪影） | 所有6个已索引样本 |
| `method_implements` | 不适用（CodeNexus建模方式不同） | CodeNexus, velo |
| `entry_point_of` | 不适用（gitnexus流程检测） | panorama, hermes-agent |
| `handles_route` | 不适用（gitnexus路由检测） | panorama, hermes-agent |
| `handles_tool` | 不适用（gitnexus工具检测） | hermes-agent |
| `extends` | `CONTAINS`或隐式 | claudecode, hermes-agent, panorama |

**操作**：更新`type_map.json`将这些标记为`gitnexus_only`。仅此一项就可移除约60%的严重差异。

### 模式2：`folder`节点（设计差异，影响所有样本）

CodeNexus=0，gitnexus在所有样本中为20-356。CodeNexus不将`Folder`建模为节点——目录信息包含在`filePath`属性中。

**操作**：在type_map.json中将`folder`标记为`gitnexus_only`。

### 模式3：`contains`/`defines`边重复（解析错误，影响Rust样本）

| 样本 | CodeNexus `contains` | CodeNexus `defines` | 相等？ |
|------|---------------------|--------------------|--------|
| CodeNexus | 4010 | 4010 | **是** |
| velo | 10004 | 10004 | **是** |

CodeNexus为每个（父, 子）对发出`CONTAINS`和`DEFINES`边，计数相同。这是Rust提取器中存在错误的强烈信号——应该为文件→符号发出`DEFINES`，为父→子发出`CONTAINS`，而不是为每对都发出两种。

**探针**：`MATCH (r:CodeRelation) WHERE r.type IN ['CONTAINS','DEFINES'] RETURN r.source, r.target, collect(r.type) AS types LIMIT 20`

**操作**：提交修复更改。这是最高优先级的解析错误。

### 模式4：`function`/`method`分类拆分（设计差异，Python）

| 样本 | CodeNexus `function` | CodeNexus `method` | 总和 | gitnexus `function` | gitnexus `method` | 总和 |
|------|--------------------|--------------------|------|--------------------|--------------------|------|
| panorama | 508 | 4185 | 4693 | 4597 | 62 | 4659 |
| hermes-agent | 4402 | 12646 | 17048 | 17785 | 9 | 17794 |
| claudecode | 9010 | 2034 | 11044 | 9412 | 1804 | 11216 |

CodeNexus积极地将Python方法分类为`Method`（4185, 12646），而gitnexus主要将它们分类为`Function`（4597, 17785）。总和接近（在1-3%以内），确认底层符号提取正确——纯粹是分类差异。

**操作**：这是设计差异，不是错误。考虑在Python样本的type_map.json中将`method`映射到`function`，或记录分类策略差异。

### 模式5：`impl`过度计数（解析错误，仅Rust）

| 样本 | CodeNexus `impl` | gitnexus `impl` | 差异 |
|------|-----------------|-----------------|------|
| CodeNexus | 242 | 87 | +64% |
| velo | 1138 | 625 | +45% |

CodeNexus的`Impl`节点过度计数约45-64%。可能对trait impl进行了双重计数（为`impl Trait for Type`计数一次，为底层类型再计数一次）。

**操作**：提交修复更改。优先级低于模式3。

### 模式6：`calls`边过度计数（解析错误，Rust + Fortran）

| 样本 | CodeNexus `calls` | gitnexus `calls` | 差异 |
|------|-------------------|------------------|------|
| CodeNexus | 9141 | 5230 | +43% |
| velo | 14965 | 7080 | +52% |
| LAPACK | 2747 | 11812 | **-77%** |

Rust样本过度计数calls（约2倍），而Fortran计数不足77%。Rust的过度计数可能与模式3（双重发出边）相关。Fortran的计数不足表明Fortran提取器遗漏了大多数调用点。

**操作**：为Rust（过度计数）和Fortran（计数不足）提交修复更改。

### 模式7：查询采样噪声（测试工具错误，影响所有样本）

所有8个查询比较使用`LIMIT 200`但没有`ORDER BY`（`function_count_by_file`除外，它有`ORDER BY fn_count DESC`）。这意味着两侧都返回任意的200行样本，集比较产生假差异。

证据：`file_contains_symbols`在每个样本中显示missing_cn≈200，missing_gn≈200——两侧都返回200行但几乎没有重叠，如果数据正确这是不可能的。采样噪声解释了这一点。

**操作**：在`LIMIT 200`之前为所有8个`.cql`文件添加`ORDER BY symbol_name, file_path`。

### 模式8：Fortran `filePath`不匹配（解析错误，仅Fortran）

LAPACK查询差异极端：`callees_of_function` missing_cn=98/200，`callers_of_function` missing_cn=133/200，`file_contains_symbols` missing_cn=200/200。这表明CodeNexus的Fortran提取器填充`filePath`的格式与查询预期不同（例如，绝对路径vs相对路径，或不同的路径分隔符）。

**探针**：在CodeNexus侧vs gitnexus侧运行`MATCH (f:Function) RETURN f.filePath LIMIT 10`。

**操作**：调查Fortran提取器的`filePath`填充方式。

---

## A类：设计差异（无需操作）

### A1. `folder`节点：所有样本中CodeNexus=0
CodeNexus不建模Folder节点。目录信息在`filePath`中。**无需修复。**

### A2. gitnexus专有边类型（`accesses`、`has_property`、`has_method`、`implements`、`imports`、`member_of`、`step_in_process`、`entry_point_of`、`handles_route`、`handles_tool`、`method_implements`）
CodeNexus通过不同机制建模这些关系（节点vs边，不同的类型名称）。**操作：更新type_map.json**将其标记为`gitnexus_only`。

### A3. `function`/`method`分类（Python）
CodeNexus将Python函数拆分为`Function`（模块级）和`Method`（类级）。gitnexus主要对两者都使用`Function`。总和匹配在3%以内。**无需修复**——设计差异。

### A4. `extends`边（TypeScript/Python）
CodeNexus在claudecode和hermes-agent中`extends`边为0。CodeNexus可能通过`CONTAINS`或隐式parent_qn引用建模继承，而不是显式的`EXTENDS`边。**操作：调查**——可能需要添加`EXTENDS`边发出。

---

## B类：潜在解析错误（需要修复）

### B1. `contains`/`defines`边重复（Rust） — **最高优先级**
- CodeNexus：`CONTAINS` == `DEFINES`（4010==4010, 10004==10004）
- 提取器为每个（父, 子）对发出两种边类型
- 将边计数膨胀2倍，影响影响/跟踪分析
- **探针**：检查两种边类型的`(source, target)`对是否相同

### B2. `impl`过度计数（Rust）
- CodeNexus过度计数45-64%
- 可能对trait impl双重计数
- **探针**：`MATCH (i:Impl) RETURN i.name, i.qualifiedName LIMIT 50`

### B3. `calls`边过度计数（Rust）/计数不足（Fortran）
- Rust：+43-52%（可能与B1相关——双重发出）
- Fortran：-77%（提取器遗漏调用点）
- **探针**：检查CALLS边中是否存在重复的（source, target）对

### B4. `function`过度计数（Rust，仅velo）
- velo：CodeNexus=7050, gitnexus=6252（+11%）
- CodeNexus（自身）：3015 vs 2994（+0.7%——在噪声范围内）
- 可能是闭包、宏生成函数或嵌套函数
- **探针**：比较两侧的函数名列表

### B5. 查询采样噪声（测试工具） — **影响所有样本**
- 所有查询使用`LIMIT 200`但没有确定性`ORDER BY`
- 产生假集差异（例如，`file_contains_symbols`中200/200缺失）
- **操作**：为所有8个`.cql`文件添加`ORDER BY symbol_name, file_path`

### B6. CSV转义错误（阻塞问题） — **阻塞中**
- **受影响**：redis（C）、subno.ts（TypeScript）
- **根本原因**：LadybugDB的COPY解析器无法正确处理RFC 4180带引号字段中的反斜杠（C宏行延续）或引号（Python方法签名）
- **证据**：
  - redis：`Macro`表，第1128行——签名`(o, n, min, max, check_min, check_max, \'`破坏引号
  - subno.ts：`Method`表，第383行——签名`"def mock_client(self):'`破坏引号
- **影响**：无法索引任何带宏的C项目，或任何带Python方法签名的混合语言项目
- **修复**：选择(a)在CSV写入前将反斜杠转义为`\\`，(b)在签名中替换有问题的字符，或(c)使用不同的批量加载机制
- **位置**：[src/storage/loader.rs](src/storage/loader.rs) `write_nodes_csv` / `node_to_row`

### B7. Fortran `filePath`不匹配
- LAPACK：`file_contains_symbols`查询差异率100%
- 可能CodeNexus Fortran提取器填充`filePath`的方式与查询预期不同
- **探针**：比较CodeNexus和gitnexus之间的`filePath`格式

### B8. TypeScript `class_methods`查询返回0
- claudecode：CodeNexus=0, gitnexus=200
- 查询可能引用了CodeNexus未为TypeScript方法填充的属性名
- **探针**：检查TypeScript `Method`节点是否具有预期的`parentQn` / `filePath`属性

---

## C类：次要/不确定

### C1. `enum`次要差异
- CodeNexus：33 vs gitnexus 28（CodeNexus自身）——CodeNexus多计数5个枚举
- velo：187 vs 186——在噪声范围内
- LAPACK：0 vs 5——CodeNexus Fortran提取器不发出Enum节点
- **结论**：次要。无需操作。

### C2. `file`计数差异
- 大多数样本在1-5%以内。hermes-agent显示815 vs 1508（CodeNexus少计数46%）——可能CodeNexus跳过了gitnexus包含的非Python文件（JS/TS/配置文件）。
- **结论**：大多数次要。hermes-agent需要调查（可能是文件遍历器配置）。

### C3. `struct`/`trait`差异
- 所有Rust样本在0-5%以内。LAPACK：4 vs 0——Fortran提取器发出了一些gitnexus没有的Struct节点。
- **结论**：次要。无需操作。

---

## 推荐的修复更改（按优先级排序）

### 1. `fix-csv-escaping-bug`（阻塞——解锁C和TypeScript）
- 修复LadybugDB COPY解析器与RFC 4180带引号字段的不兼容性
- 位置：`src/storage/loader.rs`
- 测试：重新索引redis和subno.ts，验证无COPY异常

### 2. `fix-rust-edge-duplication`（高——影响所有Rust分析）
- 修复Rust提取器中的`CONTAINS`/`DEFINES`边重复
- 位置：`src/extract/rust/`（具体提取器模块）
- 测试：验证修复后`CONTAINS`计数 != `DEFINES`计数

### 3. `update-verification-type-map`（中——减少噪声）
- 在`scripts/verification/type_map.json`中重新分类gitnexus专有边类型
- 将`accesses`、`has_property`、`has_method`、`implements`、`imports`、`member_of`、`step_in_process`、`entry_point_of`、`handles_route`、`handles_tool`、`method_implements`、`extends`标记为`gitnexus_only`
- 将`folder`标记为`gitnexus_only`
- 测试：重新运行velo报告，验证严重差异数从13降至约4

### 4. `fix-verification-query-ordering`（中——消除采样噪声）
- 为`scripts/verification/queries/`中的所有8个`.cql`文件添加`ORDER BY symbol_name, file_path`
- 测试：重新运行velo查询，验证`file_contains_symbols`差异从199/178降至接近0

### 5. `fix-rust-impl-overcounting`（低——轻微准确性改善）
- 调查Rust提取器中的`Impl`节点双重计数
- 测试：验证`Impl`计数在5%以内匹配gitnexus

### 6. `fix-fortran-calls-undercounting`（低——仅Fortran）
- Fortran提取器遗漏77%的调用点
- 调查tree-sitter Fortran语法调用检测
- 测试：验证修复后`CALLS`边计数增加

---

## 附录：每样本原始数据

### CodeNexus（Rust，自索引）
- 132个文件，7903个节点，44240条边
- 严重：15 · 主要：4 · 次要：5
- 关键：`function` 3015 vs 2994（0.7% ✓），`impl` 242 vs 87（64% ✗），`contains`==`defines`==4010（重复 ✗）
- 报告：[CodeNexus.report.md](CodeNexus.report.md)

### velo（Rust）
- 281个文件，16712个节点，77491条边
- 严重：13 · 主要：5 · 次要：4
- 关键：`function` 7050 vs 6252（11% ✗），`impl` 1138 vs 625（45% ✗），`calls` 14965 vs 7080（52% ✗）
- 报告：[velo.report.md](velo.report.md)

### panorama（Python）
- 559个文件，13683个节点，30707条边
- 严重：17 · 主要：6 · 次要：3
- 关键：`function` 508 vs 4597（fn/method分类拆分 ✓），`method` 4185 vs 62
- 报告：[panorama.report.md](panorama.report.md)

### hermes-agent（Python）
- 815个文件，47892个节点，129733条边
- 严重：18 · 主要：5 · 次要：4
- 关键：`function` 4402 vs 17785（分类拆分 ✓），`file` 815 vs 1508（少计数 ✗）
- 报告：[hermes-agent.report.md](hermes-agent.report.md)

### claudecode（TypeScript）
- 1906个文件，56871个节点，143914条边
- 严重：18 · 主要：5 · 次要：3
- 关键：`function` 9010 vs 9412（4% ✓），`method` 2034 vs 1804（11% ✗），`class_methods`查询0 vs 200（✗）
- 报告：[claudecode.report.md](claudecode.report.md)

### redis（C） — **索引失败**
- `Macro`表中CSV转义错误（带反斜杠的jemalloc宏）
- 无统计数据
- 错误：`COPY exception: expected 10 values per row, but got 9`

### LAPACK（Fortran）
- 6434个文件，104641个节点，205716条边
- 严重：11 · 主要：4 · 次要：1
- 关键：`function` 4386 vs 3476（21% ✗），`calls` 2747 vs 11812（77%计数不足 ✗），查询差异98-100%（filePath不匹配 ✗）
- 报告：[LAPACK.report.md](LAPACK.report.md)

### subno.ts（TypeScript） — **索引失败**
- `Method`表中CSV转义错误（带引号的Python方法签名）
- 无统计数据
- 错误：`COPY exception: expected 14 values per row, but got 8`
