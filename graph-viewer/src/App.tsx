import { useEffect, useState, useCallback, useMemo, useRef } from "react";
import { useGraphData } from "./hooks/useGraphData";
import { useTrace } from "./hooks/useTrace";
import { GraphScene, computeCameraTarget } from "./components/GraphScene";
import { FilterPanel } from "./components/FilterPanel";
import { Sidebar } from "./components/Sidebar";
import { NodeModal } from "./components/NodeModal";
import { Button } from "./components/ui/button";
import { generateDemoData } from "./lib/demoData";
import { loadLbugFile, closeDatabase } from "./api/client";
import { useI18n } from "./lib/i18n";
import { LanguageSwitcher } from "./components/LanguageSwitcher";
import LightRays from "./components/LightRays";
import type { GraphNode, GraphData } from "./lib/types";
import type { CameraTarget } from "./components/GraphScene";

const EMPTY_SET = new Set<string>();

export function App() {
  const { data, loading, error, fetchData } = useGraphData();
  const { t } = useI18n();

  /* 筛选状态 */
  const [enabledLabels, setEnabledLabels] = useState<Set<string>>(new Set());
  const [enabledEdgeTypes, setEnabledEdgeTypes] = useState<Set<string>>(new Set());
  const [fileFilter, setFileFilter] = useState("");
  const [projectFilter] = useState("");
  const [showLabels, setShowLabels] = useState(true);
  const [maxNodes, setMaxNodes] = useState(100);

  /* 选择状态 */
  const [selectedNode, setSelectedNode] = useState<GraphNode | null>(null);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [highlightedIds, setHighlightedIds] = useState<Set<string> | null>(null);
  const [cameraTarget, setCameraTarget] = useState<CameraTarget | null>(null);

  /* 文件加载状态 */
  const [fileLoaded, setFileLoaded] = useState(false);
  const [fileName, setFileName] = useState("");
  const [fileLoading, setFileLoading] = useState(false);
  const [fileError, setFileError] = useState<string | null>(null);
  const [demoMode, setDemoMode] = useState(false);
  const [demoData, setDemoData] = useState<GraphData | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [isDragOver, setIsDragOver] = useState(false);

  /* 文件加载处理 */
  const handleFileLoad = useCallback(async (file: File) => {
    if (!file.name.endsWith(".lbug")) {
      setFileError("请选择 .lbug 文件");
      return;
    }
    setFileLoading(true);
    setFileError(null);
    try {
      await loadLbugFile(file);
      setFileName(file.name);
      setFileLoaded(true);
    } catch (e) {
      setFileError(e instanceof Error ? e.message : "加载数据库失败");
    } finally {
      setFileLoading(false);
    }
  }, []);

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

  /* 加载数据 */
  useEffect(() => {
    if (demoMode) {
      setDemoData(generateDemoData());
      return;
    }
    if (fileLoaded) fetchData(maxNodes);
  }, [fileLoaded, fetchData, demoMode, maxNodes]);

  /* 初始化筛选器 */
  useEffect(() => {
    const d = demoMode ? demoData : data;
    if (!d) return;
    setEnabledLabels(new Set(d.nodes.map((n) => n.label)));
    setEnabledEdgeTypes(new Set(d.edges.map((e) => e.edge_type)));
  }, [data, demoData, demoMode]);

  /* 追踪 */
  const activeNodes = demoMode ? (demoData?.nodes ?? []) : (data?.nodes ?? []);
  const activeEdges = demoMode ? (demoData?.edges ?? []) : (data?.edges ?? []);
  const { traceNodeIds, traceEdgeIds, traceMode, clearTrace } =
    useTrace(activeNodes, activeEdges);

  const activeData = demoMode ? demoData : data;

  /* 同步派生有效筛选集 — 用 useMemo 稳定引用，避免每次渲染创建新 Set */
  const allDataLabels = useMemo(
    () => activeData ? new Set(activeData.nodes.map((n) => n.label)) : EMPTY_SET,
    [activeData],
  );
  const allDataEdgeTypes = useMemo(
    () => activeData ? new Set(activeData.edges.map((e) => e.edge_type)) : EMPTY_SET,
    [activeData],
  );
  /* 如果用户选择了全部（或尚未初始化），直接使用数据中的全集 */
  const effectiveLabels = (enabledLabels.size === 0 || enabledLabels.size >= allDataLabels.size)
    ? allDataLabels : enabledLabels;
  const effectiveEdgeTypes = (enabledEdgeTypes.size === 0 || enabledEdgeTypes.size >= allDataEdgeTypes.size)
    ? allDataEdgeTypes : enabledEdgeTypes;

  /* 计算过滤后的数据 — 无筛选时直接返回原数组避免内存分配 */
  const filteredData: GraphData | null = useMemo(() => {
    if (!activeData) return null;

    const noLabelFilter = effectiveLabels === allDataLabels;
    const noFileFilter = !fileFilter;
    const noProjectFilter = !projectFilter;

    /* 无筛选 — 直接返回原数据，零分配 */
    if (noLabelFilter && noFileFilter && noProjectFilter) {
      return activeData;
    }

    const nodes = activeData.nodes.filter((n) => {
      if (!effectiveLabels.has(n.label)) return false;
      if (fileFilter && n.file_path && !n.file_path.includes(fileFilter)) return false;
      if (projectFilter && n.project && !n.project.toLowerCase().includes(projectFilter.toLowerCase())) return false;
      return true;
    });

    const nodeIds = new Set(nodes.map((n) => n.id));
    const edges = activeData.edges.filter(
      (e) => effectiveEdgeTypes.has(e.edge_type) && nodeIds.has(e.source) && nodeIds.has(e.target),
    );

    return { nodes, edges, total_nodes: activeData.total_nodes, total_edges: activeData.total_edges };
  }, [activeData, effectiveLabels, effectiveEdgeTypes, allDataLabels, fileFilter, projectFilter]);

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
    if (!data) return;
    setEnabledLabels(new Set(data.nodes.map((n) => n.label)));
    setEnabledEdgeTypes(new Set(data.edges.map((e) => e.edge_type)));
  }, [data]);

  const disableAll = useCallback(() => {
    setEnabledLabels(new Set());
    setEnabledEdgeTypes(new Set());
  }, []);

  const enterDemoMode = useCallback(() => {
    setDemoMode(true);
    setFileLoaded(true);
  }, []);

  const handleBack = useCallback(async () => {
    await closeDatabase();
    setFileLoaded(false);
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
  if (!fileLoaded) {
    return (
      <div className="h-screen flex flex-col bg-ambient text-foreground overflow-hidden relative">
        {/* LightRays Background */}
        <div style={{ width: '100%', height: '100%', position: 'absolute', inset: 0, zIndex: 0 }}>
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
        </div>

        {/* Navigation */}
        <nav className="flex items-center justify-between px-8 py-5 shrink-0 relative" style={{ zIndex: 1 }}>
          <div className="flex items-center gap-3">
            <img src="/CodeNexus.png" alt="CodeNexus" className="w-9 h-9 rounded-lg object-contain opacity-90" />
            <span className="text-lg font-semibold tracking-tight text-foreground/90">CodeNexus</span>
          </div>
          <div className="flex items-center gap-3">
            <span className="text-sm text-foreground/25 font-mono">{t("nav.subtitle")}</span>
            <LanguageSwitcher />
          </div>
        </nav>

        {/* Hero */}
        <div className="flex-1 flex items-center justify-center px-6 relative" style={{ zIndex: 1 }}>
          <div className="w-full max-w-2xl space-y-10">
            {/* Title */}
            <div className="text-center space-y-3">
              <h1 className="text-4xl md:text-5xl font-semibold tracking-tight text-foreground/90 leading-tight">
                {t("hero.title")}
              </h1>
              <p className="text-base text-foreground/40 max-w-md mx-auto">
                {t("hero.subtitle")}
              </p>
            </div>

            {/* File Drop Zone */}
            <div className="glass rounded-2xl p-6 space-y-5">
              <input
                ref={fileInputRef}
                type="file"
                accept=".lbug"
                className="hidden"
                onChange={handleFileInput}
              />
              <div
                className={`flex flex-col items-center justify-center py-10 px-6 rounded-xl border-2 border-dashed transition-colors cursor-pointer ${
                  isDragOver
                    ? "border-primary/50 bg-primary/5"
                    : "border-white/[0.08] hover:border-white/[0.15]"
                }`}
                onClick={() => fileInputRef.current?.click()}
                onDragOver={handleDragOver}
                onDragLeave={handleDragLeave}
                onDrop={handleDrop}
              >
                {fileLoading ? (
                  <div className="flex flex-col items-center gap-3">
                    <div className="w-8 h-8 border-2 border-primary/30 border-t-primary rounded-full animate-spin" />
                    <p className="text-sm text-foreground/50">{t("landing.loadingFile")}</p>
                  </div>
                ) : (
                  <div className="flex flex-col items-center gap-3">
                    <svg className="w-10 h-10 text-foreground/20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                      <path d="M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4" />
                      <polyline points="17 8 12 3 7 8" />
                      <line x1="12" y1="3" x2="12" y2="15" />
                    </svg>
                    <p className="text-sm text-foreground/50">{t("landing.dropFile")}</p>
                    <p className="text-xs text-foreground/30">{t("landing.orSelectFile")}</p>
                  </div>
                )}
              </div>
              {fileError && (
                <p className="text-xs text-destructive/80 text-center">{fileError}</p>
              )}
              <p className="text-[11px] text-foreground/20 text-center">{t("landing.acceptsLbug")}</p>

              {/* Demo mode */}
              <div className="flex items-center gap-3 pt-2 border-t border-white/[0.04]">
                <div className="h-px flex-1 bg-white/[0.03]" />
                <Button variant="outline" size="sm" onClick={enterDemoMode} className="text-xs">
                  {t("landing.demo")}
                </Button>
                <div className="h-px flex-1 bg-white/[0.03]" />
              </div>
            </div>
          </div>
        </div>
      </div>
    );
  }

  /* ── Loading State ─────────────────────────────────── */
  if (loading) {
    return (
      <div className="h-screen flex items-center justify-center bg-ambient">
        <div className="text-center space-y-8">
          {/* Logo + 动画光环 */}
          <div className="relative w-24 h-24 mx-auto">
            {/* 旋转光环 */}
            <div className="absolute inset-0 rounded-full border-2 border-transparent border-t-primary/60 border-r-primary/20 animate-spin" />
            <div className="absolute inset-1 rounded-full border border-transparent border-b-accent/40 border-l-accent/10 animate-spin" style={{ animationDirection: 'reverse', animationDuration: '1.5s' }} />
            {/* Logo */}
            <div className="absolute inset-0 flex items-center justify-center">
              <img
                src="/CodeNexus.png"
                alt="CodeNexus"
                className="w-14 h-14 rounded-xl object-contain animate-pulse"
              />
            </div>
          </div>
          {/* 文字信息 */}
          <div className="space-y-2">
            <p className="text-base text-foreground/60 font-medium">{t("loading.text")}</p>
            <p className="text-sm text-foreground/30 font-mono">{fileName}</p>
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
          <p className="text-foreground/30 text-sm">{t("empty.text")}</p>
          <Button variant="outline" size="sm" onClick={handleBack}>{t("error.back")}</Button>
        </div>
      </div>
    );
  }

  /* ── Graph View ────────────────────────────────────── */
  return (
    <div className="h-screen flex flex-col bg-background text-foreground">
      {/* Header */}
      <header className="flex items-center justify-between px-6 h-16 border-b border-border/40 bg-background/80 backdrop-blur-md shrink-0">
        <div className="flex items-center gap-5">
          <button onClick={handleBack} className="flex items-center gap-3 group">
            <img src="/CodeNexus.png" alt="CodeNexus" className="w-10 h-10 rounded-lg object-contain opacity-80 group-hover:opacity-100 transition-opacity" />
            <span className="text-lg font-semibold tracking-tight text-foreground/90 group-hover:text-primary transition-colors">
              CodeNexus
            </span>
          </button>
          {/* 文件信息 + 重新选择 */}
          <div className="flex items-center gap-2">
            <span className="text-sm text-foreground/40">{t("header.file")}</span>
            <span className="text-sm text-primary font-medium max-w-40 truncate">{fileName}</span>
            <button
              onClick={handleReselectFile}
              className="text-xs text-foreground/30 hover:text-foreground/60 transition-colors ml-1"
              title={t("header.selectFile")}
            >
              <svg className="w-3.5 h-3.5" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
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
              <button onClick={clearTrace} className="text-primary/30 hover:text-primary/70 transition-colors">
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                  <path d="M4 4l8 8M12 4l-8 8" />
                </svg>
              </button>
            </div>
          )}
          <div className="flex items-center gap-2">
            <select
              value={maxNodes}
              onChange={(e) => setMaxNodes(Number(e.target.value))}
              className="bg-white/[0.04] border border-white/[0.08] rounded-lg px-2 py-1 text-sm text-foreground/60 outline-none cursor-pointer hover:border-primary/30 transition-colors"
            >
              <option value={50}>50</option>
              <option value={100}>100</option>
              <option value={200}>200</option>
              <option value={500}>500</option>
            </select>
            <div className="text-sm text-white/60 font-mono tabular-nums">
              {filteredData.nodes.length.toLocaleString()} {t("hud.nodes")} / {filteredData.edges.length.toLocaleString()} {t("hud.edges")}
            </div>
          </div>
          <LanguageSwitcher />
        </div>
      </header>

      {/* Main */}
      <main className="flex-1 flex min-h-0">
        {/* Left Panel */}
        <div className="w-[260px] border-r border-border/30 flex flex-col h-full bg-background/90 backdrop-blur-md shrink-0">
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
                <p className="text-foreground/30 text-sm">{t("hud.noFiltered")}</p>
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

              {/* HUD */}
              <div className="absolute top-3 left-3 text-sm text-white/20 pointer-events-none font-mono space-y-0.5">
                <p>{filteredData.nodes.length.toLocaleString()} {t("hud.nodes")} / {filteredData.edges.length.toLocaleString()} {t("hud.edges")}</p>
                {activeData.nodes.length > filteredData.nodes.length && (
                  <p className="text-white/15">{t("hud.filtered")} {activeData.nodes.length.toLocaleString()} {t("hud.filteredSuffix")}</p>
                )}
                {traceNodeIds.size > 0 && (
                  <p className="text-primary/40">{t("hud.trace")} {traceNodeIds.size} {t("hud.traceNodes")}, {traceEdgeIds.size} {t("hud.traceEdges")}</p>
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
        </div>

        {/* Node Detail Sidebar */}
        {selectedNode && filteredData && (
          <NodeModal
            node={selectedNode}
            allNodes={filteredData.nodes}
            allEdges={filteredData.edges}
            onClose={() => {
              setSelectedNode(null);
              setHighlightedIds(null);
              setSelectedPath(null);
              clearTrace();
            }}
            onNavigate={(node) => handleNodeClick(node)}
          />
        )}
      </main>
    </div>
  );
}
