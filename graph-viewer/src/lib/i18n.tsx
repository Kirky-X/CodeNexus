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

/* 翻译字典 */
const dictionaries: Record<Locale, Record<string, string>> = {
  en: {
    /* Landing */
    "nav.subtitle": "Graph Viewer",
    "hero.title": "Code Knowledge Graph",
    "hero.subtitle": "Interactive visualization of code relationships, dependencies, and call traces",
    "tab.project": "Project",
    "tab.path": "LBUG Path",
    "input.project.placeholder": "Enter project name...",
    "input.path.placeholder": "/path/to/database.lbug",
    "input.path.hint": "Enter the absolute path to an .lbug database file. The file must exist on the server filesystem.",
    "landing.discovered": "Discovered Projects",
    "landing.nodes": "nodes",
    "landing.empty": "empty",
    "landing.demo": "Demo Mode",
    "landing.hint": "Requires Rust backend service on port 9800",
    /* Loading / Error / Empty */
    "loading.text": "Loading graph data",
    "error.title": "Failed to load graph",
    "error.retry": "Retry",
    "error.back": "Back",
    "empty.text": "No graph data available",
    /* Header */
    "header.db": "DB",
    "header.callTrace": "Call Trace",
    "header.variableTrace": "Variable Trace",
    "header.auto": "Auto",
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
    "hud.resetFilters": "Reset Filters",
    /* FilterPanel */
    "filter.projectName": "Project",
    "filter.projectPlaceholder": "Filter by project...",
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
    "tab.project": "项目名",
    "tab.path": "LBUG 路径",
    "input.project.placeholder": "输入项目名称...",
    "input.path.placeholder": "/path/to/database.lbug",
    "input.path.hint": "输入 .lbug 数据库文件的绝对路径，文件必须存在于服务器文件系统中。",
    "landing.discovered": "已发现的项目",
    "landing.nodes": "节点",
    "landing.empty": "空",
    "landing.demo": "演示模式",
    "landing.hint": "需要 Rust 后端服务运行在端口 9800",
    /* Loading / Error / Empty */
    "loading.text": "正在加载图数据",
    "error.title": "图数据加载失败",
    "error.retry": "重试",
    "error.back": "返回",
    "empty.text": "暂无图数据",
    /* Header */
    "header.db": "数据库",
    "header.callTrace": "调用追踪中",
    "header.variableTrace": "变量追踪中",
    "header.auto": "自动",
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
    "hud.resetFilters": "重置筛选",
    /* FilterPanel */
    "filter.projectName": "项目名称",
    "filter.projectPlaceholder": "按项目名筛选...",
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
  const [locale, setLocale] = useState<Locale>(detectLocale);

  const t = useCallback(
    (key: string): string => dictionaries[locale][key] ?? key,
    [locale],
  );

  /* 持久化用户选择 */
  useEffect(() => {
    try { localStorage.setItem("codenexus-locale", locale); } catch {}
  }, [locale]);

  /* 初始化时读取持久化偏好 */
  useEffect(() => {
    try {
      const saved = localStorage.getItem("codenexus-locale");
      if (saved === "zh" || saved === "en") setLocale(saved);
    } catch {}
  }, []);

  return (
    <I18nContext.Provider value={{ locale, setLocale, t }}>
      {children}
    </I18nContext.Provider>
  );
}

export function useI18n() {
  return useContext(I18nContext);
}
