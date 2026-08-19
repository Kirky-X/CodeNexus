import { useCallback, useState } from "react";
import type { TraceResult, TraceMode, GraphNode, GraphEdge } from "../lib/types";
import { CALL_EDGE_TYPES, VARIABLE_EDGE_TYPES } from "../lib/types";
import { fetchTrace } from "../api/client";

export interface UseTraceResult {
  traceResult: TraceResult | null;
  traceMode: TraceMode;
  traceNodeIds: Set<string>;
  traceEdgeIds: Set<string>;
  loading: boolean;
  startCallTrace: (project: string, node: GraphNode) => void;
  startVariableTrace: (project: string, node: GraphNode) => void;
  clearTrace: () => void;
}

/**
 * 追踪 Hook — 支持函数调用追踪和变量使用追踪
 * 函数调用追踪：沿 CALLS/FFI_CALLS/HTTP_CALLS/ASYNC_CALLS 边遍历
 * 变量使用追踪：沿 READS/WRITES/ACCESSES/DATAFLOWS 边遍历
 */
export function useTrace(allNodes: GraphNode[], allEdges: GraphEdge[]): UseTraceResult {
  const [traceResult, setTraceResult] = useState<TraceResult | null>(null);
  const [traceMode, setTraceMode] = useState<TraceMode>("none");
  const [traceNodeIds, setTraceNodeIds] = useState<Set<string>>(new Set());
  const [traceEdgeIds, setTraceEdgeIds] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);

  /* 前端本地追踪 — 从已有数据中 BFS 遍历 */
  const localTrace = useCallback(
    (node: GraphNode, edgeTypes: string[], mode: TraceMode, direction: "downstream" | "upstream" | "both") => {
      const nodeMap = new Map<string, GraphNode>();
      for (const n of allNodes) nodeMap.set(n.id, n);

      const visitedNodes = new Set<string>([node.id]);
      const visitedEdges = new Set<string>();
      const queue: string[] = [node.id];
      const paths: { nodes: GraphNode[]; edges: GraphEdge[] }[] = [];

      /* BFS */
      const maxDepth = 10;
      let depth = 0;
      while (queue.length > 0 && depth < maxDepth) {
        const nextQueue: string[] = [];
        for (const currentId of queue) {
          for (const edge of allEdges) {
            if (!edgeTypes.includes(edge.edge_type)) continue;

            let nextId: string | null = null;
            if (direction === "downstream" || direction === "both") {
              if (edge.source === currentId) nextId = edge.target;
            }
            if ((direction === "upstream" || direction === "both") && edge.target === currentId) {
              nextId = edge.source;
            }
            if (!nextId || visitedNodes.has(nextId)) continue;

            visitedNodes.add(nextId);
            visitedEdges.add(edge.id);
            nextQueue.push(nextId);

            const targetNode = nodeMap.get(nextId);
            if (targetNode) {
              paths.push({
                nodes: [node, targetNode],
                edges: [edge],
              });
            }
          }
        }
        queue.length = 0;
        queue.push(...nextQueue);
        depth++;
      }

      setTraceResult({ origin: node, paths, direction });
      setTraceMode(mode);
      setTraceNodeIds(visitedNodes);
      setTraceEdgeIds(visitedEdges);
    },
    [allNodes, allEdges],
  );

  const startCallTrace = useCallback(
    async (project: string, node: GraphNode) => {
      setLoading(true);
      try {
        /* 先尝试后端追踪 */
        const result = await fetchTrace(project, node.id, "call");
        applyTraceResult(result, "call");
      } catch {
        /* 降级为前端本地追踪 */
        localTrace(node, CALL_EDGE_TYPES, "call", "both");
      } finally {
        setLoading(false);
      }
    },
    [localTrace],
  );

  const startVariableTrace = useCallback(
    async (project: string, node: GraphNode) => {
      setLoading(true);
      try {
        const result = await fetchTrace(project, node.id, "variable");
        applyTraceResult(result, "variable");
      } catch {
        localTrace(node, VARIABLE_EDGE_TYPES, "variable", "both");
      } finally {
        setLoading(false);
      }
    },
    [localTrace],
  );

  const applyTraceResult = (result: TraceResult, mode: TraceMode) => {
    setTraceResult(result);
    setTraceMode(mode);
    const nodeIds = new Set<string>([result.origin.id]);
    const edgeIds = new Set<string>();
    for (const path of result.paths) {
      for (const n of path.nodes) nodeIds.add(n.id);
      for (const e of path.edges) edgeIds.add(e.id);
    }
    setTraceNodeIds(nodeIds);
    setTraceEdgeIds(edgeIds);
  };

  const clearTrace = useCallback(() => {
    setTraceResult(null);
    setTraceMode("none");
    setTraceNodeIds(new Set());
    setTraceEdgeIds(new Set());
  }, []);

  return {
    traceResult, traceMode, traceNodeIds, traceEdgeIds, loading,
    startCallTrace, startVariableTrace, clearTrace,
  };
}
