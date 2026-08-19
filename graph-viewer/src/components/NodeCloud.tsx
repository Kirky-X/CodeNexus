/* 3D 节点云 — 使用 instanced mesh 高性能渲染 */

import { useRef, useMemo, useCallback } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";
import type { GraphNode, GraphEdge } from "../lib/types";
import { colorForLabel } from "../lib/colors";
import { EXPLODE_DURATION, sharedExplodeEased } from "../lib/explosion";

interface NodeCloudProps {
  nodes: GraphNode[];
  edges: GraphEdge[];
  highlightedIds: Set<string> | null;
  traceNodeIds: Set<string>;
  onHover: (node: GraphNode | null) => void;
  onClick: (node: GraphNode) => void;
}

const SPHERE_RADIUS = 4;
const tempObject = new THREE.Object3D();
const tempColor = new THREE.Color();

/* 简易字符串哈希 — 用于根据文件路径产生色调偏移 */
function hashStr(s: string): number {
  let h = 0;
  for (let i = 0; i < s.length; i++) {
    h = ((h << 5) - h + s.charCodeAt(i)) | 0;
  }
  return (h & 0x7fffffff) / 0x7fffffff; /* 0..1 */
}

/* 将 hex 颜色转为 HSL 偏移后的新 RGB */
function shiftHue(hex: string, hueDelta: number, satMul: number, lightMul: number): THREE.Color {
  const c = new THREE.Color(hex);
  const hsl = { h: 0, s: 0, l: 0 };
  c.getHSL(hsl);
  hsl.h = (hsl.h + hueDelta) % 1.0;
  if (hsl.h < 0) hsl.h += 1.0;
  hsl.s = Math.min(1, Math.max(0.15, hsl.s * satMul));
  hsl.l = Math.min(0.75, Math.max(0.25, hsl.l * lightMul));
  c.setHSL(hsl.h, hsl.s, hsl.l);
  return c;
}

