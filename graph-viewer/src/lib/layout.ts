/* 3D 球面布局引擎 — 前端计算，用于无后端坐标时的降级方案 */

import type { GraphNode, GraphEdge } from "./types";

/** Fibonacci sphere 黄金角 — 产生均匀球面分布 */
const GOLDEN_ANGLE = Math.PI * (3 - Math.sqrt(5));

/** 球面半径 */
const SPHERE_RADIUS = 400;

/**
 * Fibonacci sphere 坐标 — 所有节点均匀分布在同一半径的球面上
 */
function spherePosition(index: number, total: number): { x: number; y: number; z: number } {
  const phi = Math.acos(1 - (2 * (index + 0.5)) / total);
  const theta = GOLDEN_ANGLE * index;
  return {
    x: SPHERE_RADIUS * Math.sin(phi) * Math.cos(theta),
    y: SPHERE_RADIUS * Math.sin(phi) * Math.sin(theta),
    z: SPHERE_RADIUS * Math.cos(phi),
  };
}

/**
 * 3D 球面布局算法
 * 所有节点均匀分布在同一球面上（爆炸的球）。若后端已提供完整坐标则直接使用。
 */
export function computeForceLayout(
  nodes: GraphNode[],
  _edges: GraphEdge[],
  _options?: unknown,
): GraphNode[] {
  /* 所有节点都已定位（后端计算）→ 直接返回，零分配 */
  const allHaveCoords = nodes.every((n) => n.x !== 0 || n.y !== 0 || n.z !== 0);
  if (allHaveCoords) return nodes;

  /* 剩余节点（含 stub）统一分配球面坐标 */
  const total = nodes.length;
  return nodes.map((n, i) => {
    const { x, y, z } = spherePosition(i, total);
    return { ...n, x, y, z };
  });
}
