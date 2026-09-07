/* 后端 API 客户端 — 与 Rust 图数据服务通信 */

import type { GraphData, ProjectInfo, SchemaInfo, TraceResult, TraceMode } from "../lib/types";

const API_BASE = "/api";

async function fetchJson<T>(path: string, params?: Record<string, string>): Promise<T> {
  const url = new URL(`${API_BASE}${path}`, window.location.origin);
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      if (v) url.searchParams.set(k, v);
    }
  }
  let res: Response;
  try {
    res = await fetch(url.toString());
  } catch {
    throw new Error("后端服务不可用，请确认 Rust 后端已启动（端口 9800）");
  }
  if (!res.ok) {
    const body = await res.json().catch(() => ({ error: res.statusText }));
    throw new Error(body.error ?? `HTTP ${res.status}`);
  }
  return res.json();
}

/* 获取已索引项目列表 */
export async function fetchProjects(): Promise<ProjectInfo[]> {
  return fetchJson<ProjectInfo[]>("/projects");
}

/* 获取图数据 — 支持 project 名或 lbug_path */
export async function fetchGraphData(
  project: string,
  maxNodes = 100,
  fileFilter?: string,
  lbugPath?: string,
): Promise<GraphData> {
  const params: Record<string, string> = {
    max_nodes: String(maxNodes),
  };
  if (lbugPath) {
    params.lbug_path = lbugPath;
  } else {
    params.project = project;
  }
  if (fileFilter) params.file_path = fileFilter;
  return fetchJson<GraphData>("/graph", params);
}

/* 获取 schema 统计信息 */
export async function fetchSchema(project: string, lbugPath?: string): Promise<SchemaInfo> {
  const params: Record<string, string> = {};
  if (lbugPath) {
    params.lbug_path = lbugPath;
  } else {
    params.project = project;
  }
  return fetchJson<SchemaInfo>("/schema", params);
}

/* 执行追踪查询 */
export async function fetchTrace(
  project: string,
  nodeId: string,
  mode: TraceMode,
  direction: "downstream" | "upstream" | "both" = "both",
  maxDepth = 10,
  lbugPath?: string,
): Promise<TraceResult> {
  const params: Record<string, string> = {
    node_id: nodeId,
    mode,
    direction,
    max_depth: String(maxDepth),
  };
  if (lbugPath) {
    params.lbug_path = lbugPath;
  } else {
    params.project = project;
  }
  return fetchJson<TraceResult>("/trace", params);
}
