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

export function useGraphData(): UseGraphDataResult {
  const [data, setData] = useState<GraphData | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const fetchData = useCallback(async (maxNodes = 100) => {
    setLoading(true);
    setError(null);
    try {
      /* 保留 qualified_name/project 等字段——节点详情面板需要展示它们；
       * 单次加载 ≤500 节点，字段级剥离省不出可观内存 */
      const raw = await fetchGraphData("", maxNodes);
      const nodes = computeForceLayout(raw.nodes, raw.edges);
      setData({ nodes, edges: raw.edges, total_nodes: raw.total_nodes, total_edges: raw.total_edges });
    } catch (e) {
      setError(e instanceof Error ? e.message : "加载图数据失败");
    } finally {
      setLoading(false);
    }
  }, []);

  return { data, loading, error, fetchData };
}
