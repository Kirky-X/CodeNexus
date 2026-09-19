// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

/* 3D 边线渲染 — 合并为单个 LineSegments（1 次 draw call），支持爆炸动画
 *
 * 性能设计：旧实现每条边一个 Line2 组件（每边独立 draw call + 每帧
 * setPositions 数组分配），500 条边时产生 500×60fps 的 GC 压力。
 * 现合并为单一几何：位置缓冲仅在爆炸动画期间更新，之后冻结；
 * 高亮/追踪状态变化时重建颜色缓冲。深色背景下用顶点色乘法表达
 * 透明度衰减（暗边 ≈ 背景色），省去逐边材质。 */

import { useRef, useMemo, useEffect } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";
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

/** 每条边的可见性状态 → 颜色（写入 out 避免分配） */
function edgeColor(
  edge: GraphEdge,
  highlightedIds: Set<string> | null,
  traceEdgeIds: Set<string>,
  out: THREE.Color,
): void {
  if (traceEdgeIds.has(edge.id)) {
    out.set(TRACE_COLORS.callChain);
    return;
  }
  out.set(colorForEdgeType(edge.edge_type));
  const isRelevant =
    !highlightedIds ||
    (highlightedIds.has(edge.source) && highlightedIds.has(edge.target));
  if (!isRelevant) out.multiplyScalar(0.04); /* 非相关边：≈背景色，视觉隐没 */
}

interface DrawableEdge {
  src: GraphNode;
  tgt: GraphNode;
  edge: GraphEdge;
}

export function EdgeLines({ nodes, edges, highlightedIds, traceEdgeIds }: EdgeLinesProps) {
  const timeRef = useRef(0);

  const nodeMap = useMemo(() => {
    const map = new Map<string, GraphNode>();
    for (const n of nodes) map.set(n.id, n);
    return map;
  }, [nodes]);

  /* 保留两端都在已加载节点中的边 */
  const drawable = useMemo(() => {
    const list: DrawableEdge[] = [];
    for (const edge of edges) {
      const src = nodeMap.get(edge.source);
      const tgt = nodeMap.get(edge.target);
      if (src && tgt) list.push({ src, tgt, edge });
    }
    return list;
  }, [edges, nodeMap]);

  const positions = useMemo(
    () => new Float32Array(drawable.length * 2 * 3),
    [drawable.length],
  );
  const colors = useMemo(
    () => new Float32Array(drawable.length * 2 * 3),
    [drawable.length],
  );

  const geometry = useMemo(() => {
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", new THREE.BufferAttribute(positions, 3));
    g.setAttribute("color", new THREE.BufferAttribute(colors, 3));
    return g;
  }, [positions, colors]);

  /* 高亮/追踪变化 → 重写颜色缓冲；数据变化 → 立即按当前缓动值写位置
   * （位置逐帧更新在爆炸结束后冻结，数据变更时必须在此补一次写入） */
  useEffect(() => {
    const color = new THREE.Color();
    for (let i = 0; i < drawable.length; i++) {
      edgeColor(drawable[i].edge, highlightedIds, traceEdgeIds, color);
      const o = i * 6;
      colors[o] = color.r; colors[o + 1] = color.g; colors[o + 2] = color.b;
      colors[o + 3] = color.r; colors[o + 4] = color.g; colors[o + 5] = color.b;
    }
    (geometry.getAttribute("color") as THREE.BufferAttribute).needsUpdate = true;
    writePositions(geometry, drawable, sharedExplodeEased.current);
  }, [drawable, highlightedIds, traceEdgeIds, geometry, colors]);

  /* 爆炸动画缓动 — 更新共享值（节点云消费同一时间轴） */
  useFrame((_, delta) => {
    const dt = Math.min(delta, 0.05);
    timeRef.current += dt;
    sharedExplodeEased.current = easeOutCubic(Math.min(1, timeRef.current / EXPLODE_DURATION));
  });

  /* 位置缓冲：仅爆炸动画期间逐帧更新，完成后冻结（边无漂移） */
  useFrame(() => {
    if (timeRef.current > EXPLODE_DURATION + 0.1) return;
    writePositions(geometry, drawable, sharedExplodeEased.current);
  });

  return (
    <lineSegments geometry={geometry} frustumCulled={false}>
      <lineBasicMaterial vertexColors transparent opacity={0.9} depthWrite={false} />
    </lineSegments>
  );
}

function writePositions(
  geometry: THREE.BufferGeometry,
  drawable: DrawableEdge[],
  e: number,
): void {
  const posAttr = geometry.getAttribute("position") as THREE.BufferAttribute;
  const arr = posAttr.array as Float32Array;
  for (let i = 0; i < drawable.length; i++) {
    const { src, tgt } = drawable[i];
    const o = i * 6;
    arr[o] = src.x * e; arr[o + 1] = src.y * e; arr[o + 2] = src.z * e;
    arr[o + 3] = tgt.x * e; arr[o + 4] = tgt.y * e; arr[o + 5] = tgt.z * e;
  }
  posAttr.needsUpdate = true;
}
