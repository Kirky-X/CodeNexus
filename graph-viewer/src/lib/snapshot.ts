/* 图快照模式 — 加载 `?snapshot=` 静态图数据，短路 LadybugDB worker 路径。
 *
 * 快照由 `codenexus diagram --viewer_url` / `arch_diff --viewer_url` 生成的
 * iframe 以 data:application/json 查询值内嵌传入（见 src/diagram/viewer.rs），
 * 也可直接以未编码 JSON 通过 `?snapshot=` 传入。
 * 结构：{ version: 1, nodes: [{id,label,sublabel?,group,change}], edges: [{source,target,label?,change}] }
 */

import type { GraphData, GraphNode, GraphEdge } from "./types";

export interface ViewerSnapshotNode {
  id: string;
  label: string;
  sublabel?: string;
  group: string;
  change?: SnapshotChange;
}

export interface ViewerSnapshotEdge {
  source: string;
  target: string;
  label?: string;
  change?: SnapshotChange;
}

export type SnapshotChange = "none" | "added" | "removed" | "changed";

export interface ViewerSnapshot {
  version: number;
  nodes: ViewerSnapshotNode[];
  edges: ViewerSnapshotEdge[];
}

/** 快照负载上限（字节）— 与 Rust 侧文档约定一致 */
export const MAX_SNAPSHOT_BYTES = 8 * 1024 * 1024;

/** 快照解析失败（格式/大小/缺字段） */
export class SnapshotError extends Error {}

const CHANGE_STATES: ReadonlySet<string> = new Set(["none", "added", "removed", "changed"]);

/** 从 URL query 中提取快照；无 `snapshot` 参数返回 null（走原加载路径）。
 *  URLSearchParams 已做一次百分号解码，data URI 前缀之后的即是原始 JSON。
 * @throws SnapshotError 参数存在但内容非法/超限时抛出。
 */
export function readSnapshotFromUrl(search: string): ViewerSnapshot | null {
  const params = new URLSearchParams(search);
  const raw = params.get("snapshot");
  if (raw === null || raw === "") return null;
  let jsonText: string;
  if (raw.startsWith("data:")) {
    const comma = raw.indexOf(",");
    if (comma === -1) {
      throw new SnapshotError("snapshot data URI 缺少逗号分隔符");
    }
    jsonText = raw.slice(comma + 1);
  } else {
    jsonText = raw;
  }
  if (jsonText.length > MAX_SNAPSHOT_BYTES) {
    throw new SnapshotError(
      `snapshot 超过大小上限（${jsonText.length} > ${MAX_SNAPSHOT_BYTES} 字节）`,
    );
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(jsonText);
  } catch {
    throw new SnapshotError("snapshot 不是合法 JSON");
  }
  return validateSnapshot(parsed);
}

function validateSnapshot(parsed: unknown): ViewerSnapshot {
  if (typeof parsed !== "object" || parsed === null) {
    throw new SnapshotError("snapshot 必须是 JSON 对象");
  }
  const obj = parsed as Record<string, unknown>;
  if (obj.version !== 1) {
    throw new SnapshotError(`不支持的 snapshot 版本: ${String(obj.version)}`);
  }
  if (!Array.isArray(obj.nodes)) {
    throw new SnapshotError("snapshot.nodes 必须是数组");
  }
  if (!Array.isArray(obj.edges)) {
    throw new SnapshotError("snapshot.edges 必须是数组");
  }
  const nodes = obj.nodes.map((n, i) => {
    const node = n as Record<string, unknown>;
    if (typeof node.id !== "string" || node.id === "") {
      throw new SnapshotError(`snapshot.nodes[${i}].id 缺失`);
    }
    if (typeof node.label !== "string") {
      throw new SnapshotError(`snapshot.nodes[${i}].label 缺失`);
    }
    const change = typeof node.change === "string" && CHANGE_STATES.has(node.change)
      ? (node.change as SnapshotChange)
      : "none";
    return {
      id: node.id,
      label: node.label,
      sublabel: typeof node.sublabel === "string" ? node.sublabel : undefined,
      group: typeof node.group === "string" ? node.group : "Module",
      change,
    };
  });
  const ids = new Set(nodes.map((n) => n.id));
  const edges = obj.edges.map((e, i) => {
    const edge = e as Record<string, unknown>;
    if (typeof edge.source !== "string" || typeof edge.target !== "string") {
      throw new SnapshotError(`snapshot.edges[${i}] 缺少 source/target`);
    }
    if (!ids.has(edge.source) || !ids.has(edge.target)) {
      throw new SnapshotError(`snapshot.edges[${i}] 引用不存在的节点`);
    }
    const change = typeof edge.change === "string" && CHANGE_STATES.has(edge.change)
      ? (edge.change as SnapshotChange)
      : "none";
    return {
      source: edge.source,
      target: edge.target,
      label: typeof edge.label === "string" ? edge.label : undefined,
      change,
    };
  });
  return { version: 1, nodes, edges };
}

/** 球面螺旋分布 — 与 demoData 相同的均匀 3D 坐标策略 */
function sphereCoords(index: number, total: number, radius = 40): { x: number; y: number; z: number } {
  if (total <= 0) return { x: 0, y: 0, z: 0 };
  const phi = Math.acos(1 - (2 * (index + 0.5)) / total);
  const theta = Math.PI * (1 + Math.sqrt(5)) * index;
  return {
    x: radius * Math.sin(phi) * Math.cos(theta),
    y: radius * Math.sin(phi) * Math.sin(theta),
    z: radius * Math.cos(phi),
  };
}

/** 将快照映射为内部图模型（GraphData），供场景与筛选器直接消费。
 *  group 作为 NodeLabel 呈现（未知分组沿用默认颜色），change 附加在节点/边上。
 */
export function snapshotToGraphData(snapshot: ViewerSnapshot): GraphData {
  const total = snapshot.nodes.length;
  const nodes: GraphNode[] = snapshot.nodes.map((n, i) => {
    const c = sphereCoords(i, total);
    return {
      id: n.id,
      // 未知分组字符串由 colors.colorForLabel 的默认色兜底
      label: n.group as GraphNode["label"],
      name: n.label,
      project: "snapshot",
      qualified_name: n.sublabel,
      ...c,
      change: n.change ?? "none",
    };
  });
  const edges: GraphEdge[] = snapshot.edges.map((e, i) => ({
    id: `snapshot-edge-${i}`,
    source: e.source,
    target: e.target,
    // 模块级依赖在场景中按 CALLS 语义渲染
    edge_type: "CALLS" as GraphEdge["edge_type"],
    confidence: 1.0,
    project: "snapshot",
    change: e.change ?? "none",
  }));
  return {
    nodes,
    edges,
    total_nodes: nodes.length,
    total_edges: edges.length,
  };
}
