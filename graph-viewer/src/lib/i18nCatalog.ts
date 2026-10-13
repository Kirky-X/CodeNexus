// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

/* 国际化语言包与取值函数 — React-free，供非组件模块直接引用
 *
 * worker（lbugWasm/lbug.worker）与非组件模块（api/client、lib/snapshot）
 * 都从这里取值；React 组件走 i18n.tsx 的 useI18n().t()，同一份字典，
 * 避免出现第二套文案，也避免把 React 拖进 worker 包。 */

export type Locale = "zh" | "en";

/* 检测浏览器语言 */
function detectLocale(): Locale {
  if (typeof navigator === "undefined") return "en";
  const lang = navigator.language.toLowerCase();
  return lang.startsWith("zh") ? "zh" : "en";
}

/* 初始语言：持久化偏好优先，否则检测浏览器语言。
 * 不能先渲染检测结果再异步读偏好——那样挂载时的持久化 effect
 * 会先覆写 localStorage，用户选择永远丢失。 */
function initialLocale(): Locale {
  try {
    const saved = localStorage.getItem("codenexus-locale");
    if (saved === "zh" || saved === "en") return saved;
  } catch { /* 隐私模式、worker 线程（无 localStorage）等场景：退回检测 */ }
  return detectLocale();
}

/* 当前生效 locale — 与 Provider 初始化同一优先级（持久化偏好 > 浏览器检测）。
 * worker 内读不到 localStorage，只能落到浏览器语言检测；用户显式切换的偏好
 * 在 worker 抛出的错误文案里可能与之不一致。 */
export function resolveLocale(): Locale {
  return initialLocale();
}

/* 消息模板占位符 — 提升为模块常量，避免每次带 vars 的 translate 重复编译正则 */
const VAR_PATTERN = /\{(\w+)\}/g;

/* 语言包取值 — vars 替换消息模板中的 `{name}` 占位符，未提供的占位符原样保留 */
export function translate(locale: Locale, key: string, vars?: Record<string, string | number>): string {
  const message = dictionaries[locale][key] ?? key;
  if (!vars) return message;
  return message.replace(VAR_PATTERN, (m, name) => (name in vars ? String(vars[name]) : m));
}

