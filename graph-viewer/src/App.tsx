// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

import { useEffect, useState, useCallback, useMemo, useRef, lazy, Suspense } from "react";
import { useGraphData } from "./hooks/useGraphData";
import { useTrace } from "./hooks/useTrace";
import { GraphScene, computeCameraTarget } from "./components/GraphScene";
import { FilterPanel } from "./components/FilterPanel";
import { Sidebar } from "./components/Sidebar";
import { NodeModal } from "./components/NodeModal";
import { Button } from "./components/ui/button";
import { generateDemoData } from "./lib/demoData";
import { readSnapshotFromUrl, snapshotToGraphData, SnapshotError } from "./lib/snapshot";
import { loadLbugFile, closeDatabase } from "./api/client";
import { computeLoadBudget, detectMemoryProfile, type LoadBudget } from "./lib/memoryBudget";
import { useI18n } from "./lib/i18n";
import { LanguageSwitcher } from "./components/LanguageSwitcher";
/* 懒加载 — LightRays 拖入 ogl 着色器库，拆出主包让首屏更早可交互 */
const LightRays = lazy(() => import("./components/LightRays"));
import type { GraphNode, GraphData } from "./lib/types";
import type { CameraTarget } from "./components/GraphScene";

const EMPTY_SET = new Set<string>();

/* 视图模式 — 单一事实源，四态互斥。
 * 独立布尔（fileLoaded/demoMode）曾被证明会互相污染：进过 demo 后加载
 * 文件仍显示演示数据。所有数据源选择只看 mode。 */
type ViewMode = "landing" | "file" | "demo" | "snapshot";

