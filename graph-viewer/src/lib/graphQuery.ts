/* 图数据查询 — 从 WASM LadybugDB 读取节点和边数据
 *
 * 移植自 server/src/graph_query.rs，使用相同的 Cypher 查询语句
 */

import type { LbugDatabase } from "./lbugWasm";
import type { GraphNode, GraphEdge, GraphData, SchemaInfo, TraceResult, TracePath } from "./types";

/* ── 常量 ─────────────────────────────────────────── */

/** 可查询的节点类型（按优先级排序，对齐后端 graph_query.rs） */
const QUERYABLE_LABELS: string[] = [
  "Function", "Method", "Class", "Struct", "Enum", "Trait", "Interface",
  "Impl", "Constructor", "Variable", "GlobalVar", "Const", "Static",
  "Macro", "TypeAlias", "Namespace", "Module", "Test", "Handler",
  "Middleware", "Service", "Endpoint", "Route", "Event", "Property",
  "Field", "Record", "Template", "Union", "Variant", "Annotation",
  "Delegate", "Typedef", "Section", "Database", "Config",
];

/** 球面半径 */
const SPHERE_RADIUS = 400;

/* ── Fibonacci sphere 坐标 ─────────────────────────── */

/** 3D 均匀球面坐标（Fibonacci sphere） */
function spherePosition(index: number, total: number): { x: number; y: number; z: number } {
  const phi = Math.acos(1 - (2 * (index + 0.5)) / Math.max(total, 1));
  const theta = Math.PI * (1 + Math.sqrt(5)) * index;
  return {
    x: SPHERE_RADIUS * Math.sin(phi) * Math.cos(theta),
    y: SPHERE_RADIUS * Math.sin(phi) * Math.sin(theta),
    z: SPHERE_RADIUS * Math.cos(phi),
  };
}

/** 为所有节点分配 Fibonacci sphere 坐标 */
function assignSpherePositions(nodes: GraphNode[]): void {
  const total = nodes.length;
  for (let i = 0; i < total; i++) {
    const { x, y, z } = spherePosition(i, total);
    nodes[i].x = x;
    nodes[i].y = y;
    nodes[i].z = z;
  }
}

/* ── 辅助函数 ─────────────────────────────────────── */

/** 从查询结果行中取字符串值 */
function getStr(row: Record<string, unknown>, name: string): string | undefined {
  const v = row[name];
  if (v == null) return undefined;
  return String(v);
}

/** 从查询结果行中取数值 */
function getNum(row: Record<string, unknown>, name: string): number | undefined {
  const v = row[name];
  if (v == null) return undefined;
  const n = Number(v);
  return Number.isNaN(n) ? undefined : n;
}

