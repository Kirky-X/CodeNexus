/* 节点标签 → 颜色映射（按语义分组着色） */

const LABEL_COLORS: Record<string, string> = {
  // 结构 — 暖色系
  Project: "#f472b6",
  Folder: "#34d399",
  File: "#60a5fa",
  Module: "#fb923c",
  // 类型定义 — 紫色系
  Class: "#c084fc",
  Struct: "#c084fc",
  Enum: "#c084fc",
  Trait: "#a78bfa",
  Impl: "#a78bfa",
  Interface: "#a78bfa",
  Union: "#c084fc",
  Variant: "#c084fc",
  Field: "#d8b4fe",
  Record: "#c084fc",
  Typedef: "#d8b4fe",
  // 可调用 — 青色系
  Function: "#22d3ee",
  Method: "#22d3ee",
  Constructor: "#67e8f9",
  // 变量 — 冷灰色系
  Variable: "#94a3b8",
  GlobalVar: "#a1b5c9",
  Parameter: "#94a3b8",
  Const: "#7c8da3",
  Static: "#7c8da3",
  Property: "#94a3b8",
  // 元信息 — 琥珀色系
  Macro: "#fbbf24",
  TypeAlias: "#fcd34d",
  Namespace: "#f59e0b",
  Delegate: "#fcd34d",
  Annotation: "#fcd34d",
  // 模板
  Template: "#fb923c",
  // 运行时 — 玫系
  Event: "#f472b6",
  Handler: "#fb7185",
  Middleware: "#f87171",
  Service: "#f472b6",
  Endpoint: "#fb923c",
  Route: "#fbbf24",
  Process: "#f87171",
  // 基础设施 — 绿色系
  Database: "#34d399",
  Config: "#4ade80",
  // 质量/文档
  Test: "#4ade80",
  Section: "#86efac",
  // 扩展
  Community: "#a78bfa",
  Tool: "#c4b5fd",
  Embedding: "#8b5cf6",
};

const DEFAULT_COLOR = "#94a3b8";

export function colorForLabel(label: string): string {
  return LABEL_COLORS[label] ?? DEFAULT_COLOR;
}

/* 边类型 → 颜色 */
const EDGE_COLORS: Record<string, string> = {
  CONTAINS: "#34d399",
  DEFINES: "#60a5fa",
  MEMBER_OF: "#fb923c",
  CALLS: "#22d3ee",
  FFI_CALLS: "#06b6d4",
  DATAFLOWS: "#a78bfa",
  READS: "#94a3b8",
  WRITES: "#f87171",
  IMPLEMENTS: "#c084fc",
  EXTENDS: "#a78bfa",
  USES_TYPE: "#818cf8",
  REFERENCES: "#94a3b8",
  IMPORTS: "#34d399",
  INCLUDES: "#4ade80",
  HAS_METHOD: "#22d3ee",
  HAS_PROPERTY: "#94a3b8",
  ACCESSES: "#7c8da3",
  METHOD_OVERRIDES: "#e879f9",
  METHOD_IMPLEMENTS: "#c084fc",
  USAGE: "#7c8da3",
  TESTS: "#4ade80",
  HTTP_CALLS: "#fbbf24",
  ASYNC_CALLS: "#38bdf8",
  EMITS: "#f472b6",
  LISTENS_ON: "#fb7185",
  REEXPORTS: "#2dd4bf",
};

const DEFAULT_EDGE_COLOR = "#64748b";

export function colorForEdgeType(type: string): string {
  return EDGE_COLORS[type] ?? EDGE_COLORS[type.toUpperCase()] ?? DEFAULT_EDGE_COLOR;
}

/* 追踪路径颜色 */
export const TRACE_COLORS = {
  callChain: "#22d3ee",
  variableUse: "#fbbf24",
  origin: "#f87171",
  highlight: "#818cf8",
} as const;
