import { useCallback, useRef, useState } from "react";
import type { GraphData } from "../lib/types";
import { fetchGraphData } from "../api/client";
import { computeForceLayout } from "../lib/layout";

export interface UseGraphDataResult {
  data: GraphData | null;
  loading: boolean;
  error: string | null;
  fetchData: (project: string, maxNodes?: number, fileFilter?: string, lbugPath?: string) => Promise<void>;
  /** 静默刷新 — 不触发 loading 状态，用于自动刷新 */
  silentRefresh: (project: string, maxNodes?: number, fileFilter?: string, lbugPath?: string) => Promise<void>;
}

/** 剥离前端不需要的字段，减少内存占用 */
function trimNodes(nodes: GraphData["nodes"]): GraphData["nodes"] {
  for (const n of nodes) {
    delete (n as unknown as Record<string, unknown>).qualified_name;
    delete (n as unknown as Record<string, unknown>).project;
  }
  return nodes;
}

export function useGraphData(): UseGraphDataResult {
  const [data, setData] = useState<GraphData | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const lastParams = useRef<{ project: string; maxNodes: number; fileFilter?: string; lbugPath?: string } | null>(null);

  const fetchData = useCallback(async (project: string, maxNodes = 100, fileFilter?: string, lbugPath?: string) => {
    lastParams.current = { project, maxNodes, fileFilter, lbugPath };
    setLoading(true);
    setError(null);
    try {
      const raw = await fetchGraphData(project, maxNodes, fileFilter, lbugPath);
      const nodes = computeForceLayout(trimNodes(raw.nodes), raw.edges);
      setData({ nodes, edges: raw.edges, total_nodes: raw.total_nodes, total_edges: raw.total_edges });
    } catch (e) {
      setError(e instanceof Error ? e.message : "加载图数据失败");
    } finally {
      setLoading(false);
    }
  }, []);

  const silentRefresh = useCallback(async (project: string, maxNodes = 100, fileFilter?: string, lbugPath?: string) => {
    lastParams.current = { project, maxNodes, fileFilter, lbugPath };
    try {
      const raw = await fetchGraphData(project, maxNodes, fileFilter, lbugPath);
      const nodes = computeForceLayout(trimNodes(raw.nodes), raw.edges);
      setData({ nodes, edges: raw.edges, total_nodes: raw.total_nodes, total_edges: raw.total_edges });
      setError(null);
    } catch (e) {
      /* 静默刷新失败不覆盖已有数据，仅记录错误 */
      console.warn("[auto-refresh] failed:", e);
    }
  }, []);

  return { data, loading, error, fetchData, silentRefresh };
}
