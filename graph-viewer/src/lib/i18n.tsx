/* 国际化 — 轻量级 i18n 上下文，支持中/英文自动检测 */

import { createContext, useContext, useState, useCallback, useEffect, type ReactNode } from "react";

export type Locale = "zh" | "en";

interface I18nContextValue {
  locale: Locale;
  setLocale: (l: Locale) => void;
  t: (key: string) => string;
}

const I18nContext = createContext<I18nContextValue>({
  locale: "en",
  setLocale: () => {},
  t: (k) => k,
});

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
  } catch { /* 隐私模式等场景：退回检测 */ }
  return detectLocale();
}

/* 翻译字典 */
const dictionaries: Record<Locale, Record<string, string>> = {
  en: {
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
    "empty.text": "No graph data available",
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
    "empty.text": "暂无图数据",
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

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocale] = useState<Locale>(initialLocale);

  const t = useCallback(
    (key: string): string => dictionaries[locale][key] ?? key,
    [locale],
  );

  /* 持久化用户选择 */
  useEffect(() => {
    try { localStorage.setItem("codenexus-locale", locale); } catch {}
  }, [locale]);

  return (
    <I18nContext.Provider value={{ locale, setLocale, t }}>
      {children}
    </I18nContext.Provider>
  );
}

export function useI18n() {
  return useContext(I18nContext);
}
