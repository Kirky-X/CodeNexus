// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

import { useCallback, useState } from "react";
import type { GraphData } from "../lib/types";
import { fetchGraphData } from "../api/client";
import { computeForceLayout } from "../lib/layout";
import { resolveLocale, translate } from "../lib/i18nCatalog";

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
      /* 兜底文案走冷路径直读 catalog——若依赖 useI18n 的 t，切语言会重建
       * fetchData 并触发 App 的数据加载 effect 重跑，代价是整图重新加载 */
      setError(e instanceof Error ? e.message : translate(resolveLocale(), "error.title"));
    } finally {
      setLoading(false);
    }
  }, []);

  return { data, loading, error, fetchData };
}