/* 翻译字典 */
const dictionaries: Record<Locale, Record<string, string>> = {
  en: {
    /* Document */
    "webui.documentTitle": "CodeNexus Code Knowledge Graph",
    /* Landing */
    "nav.subtitle": "Graph Viewer",
    "hero.title": "Code Knowledge Graph",
    "hero.subtitle": "Interactive visualization of code relationships, dependencies, and call traces",
    "landing.dropFile": "Drop .lbug file here",
    "landing.orSelectFile": "or click to select file",
    "landing.acceptsLbug": "Accepts .lbug database files",
    "landing.demo": "Demo Mode",
    "landing.loadingFile": "Loading database...",
    /* Loading / Error / Empty */
    "loading.text": "Loading graph data",
    "loading.stageCopy": "Reading database into local engine…",
    "loading.stageOpen": "Opening database…",
    "loading.stageAnalyze": "Analyzing graph relations…",
    "loading.largeFile": "Large file — this can take a while",
    "loading.fileMayOOM": "This file may exceed available memory — loading could fail",
    "error.title": "Failed to load graph",
    "error.retry": "Retry",
    "error.back": "Back",
    "error.worker": "Worker error",
    "error.invalidFileType": "Please select a .lbug file",
    "error.loadDatabase": "Failed to load database",
    "error.snapshot": "Failed to parse snapshot",
    "empty.text": "No graph data available",
    /* Worker / WASM / snapshot — 非组件模块（api/client、lib/lbugWasm、lib/lbug.worker、lib/snapshot）取值 */
    "error.wasmInit": "WASM engine initialization failed: {detail}",
    "error.vfsWrite": "Failed to write the virtual file system: {detail}",
    "error.dbOpen": "Failed to open database: {detail}",
    "error.cypherQuery": "Cypher query failed: {detail}",
    "error.missingFile": "Missing file",
    "error.dbNotLoaded": "Database not loaded",
    "error.unknownMessageType": "Unknown message type: {type}",
    "error.snapshotNoComma": "snapshot data URI is missing the comma separator",
    "error.snapshotTooLarge": "snapshot exceeds the size limit ({size} > {max} bytes)",
    "error.snapshotInvalidJson": "snapshot is not valid JSON",
    "error.snapshotNotObject": "snapshot must be a JSON object",
    "error.snapshotVersion": "unsupported snapshot version: {version}",
    "error.snapshotNodesType": "snapshot.nodes must be an array",
    "error.snapshotEdgesType": "snapshot.edges must be an array",
    "error.snapshotNodeId": "snapshot.nodes[{index}].id is missing",
    "error.snapshotNodeLabel": "snapshot.nodes[{index}].label is missing",
    "error.snapshotEdgeEndpoints": "snapshot.edges[{index}] is missing source/target",
    "error.snapshotEdgeDangling": "snapshot.edges[{index}] references a node that does not exist",
    "log.workerDeserializeFailed": "[worker onmessageerror] deserialization failed",
    "log.lbugWasmInitFailed": "[lbugWasm] WASM module initialization failed:",
    "log.lbugWasmVfsWriteFailed": "[lbugWasm] failed to write to VFS:",
    "log.lbugWasmDbLoaded": "[lbugWasm] loaded {name} ({size}MB, bufferPool={pool})",
    "log.lbugWasmDbOpenFailed": "[lbugWasm] failed to open database:",
    "common.default": "default",
    /* GraphScene WebGL2 fallback */
    "scene.webgl2Title": "Failed to initialize WebGL2 context",
    "scene.webgl2Detail": "Your browser or GPU does not support WebGL2. Try a modern browser such as Chrome/Edge, or update your graphics driver and retry.",
    /* ErrorBoundary */
    "error.boundaryTitle": "Something went wrong",
    "error.boundaryReload": "Reload",
    "error.boundaryOOM": "Out of memory: this file is too large for this device. Close other tabs and retry, or use a smaller database file.",
    "error.boundaryWebGL": "Graphics initialization failed: your browser or GPU does not support WebGL2. Try a different browser or update your graphics driver.",
    /* Header */
    "header.file": "File",
    "header.callTrace": "Call Trace",
    "header.variableTrace": "Variable Trace",
    "header.selectFile": "Select File",
    "header.clearTrace": "Clear trace",
    "header.memoryMode": "Memory saver",
    "header.nodeLimit": "Node limit",
    "header.home": "Back to start",
    /* Graph HUD */
    "hud.nodes": "nodes",
    "hud.edges": "edges",
    "hud.filtered": "Filtered from",
    "hud.filteredSuffix": "nodes",
    "hud.trace": "Trace:",
    "hud.traceNodes": "nodes",
    "hud.traceEdges": "edges",
    "hud.clear": "Clear",
    "hud.noFiltered": "All nodes filtered",
    "hud.noRelations": "No relation data in this database — showing nodes only",
    "hud.resetFilters": "Reset Filters",
    /* FilterPanel */
    "filter.title": "Filters",
    "filter.filePath": "File Path",
    "filter.filePlaceholder": "Filter by file path...",
    "filter.nodeTypes": "Node Types",
    "filter.selectAll": "All",
    "filter.selectNone": "None",
    "filter.edgeTypes": "Edge Types",
    "filter.showLabels": "Show Labels",
    /* Sidebar */
    "sidebar.title": "File Tree",
    "sidebar.searchPlaceholder": "Search nodes or files...",
    "sidebar.noResults": "No matches",
    "sidebar.clearSelection": "Clear Selection",
    /* NodeModal */
    "modal.path": "Path",
    "modal.qn": "QN",
    "modal.project": "Project",
    "modal.outbound": "Outbound",
    "modal.inbound": "Inbound",
    "modal.total": "Total",
    "modal.references": "References",
    "modal.referencedBy": "Referenced By",
    "modal.noConnections": "No connections found",
    "modal.close": "Close",
    "modal.callTrace": "Call Trace",
    "modal.variableTrace": "Variable Trace",
    /* NodeTooltip */
    "tooltip.project": "Project:",
    /* NODE_LABEL_GROUPS */
    "group.structure": "Structure",
    "group.typeDef": "Type Def",
    "group.callable": "Callable",
    "group.variable": "Variable",
    "group.meta": "Meta",
    "group.template": "Template",
    "group.runtime": "Runtime",
    "group.infra": "Infra",
    "group.quality": "Quality",
    "group.extension": "Extension",
  },

  zh: {
    /* Document */
    "webui.documentTitle": "CodeNexus 代码知识图谱",
    /* Landing */
    "nav.subtitle": "图谱查看器",
    "hero.title": "代码知识图谱",
    "hero.subtitle": "交互式代码关系、依赖与调用追踪可视化",
    "landing.dropFile": "拖放 .lbug 文件到此处",
    "landing.orSelectFile": "或点击选择文件",
    "landing.acceptsLbug": "支持 .lbug 数据库文件",
    "landing.demo": "演示模式",
    "landing.loadingFile": "正在加载数据库...",
    /* Loading / Error / Empty */
    "loading.text": "正在加载图数据",
    "loading.stageCopy": "正在读取数据库到本地引擎…",
    "loading.stageOpen": "正在打开数据库…",
    "loading.stageAnalyze": "正在分析图关系…",
    "loading.largeFile": "大文件加载中，可能需要较长时间",
    "loading.fileMayOOM": "文件可能超出当前可用内存——加载可能失败",
    "error.title": "图数据加载失败",
    "error.retry": "重试",
    "error.back": "返回",
    "error.worker": "Worker 错误",
    "error.invalidFileType": "请选择 .lbug 文件",
    "error.loadDatabase": "加载数据库失败",
    "error.snapshot": "snapshot 解析失败",
    "empty.text": "暂无图数据",
    /* Worker / WASM / snapshot — 非组件模块（api/client、lib/lbugWasm、lib/lbug.worker、lib/snapshot）取值 */
    "error.wasmInit": "WASM 引擎初始化失败: {detail}",
    "error.vfsWrite": "写入虚拟文件系统失败: {detail}",
    "error.dbOpen": "打开数据库失败: {detail}",
    "error.cypherQuery": "Cypher 查询失败: {detail}",
    "error.missingFile": "缺少文件",
    "error.dbNotLoaded": "未加载数据库",
    "error.unknownMessageType": "未知消息类型: {type}",
    "error.snapshotNoComma": "snapshot data URI 缺少逗号分隔符",
    "error.snapshotTooLarge": "snapshot 超过大小上限（{size} > {max} 字节）",
    "error.snapshotInvalidJson": "snapshot 不是合法 JSON",
    "error.snapshotNotObject": "snapshot 必须是 JSON 对象",
    "error.snapshotVersion": "不支持的 snapshot 版本: {version}",
    "error.snapshotNodesType": "snapshot.nodes 必须是数组",
    "error.snapshotEdgesType": "snapshot.edges 必须是数组",
    "error.snapshotNodeId": "snapshot.nodes[{index}].id 缺失",
    "error.snapshotNodeLabel": "snapshot.nodes[{index}].label 缺失",
    "error.snapshotEdgeEndpoints": "snapshot.edges[{index}] 缺少 source/target",
    "error.snapshotEdgeDangling": "snapshot.edges[{index}] 引用不存在的节点",
    "log.workerDeserializeFailed": "[worker onmessageerror] 反序列化失败",
    "log.lbugWasmInitFailed": "[lbugWasm] WASM 模块初始化失败:",
    "log.lbugWasmVfsWriteFailed": "[lbugWasm] 写入 VFS 失败:",
    "log.lbugWasmDbLoaded": "[lbugWasm] 已加载 {name}（{size}MB，bufferPool={pool}）",
    "log.lbugWasmDbOpenFailed": "[lbugWasm] 打开数据库失败:",
    "common.default": "默认",
    /* GraphScene WebGL2 fallback */
    "scene.webgl2Title": "无法初始化 WebGL2 图形上下文",
    "scene.webgl2Detail": "当前浏览器或显卡不支持 WebGL2。请更换 Chrome/Edge 等现代浏览器，或更新显卡驱动后重试。",
    /* ErrorBoundary */
    "error.boundaryTitle": "出错了",
    "error.boundaryReload": "重新加载",
    "error.boundaryOOM": "内存不足：该文件对当前设备而言过大，请关闭其他标签页后重试，或使用更小的数据库文件。",
    "error.boundaryWebGL": "图形初始化失败：浏览器或显卡不支持 WebGL2，请更换浏览器或更新显卡驱动。",
    /* Header */
    "header.file": "文件",
    "header.callTrace": "调用追踪中",
    "header.variableTrace": "变量追踪中",
    "header.selectFile": "选择文件",
    "header.clearTrace": "清除追踪",
    "header.memoryMode": "省内存模式",
    "header.nodeLimit": "节点上限",
    "header.home": "返回首页",
    /* Graph HUD */
    "hud.nodes": "节点",
    "hud.edges": "边",
    "hud.filtered": "已从",
    "hud.filteredSuffix": "个节点中筛选",
    "hud.trace": "追踪:",
    "hud.traceNodes": "个节点",
    "hud.traceEdges": "条边",
    "hud.clear": "清除",
    "hud.noFiltered": "所有节点已被过滤",
    "hud.noRelations": "库中未检测到关系数据——仅展示节点",
    "hud.resetFilters": "重置筛选",
    /* FilterPanel */
    "filter.title": "筛选",
    "filter.filePath": "文件路径",
    "filter.filePlaceholder": "按文件路径筛选...",
    "filter.nodeTypes": "节点类型",
    "filter.selectAll": "全选",
    "filter.selectNone": "全不选",
    "filter.edgeTypes": "关系类型",
    "filter.showLabels": "显示标签",
    /* Sidebar */
    "sidebar.title": "文件目录",
    "sidebar.searchPlaceholder": "搜索节点或文件...",
    "sidebar.noResults": "无匹配结果",
    "sidebar.clearSelection": "清除选择",
    /* NodeModal */
    "modal.path": "路径",
    "modal.qn": "限定名",
    "modal.project": "项目",
    "modal.outbound": "出向",
    "modal.inbound": "入向",
    "modal.total": "总计",
    "modal.references": "引用",
    "modal.referencedBy": "被引用",
    "modal.noConnections": "无连接关系",
    "modal.close": "关闭",
    "modal.callTrace": "函数调用追踪",
    "modal.variableTrace": "变量使用追踪",
    /* NodeTooltip */
    "tooltip.project": "项目:",
    /* NODE_LABEL_GROUPS */
    "group.structure": "结构",
    "group.typeDef": "类型定义",
    "group.callable": "可调用",
    "group.variable": "变量",
    "group.meta": "元信息",
    "group.template": "模板",
    "group.runtime": "运行时",
    "group.infra": "基础设施",
    "group.quality": "质量/文档",
    "group.extension": "扩展",
  },
};
