# 🚀 发布对账清单（Release Reconciliation Checklist）

> 发布每个版本前逐项核对。背景：skill 文档曾出现 "28 subcommands / 8 languages"、
> README 曾出现 "6 个可运行示例"（实际 14 个）的数字漂移——skill 是 agent 的第一入口，
> 过期数字会直接造成调用错误，README 示例索引缺失会让新示例不可发现。
> 本清单确保**代码为唯一事实源，文档数字在发布前对账**。

## 对账项（代码 → 文档）

| # | 事实源（数这里） | 当前基准 | 必须对齐的文档 | 对齐方法 |
|---|------------------|----------|----------------|----------|
| 1 | CLI 子命令数：`grep -rn "cli = true" src/service/*.rs`（排除 `mod.rs` 注释行） | 37 个注册子命令（`codenexus mcp` 是 main.rs 字符串拦截，非注册子命令） | `README.md` / `README_EN.md`「CLI 命令」节、`docs/USER_GUIDE.md`、`skill/SKILL.md`、`skill/references/commands.md` | 注册数写作 "37 subcommands"；含 `mcp` 服务模式的清单写作 "37 + mcp"，勿笼统写 38 |
| 2 | MCP 工具数：`grep -rn 'tool_name = "' src/service/*.rs \| grep -v hook` | 10 个（query/trace/impact/search/context/architecture/diagram/arch_diff/dead_code/detect_changes） | `README.md` / `README_EN.md`「MCP 集成」节、`docs/USER_GUIDE.md`、`docs/FAQ.md`、`skill/references/commands.md` | 新增 MCP 工具时同步 `sdforge::mcp` 工具面描述 |
| 3 | 语言数：`grep -c '^lang-' Cargo.toml` | 22 种（每语言一个 `lang-*` feature） | `skill/SKILL.md`、`skill/references/appendix.md` 语言表、README feature 表 | appendix 语言表的扩展名以 `src/model/language.rs` 为准，节点类型以 `src/parse/<lang>.rs` 的 `NodeLabel::` 为准 |
| 4 | 示例数：`grep -c '\[\[bin\]\]' examples/Cargo.toml` | 14 个 | `README.md` / `README_EN.md`「示例」节（表格 + run-all 循环） | 新增示例 bin 必须同 PR 更新两份 README 的表格与 for 循环 |
| 5 | 命令参数：`src/service/<cmd>.rs` 的 `#[forge]` CLI wrapper 签名 | — | `skill/references/commands.md` 对应条目 | 新增/改签名命令必须同步 commands.md 的 Options 列表与 sentinel 默认值（`src/main.rs` `SENTINEL_DEFAULTS`） |
| 6 | advisory 忽略清单 | `deny.toml`（单一事实源） | `.cargo/audit.toml`（同步镜像；cargo-audit 只读该路径或 `~/.cargo/audit.toml`，根目录同名文件不会被加载）；CI audit-check ignore 由 deny.toml grep 生成 | 只改 deny.toml 并更新复核日期，再手动同步 `.cargo/audit.toml`；注释中只允许出现被 ignore 的 RUSTSEC 编号 |
| 7 | 基库依赖口径 | `Cargo.toml`（trait-kit/sdforge/oxcache/inklog/limiteron/lbug/vecboost 等 base 库一律 crates.io version req，**无** path / `[patch.crates-io]`——vecboost 0.3.0-rc.2 起回归 crates.io，git patch 临时安排已随上游 rc.2 发布解除） | `Cargo.lock`（刷新后同 commit 入库） | 升级基库时同步 version req 至已发布最新并刷新 lockfile；禁止重新引入跨仓 path/patch（本地联调用 `cargo patch` 类临时手段且不入库）；release.yml 的 patch 守卫保留为常驻保险（再出现 patch 生效期间会拒绝 crates.io 发布） |
| 8 | 自上个 tag 以来的行为变更：`git log --oneline $(git describe --tags --abbrev=0)..HEAD`（重点 feat/fix/perf） | — | `docs/CHANGELOG.md` 的 `## [Unreleased]` | tag 后的行为修复/新增/变更须先登记 `[Unreleased]`；发布时整体并入新版本段，不保留空段 |

## 机械校验（发布前跑一遍）

```bash
# 1. 命令数（应为 37）
grep -rn "cli = true" src/service/*.rs | grep -v "//" | wc -l
# 2. MCP 工具数（应为 10）
grep -rn 'tool_name = "' src/service/*.rs | grep -v hook | wc -l
# 3. 语言数（应为 22）
grep -c '^lang-' Cargo.toml
# 4. 示例数（应为 14）
grep -c '\[\[bin\]\]' examples/Cargo.toml
# 5. 文档中不得再出现的过期口径
grep -rn "28 subcommand\|8 languages\|Supported languages (8)\|6 个可运行示例\|6 runnable" README.md README_EN.md docs/ skill/ && echo "发现过期数字！" || echo "OK"

# 6. Unreleased 登记（上个 tag 之后存在 feat/fix/perf 时必须存在）
if git log --oneline "$(git describe --tags --abbrev=0)"..HEAD --grep='^\(feat\|fix\|perf\)' | grep -q .; then
  grep -q '^## \[Unreleased\]' docs/CHANGELOG.md && echo "OK" || echo "缺少 [Unreleased] 登记！"
else
  echo "OK（无行为变更）"
fi
```

> 上述 1–4 的期望值随版本增长，**以对账项表格中事实源命令的输出为准**，不要回填本文件的数字。
