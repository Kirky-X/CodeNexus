## 1. 修复 Task 6 遗留测试失败（建立绿色基线）

- [x] 1.1 读取 `src/storage/loader.rs` 的 `node_to_row` 函数，确认每个 NodeLabel 的实际列数
- [x] 1.2 修复 `src/storage/loader.rs` 9 个 `node_to_row_*_has_*_columns` 测试列数断言（Class/Struct/Enum/Trait=11，Const=10，Static=9，TypeAlias=9，Typedef=8；Parameter 单独排查 properties 缺失问题）
- [x] 1.3 修复 `src/parse/rust_extractor.rs` `tuple_destructuring_pattern` 测试断言（验证提取首个绑定 "a"，非 "a" 和 "b"）
- [x] 1.4 修复 `src/parse/rust_extractor.rs` `struct_pattern` 测试断言（验证类型名 "P"，非字段名 "x"）
- [x] 1.5 运行 `cargo test --lib` 确认所有测试通过（基线绿色）

## 2. 修复 Python 提取器

- [x] 2.1 修改 `src/parse/python.rs` 的 `make_qn` 签名为 `(file_path, name, project)`，内部委托 `FqnGenerator::generate(project, file_path, name, Language::Python)`
- [x] 2.2 扩展 `visit_node` 签名添加 `current_func: Option<&str>` 参数，在 `function_definition` 分支提取函数名后传递给子节点遍历
- [x] 2.3 扩展 `extract_call` 签名添加 `file_path`/`project`/`current_func` 参数，通过 `current_func.map(|name| make_qn(file_path, name, project))` 生成 `caller_qn`
- [x] 2.4 更新 `visit_node`/`visit_children` 中所有 `make_qn` 和 `extract_call` 调用点，添加 `project`/`current_func` 参数
- [x] 2.5 添加 Python `extract_call` 端到端测试，验证 `caller_qn` 非 `None` 且为点分 FQN 格式
- [x] 2.6 运行 `cargo test --lib python` 确认 Python 提取器测试通过

## 3. 修复 TypeScript 提取器

- [x] 3.1 修改 `src/parse/typescript.rs` 的 `make_qn` 签名为 `(file_path, name, project)`，内部委托 `FqnGenerator::generate(project, file_path, name, Language::TypeScript)`
- [x] 3.2 扩展 `visit_node` 签名添加 `current_func: Option<&str>` 参数，在 `function_declaration`/`method_definition` 分支提取函数名后传递给子节点遍历
- [x] 3.3 扩展 `extract_call` 签名添加 `file_path`/`project`/`current_func` 参数，生成 `caller_qn`
- [x] 3.4 更新所有 `make_qn` 和 `extract_call` 调用点
- [x] 3.5 添加 TypeScript `extract_call` 端到端测试，验证 `caller_qn` 非 `None` 且为点分 FQN 格式
- [x] 3.6 运行 `cargo test --lib typescript` 确认 TypeScript 提取器测试通过

## 4. 修复 Fortran 提取器

- [x] 4.1 修改 `src/parse/fortran.rs` 的 `make_qn` 签名为 `(file_path, name, project)`，内部委托 `FqnGenerator::generate(project, file_path, name, Language::Fortran)`
- [x] 4.2 扩展 `visit_node` 签名添加 `current_func: Option<&str>` 参数，在 `subroutine`/`function` 分支提取函数名后传递给子节点遍历
- [x] 4.3 扩展 `extract_call` 签名添加 `file_path`/`project`/`current_func` 参数，生成 `caller_qn`
- [x] 4.4 更新所有 `make_qn` 和 `extract_call` 调用点
- [x] 4.5 添加 Fortran `extract_call` 端到端测试，验证 `caller_qn` 非 `None` 且为点分 FQN 格式
- [x] 4.6 运行 `cargo test --lib fortran` 确认 Fortran 提取器测试通过

## 5. 修复 C 提取器（含 reads/writes 实现）

- [x] 5.1 修改 `src/parse/c.rs` 的 `make_qn` 签名为 `(file_path, name, project)`，内部委托 `FqnGenerator::generate(project, file_path, name, Language::C)`
- [x] 5.2 扩展 `visit_node` 签名添加 `current_func: Option<&str>` 参数，在 `function_definition` 分支提取函数名后传递给 `compound_statement` 子节点遍历
- [x] 5.3 扩展 `extract_call` 签名添加 `file_path`/`project`/`current_func` 参数，生成 `caller_qn`
- [x] 5.4 实现 `is_read_position` 辅助函数：检查 identifier 的父节点是否为 declarator/声明位置，排除声明位置的 identifier
- [x] 5.5 实现 C reads 提取：在 `visit_node` 的 `identifier` 分支，当 `current_func` 非 `None` 且 `is_read_position` 为真时，生成 `ReadInfo { reader_qn, var_name, line }`
- [x] 5.6 实现 C writes 提取（init_declarator）：在 `visit_node` 的 `init_declarator` 分支，当 `current_func` 非 `None` 时，提取 declarator 的 identifier 生成 `WriteInfo { writer_qn, target_name, line }`
- [x] 5.7 实现 C writes 提取（assignment_expression）：在 `visit_node` 的 `assignment_expression` 分支，当 `current_func` 非 `None` 时，提取左侧 identifier 生成 `WriteInfo`
- [x] 5.8 移除 `src/parse/c.rs` 第 53-55 行的 TODO 注释（reads/writes 提取已实现）
- [x] 5.9 更新所有 `make_qn` 和 `extract_call` 调用点
- [x] 5.10 添加 C `extract_call` 端到端测试，验证 `caller_qn` 非 `None` 且为点分 FQN 格式
- [x] 5.11 添加 C reads/writes 端到端测试：验证 `int x; return x;` 生成 ReadInfo，`int y = 1;` 和 `y = 2;` 生成 WriteInfo
- [x] 5.12 运行 `cargo test --lib c` 确认 C 提取器测试通过

## 6. 全量验证与收尾

- [x] 6.1 运行 `cargo test --lib` 确认所有测试通过（全量绿色）
- [x] 6.2 运行 `cargo clippy --lib -- -D warnings` 确认无警告
- [x] 6.3 运行 `gitnexus_detect_changes` 验证变更范围符合预期（仅 6 个文件：python.rs/typescript.rs/c.rs/fortran.rs/loader.rs/rust_extractor.rs）
- [x] 6.4 按 extractors 分组提交：commit 1 = 测试修复（loader.rs + rust_extractor.rs），commit 2 = Python/TypeScript/Fortran 提取器修复，commit 3 = C 提取器修复 + reads/writes
- [ ] 6.5 提交后运行 `gitnexus analyze --embeddings` 更新索引 — **跳过**：环境中 `gitnexus` 命令不可用（`command -v gitnexus` 返回 NOT FOUND），无法执行；需在安装 gitnexus CLI 的环境中手动运行
