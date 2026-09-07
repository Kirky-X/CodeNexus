/* 3D 节点标签 — 基于相机距离动态显示，支持爆炸动画 */

import { useMemo, useRef } from "react";
import { Html } from "@react-three/drei";
import { useThree, useFrame } from "@react-three/fiber";
import { useState } from "react";
import * as THREE from "three";
import type { GraphNode } from "../lib/types";
import { colorForLabel } from "../lib/colors";

interface NodeLabelsProps {
  nodes: GraphNode[];
  highlightedIds: Set<string> | null;
  traceNodeIds: Set<string>;
}

/* 优先级排序：关键类型优先，然后按连接名字母序 */
const HIGH_PRIORITY_LABELS = new Set([
  "Function", "Method", "Class", "Struct", "Enum", "Trait",
  "Interface", "Impl", "Constructor", "Module", "Service",
]);

const EXPLODE_DURATION = 1.2;

/* 单个标签 — 支持爆炸动画 */
function NodeLabel({ node }: { node: GraphNode }) {
  const groupRef = useRef<THREE.Group>(null);
  const timeRef = useRef(0);

  useFrame((_, delta) => {
    const group = groupRef.current;
    if (!group) return;
    const dt = Math.min(delta, 0.05);
    timeRef.current += dt;
    const progress = Math.min(1, timeRef.current / EXPLODE_DURATION);
    const e = 1 - Math.pow(1 - progress, 3);
    group.position.set(node.x * e, (node.y + 8) * e, node.z * e);
  });

  return (
    <group ref={groupRef}>
      <Html
        center
        distanceFactor={500}
        style={{ pointerEvents: "none" }}
      >
        <div
          className="whitespace-nowrap px-1.5 py-0.5 rounded-md font-mono border"
          style={{
            color: colorForLabel(node.label),
            fontSize: "10px",
            backgroundColor: "rgba(11, 17, 32, 0.8)",
            borderColor: colorForLabel(node.label) + "30",
            textShadow: `0 0 6px ${colorForLabel(node.label)}40`,
          }}
        >
          {node.name}
        </div>
      </Html>
    </group>
  );
}

export function NodeLabels({ nodes, highlightedIds, traceNodeIds }: NodeLabelsProps) {
  const { camera } = useThree();
  const [camDist, setCamDist] = useState(800);

  /* 每帧跟踪相机距离（节流更新） */
  useFrame(() => {
    const d = camera.position.length();
    /* 只有变化超过阈值才更新 state，避免过度渲染 */
    if (Math.abs(d - camDist) > 20) setCamDist(d);
  });

  const visibleNodes = useMemo(() => {
    /* 高亮/追踪模式：只显示相关节点 */
    if (highlightedIds) {
      return nodes.filter((n) => highlightedIds.has(n.id));
    }
    if (traceNodeIds.size > 0) {
      return nodes.filter((n) => traceNodeIds.has(n.id));
    }

    /* 基于相机距离动态决定显示数量：
     * 相机越近，显示越多标签
     * camDist < 200  → 全部
     * camDist ~500   → 200
     * camDist ~1500  → 80
     * camDist > 3000 → 30
     */
    const maxLabels = Math.max(30, Math.min(nodes.length, Math.round(60000 / camDist)));

    /* 按优先级排序：高优先级类型在前，然后按名称长度（短名优先） */
    const sorted = [...nodes].sort((a, b) => {
      const aPri = HIGH_PRIORITY_LABELS.has(a.label) ? 0 : 1;
      const bPri = HIGH_PRIORITY_LABELS.has(b.label) ? 0 : 1;
      if (aPri !== bPri) return aPri - bPri;
      return a.name.length - b.name.length;
    });

    return sorted.slice(0, maxLabels);
  }, [nodes, highlightedIds, traceNodeIds, camDist]);

  return (
    <group>
      {visibleNodes.map((node) => (
        <NodeLabel
          key={node.id}
          node={node}
        />
      ))}
    </group>
  );
}
