/* 3D 边线渲染 — 使用 Line2 高性能渲染，支持爆炸动画 */

import { useRef, useMemo } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";
import { Line2 } from "three/examples/jsm/lines/Line2.js";
import { LineMaterial } from "three/examples/jsm/lines/LineMaterial.js";
import { LineGeometry } from "three/examples/jsm/lines/LineGeometry.js";
import type { GraphNode, GraphEdge } from "../lib/types";
import { colorForEdgeType, TRACE_COLORS } from "../lib/colors";
import { EXPLODE_DURATION, sharedExplodeEased, easeOutCubic } from "../lib/explosion";

interface EdgeLinesProps {
  nodes: GraphNode[];
  edges: GraphEdge[];
  highlightedIds: Set<string> | null;
  traceEdgeIds: Set<string>;
  traceNodeIds: Set<string>;
}

/* 单条边 — 支持爆炸动画 */
function EdgeLine({
  src, tgt, color, opacity, lineWidth,
}: {
  src: GraphNode; tgt: GraphNode; color: string; opacity: number;
  lineWidth: number;
}) {
  const lineRef = useRef<Line2>(null);

  /* 每帧更新边线端点（爆炸动画从中心展开） */
  useFrame(() => {
    const line = lineRef.current;
    if (!line) return;
    const e = sharedExplodeEased.current;
    line.geometry.setPositions([
      src.x * e, src.y * e, src.z * e,
      tgt.x * e, tgt.y * e, tgt.z * e,
    ]);
  });

  const material = useMemo(
    () => new LineMaterial({
      color: new THREE.Color(color).getHex(),
      linewidth: lineWidth,
      transparent: true,
      opacity,
    }),
    [color, lineWidth, opacity],
  );

  const geometry = useMemo(() => {
    const g = new LineGeometry();
    g.setPositions([0, 0, 0, 0, 0, 0]);
    return g;
  }, []);

  return (
    <primitive
      ref={lineRef}
      object={useMemo(() => new Line2(geometry, material), [geometry, material])}
    />
  );
}

export function EdgeLines({ nodes, edges, highlightedIds, traceEdgeIds }: EdgeLinesProps) {
  const timeRef = useRef(0);

  const nodeMap = useMemo(() => {
    const map = new Map<string, GraphNode>();
    for (const n of nodes) map.set(n.id, n);
    return map;
  }, [nodes]);

  /* 将边按是否追踪分组 */
  const { traceEdges, normalEdges } = useMemo(() => {
    const trace: GraphEdge[] = [];
    const normal: GraphEdge[] = [];
    for (const edge of edges) {
      const src = nodeMap.get(edge.source);
      const tgt = nodeMap.get(edge.target);
      if (!src || !tgt) continue;
      if (traceEdgeIds.has(edge.id)) {
        trace.push(edge);
      } else {
        normal.push(edge);
      }
    }
    return { traceEdges: trace, normalEdges: normal };
  }, [edges, nodeMap, traceEdgeIds]);

  /* 爆炸动画缓动 — 更新共享值 */
  useFrame((_, delta) => {
    const dt = Math.min(delta, 0.05);
    timeRef.current += dt;
    const progress = Math.min(1, timeRef.current / EXPLODE_DURATION);
    sharedExplodeEased.current = easeOutCubic(progress);
  });

  return (
    <group>
      {/* 普通边 */}
      {normalEdges.map((edge) => {
        const src = nodeMap.get(edge.source)!;
        const tgt = nodeMap.get(edge.target)!;
        const isRelevant =
          !highlightedIds ||
          (highlightedIds.has(edge.source) && highlightedIds.has(edge.target));
        const color = colorForEdgeType(edge.edge_type);
        const opacity = isRelevant ? 0.7 : 0.02;
        const lineWidth = isRelevant && highlightedIds ? 2 : 1;

        return (
          <EdgeLine
            key={edge.id}
            src={src} tgt={tgt}
            color={color} opacity={opacity} lineWidth={lineWidth}
          />
        );
      })}

      {/* 追踪路径边 — 高亮脉冲 */}
      {traceEdges.map((edge) => {
        const src = nodeMap.get(edge.source)!;
        const tgt = nodeMap.get(edge.target)!;
        return (
          <EdgeLine
            key={`trace-${edge.id}`}
            src={src} tgt={tgt}
            color={TRACE_COLORS.callChain}
            opacity={0.9} lineWidth={2.5}
          />
        );
      })}
    </group>
  );
}
