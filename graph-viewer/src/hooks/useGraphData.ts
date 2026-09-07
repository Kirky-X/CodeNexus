import { useCallback, useState } from "react";
import type { GraphData } from "../lib/types";
import { fetchGraphData } from "../api/client";
import { computeForceLayout } from "../lib/layout";

export interface UseGraphDataResult {
  data: GraphData | null;
  loading: boolean;
  error: string | null;
  fetchData: (maxNodes?: number) => Promise<void>;
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

  const fetchData = useCallback(async (maxNodes = 100) => {
    setLoading(true);
    setError(null);
    try {
      const raw = await fetchGraphData("", maxNodes);
      const nodes = computeForceLayout(trimNodes(raw.nodes), raw.edges);
      setData({ nodes, edges: raw.edges, total_nodes: raw.total_nodes, total_edges: raw.total_edges });
    } catch (e) {
      setError(e instanceof Error ? e.message : "加载图数据失败");
    } finally {
      setLoading(false);
    }
  }, []);

  return { data, loading, error, fetchData };
}
