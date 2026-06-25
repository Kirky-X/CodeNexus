## Why

`fix-id-fqn-consistency` spec 修复了 `rust_extractor.rs` 的两个根因 bug（`make_qn` 格式不匹配 FqnGenerator + `extract_call` 的 `caller_qn: None`），但同一 bug 存在于其他四个语言提取器中（python.rs、typescript.rs、c.rs、fortran.rs）。这些提取器的节点 ID（`make_qn` 的 `file::name` 格式）与符号表/resolve 阶段（FqnGenerator 的 `project.dir.file.name` 点分格式）不一致，导致 CALLS 边无法匹配节点 ID，trace 命令对这些语言返回空结果。

## What Changes

- 修复 `src/parse/python.rs`：`make_qn` 委托 `FqnGenerator::generate`；`extract_call` 接收 `file_path`/`project`/`current_func` 参数，生成 `caller_qn`。
- 修复 `src/parse/typescript.rs`：同上。
- 修复 `src/parse/c.rs`：同上；同时实现 reads/writes 提取（BR-TRACE-005/006，当前标记为 TODO）。
- 修复 `src/parse/fortran.rs`：同上。
- 补充各提取器的 `extract_call` 端到端测试，验证 CALLS 边生成且 `caller_qn` 非 None。
- 修复 `fix-id-fqn-consistency` spec Task 6 遗留的 9 个 loader.rs 测试失败（`node_to_row` 列数断言错误）和 2 个 rust_extractor.rs 测试失败（tuple/struct pattern 写入断言）。

## Capabilities

### New Capabilities

- `multi-lang-extractor-fqn`: 多语言提取器 FQN 一致性 — 确保 Python/TypeScript/C/Fortran 提取器的 `make_qn` 和 `extract_call` 与 Rust 提取器保持一致，生成 spec 合规的点分 FQN 格式。

### Modified Capabilities

## Impact

- **Affected code**：
  - `src/parse/python.rs` — `make_qn` + `extract_call` + `visit_node` 调用点
  - `src/parse/typescript.rs` — 同上
  - `src/parse/c.rs` — 同上 + reads/writes 提取实现
  - `src/parse/fortran.rs` — 同上
  - `src/storage/loader.rs` — 修复 `node_to_row` 测试列数断言
  - `src/parse/rust_extractor.rs` — 修复 tuple/struct pattern 测试断言
- **Affected specs**：`fix-id-fqn-consistency`（前置 spec，本变更为其遗留问题的后续修复）
- **Risk**：四个提取器修改范围相似，可通过统一模式降低风险。C 提取器的 reads/writes 实现是新增功能，需额外的 tree-sitter 节点遍历逻辑。
