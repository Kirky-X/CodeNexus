/* 图数据查询 — 从 WASM LadybugDB 读取节点和边数据
 *
 * 移植自 server/src/graph_query.rs，使用相同的 Cypher 查询语句
 */

import type { LbugDatabase } from "./lbugWasm";
import type { GraphNode, GraphEdge, GraphData, SchemaInfo } from "./types";

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

/** 从 qualifiedName 提取短名兜底（部分节点无 name 属性）：
 * "proj_xxx.src.stp.session.rs.expire#MockDao" → "expire"
 * "proj_xxx.src.stp.token.rs." → "token"（容忍尾部空段/扩展名） */
function shortNameFromQn(qn: string): string {
  const body = qn.split("#")[0] ?? qn;
  const segs = body.split(".").filter(Boolean);
  let last = segs[segs.length - 1] ?? "";
  if (segs.length >= 2 && last.length <= 3) last = segs[segs.length - 2];
  return last || qn;
}

/* ── 图数据查询 ───────────────────────────────────── */

/**
 * 查询图数据（节点 + 边）
 *
 * 边优先加载：先扫描关系表，按端点热度（度数）选出最强的边和节点，
 * 再批量解析节点详情。早期实现按 label 配额盲取节点、为缺失端点造
 * stub——真实库（10 万+ 节点）里盲取的节点与边几乎不相交，100 条边
 * 全部连向 stub 幽灵节点，视觉上呈现为向一点汇聚的扇形。
 */
export function queryGraph(
  db: LbugDatabase,
  projectName: string,
  maxNodes = 100,
  /** 关系扫描行数上限 — 内存预算模块按设备档位传入（省内存档 50k） */
  scanLimitOverride?: number,
): GraphData {
  /* 1. 扫描关系表 */
  const scanLimit = scanLimitOverride ?? Math.min(Math.max(maxNodes * 200, 10_000), 200_000);
  let relRows: Record<string, unknown>[];
  try {
    relRows = db.query(
      `MATCH (r:CodeRelation) RETURN r.source AS src_name, r.target AS tgt_name, r.type AS edge_type LIMIT ${scanLimit}`,
    );
  } catch {
    relRows = [];
  }

  if (relRows.length === 0) {
    /* 无关系表（旧格式库）→ 退回按 label 配额加载纯节点 */
    return queryGraphByLabelQuota(db, projectName, maxNodes);
  }

  const relations = relRows
    .map((row) => ({
      src: getStr(row, "src_name") ?? "",
      tgt: getStr(row, "tgt_name") ?? "",
      type: getStr(row, "edge_type") ?? "UNKNOWN",
    }))
    .filter((r) => r.src && r.tgt);

  /* 2. 端点热度（度数） */
  const degree = new Map<string, number>();
  for (const rel of relations) {
    degree.set(rel.src, (degree.get(rel.src) ?? 0) + 1);
    degree.set(rel.tgt, (degree.get(rel.tgt) ?? 0) + 1);
  }

  /* 3. 贪心选边：按端点热度降序，但每个端点设配额上限，
   *    避免预算被超级热点（如被所有测试调用的 dao）的平行边吃光 */
  const scored = relations
    .map((rel, i) => ({ rel, i, score: (degree.get(rel.src) ?? 0) + (degree.get(rel.tgt) ?? 0) }))
    .sort((a, b) => b.score - a.score || a.i - b.i);

  const PER_NODE_CAP = 4;
  const pickedCount = new Map<string, number>();
  const picked: typeof relations = [];
  for (const { rel } of scored) {
    if (picked.length >= maxNodes) break;
    const cSrc = pickedCount.get(rel.src) ?? 0;
    const cTgt = pickedCount.get(rel.tgt) ?? 0;
    if (cSrc >= PER_NODE_CAP || cTgt >= PER_NODE_CAP) continue;
    pickedCount.set(rel.src, cSrc + 1);
    pickedCount.set(rel.tgt, cTgt + 1);
    picked.push(rel);
  }

  /* 4. 入选边的端点按热度取前 maxNodes 个作为节点预算 */
  const endpointQns = new Set<string>();
  for (const rel of picked) {
    endpointQns.add(rel.src);
    endpointQns.add(rel.tgt);
  }
  const nodeQns = [...endpointQns]
    .sort((a, b) => (degree.get(b) ?? 0) - (degree.get(a) ?? 0))
    .slice(0, maxNodes);
  const qnSet = new Set(nodeQns);
  const keptRelations = picked.filter((r) => qnSet.has(r.src) && qnSet.has(r.tgt));

  /* 5. 批量解析节点详情（IN 分批，避免超长 Cypher）。
   *    端点分两类：File 节点以 file_* id 为键（无 qualifiedName 属性），
   *    其余按 qualifiedName 解析 */
  const BATCH = 40;
  const nodes: GraphNode[] = [];
  const idByEndpoint = new Map<string, string>();
  let nodeCounter = 0;

  const fileEndpoints = nodeQns.filter((qn) => qn.startsWith("file_"));
  const qnEndpoints = nodeQns.filter((qn) => !qn.startsWith("file_"));

  /* 5a. File 端点 — 按 id 精确解析 */
  for (let i = 0; i < fileEndpoints.length; i += BATCH) {
    const batch = fileEndpoints.slice(i, i + BATCH);
    const list = batch.map((id) => `'${escapeCypherStr(id)}'`).join(",");
    let rows: Record<string, unknown>[];
    try {
      rows = db.query(
        `MATCH (n:File) WHERE n.id IN [${list}] ` +
          `RETURN n.id AS id, n.name AS name, n.filePath AS file_path, labels(n) AS labels`,
      );
    } catch {
      continue;
    }
    for (const row of rows) {
      const endpoint = getStr(row, "id") ?? "";
      if (!endpoint || idByEndpoint.has(endpoint)) continue;
      const filePath = getStr(row, "file_path");
      const synthId = `n${nodeCounter++}`;
      idByEndpoint.set(endpoint, synthId);
      nodes.push({
        id: synthId,
        label: parseLabel(row, "File") as GraphNode["label"],
        name: filePath ? (filePath.split("/").pop() ?? filePath) : (getStr(row, "name") ?? endpoint),
        file_path: filePath,
        project: projectName,
        qualified_name: undefined,
        start_line: undefined,
        end_line: undefined,
        x: 0, y: 0, z: 0,
      });
    }
  }

  /* 5b. 其余端点 — 按 qualifiedName 解析 */
  for (let i = 0; i < qnEndpoints.length; i += BATCH) {
    const batch = qnEndpoints.slice(i, i + BATCH);
    const list = batch.map((qn) => `'${escapeCypherStr(qn)}'`).join(",");
    let rows: Record<string, unknown>[];
    try {
      rows = db.query(
        `MATCH (n) WHERE n.qualifiedName IN [${list}] ` +
          `RETURN n.name AS name, n.qualifiedName AS qualified_name, n.filePath AS file_path, ` +
          `n.project AS project, n.startLine AS start_line, n.endLine AS end_line, labels(n) AS labels`,
      );
    } catch {
      continue;
    }
    for (const row of rows) {
      const qn = getStr(row, "qualified_name") ?? "";
      if (!qn || idByEndpoint.has(qn)) continue;
      const synthId = `n${nodeCounter++}`;
      idByEndpoint.set(qn, synthId);
      const name = getStr(row, "name") ?? "";
      nodes.push({
        id: synthId,
        label: parseLabel(row, "Node") as GraphNode["label"],
        name: name || shortNameFromQn(qn),
        file_path: getStr(row, "file_path"),
        project: getStr(row, "project") ?? projectName,
        qualified_name: qn,
        start_line: getNum(row, "start_line"),
        end_line: getNum(row, "end_line"),
        x: 0, y: 0, z: 0,
      });
    }
  }

  /* 6. 组装边（两端都已解析的才保留） */
  const edges: GraphEdge[] = [];
  for (const rel of keptRelations) {
    const source = idByEndpoint.get(rel.src);
    const target = idByEndpoint.get(rel.tgt);
    if (!source || !target) continue;
    edges.push({
      id: `e${edges.length}`,
      source,
      target,
      edge_type: rel.type as GraphEdge["edge_type"],
      confidence: 1.0,
      start_line: undefined,
      project: "",
    });
  }

  /* 7. 球面坐标 */
  assignSpherePositions(nodes);

  /* 8. 全库真实总数 — 采样只是展示子集，UI 需要如实告知比例 */
  const totals = queryTotals(db, nodes.length, edges.length);

  return {
    nodes,
    edges,
    total_nodes: totals.total_nodes,
    total_edges: totals.total_edges,
  };
}