export function NodeCloud({ nodes, edges, highlightedIds, traceNodeIds, onHover, onClick }: NodeCloudProps) {
  const meshRef = useRef<THREE.InstancedMesh>(null);
  const glowRef = useRef<THREE.InstancedMesh>(null);
  const materialRef = useRef<THREE.MeshStandardMaterial>(null);
  /* 弹性动画状态 */
  const hoveredRef = useRef<number | null>(null);
  const lastPointerMoveRef = useRef(0); /* 记录最后一次 pointerMove 时间 */
  const clickBounceRef = useRef<Map<number, number>>(new Map());
  const timeRef = useRef(0);
  /* 弹簧物理：[scaleMultiplier, velocity] per node */
  const springStateRef = useRef<[number, number][]>([]);
  /* 初始化爆炸动画起始时间 */
  const explodeStartRef = useRef<number | null>(null);
  /* 使用 ref 存储最新的 props，避免闭包问题 */
  const highlightedIdsRef = useRef(highlightedIds);
  const traceNodeIdsRef = useRef(traceNodeIds);
  highlightedIdsRef.current = highlightedIds;
  traceNodeIdsRef.current = traceNodeIds;

  /* 节点索引映射 */
  const nodeByIndex = useMemo(() => {
    const map = new Map<number, GraphNode>();
    nodes.forEach((n, i) => map.set(i, n));
    return map;
  }, [nodes]);

  /* 计算每个节点的连接数（用于大小变化） */
  const connectionCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const e of edges) {
      counts.set(e.source, (counts.get(e.source) ?? 0) + 1);
      counts.set(e.target, (counts.get(e.target) ?? 0) + 1);
    }
    return counts;
  }, [edges]);

  /* 预计算每个节点的颜色和大小 */
  const nodeVisuals = useMemo(() => {
    const maxConn = Math.max(1, ...connectionCounts.values());
    return nodes.map((node) => {
      const baseHex = colorForLabel(node.label);
      /* 根据文件路径产生色调偏移 (±0.08 = ±约30度色相) */
      const pathHash = hashStr(node.file_path ?? node.name);
      const hueDelta = (pathHash - 0.5) * 0.16;
      /* 根据节点名产生饱和度和亮度微调 */
      const nameHash = hashStr(node.name);
      const satMul = 0.8 + nameHash * 0.4; /* 0.8..1.2 */
      const lightMul = 0.85 + nameHash * 0.3; /* 0.85..1.15 */
      const color = shiftHue(baseHex, hueDelta, satMul, lightMul);

      /* 连接越多越大: 基础 1.0, 最高 1.5 */
      const conn = connectionCounts.get(node.id) ?? 0;
      const sizeScale = 1.0 + Math.min(0.5, (conn / maxConn) * 0.5);

      return { color, sizeScale };
    });
  }, [nodes, connectionCounts]);

  /* Ref 回调：在 mesh 创建的瞬间将所有实例缩放到 0（不可见） */
  /* 这比 useEffect / useLayoutEffect 更早，发生在 commit 阶段，确保 R3F 首帧渲染时节点不可见 */
  const initMeshRef = useCallback((mesh: THREE.InstancedMesh | null) => {
    (meshRef as React.MutableRefObject<THREE.InstancedMesh | null>).current = mesh;
    if (!mesh) return;
    for (let i = 0; i < mesh.count; i++) {
      tempObject.position.set(0, 0, 0);
      tempObject.scale.setScalar(0);
      tempObject.updateMatrix();
      mesh.setMatrixAt(i, tempObject.matrix);
    }
    mesh.instanceMatrix.needsUpdate = true;
  }, []);

  const initGlowRef = useCallback((mesh: THREE.InstancedMesh | null) => {
    (glowRef as React.MutableRefObject<THREE.InstancedMesh | null>).current = mesh;
    if (!mesh) return;
    for (let i = 0; i < mesh.count; i++) {
      tempObject.position.set(0, 0, 0);
      tempObject.scale.setScalar(0);
      tempObject.updateMatrix();
      mesh.setMatrixAt(i, tempObject.matrix);
    }
    mesh.instanceMatrix.needsUpdate = true;
  }, []);

  /* 弹簧参数 — 过阻尼，无超调 */
  const SPRING_K = 200;
  const SPRING_DAMPING = 30;
  /* 交互宽限期（秒）— 爆炸刚展开时节点很小，短暂锁住避免误触；与爆炸时长解耦 */
  const INTERACTION_LOCK = 0.4;

  /* 更新实例矩阵和颜色 — 弹簧物理 + 呼吸 + 悬浮 + 弹跳 + 漂移 + 爆炸动画 */
  useFrame((_, delta) => {
    if (!meshRef.current) return;
    const mesh = meshRef.current;
    const glow = glowRef.current;
    const dt = Math.min(delta, 0.05);
    timeRef.current += dt;
    const t = timeRef.current;

    /* 初始化爆炸起始时间（仅用于漂移相位计算） */
    if (explodeStartRef.current === null) {
      explodeStartRef.current = t;
    }
    const explodeElapsed = t - explodeStartRef.current;

    /* 爆炸位置缓动值由边线组件（EdgeLines）每帧更新，节点与边线共用同一时间轴同步展开 */
    const explodeEased = sharedExplodeEased.current;

    /* 确保弹簧状态数组长度匹配，初始化为缩放 0（爆炸动画起点） */
    const springs = springStateRef.current;
    while (springs.length < nodes.length) springs.push([0, 0]);
    if (springs.length > nodes.length) springs.length = nodes.length;

    /* 清理过期弹跳 */
    for (const [idx, start] of clickBounceRef.current) {
      if (t - start > 1.2) clickBounceRef.current.delete(idx);
    }

    /* 全局焦点状态（所有节点共享） */
    const currentHighlighted = highlightedIdsRef.current;
    const currentTraced = traceNodeIdsRef.current;
    const hasFocus = currentHighlighted !== null || currentTraced.size > 0;

    /* 自动过期悬浮状态 — onPointerOut 在 instancedMesh 上不可靠，用时间戳兜底 */
    if (hoveredRef.current !== null && t - lastPointerMoveRef.current > 0.3) {
      hoveredRef.current = null;
      onHover(null);
    }

    for (let i = 0; i < nodes.length; i++) {
      const node = nodes[i];
      const visual = nodeVisuals[i];

      const isHighlighted = currentHighlighted?.has(node.id) ?? false;
      const isTraced = currentTraced.has(node.id);
      const isHovered = hoveredRef.current === i;
      const clickTime = clickBounceRef.current.get(i);

      /* 计算目标缩放 */
      let baseScale: number;
      let colorMul: number;
      if (isHighlighted || isTraced) {
        baseScale = visual.sizeScale * 1.4;
        colorMul = 1.0;
      } else if (hasFocus) {
        baseScale = visual.sizeScale;
        colorMul = 0.6;
      } else {
        baseScale = visual.sizeScale;
        colorMul = 1.0;
      }

      /* 呼吸动画 — ±5% 轻微脉动 */
      const breathPhase = i * 0.37;
      const breath = 1.0 + Math.sin(t * 1.5 + breathPhase) * 0.05;

      /* 目标弹簧缩放 */
      let targetScale = baseScale * breath;
      /* 悬浮时仅颜色增亮，不放大 */

      /* 点击弹跳 — 叠加脉冲 */
      if (clickTime !== undefined) {
        const elapsed = t - clickTime;
        const bounce = Math.sin(elapsed * 14) * Math.exp(-elapsed * 4) * 0.8;
        targetScale *= (1.0 + bounce);
      }

      /* 弹簧物理更新 — 阻尼谐振子 */
      const [curScale, velocity] = springs[i];
      const force = -SPRING_K * (curScale - targetScale) - SPRING_DAMPING * velocity;
      const newVel = velocity + force * dt;
      const newScale = curScale + newVel * dt;
      springs[i][0] = newScale;
      springs[i][1] = newVel;

      const scale = newScale;

      /* 爆炸动画：从中心点展开到最终位置 — 与边线共享同一缓动值，保证同步 */
      const finalX = node.x;
      const finalY = node.y;
      const finalZ = node.z;
      const explodedX = finalX * explodeEased;
      const explodedY = finalY * explodeEased;
      const explodedZ = finalZ * explodeEased;

    /* 位置漂移 — 轻柔浮动效果（动画完成后才生效） */
    const driftPhase = Math.min(1, explodeElapsed / (EXPLODE_DURATION + 0.5));
      const driftX = Math.sin(t * 0.3 + i * 0.71) * 2.5 * driftPhase;
      const driftY = Math.cos(t * 0.4 + i * 0.53) * 2.5 * driftPhase;
      const driftZ = Math.sin(t * 0.35 + i * 0.97) * 2.5 * driftPhase;
      tempObject.position.set(explodedX + driftX, explodedY + driftY, explodedZ + driftZ);
      tempObject.scale.setScalar(scale);
      tempObject.updateMatrix();
      mesh.setMatrixAt(i, tempObject.matrix);

      /* 发光层同步 */
      if (glow) {
        const glowScale = scale * (isHovered ? 1.2 : 1.3);
        tempObject.scale.setScalar(glowScale);
        tempObject.updateMatrix();
        glow.setMatrixAt(i, tempObject.matrix);
      }

      tempColor.copy(visual.color);
      if (colorMul !== 1.0) {
        tempColor.multiplyScalar(colorMul);
      }
      /* 悬浮时轻微增亮 */
      if (isHovered) {
        tempColor.multiplyScalar(1.15);
      }

      colorAttr.array[i * 3] = tempColor.r;
      colorAttr.array[i * 3 + 1] = tempColor.g;
      colorAttr.array[i * 3 + 2] = tempColor.b;

      /* 发光层颜色同步变暗 */
      glowColorAttr.array[i * 3] = tempColor.r;
      glowColorAttr.array[i * 3 + 1] = tempColor.g;
      glowColorAttr.array[i * 3 + 2] = tempColor.b;
    }

    mesh.instanceMatrix.needsUpdate = true;
    colorAttr.needsUpdate = true;
    if (glow) {
      glow.instanceMatrix.needsUpdate = true;
      glowColorAttr.needsUpdate = true;
    }

    /* 自发光强度保持不变 — 淡出效果只依赖颜色乘数，避免节点过度变暗 */
    if (materialRef.current) {
      const targetEmissive = 0.6;
      materialRef.current.emissiveIntensity += (targetEmissive - materialRef.current.emissiveIntensity) * 0.1;
    }
  });

  /* 初始化颜色缓冲 */
  const colorAttr = useMemo(() => {
    const arr = new Float32Array(nodes.length * 3);
    nodeVisuals.forEach((visual, i) => {
      arr[i * 3] = visual.color.r;
      arr[i * 3 + 1] = visual.color.g;
      arr[i * 3 + 2] = visual.color.b;
    });
    return new THREE.BufferAttribute(arr, 3);
  }, [nodes, nodeVisuals]);

  /* 发光层颜色（与主体相同） */
  const glowColorAttr = useMemo(() => {
    const arr = new Float32Array(nodes.length * 3);
    nodeVisuals.forEach((visual, i) => {
      arr[i * 3] = visual.color.r;
      arr[i * 3 + 1] = visual.color.g;
      arr[i * 3 + 2] = visual.color.b;
    });
    return new THREE.BufferAttribute(arr, 3);
  }, [nodes, nodeVisuals]);

  const handlePointerMove = useCallback(
    (e: { instanceId?: number; stopPropagation?: () => void }) => {
      /* 首帧宽限期（爆炸刚展开时节点体积很小，避免误触） */
      if (explodeStartRef.current !== null && timeRef.current - explodeStartRef.current < INTERACTION_LOCK) {
        return;
      }
      e.stopPropagation?.();
      const idx = e.instanceId;
      if (idx !== undefined) {
        hoveredRef.current = idx;
        lastPointerMoveRef.current = timeRef.current;
        onHover(nodeByIndex.get(idx) ?? null);
      }
    },
    [nodeByIndex, onHover],
  );

  const handlePointerOut = useCallback(() => {
    hoveredRef.current = null;
    onHover(null);
  }, [onHover]);

  const handleClick = useCallback(
    (e: { instanceId?: number; stopPropagation?: () => void }) => {
      /* 首帧宽限期 */
      if (explodeStartRef.current !== null && timeRef.current - explodeStartRef.current < INTERACTION_LOCK) {
        return;
      }
      e.stopPropagation?.();
      const idx = e.instanceId;
      if (idx !== undefined) {
        /* 记录点击时间用于弹跳动画 */
        clickBounceRef.current.set(idx, timeRef.current);
        const node = nodeByIndex.get(idx);
        if (node) onClick(node);
      }
    },
    [nodeByIndex, onClick],
  );

  if (nodes.length === 0) return null;

  return (
    <group>
      {/* 主体节点球 */}
      <instancedMesh
        ref={initMeshRef}
        args={[undefined, undefined, nodes.length]}
        onPointerMove={handlePointerMove}
        onPointerOut={handlePointerOut}
        onClick={handleClick}
      >
        <sphereGeometry args={[SPHERE_RADIUS, 16, 12]}>
          <instancedBufferAttribute attach="attributes-color" args={[colorAttr.array, 3]} />
        </sphereGeometry>
        <meshStandardMaterial
          ref={materialRef}
          vertexColors
          roughness={0.25}
          metalness={0.15}
          emissive="#222"
          emissiveIntensity={0.6}
          toneMapped={false}
        />
      </instancedMesh>

      {/* 外层发光壳 — 稍大的透明球体产生光晕感 */}
      <instancedMesh
        ref={initGlowRef}
        args={[undefined, undefined, nodes.length]}
        raycast={() => null}
      >
        <sphereGeometry args={[SPHERE_RADIUS * 1.6, 8, 6]}>
          <instancedBufferAttribute attach="attributes-color" args={[glowColorAttr.array, 3]} />
        </sphereGeometry>
        <meshBasicMaterial
          vertexColors
          transparent
          opacity={0.08}
          depthWrite={false}
          blending={THREE.AdditiveBlending}
          toneMapped={false}
        />
      </instancedMesh>
    </group>
  );
}
