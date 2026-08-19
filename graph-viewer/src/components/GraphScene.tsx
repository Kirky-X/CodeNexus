/* 3D 图场景 — 组合 Canvas + 节点云 + 边线 + 标签 + 相机控制 */

import { useState, useRef, useCallback } from "react";
import { Canvas, useThree, useFrame } from "@react-three/fiber";
import { OrbitControls } from "@react-three/drei";
import { EffectComposer, Bloom } from "@react-three/postprocessing";
import * as THREE from "three";
import type { OrbitControls as OrbitControlsImpl } from "three-stdlib";
import { NodeCloud } from "./NodeCloud";
import { EdgeLines } from "./EdgeLines";
import { NodeLabels } from "./NodeLabels";
import { NodeTooltip } from "./NodeTooltip";
import type { GraphNode, GraphData } from "../lib/types";

/* 爆炸动画就绪检测：在首帧 useFrame 时通知父组件显示 Canvas */
function ExplosionReady({ onReady }: { onReady: () => void }) {
  const fired = useRef(false);
  useFrame(() => {
    if (!fired.current) {
      fired.current = true;
      onReady();
    }
  });
  return null;
}

interface GraphSceneProps {
  data: GraphData;
  highlightedIds: Set<string> | null;
  traceNodeIds: Set<string>;
  traceEdgeIds: Set<string>;
  showLabels: boolean;
  cameraTarget: CameraTarget | null;
  onNodeClick: (node: GraphNode) => void;
}

export interface CameraTarget {
  position: THREE.Vector3;
  lookAt: THREE.Vector3;
}

/* 相机飞行动画 */
function CameraAnimator({
  target,
  controlsRef,
}: {
  target: CameraTarget | null;
  controlsRef: React.RefObject<OrbitControlsImpl | null>;
}) {
  const { camera } = useThree();
  const targetRef = useRef<CameraTarget | null>(null);
  const progress = useRef(1);
  /* 用 ref 跟踪上一次的 target 对象，只在变化时才启动动画 */
  const prevTargetRef = useRef<CameraTarget | null>(null);

  useFrame(() => {
    /* 仅当 target 对象引用发生变化时才重置进度 */
    if (target && target !== prevTargetRef.current) {
      prevTargetRef.current = target;
      targetRef.current = target;
      progress.current = 0;
    } else if (!target) {
      prevTargetRef.current = null;
    }
    if (!targetRef.current || progress.current >= 1) return;
    progress.current = Math.min(1, progress.current + 0.02);
    const t = 1 - Math.pow(1 - progress.current, 3);
    camera.position.lerp(targetRef.current.position, t * 0.08);
    const controls = controlsRef.current;
    if (controls) {
      controls.target.lerp(targetRef.current.lookAt, t * 0.08);
      controls.update();
    }
  });

  return null;
}

/* 计算相机目标位置 */
export function computeCameraTarget(
  nodes: GraphNode[],
  ids: Set<string>,
): CameraTarget | null {
  if (ids.size === 0) return null;
  let cx = 0, cy = 0, cz = 0, count = 0;
  for (const node of nodes) {
    if (ids.has(node.id)) { cx += node.x; cy += node.y; cz += node.z; count++; }
  }
  if (count === 0) return null;
  cx /= count; cy /= count; cz /= count;

  let maxDist = 0;
  for (const node of nodes) {
    if (ids.has(node.id)) {
      const d = Math.sqrt((node.x - cx) ** 2 + (node.y - cy) ** 2 + (node.z - cz) ** 2);
      if (d > maxDist) maxDist = d;
    }
  }
  const distance = Math.max(200, maxDist * 3);
  const lookAt = new THREE.Vector3(cx, cy, cz);
  const position = new THREE.Vector3(cx + distance * 0.2, cy + distance * 0.15, cz + distance);
  return { position, lookAt };
}

export function GraphScene({
  data, highlightedIds, traceNodeIds, traceEdgeIds,
  showLabels, cameraTarget, onNodeClick,
}: GraphSceneProps) {
  const [hovered, setHovered] = useState<GraphNode | null>(null);
  const controlsRef = useRef<OrbitControlsImpl | null>(null);
  /* Canvas 初始化完成前隐藏，避免爆炸动画前节点闪现 */
  const [ready, setReady] = useState(false);

  const handleHover = useCallback((node: GraphNode | null) => setHovered(node), []);

  return (
    <div style={{ width: '100%', height: '100%', visibility: ready ? 'visible' : 'hidden' }}>
    <Canvas
      camera={{ position: [0, 0, 800], fov: 50, near: 0.1, far: 100000 }}
      style={{ background: "#18181b" }}
      dpr={[1, 1.5]}
      gl={{ antialias: false, alpha: false, powerPreference: "high-performance" }}
    >
      <color attach="background" args={["#18181b"]} />
      <ambientLight intensity={0.5} />
      <pointLight position={[500, 500, 500]} intensity={0.6} />
      <pointLight position={[-300, -200, -300]} intensity={0.4} color="#818cf8" />

      <EdgeLines
        nodes={data.nodes}
        edges={data.edges}
        highlightedIds={highlightedIds}
        traceEdgeIds={traceEdgeIds}
        traceNodeIds={traceNodeIds}
      />
      <NodeCloud
        nodes={data.nodes}
        edges={data.edges}
        highlightedIds={highlightedIds}
        traceNodeIds={traceNodeIds}
        onHover={handleHover}
        onClick={onNodeClick}
      />
      {showLabels && (
        <NodeLabels nodes={data.nodes} highlightedIds={highlightedIds} traceNodeIds={traceNodeIds} />
      )}
      {hovered && <NodeTooltip node={hovered} />}

      <ExplosionReady onReady={() => setReady(true)} />
      <CameraAnimator target={cameraTarget} controlsRef={controlsRef} />

      <EffectComposer multisampling={0}>
        <Bloom luminanceThreshold={0.2} luminanceSmoothing={0.9} intensity={1.5} mipmapBlur radius={0.8} />
      </EffectComposer>

      <OrbitControls
        ref={controlsRef}
        enableDamping
        dampingFactor={0.08}
        rotateSpeed={0.5}
        zoomSpeed={1.5}
        minDistance={10}
        maxDistance={50000}
      />
    </Canvas>
    </div>
  );
}