/** 全库节点/边总数（count 查询失败时以采样数兜底，UI 退化为不显示比例） */
function queryTotals(
  db: LbugDatabase,
  fallbackNodes: number,
  fallbackEdges: number,
): { total_nodes: number; total_edges: number } {
  let totalNodes = fallbackNodes;
  let totalEdges = fallbackEdges;
  try {
    totalNodes = Number(db.query("MATCH (n) RETURN count(n) AS cnt")[0]?.cnt ?? fallbackNodes);
  } catch { /* 旧格式库等场景：保留采样数 */ }
  try {
    totalEdges = Number(db.query("MATCH (r:CodeRelation) RETURN count(*) AS cnt")[0]?.cnt ?? fallbackEdges);
  } catch { /* 无边表 */ }
  return { total_nodes: totalNodes, total_edges: totalEdges };
}

/**
 * 兜底加载：无关系表时按 label 配额盲取节点（纯节点展示，无边）
 * 对齐后端 graph_query.rs::query_graph 的原始行为
 */
function queryGraphByLabelQuota(
  db: LbugDatabase,
  projectName: string,
  maxNodes = 100,
): GraphData {
  const nodes: GraphNode[] = [];
  let nodeCounter = 0;
  const perTypeLimit = Math.min(Math.max(Math.floor(maxNodes / 6), 50), 500);

  for (const label of QUERYABLE_LABELS) {
    if (nodes.length >= maxNodes) break;
    const typeLimit = Math.min(perTypeLimit, maxNodes - nodes.length);

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
      if (nodes.length >= maxNodes) break;
      nodes.push({
        id: `n${nodeCounter++}`,
        label: parseLabel(row, label) as GraphNode["label"],
        name: getStr(row, "name") ?? "",
        file_path: getStr(row, "file_path"),
        project: getStr(row, "project") ?? projectName,
        qualified_name: getStr(row, "qualified_name"),
        start_line: getNum(row, "start_line"),
        end_line: getNum(row, "end_line"),
        x: 0, y: 0, z: 0,
      });
    }
  }

  assignSpherePositions(nodes);

  const totals = queryTotals(db, nodes.length, 0);

  return {
    nodes,
    edges: [],
    total_nodes: totals.total_nodes,
    total_edges: totals.total_edges,
  };
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
