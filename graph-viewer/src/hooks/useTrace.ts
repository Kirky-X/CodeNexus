/* 追踪 Hook — 在当前加载的图子图上做 BFS 遍历
 *
 * 追踪语义 = 高亮"用户正在看的这张图"里的调用/数据流路径，
 * 因此只在已加载节点/边上遍历（与渲染范围一致），不查全库——
 * 全库变长路径查询依赖多关系表 schema（CALLS 等作为表名），
 * 真实库是单表 CodeRelation + type 属性，且查到的库外节点无法
 * 在画布上高亮，无视觉意义。 */

import { useCallback, useState } from "react";
import type { TraceResult, TraceMode, GraphNode, GraphEdge } from "../lib/types";
import { CALL_EDGE_TYPES, VARIABLE_EDGE_TYPES } from "../lib/types";

export interface UseTraceResult {
  traceResult: TraceResult | null;
  traceMode: TraceMode;
  traceNodeIds: Set<string>;
  traceEdgeIds: Set<string>;
  startCallTrace: (node: GraphNode) => void;
  startVariableTrace: (node: GraphNode) => void;
  clearTrace: () => void;
}

export function useTrace(allNodes: GraphNode[], allEdges: GraphEdge[]): UseTraceResult {
  const [traceResult, setTraceResult] = useState<TraceResult | null>(null);
  const [traceMode, setTraceMode] = useState<TraceMode>("none");
  const [traceNodeIds, setTraceNodeIds] = useState<Set<string>>(new Set());
  const [traceEdgeIds, setTraceEdgeIds] = useState<Set<string>>(new Set());

  /* 前端本地 BFS 遍历 */
  const localTrace = useCallback(
    (node: GraphNode, edgeTypes: string[], mode: TraceMode) => {
      const nodeMap = new Map<string, GraphNode>();
      for (const n of allNodes) nodeMap.set(n.id, n);

      const visitedNodes = new Set<string>([node.id]);
      const visitedEdges = new Set<string>();
      const queue: string[] = [node.id];
      const paths: { nodes: GraphNode[]; edges: GraphEdge[] }[] = [];

      const maxDepth = 10;
      let depth = 0;
      while (queue.length > 0 && depth < maxDepth) {
        const nextQueue: string[] = [];
        for (const currentId of queue) {
          for (const edge of allEdges) {
            if (!edgeTypes.includes(edge.edge_type)) continue;

            /* 双向遍历：调用与被调用、读与被读都属使用路径 */
            let nextId: string | null = null;
            if (edge.source === currentId) nextId = edge.target;
            if (!nextId && edge.target === currentId) nextId = edge.source;
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

      setTraceResult({ origin: node, paths, direction: "both" });
      setTraceMode(mode);
      setTraceNodeIds(visitedNodes);
      setTraceEdgeIds(visitedEdges);
    },
    [allNodes, allEdges],
  );

  const startCallTrace = useCallback(
    (node: GraphNode) => localTrace(node, CALL_EDGE_TYPES, "call"),
    [localTrace],
  );

  const startVariableTrace = useCallback(
    (node: GraphNode) => localTrace(node, VARIABLE_EDGE_TYPES, "variable"),
    [localTrace],
  );

  const clearTrace = useCallback(() => {
    setTraceResult(null);
    setTraceMode("none");
    setTraceNodeIds(new Set());
    setTraceEdgeIds(new Set());
  }, []);

  return {
    traceResult, traceMode, traceNodeIds, traceEdgeIds,
    startCallTrace, startVariableTrace, clearTrace,
  };
}