export function App() {
  const { data, loading, error, fetchData } = useGraphData();
  const { t } = useI18n();

  /* 筛选状态 — null = 未初始化（跟随数据全集）；
   * 空 Set = 用户显式"全不选"，必须与未初始化区分 */
  const [enabledLabels, setEnabledLabels] = useState<Set<string> | null>(null);
  const [enabledEdgeTypes, setEnabledEdgeTypes] = useState<Set<string> | null>(null);
  const [fileFilter, setFileFilter] = useState("");
  const [showLabels, setShowLabels] = useState(true);
  /* 小屏（<lg）筛选面板抽屉开关 — 桌面端常驻，不受此状态影响 */
  const [filterPanelOpen, setFilterPanelOpen] = useState(false);
  const [maxNodes, setMaxNodes] = useState(() => {
    try {
      const v = Number(localStorage.getItem("codenexus-maxNodes"));
      return [50, 100, 200, 500].includes(v) ? v : 100;
    } catch { return 100; }
  });

  /* 选择状态 */
  const [selectedNode, setSelectedNode] = useState<GraphNode | null>(null);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [highlightedIds, setHighlightedIds] = useState<Set<string> | null>(null);
  const [cameraTarget, setCameraTarget] = useState<CameraTarget | null>(null);

  /* 文件加载状态 */
  const [fileName, setFileName] = useState("");
  const [fileLoading, setFileLoading] = useState(false);
  const [fileError, setFileError] = useState<string | null>(null);
  const [mode, setMode] = useState<ViewMode>("landing");
  const [demoData, setDemoData] = useState<GraphData | null>(null);
  /* 快照模式 — `?snapshot=` 内嵌图数据（diagram/arch_diff --viewer_url 生成），
   * 存在时短路 server/worker 加载路径 */
  const [snapshotData, setSnapshotData] = useState<GraphData | null>(null);
  const [snapshotError, setSnapshotError] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [isDragOver, setIsDragOver] = useState(false);
  /* 内存预算 — 大文件加载时钳制节点上限并启用省内存档 */
  const [loadBudget, setLoadBudget] = useState<LoadBudget | null>(null);
  /* 加载阶段 — 大文件加载分两步给出反馈，避免黑盒假死感 */
  const [loadStage, setLoadStage] = useState<"copy" | "open" | "analyze" | null>(null);
  /* 文件加载序号 — file 模式内重选文件时驱动重新查询 */
  const [loadSeq, setLoadSeq] = useState(0);

  /* 文件加载处理 */
  const handleFileLoad = useCallback(async (file: File) => {
    if (!file.name.endsWith(".lbug")) {
      setFileError("请选择 .lbug 文件");
      return;
    }
    if (fileLoading) return; /* 加载中防重复触发 */
    setFileLoading(true);
    setFileError(null);
    try {
      const budget = computeLoadBudget(file.size, detectMemoryProfile());
      setLoadBudget(budget);
      if (maxNodes > budget.maxNodesCap) setMaxNodes(budget.maxNodesCap);
      await loadLbugFile(file, budget, (stage) => setLoadStage(stage));
      setLoadStage("analyze");
      setFileName(file.name);
      /* 切入文件模式并清掉残留的快照/演示数据源，杜绝模式污染。
       * loadSeq 区分同模式下的连续加载：file 模式中重选另一个 .lbug 时
       * setMode 为同值 no-op，靠 seq 变化重新触发查询 */
      setSnapshotData(null);
      setSnapshotError(null);
      setFileFilter("");
      setLoadSeq((n) => n + 1);
      setMode("file");
    } catch (e) {
      setFileError(e instanceof Error ? e.message : "加载数据库失败");
    } finally {
      setLoadStage(null);
      setFileLoading(false);
    }
  }, [maxNodes, fileLoading]);

  const handleFileInput = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (file) handleFileLoad(file);
  }, [handleFileLoad]);

  /* 拖放处理 */
  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    setIsDragOver(true);
  }, []);
  const handleDragLeave = useCallback(() => setIsDragOver(false), []);
  const handleDrop = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    setIsDragOver(false);
    const file = e.dataTransfer.files[0];
    if (file) handleFileLoad(file);
  }, [handleFileLoad]);

  /* 持久化节点上限偏好 */
  useEffect(() => {
    try { localStorage.setItem("codenexus-maxNodes", String(maxNodes)); } catch {}
  }, [maxNodes]);

  /* 加载数据 — 由模式驱动：demo 生成演示数据，file 走 worker 查询；
   * loadSeq 变化代表同模式下加载了新文件 */
  useEffect(() => {
    if (mode === "demo") {
      setDemoData(generateDemoData());
      return;
    }
    if (mode === "file") fetchData(maxNodes);
  }, [mode, loadSeq, fetchData, maxNodes]);

  /* 快照模式初始化 — 仅在挂载时解析一次 URL；合法快照直接进入图视图 */
  useEffect(() => {
    try {
      const snap = readSnapshotFromUrl(window.location.search);
      if (snap) {
        setSnapshotData(snapshotToGraphData(snap));
        setMode("snapshot");
      }
    } catch (e) {
      setSnapshotError(e instanceof SnapshotError ? e.message : "snapshot 解析失败");
    }
  }, []);

  /* 当前生效数据源 — 按模式互斥选择， landing 下为 null */
  const activeData = mode === "demo" ? demoData : mode === "snapshot" ? snapshotData : data;

  /* 初始化筛选器 — 数据源切换时重置为全集 */
  useEffect(() => {
    if (!activeData) return;
    setEnabledLabels(new Set(activeData.nodes.map((n) => n.label)));
    setEnabledEdgeTypes(new Set(activeData.edges.map((e) => e.edge_type)));
  }, [activeData]);

  /* 追踪 */
  const activeNodes = activeData?.nodes ?? [];
  const activeEdges = activeData?.edges ?? [];
  const { traceNodeIds, traceEdgeIds, traceMode, clearTrace, startCallTrace, startVariableTrace } =
    useTrace(activeNodes, activeEdges);

  /* 节点详情关闭 — 面板关闭按钮、小屏遮罩与 Escape 共用同一清理路径 */
  const closeNodeModal = useCallback(() => {
    setSelectedNode(null);
    setHighlightedIds(null);
    setSelectedPath(null);
    clearTrace();
  }, [clearTrace]);

  /* Escape 关闭：优先收起小屏抽屉，其次关节点详情 */
  useEffect(() => {
    if (!filterPanelOpen && !selectedNode) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (filterPanelOpen) setFilterPanelOpen(false);
      else closeNodeModal();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [filterPanelOpen, selectedNode, closeNodeModal]);

  /* 同步派生有效筛选集 — 用 useMemo 稳定引用，避免每次渲染创建新 Set */
  const allDataLabels = useMemo(
    () => activeData ? new Set(activeData.nodes.map((n) => n.label)) : EMPTY_SET,
    [activeData],
  );
  const allDataEdgeTypes = useMemo(
    () => activeData ? new Set(activeData.edges.map((e) => e.edge_type)) : EMPTY_SET,
    [activeData],
  );
  /* 如果用户选择了全部（或尚未初始化），直接使用数据中的全集；
   * null（未初始化）与空集（显式全不选）语义不同 */
  const effectiveLabels = enabledLabels === null || enabledLabels.size >= allDataLabels.size
    ? allDataLabels : enabledLabels;
  const effectiveEdgeTypes = enabledEdgeTypes === null || enabledEdgeTypes.size >= allDataEdgeTypes.size
    ? allDataEdgeTypes : enabledEdgeTypes;

  /* 计算过滤后的数据 — 无筛选时直接返回原数组避免内存分配 */
  const filteredData: GraphData | null = useMemo(() => {
    if (!activeData) return null;

    const noLabelFilter = effectiveLabels === allDataLabels;
    const noFileFilter = !fileFilter;

    /* 无筛选 — 直接返回原数据，零分配 */
    if (noLabelFilter && noFileFilter) {
      return activeData;
    }

    const nodes = activeData.nodes.filter((n) => {
      if (!effectiveLabels.has(n.label)) return false;
      if (fileFilter && n.file_path && !n.file_path.includes(fileFilter)) return false;
      return true;
    });

    const nodeIds = new Set(nodes.map((n) => n.id));
    const edges = activeData.edges.filter(
      (e) => effectiveEdgeTypes.has(e.edge_type) && nodeIds.has(e.source) && nodeIds.has(e.target),
    );

    return { nodes, edges, total_nodes: activeData.total_nodes, total_edges: activeData.total_edges };
  }, [activeData, effectiveLabels, effectiveEdgeTypes, allDataLabels, fileFilter]);

  /* 用 ref 存储 filteredData，确保 handleNodeClick 引用稳定 */
  const filteredDataRef = useRef(filteredData);
  filteredDataRef.current = filteredData;

  /* 节点点击 — 零依赖，引用永远不变 */
  const handleNodeClick = useCallback(
    (node: GraphNode) => {
      const fd = filteredDataRef.current;
      if (!fd) return;
      setSelectedNode(node);
      const connectedIds = new Set<string>([node.id]);
      for (const edge of fd.edges) {
        if (edge.source === node.id) connectedIds.add(edge.target);
        if (edge.target === node.id) connectedIds.add(edge.source);
      }
      setHighlightedIds(connectedIds);
      setSelectedPath(node.file_path ?? null);
      setCameraTarget(computeCameraTarget(fd.nodes, connectedIds));
    },
    [],
  );

  /* 文件路径选择 — 同样使用 ref 保持稳定 */
  const handleSelectPath = useCallback(
    (path: string, nodeIds: Set<string>) => {
      const fd = filteredDataRef.current;
      if (!fd || !path || nodeIds.size === 0) {
        setHighlightedIds(null);
        setSelectedPath(null);
        setCameraTarget(null);
        return;
      }
      setSelectedPath(path);
      setHighlightedIds(nodeIds);
      setCameraTarget(computeCameraTarget(fd.nodes, nodeIds));
    },
    [],
  );

  /* 筛选操作 */
  const toggleLabel = useCallback((label: string) => {
    setEnabledLabels((prev) => {
      const next = new Set(prev);
      if (next.has(label)) next.delete(label); else next.add(label);
      return next;
    });
  }, []);

  const toggleEdgeType = useCallback((type: string) => {
    setEnabledEdgeTypes((prev) => {
      const next = new Set(prev);
      if (next.has(type)) next.delete(type); else next.add(type);
      return next;
    });
  }, []);

  const enableAll = useCallback(() => {
    if (!activeData) return;
    setEnabledLabels(new Set(activeData.nodes.map((n) => n.label)));
    setEnabledEdgeTypes(new Set(activeData.edges.map((e) => e.edge_type)));
  }, [activeData]);

  const disableAll = useCallback(() => {
    /* 空 Set = 显式全不选（区别于 null 未初始化），图会清空并出现重置入口 */
    setEnabledLabels(new Set());
    setEnabledEdgeTypes(new Set());
  }, []);

  const enterDemoMode = useCallback(() => {
    setMode("demo");
  }, []);

  const handleBack = useCallback(async () => {
    await closeDatabase();
    setMode("landing");
    setLoadBudget(null);
    setFileName("");
    setSelectedNode(null);
    setHighlightedIds(null);
    setSelectedPath(null);
    setCameraTarget(null);
    setFileError(null);
    clearTrace();
  }, [clearTrace]);

  const handleReselectFile = useCallback(() => {
    fileInputRef.current?.click();
  }, []);

  /* ── Landing Page ─────────────────────────────────── */
  if (mode === "landing") {
    return (
      <div className="grain h-screen flex flex-col bg-ambient text-foreground overflow-hidden relative">
        {/* LightRays Background */}
        <div style={{ width: '100%', height: '100%', position: 'absolute', inset: 0, zIndex: 0 }}>
          <Suspense fallback={null}>
            <LightRays
              raysOrigin="top-center"
              raysColor="#ffffff"
              raysSpeed={1.5}
              lightSpread={0.8}
              rayLength={1.2}
              followMouse={true}
              mouseInfluence={0.2}
              noiseAmount={0.06}
              distortion={0.5}
              className="custom-rays"
              fadeDistance={0.7}
            />
          </Suspense>
        </div>

        {/* Navigation */}
        <nav className="flex items-center justify-between px-8 py-5 shrink-0 relative" style={{ zIndex: 1 }}>
          <div className="flex items-center gap-3">
            <img src="/CodeNexus-128.png" alt="CodeNexus" className="w-9 h-9 rounded-xl object-contain ring-1 ring-white/10" />
            <span className="text-lg font-semibold tracking-tight text-foreground">CodeNexus</span>
          </div>
          <div className="flex items-center gap-3">
            <span className="text-[11px] uppercase tracking-[0.22em] text-fg-subtle font-medium">{t("nav.subtitle")}</span>
            <LanguageSwitcher />
          </div>
        </nav>

        {/* Hero */}
        <div className="flex-1 flex items-center justify-center px-6 relative" style={{ zIndex: 1 }}>
          <div className="w-full max-w-2xl space-y-12">
            {/* Title */}
            <div className="text-center space-y-5">
              <span className="rise rise-1 inline-flex items-center gap-2 rounded-full border border-white/10 bg-white/[0.04] px-3.5 py-1 text-[10px] uppercase tracking-[0.22em] font-medium text-foreground/55">
                <span className="w-1.5 h-1.5 rounded-full bg-accent shadow-[0_0_8px_rgba(34,211,238,0.8)]" />
                {t("nav.subtitle")}
              </span>
              <h1 className="rise rise-2 text-5xl md:text-6xl font-semibold tracking-[-0.03em] leading-[1.05] bg-gradient-to-b from-white via-white to-white/55 bg-clip-text text-transparent">
                {t("hero.title")}
              </h1>
              <p className="rise rise-3 text-base text-fg-subtle max-w-md mx-auto leading-relaxed">
                {t("hero.subtitle")}
              </p>
            </div>

            {/* File Drop Zone — 双层嵌套：外壳铝槽 + 内芯玻璃板 */}
            <div className="rise rise-4 rounded-[1.75rem] bg-white/[0.03] p-1.5 ring-1 ring-white/10 shadow-[0_20px_60px_-20px_rgba(0,0,0,0.7)]">
              <div className="rounded-[1.375rem] bg-[#101014]/90 backdrop-blur-xl p-5 space-y-4 shadow-[inset_0_1px_1px_rgba(255,255,255,0.06)]">
                <input
                  ref={fileInputRef}
                  type="file"
                  accept=".lbug"
                  className="hidden"
                  onChange={handleFileInput}
                />
                <button
                  type="button"
                  className={`group flex w-full flex-col items-center justify-center py-9 px-6 rounded-[0.95rem] border border-dashed transition-all duration-500 cursor-pointer focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-primary/70 ${
                    isDragOver
                      ? "border-primary/60 bg-primary/[0.07] shadow-[0_0_40px_-8px_rgba(129,140,248,0.35)]"
                      : "border-white/[0.12] hover:border-white/25 hover:bg-white/[0.02]"
                  }`}
                  onClick={() => fileInputRef.current?.click()}
                  onDragOver={handleDragOver}
                  onDragLeave={handleDragLeave}
                  onDrop={handleDrop}
                >
                  {fileLoading ? (
                    <div className="flex flex-col items-center gap-3">
                      <div className="w-8 h-8 border-2 border-primary/30 border-t-primary rounded-full animate-spin" />
                      <p className="text-sm text-fg-subtle">{t("landing.loadingFile")}</p>
                    </div>
                  ) : (
                    <div className="flex flex-col items-center gap-3">
                      <svg className="w-9 h-9 text-foreground/25 transition-colors duration-500 group-hover:text-primary/60" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                        <path d="M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4" />
                        <polyline points="17 8 12 3 7 8" />
                        <line x1="12" y1="3" x2="12" y2="15" />
                      </svg>
                      <p className="text-sm text-foreground/70">{t("landing.dropFile")}</p>
                      <p className="text-xs text-fg-subtle">{t("landing.orSelectFile")}</p>
                    </div>
                  )}
                </button>
                {fileError && (
                  <p className="text-xs text-destructive/80 text-center">{fileError}</p>
                )}
                {snapshotError && (
                  <p className="text-xs text-destructive/80 text-center max-w-md break-all">
                    snapshot: {snapshotError}
                  </p>
                )}
                <p className="text-[11px] text-fg-subtle text-center">{t("landing.acceptsLbug")}</p>

                {/* Demo mode */}
                <div className="flex items-center gap-3 pt-1">
                  <div className="h-px flex-1 bg-gradient-to-r from-transparent to-white/10" />
                  <Button variant="outline" size="sm" onClick={enterDemoMode} className="text-xs rounded-full px-4 active:scale-[0.98]">
                    {t("landing.demo")}
                  </Button>
                  <div className="h-px flex-1 bg-gradient-to-l from-transparent to-white/10" />
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    );
  }

  /* ── Loading State ─────────────────────────────────── */
  /* file 模式内重选文件时，worker 重载阶段（loading 尚未置位）也要给出反馈 */
  if (loading || (fileLoading && mode === "file")) {
    return (
      <div className="h-screen flex items-center justify-center bg-ambient" role="status">
        <div className="text-center space-y-8">
          {/* Logo + 动画光环 */}
          <div className="relative w-24 h-24 mx-auto">
            {/* 旋转光环 */}
            <div className="absolute inset-0 rounded-full border-2 border-transparent border-t-primary/60 border-r-primary/20 animate-spin" />
            <div className="absolute inset-1 rounded-full border border-transparent border-b-accent/40 border-l-accent/10 animate-spin" style={{ animationDirection: 'reverse', animationDuration: '1.5s' }} />
            {/* Logo */}
            <div className="absolute inset-0 flex items-center justify-center">
              <img
                src="/CodeNexus-128.png"
                alt="CodeNexus"
                className="w-14 h-14 rounded-xl object-contain animate-pulse"
              />
            </div>
          </div>
          {/* 文字信息 */}
          <div className="space-y-2">
            <p className="text-base text-foreground/60 font-medium">
              {loadStage === "copy" ? t("loading.stageCopy")
                : loadStage === "open" ? t("loading.stageOpen")
                : loadStage === "analyze" ? t("loading.stageAnalyze")
                : t("loading.text")}
            </p>
            <p className="text-sm text-fg-subtle font-mono">{fileName}</p>
            {loadBudget?.warnLowMemory && (
              <p className="text-xs text-fg-subtle">{t("loading.largeFile")}</p>
            )}
            {loadBudget?.fileTooLarge && (
              <p className="text-xs text-destructive/70">{t("loading.fileMayOOM")}</p>
            )}
          </div>
          {/* 流动点动画 */}
          <div className="flex items-center justify-center gap-1.5">
            {[0, 1, 2].map((i) => (
              <div
                key={i}
                className="w-1.5 h-1.5 rounded-full bg-primary/50"
                style={{
                  animation: 'pulse 1.4s ease-in-out infinite',
                  animationDelay: `${i * 0.2}s`,
                }}
              />
            ))}
          </div>
        </div>
      </div>
    );
  }

  /* ── Error State ───────────────────────────────────── */
  if (error) {
    return (
      <div className="h-screen flex items-center justify-center bg-ambient">
        <div className="glass rounded-2xl p-8 max-w-md text-center space-y-5">
          <div className="w-10 h-10 rounded-full bg-destructive/10 flex items-center justify-center mx-auto">
            <svg className="w-5 h-5 text-destructive" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2">
              <circle cx="8" cy="8" r="6" />
              <path d="M8 5v3M8 10h.01" />
            </svg>
          </div>
          <div className="space-y-1">
            <p className="text-sm text-foreground/70">{t("error.title")}</p>
            <p className="text-xs text-destructive/80 font-mono break-all">{error}</p>
          </div>
          <div className="flex gap-2 justify-center">
            <Button variant="outline" size="sm" onClick={() => fetchData(maxNodes)}>
              {t("error.retry")}
            </Button>
            <Button variant="outline" size="sm" onClick={handleBack}>
              {t("error.back")}
            </Button>
          </div>
        </div>
      </div>
    );
  }

  /* ── Empty State ───────────────────────────────────── */
  if (!activeData || !filteredData || activeData.nodes.length === 0) {
    return (
      <div className="h-screen flex items-center justify-center bg-ambient">
        <div className="text-center space-y-4">
          <p className="text-fg-subtle text-sm">{t("empty.text")}</p>
          <Button variant="outline" size="sm" onClick={handleBack}>{t("error.back")}</Button>
        </div>
      </div>
    );
  }

  /* ── Graph View ────────────────────────────────────── */
  return (
    <div className="grain h-screen flex flex-col bg-background text-foreground">
      {/* Header */}
      <header className="flex items-center justify-between px-4 sm:px-6 h-16 border-b border-white/[0.06] bg-background/80 backdrop-blur-md shrink-0">
        <div className="flex items-center gap-5 min-w-0">
          <button
            onClick={() => setFilterPanelOpen((v) => !v)}
            aria-expanded={filterPanelOpen}
            aria-controls="cn-filter-panel"
            aria-label={t("filter.title")}
            className="lg:hidden p-2 -ml-2 rounded-lg text-fg-subtle hover:text-foreground hover:bg-white/[0.05] transition-colors"
          >
            <svg className="w-5 h-5" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true">
              <path d="M2 4h12M2 8h8M2 12h5" />
            </svg>
          </button>
          <button onClick={handleBack} aria-label={t("header.home")} className="flex items-center gap-3 group">
            <img src="/CodeNexus-128.png" alt="" className="w-10 h-10 rounded-lg object-contain opacity-80 group-hover:opacity-100 transition-opacity" />
            <span className="hidden sm:inline text-lg font-semibold tracking-tight text-foreground/90 group-hover:text-primary transition-colors">
              CodeNexus
            </span>
          </button>
          {/* 文件信息 + 重新选择 */}
          <div className="hidden sm:flex items-center gap-2 min-w-0">
            <span className="text-sm text-fg-subtle shrink-0">{t("header.file")}</span>
            <span className="text-xs text-primary font-medium max-w-40 truncate rounded-full bg-primary/[0.08] px-2.5 py-1">{fileName}</span>
            <button
              onClick={handleReselectFile}
              className="text-fg-subtle hover:text-foreground/80 transition-colors ml-1"
              title={t("header.selectFile")}
              aria-label={t("header.selectFile")}
            >
              <svg className="w-3.5 h-3.5" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
                <path d="M14 2l-4 4M14 2h-4m4 0v4M2 10v3a1 1 0 001 1h9" />
              </svg>
            </button>
            <input
              ref={fileInputRef}
              type="file"
              accept=".lbug"
              className="hidden"
              onChange={handleFileInput}
            />
          </div>
        </div>

        <div className="flex items-center gap-4">
          {traceMode !== "none" && (
            <div className="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-primary/8 border border-primary/15">
              <span className="text-sm text-primary/80">
                {traceMode === "call" ? t("header.callTrace") : t("header.variableTrace")}
              </span>
              <button onClick={clearTrace} aria-label={t("header.clearTrace")} className="text-primary/70 hover:text-primary transition-colors">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                  <path d="M4 4l8 8M12 4l-8 8" />
                </svg>
              </button>
            </div>
          )}
          <div className="flex items-center gap-2">
            {loadBudget?.warnLowMemory && (
              <span
                className="hidden lg:inline text-[11px] text-fg-subtle whitespace-nowrap"
                title={t("header.memoryMode")}
              >
                {t("header.memoryMode")} · ≤{loadBudget.maxNodesCap}
              </span>
            )}
            <select
              value={maxNodes}
              onChange={(e) => setMaxNodes(Number(e.target.value))}
              aria-label={t("header.nodeLimit")}
              disabled={mode !== "file"}
              className="bg-white/[0.04] border border-white/[0.08] rounded-lg px-2 py-1.5 text-sm text-foreground/60 outline-none cursor-pointer hover:border-primary/30 transition-colors disabled:cursor-not-allowed disabled:opacity-40"
            >
              {[50, 100, 200, 500]
                .filter((n) => !loadBudget || n <= loadBudget.maxNodesCap)
                .map((n) => (
                  <option key={n} value={n}>{n}</option>
                ))}
            </select>
            <div className="hidden md:block text-sm text-fg-muted font-mono tabular-nums whitespace-nowrap">
              {filteredData.nodes.length.toLocaleString()}
              {activeData.total_nodes > filteredData.nodes.length &&
                ` / ${activeData.total_nodes.toLocaleString()}`}
              {" "}{t("hud.nodes")} / {filteredData.edges.length.toLocaleString()} {t("hud.edges")}
            </div>
          </div>
          <LanguageSwitcher />
        </div>
      </header>

      {/* Main */}
      <main className="flex-1 flex min-h-0 relative">
        {/* 小屏抽屉遮罩（筛选面板 / 节点详情共用层级约定：遮罩 z-30，面板 z-40） */}
        {filterPanelOpen && (
          <div className="lg:hidden absolute inset-0 z-30 bg-black/50" onClick={() => setFilterPanelOpen(false)} aria-hidden="true" />
        )}
        {/* Left Panel — 桌面常驻；小屏为滑入抽屉 */}
        <div
          id="cn-filter-panel"
          className={`absolute lg:relative inset-y-0 left-0 z-40 w-[260px] border-r border-border/30 flex flex-col h-full bg-background/95 lg:bg-background/90 backdrop-blur-md shrink-0 transition-[transform,visibility] duration-300 ease-out ${
            filterPanelOpen
              ? "visible translate-x-0"
              : /* 关闭态 visibility:hidden — 滑出屏幕的同时移出 Tab 焦点序 */
                "invisible -translate-x-full lg:visible lg:translate-x-0"
          }`}
        >
          <FilterPanel
            data={activeData}
            enabledLabels={effectiveLabels}
            enabledEdgeTypes={effectiveEdgeTypes}
            fileFilter={fileFilter}
            showLabels={showLabels}
            onToggleLabel={toggleLabel}
            onToggleEdgeType={toggleEdgeType}
            onFileFilterChange={setFileFilter}
            onToggleShowLabels={() => setShowLabels((v) => !v)}
            onEnableAll={enableAll}
            onDisableAll={disableAll}
          />
          <Sidebar
            nodes={filteredData.nodes}
            onSelectPath={handleSelectPath}
            selectedPath={selectedPath}
          />
        </div>

        {/* Graph */}
        <div className="flex-1 relative overflow-hidden">
          {filteredData.nodes.length === 0 ? (
            <div className="flex items-center justify-center h-full">
              <div className="text-center space-y-3">
                <p className="text-fg-subtle text-sm">{t("hud.noFiltered")}</p>
                <Button size="sm" onClick={enableAll}>{t("hud.resetFilters")}</Button>
              </div>
            </div>
          ) : (
            <>
              <GraphScene
                data={filteredData}
                highlightedIds={highlightedIds}
                traceNodeIds={traceNodeIds}
                traceEdgeIds={traceEdgeIds}
                showLabels={showLabels}
                cameraTarget={cameraTarget}
                onNodeClick={handleNodeClick}
              />

              {/* HUD — 计数已在 header 展示，这里只放筛选与追踪的补充信息 */}
              <div className="absolute top-3 left-3 text-sm text-fg-subtle pointer-events-none font-mono space-y-0.5">
                {activeData.nodes.length > filteredData.nodes.length && (
                  <p>{t("hud.filtered")} {activeData.nodes.length.toLocaleString()} {t("hud.filteredSuffix")}</p>
                )}
                {mode !== "demo" && activeData.edges.length === 0 && (
                  <p className="text-amber-400/70">{t("hud.noRelations")}</p>
                )}
                {traceNodeIds.size > 0 && (
                  <p className="text-primary/60">{t("hud.trace")} {traceNodeIds.size} {t("hud.traceNodes")}, {traceEdgeIds.size} {t("hud.traceEdges")}</p>
                )}
              </div>

              {/* Actions */}
              <div className="absolute top-3 right-3 flex items-center gap-2">
                {highlightedIds && (
                  <Button size="sm" variant="outline" onClick={() => {
                    setHighlightedIds(null);
                    setSelectedPath(null);
                    setSelectedNode(null);
                    setCameraTarget(null);
                  }}>
                    {t("hud.clear")}
                  </Button>
                )}
              </div>
            </>
          )}

          {/* 文件重载失败提示 — file 模式内重选失败时图保持原样，错误必须可见 */}
          {fileError && mode === "file" && (
            <div
              role="alert"
              className="absolute bottom-3 left-1/2 -translate-x-1/2 z-50 flex items-center gap-3 px-3.5 py-2 rounded-xl bg-destructive/10 border border-destructive/30 backdrop-blur-md text-xs text-destructive/90 max-w-[80%]"
            >
              <span className="truncate">{fileError}</span>
              <button
                onClick={() => setFileError(null)}
                aria-label={t("modal.close")}
                className="text-destructive/70 hover:text-destructive transition-colors shrink-0"
              >
                <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
                  <path d="M4 4l8 8M12 4l-8 8" />
                </svg>
              </button>
            </div>
          )}
        </div>

        {/* Node Detail Sidebar — 桌面为右侧栏；小屏为右侧滑入面板 */}
        {selectedNode && filteredData && (
          <>
            <div className="lg:hidden absolute inset-0 z-30 bg-black/50" onClick={closeNodeModal} aria-hidden="true" />
            <NodeModal
              node={selectedNode}
              allNodes={filteredData.nodes}
              allEdges={filteredData.edges}
              traceMode={traceMode}
              onCallTrace={() => startCallTrace(selectedNode)}
              onVariableTrace={() => startVariableTrace(selectedNode)}
              onClose={closeNodeModal}
              onNavigate={(node) => handleNodeClick(node)}
            />
          </>
        )}
      </main>
    </div>
  );
}
