/* 代码知识图谱数据模型 — 对齐 CodeNexus 的 44 种 NodeLabel + 31 种 EdgeType */

/* ── 节点 ─────────────────────────────────────────── */

export interface GraphNode {
  id: string;
  label: NodeLabel;
  name: string;
  file_path?: string;
  project: string;
  qualified_name?: string;
  start_line?: number;
  end_line?: number;
  /* 力导向布局坐标（后端计算） */
  x: number;
  y: number;
  z: number;
}

/* CodeNexus 的 44 种节点标签，按分组分类 */
export type NodeLabel =
  | "Project" | "Folder" | "File" | "Module"
  | "Class" | "Struct" | "Enum" | "Trait" | "Impl"
  | "Function" | "Method"
  | "Variable" | "GlobalVar" | "Parameter" | "Const" | "Static"
  | "Macro" | "TypeAlias" | "Typedef" | "Namespace" | "Interface"
  | "Constructor" | "Property" | "Record" | "Delegate" | "Annotation"
  | "Template" | "Union" | "Variant" | "Field"
  | "Event" | "Handler" | "Middleware" | "Service" | "Endpoint" | "Route" | "Process"
  | "Database" | "Config"
  | "Test" | "Section"
  | "Community" | "Tool" | "Embedding";

/* 节点标签分组（用于筛选面板） */
export const NODE_LABEL_GROUPS: Record<string, NodeLabel[]> = {
  "结构": ["Project", "Folder", "File", "Module"],
  "类型定义": ["Class", "Struct", "Enum", "Trait", "Impl", "Union", "Variant", "Field", "Record", "Typedef"],
  "可调用": ["Function", "Method", "Constructor"],
  "变量": ["Variable", "GlobalVar", "Parameter", "Const", "Static", "Property"],
  "元信息": ["Macro", "TypeAlias", "Namespace", "Interface", "Delegate", "Annotation"],
  "模板": ["Template"],
  "运行时": ["Event", "Handler", "Middleware", "Service", "Endpoint", "Route", "Process"],
  "基础设施": ["Database", "Config"],
  "质量/文档": ["Test", "Section"],
  "扩展": ["Community", "Tool", "Embedding"],
};

/* ── 边 ─────────────────────────────────────────── */

export interface GraphEdge {
  id: string;
  source: string;
  target: string;
  edge_type: EdgeType;
  confidence: number;
  start_line?: number;
  project: string;
}

/* CodeNexus 的 31 种边类型 */
export type EdgeType =
  | "CONTAINS" | "DEFINES" | "MEMBER_OF"
  | "CALLS" | "FFI_CALLS" | "DATAFLOWS" | "READS" | "WRITES"
  | "IMPLEMENTS" | "EXTENDS" | "USES_TYPE" | "REFERENCES"
  | "IMPORTS" | "INCLUDES"
  | "HAS_METHOD" | "HAS_PROPERTY" | "ACCESSES"
  | "METHOD_OVERRIDES" | "METHOD_IMPLEMENTS"
  | "STEP_IN_PROCESS" | "HANDLES_ROUTE" | "FETCHES"
  | "HANDLES_TOOL" | "ENTRY_POINT_OF"
  | "USAGE" | "TESTS" | "HTTP_CALLS" | "ASYNC_CALLS"
  | "EMITS" | "LISTENS_ON" | "REEXPORTS";

/* 追踪相关的边类型 */
export const CALL_EDGE_TYPES: EdgeType[] = [
  "CALLS", "FFI_CALLS", "HTTP_CALLS", "ASYNC_CALLS",
];

export const VARIABLE_EDGE_TYPES: EdgeType[] = [
  "READS", "WRITES", "ACCESSES", "DATAFLOWS",
];

/* ── 图数据 ─────────────────────────────────────── */

export interface GraphData {
  nodes: GraphNode[];
  edges: GraphEdge[];
  total_nodes: number;
  total_edges: number;
}

export interface ProjectInfo {
  name: string;
  root_path: string;
  db_path: string;
  node_count: number;
  edge_count: number;
}

export interface SchemaInfo {
  node_labels: { label: string; count: number }[];
  edge_types: { type: string; count: number }[];
  total_nodes: number;
  total_edges: number;
}

/* ── 追踪结果 ─────────────────────────────────── */

export interface TracePath {
  nodes: GraphNode[];
  edges: GraphEdge[];
}

export interface TraceResult {
  origin: GraphNode;
  paths: TracePath[];
  direction: "downstream" | "upstream" | "both";
}

/* ── 追踪模式 ─────────────────────────────────── */

export type TraceMode = "none" | "call" | "variable";
