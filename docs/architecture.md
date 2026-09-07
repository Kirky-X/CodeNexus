# 架构分析与架构图命令

本文档说明 `architecture`、`diagram`、`arch_diff` 三个命令的输出语义。
引入于 absorb-archify 变更（0.4.0 破坏性窗口）。

## architecture

`codenexus architecture --project <p>` 输出 `ArchitectureOverview` JSON，
包含模块边界、依赖方向、分层与跨服务依赖。

### cross_service_deps 的取值语义（行为修正）

`CrossServiceDep` 的 `from_module` / `to_module` 字段文档契约为**模块名**
（文件目录路径，如 `/src/api`）。在 0.4.0 之前的实现中，这两个字段被错误
地填入了**节点 id**（caller 函数 id 与 route 节点 id）。

自本版本起修正为模块名：

- `from_module` = 发起 fetch/gRPC 等调用的函数所在文件的目录；
- `to_module` = 被调用 Route 的处理函数（`HANDLES_ROUTE` 边的 source）所在
  文件的目录；
- 调用方与被调用方位于同一模块时不再产出记录（同模块调用不构成跨服务依
  赖）；callee 无法映射到模块（如 Route 无处理函数）时同样跳过。

依赖旧 id 取值的外部脚本需要按目录路径重新对齐。

## diagram

`codenexus diagram --project <p> --output <path>` 将架构 IR 确定性编译为
自包含交互式 HTML（确定性网格布局、正交路由、暗/亮主题、焦点与上游/下游
可达、路由探测、`/` 搜索、深链），并以 BLAKE3 回执报告交付产物哈希与质检
结果。可选参数：

| 参数 | 默认 | 说明 |
| --- | --- | --- |
| `--quality` | `standard` | `showcase` 档位下任何告警升级为错误并拒绝交付 |
| `--repo_root` | 空 | 提供 Git 工作树根目录时验证源码证据并挂 `SRC n` 徽标 |
| `--repo_url` | 空 | 校验 origin URL 一致性 |
| `--title` / `--locale` | 自动 | 标题与界面语言（`en` / `zh-CN`） |

组件类型（interface / service / storage / model）来自真实的分层事实
（Controller / Service / Repository / Model 层级分类），不基于命名猜测。

## arch_diff

`codenexus arch_diff --base_project <a> --head_project <b> --output <path>`
对两个已索引项目的架构 IR 做 canonical 规范化 + 实体级对比，输出
Before / Delta / After 三节 HTML 与机器回执 JSON（`<path>.receipt.json`，
两文件原子成对写入）。每条变更携带：

- `kind`：`added` / `removed` / `changed`；
- `changed_fields`：JSON Pointer（如 `/sublabel`）；
- `classification`：`topology`（实体增删）或 `semantic`（属性变化）。

对比在模块粒度架构事实层进行；符号级行变化请使用 `detect_changes`。