/** 解析 labels 字段 — 后端返回格式为 ["Function"] 或 Function */
function parseLabel(row: Record<string, unknown>, fallback: string): string {
  const labelsVal = getStr(row, "labels");
  if (!labelsVal) return fallback;

  if (labelsVal.includes("[")) {
    /* 格式: ["Function","Node"] — 取第一个 */
    const first = labelsVal
      .replace(/[\[\]"']/g, "")
      .split(",")[0]
      ?.trim();
    return first || fallback;
  }
  return labelsVal;
}

/** 转义 Cypher 字符串中的特殊字符 */
function escapeCypherStr(s: string): string {
  return s
    .replace(/\\/g, "\\\\")
    .replace(/'/g, "\\'")
    .replace(/\n/g, "\\n")
    .replace(/\r/g, "\\r")
    .replace(/\t/g, "\\t");
}

/* ── 图数据查询 ───────────────────────────────────── */

/**
 * 查询图数据（节点 + 边）
 * 对齐后端 graph_query.rs::query_graph
 */
export function queryGraph(
  db: LbugDatabase,
  projectName: string,
  maxNodes = 100,
): GraphData {
  const limit = maxNodes;
  const nodes: GraphNode[] = [];
  const nameToId = new Map<string, string>();
  let nodeCounter = 0;

  /* 每个类型分配的配额 */
  const perTypeLimit = Math.min(Math.max(Math.floor(limit / 6), 50), 500);

  /* 按优先级查询各类型节点 */
  for (const label of QUERYABLE_LABELS) {
    if (nodes.length >= limit) break;
    const typeLimit = Math.min(perTypeLimit, limit - nodes.length);

    const cypher =
      `MATCH (n:${label}) ` +
      `RETURN n.name AS name, n.qualifiedName AS qualified_name, n.filePath AS file_path, ` +
      `n.project AS project, n.startLine AS start_line, n.endLine AS end_line, ` +
      `labels(n) AS labels ` +
      `LIMIT ${typeLimit}`;

    let rows: Record<string, unknown>[];
    try {
      rows = db.query(cypher);
    } catch {
      continue; /* 该类型不存在或查询失败 */
    }
    if (rows.length === 0) continue;

    for (const row of rows) {
      if (nodes.length >= limit) break;

      const synthId = `n${nodeCounter}`;
      nodeCounter++;
      const name = getStr(row, "name") ?? "";
      const parsedLabel = parseLabel(row, label);

      if (name) {
        nameToId.set(name, synthId);
      }
      const qn = getStr(row, "qualified_name") ?? "";
      if (qn) {
        nameToId.set(qn, synthId);
      }

      nodes.push({
        id: synthId,
        label: parsedLabel as GraphNode["label"],
        name,
        file_path: getStr(row, "file_path"),
        project: getStr(row, "project") ?? projectName,
        qualified_name: getStr(row, "qualified_name"),
        start_line: getNum(row, "start_line"),
        end_line: getNum(row, "end_line"),
        x: 0, y: 0, z: 0,
      });
    }
  }

  /* 查询边 */
  const edges = queryAllEdges(db, limit, nodes, nameToId);

  /* 分配球面坐标 */
  assignSpherePositions(nodes);

  return {
    nodes,
    edges,
    total_nodes: nodes.length,
    total_edges: edges.length,
  };
}

/**
 * 查询所有边 — 至少一端在已加载节点中，缺失端点补充为 stub 节点
 * 对齐后端 graph_query.rs::query_all_edges
 */
function queryAllEdges(
  db: LbugDatabase,
  limit: number,
  nodes: GraphNode[],
  nameToId: Map<string, string>,
): GraphEdge[] {
  /* 扫描更多边以提高命中率 */
  const scanLimit = Math.min(Math.max(limit * 200, 10_000), 200_000);
  const cypher =
    `MATCH (r:CodeRelation) RETURN r.source AS src_name, r.target AS tgt_name, r.type AS edge_type LIMIT ${scanLimit}`;

  let rows: Record<string, unknown>[];
  try {
    rows = db.query(cypher);
  } catch {
    return [];
  }

  const edges: GraphEdge[] = [];
  let edgeCounter = 0;
  let nodeCounter = nodes.length;

  for (const row of rows) {
    if (edges.length >= limit) break;

    const srcName = getStr(row, "src_name") ?? "";
    const tgtName = getStr(row, "tgt_name") ?? "";
    const edgeType = getStr(row, "edge_type") ?? "UNKNOWN";

    const srcId = nameToId.get(srcName);
    const tgtId = nameToId.get(tgtName);

    /* 至少一端必须匹配已加载节点 */
    if (!srcId && !tgtId) continue;

    /* 为缺失端点创建 stub 节点 */
    const source = srcId ?? createStubNode(srcName, nodeCounter++, nodes, nameToId);
    const target = tgtId ?? createStubNode(tgtName, nodeCounter++, nodes, nameToId);

    edges.push({
      id: `e${edgeCounter}`,
      source,
      target,
      edge_type: edgeType as GraphEdge["edge_type"],
      confidence: 1.0,
      start_line: undefined,
      project: "",
    });
    edgeCounter++;
  }

  return edges;
}

/** 创建 stub 节点（边端点缺失时补充） */
function createStubNode(
  name: string,
  counter: number,
  nodes: GraphNode[],
  nameToId: Map<string, string>,
): string {
  const synthId = `n${counter}`;
  nameToId.set(name, synthId);
  nodes.push({
    id: synthId,
    label: "Function",
    name,
    file_path: undefined,
    project: "",
    qualified_name: name,
    start_line: undefined,
    end_line: undefined,
    x: 0, y: 0, z: 0,
  });
  return synthId;
}

/* ── Schema 统计 ──────────────────────────────────── */

/**
 * 查询 schema 统计信息
 * 对齐后端 graph_query.rs::query_schema
 */
export function querySchema(db: LbugDatabase): SchemaInfo {
  const nodeLabels: { label: string; count: number }[] = [];

  for (const label of QUERYABLE_LABELS) {
    try {
      const rows = db.query(`MATCH (n:${label}) RETURN count(*) AS cnt`);
      const count = Number(rows[0]?.cnt ?? 0);
      if (count > 0) {
        nodeLabels.push({ label, count });
      }
    } catch {
      /* 类型不存在 */
    }
  }
  nodeLabels.sort((a, b) => b.count - a.count);

  /* 边统计 */
  let totalEdges = 0;
  try {
    const rows = db.query("MATCH (r:CodeRelation) RETURN count(*) AS cnt");
    totalEdges = Number(rows[0]?.cnt ?? 0);
  } catch {
    /* 无边表 */
  }

  const totalNodes = nodeLabels.reduce((sum, l) => sum + l.count, 0);

  return {
    node_labels: nodeLabels,
    edge_types: [{ type: "ALL", count: totalEdges }],
    total_nodes: totalNodes,
    total_edges: totalEdges,
  };
}

/* ── 追踪查询 ─────────────────────────────────────── */

/**
 * 执行追踪查询
 * 对齐后端 graph_query.rs::query_trace
 */
export function queryTrace(
  db: LbugDatabase,
  projectName: string,
  nodeName: string,
  mode: string,
  direction: string,
  maxDepth = 10,
): TraceResult {
  const edgeTypes =
    mode === "call"
      ? ["CALLS", "FFI_CALLS", "HTTP_CALLS", "ASYNC_CALLS"]
      : mode === "variable"
        ? ["READS", "WRITES", "ACCESSES", "DATAFLOWS"]
        : ["CALLS", "READS", "WRITES", "ACCESSES"];

  const pattern =
    direction === "downstream"
      ? `(a)-[r:${edgeTypes.join("|")}*1..${maxDepth}]->(b)`
      : direction === "upstream"
        ? `(a)<-[r:${edgeTypes.join("|")}*1..${maxDepth}]-(b)`
        : `(a)-[r:${edgeTypes.join("|")}*1..${maxDepth}]-(b)`;

  const cypher =
    `MATCH ${pattern} WHERE a.name = '${escapeCypherStr(nodeName)}' AND a.project = '${escapeCypherStr(projectName)}' ` +
    `RETURN DISTINCT b.name AS target_name LIMIT 100`;

  let rows: Record<string, unknown>[];
  try {
    rows = db.query(cypher);
  } catch {
    rows = [];
  }

  const targetNames = rows
    .map((r) => getStr(r, "target_name"))
    .filter((s): s is string => !!s);

  /* 查询起点节点 */
  const originNode = querySingleNode(db, nodeName, projectName);

  const paths: TracePath[] = [];
  for (const targetName of targetNames) {
    try {
      const targetNode = querySingleNode(db, targetName, projectName);
      paths.push({ nodes: [originNode, targetNode], edges: [] });
    } catch {
      /* 目标节点不存在 */
    }
  }

  return {
    origin: originNode,
    paths,
    direction: (direction === "downstream" || direction === "upstream" ? direction : "both") as TraceResult["direction"],
  };
}

/** 查询单个节点 */
function querySingleNode(
  db: LbugDatabase,
  nodeName: string,
  projectName: string,
): GraphNode {
  const cypher =
    `MATCH (n) WHERE n.name = '${escapeCypherStr(nodeName)}' ` +
    `RETURN n.name AS name, n.qualifiedName AS qualified_name, n.filePath AS file_path, ` +
    `n.project AS project, n.startLine AS start_line, n.endLine AS end_line, ` +
    `labels(n) AS labels ` +
    `LIMIT 1`;

  const rows = db.query(cypher);
  if (rows.length === 0) {
    throw new Error(`节点 '${nodeName}' 未找到`);
  }

  const row = rows[0];
  const name = getStr(row, "name") ?? nodeName;
  const label = parseLabel(row, "Node");

  return {
    id: name,
    label: label as GraphNode["label"],
    name,
    file_path: getStr(row, "file_path"),
    project: getStr(row, "project") ?? projectName,
    qualified_name: getStr(row, "qualified_name"),
    start_line: getNum(row, "start_line"),
    end_line: getNum(row, "end_line"),
    x: 0, y: 0, z: 0,
  };
}
