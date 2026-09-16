# graph-viewer E2E 回归

基于 Playwright（headless Chromium + SwiftShader WebGL）的黑盒 GUI 回归脚本。

## 运行前提

1. dev server 已启动：`npm run dev`（端口 5174）
2. Playwright 可用：优先 `require("playwright")`（本机安装于全局 npm root），
   找不到时可用 `PLAYWRIGHT_PATH` 环境变量指定模块路径

## 脚本

| 脚本 | 覆盖 |
|------|------|
| `regression.cjs` | 13 项主链路回归：着陆页、Demo 渲染、悬停 tooltip、节点点击、详情面板、追踪激活/清除、i18n 切换、文件树交互、HUD 去重、筛选、返回、大文件提示 |
| `garrison.cjs` | 真实 .lbug 加载（默认 `../.codenexus/garrison.lbug`，可用 `GARRISON_LBUG` 覆盖）：WASM 解析、计数、悬停、面板 |

```bash
node tests/e2e/regression.cjs
GARRISON_LBUG=/path/to/xxx.lbug node tests/e2e/garrison.cjs
```

截图证据输出到 `gui-test-screenshots/`（已在 .gitignore）。

## 已知测试约束（非应用 bug）

- headless 需 `--enable-unsafe-swiftshader` 系参数启用软件 WebGL
- 节点命中测试用栅格扫描（球体在屏幕上仅约 10px）
- 详情面板内的关系链接定位需限定在 `main > div:last-child` 容器内，
  否则会误中文件树叶子按钮（树叶子点击语义是选择文件，不是导航）
