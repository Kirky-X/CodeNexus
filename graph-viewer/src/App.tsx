import { useEffect, useState, useCallback, useMemo, useRef } from "react";
import { useGraphData } from "./hooks/useGraphData";
import { useTrace } from "./hooks/useTrace";
import { GraphScene, computeCameraTarget } from "./components/GraphScene";
import { FilterPanel } from "./components/FilterPanel";
import { Sidebar } from "./components/Sidebar";
import { NodeModal } from "./components/NodeModal";
import { Button } from "./components/ui/button";
import { generateDemoData } from "./lib/demoData";
import { fetchProjects, fetchSchema } from "./api/client";
import { useI18n } from "./lib/i18n";
import { LanguageSwitcher } from "./components/LanguageSwitcher";
import LightRays from "./components/LightRays";
import type { GraphNode, GraphData, ProjectInfo } from "./lib/types";
import type { CameraTarget } from "./components/GraphScene";

const AUTO_REFRESH_INTERVAL = 10_000; /* 10秒轮询一次 */
const EMPTY_SET = new Set<string>();

type InputMode = "project" | "path";

export function App() {
  const { data, loading, error, fetchData, silentRefresh } = useGraphData();
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

  /* 项目配置 */
  const [project, setProject] = useState<string | null>(null);
  const [lbugPath, setLbugPath] = useState<string | null>(null);
  const [demoMode, setDemoMode] = useState(false);
  const [demoData, setDemoData] = useState<GraphData | null>(null);

  /* Header 项目下拉框 */
  const [showProjectDropdown, setShowProjectDropdown] = useState(false);
  const [projectSearch, setProjectSearch] = useState("");
  const dropdownBtnRef = useRef<HTMLButtonElement>(null);
  const dropdownPanelRef = useRef<HTMLDivElement>(null);
  const [dropdownPos, setDropdownPos] = useState({ top: 0, left: 0 });

  /* 打开下拉框时计算按钮位置，用 fixed 定位避免被 header backdrop-blur 裁剪 */
  const openDropdown = useCallback(() => {
    if (dropdownBtnRef.current) {
      const rect = dropdownBtnRef.current.getBoundingClientRect();
      setDropdownPos({ top: rect.bottom + 4, left: rect.left });
    }
    setProjectSearch("");
    setShowProjectDropdown(true);
  }, []);

  /* 点击外部关闭下拉框 */
  useEffect(() => {
    if (!showProjectDropdown) return;
    const handleClickOutside = (e: MouseEvent) => {
      if (
        dropdownPanelRef.current && !dropdownPanelRef.current.contains(e.target as Node) &&
        dropdownBtnRef.current && !dropdownBtnRef.current.contains(e.target as Node)
      ) {
        setShowProjectDropdown(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [showProjectDropdown]);

  /* Landing page state */
  const [inputMode, setInputMode] = useState<InputMode>("project");
  const [discoveredProjects, setDiscoveredProjects] = useState<ProjectInfo[]>([]);
  const [projectsLoading, setProjectsLoading] = useState(false);

  /* 加载已发现项目列表 */
  useEffect(() => {
    setProjectsLoading(true);
    fetchProjects()
      .then((projects) => {
        /* 按名称去重（保留节点数最多的），然后按节点数降序排列 */
        const seen = new Map<string, ProjectInfo>();
        for (const p of projects) {
          const existing = seen.get(p.name);
          if (!existing || p.node_count > existing.node_count) {
            seen.set(p.name, p);
          }
        }
        return Array.from(seen.values()).sort((a, b) => b.node_count - a.node_count);
      })
      .then((projects) => setDiscoveredProjects(projects))
      .catch(() => setDiscoveredProjects([]))
      .finally(() => setProjectsLoading(false));
  }, []);

  /* 加载数据 */
  useEffect(() => {
    if (demoMode) {
      setDemoData(generateDemoData());
      return;
    }
    if (project) fetchData(project, maxNodes, undefined, lbugPath ?? undefined);
  }, [project, fetchData, demoMode, lbugPath, maxNodes]);

  /* 自动刷新 — 轮询 schema 检测数据变化 */
  const prevSchemaRef = useRef<{ nodes: number; edges: number } | null>(null);
  useEffect(() => {
    if (!project || demoMode) return;
    let cancelled = false;

    const poll = async () => {
      try {
        const schema = await fetchSchema(project, lbugPath ?? undefined);
        if (cancelled) return;
        const cur = { nodes: schema.total_nodes, edges: schema.total_edges };
        if (prevSchemaRef.current &&
            (prevSchemaRef.current.nodes !== cur.nodes || prevSchemaRef.current.edges !== cur.edges)) {
          /* 数据已变化，静默刷新 */
          silentRefresh(project, maxNodes, undefined, lbugPath ?? undefined);
        }
        prevSchemaRef.current = cur;
      } catch {
        /* 轮询失败不影响现有视图 */
      }
    };

    /* 立即检查一次 */
    poll();
    const timer = setInterval(poll, AUTO_REFRESH_INTERVAL);
    return () => { cancelled = true; clearInterval(timer); };
  }, [project, lbugPath, demoMode, silentRefresh]);

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
    setProject("demo");
  }, []);

  const handleProjectSelect = useCallback((projectName: string) => {
    setProject(projectName);
    setLbugPath(null);
  }, []);

  const handlePathSubmit = useCallback((path: string) => {
    setLbugPath(path);
    setProject(path);
  }, []);

  const handleBack = useCallback(() => {
    setProject(null);
    setLbugPath(null);
    setSelectedNode(null);
    setHighlightedIds(null);
    setSelectedPath(null);
    setCameraTarget(null);
    clearTrace();
  }, [clearTrace]);

  /* ── Landing Page ─────────────────────────────────── */
  if (!project) {
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

            {/* Input Card */}
            <div className="glass rounded-2xl p-6 space-y-5">
              {/* Tabs */}
              <div className="flex gap-1 p-1 rounded-xl bg-white/[0.03] border border-white/[0.04]">
                <button
                  className={`flex-1 px-4 py-2.5 rounded-lg text-sm font-medium transition-all duration-200 ${
                    inputMode === "project"
                      ? "bg-primary/10 text-primary shadow-sm"
                      : "text-foreground/40 hover:text-foreground/60"
                  }`}
                  onClick={() => setInputMode("project")}
                >
                  {t("tab.project")}
                </button>
                <button
                  className={`flex-1 px-4 py-2.5 rounded-lg text-sm font-medium transition-all duration-200 ${
                    inputMode === "path"
                      ? "bg-primary/10 text-primary shadow-sm"
                      : "text-foreground/40 hover:text-foreground/60"
                  }`}
                  onClick={() => setInputMode("path")}
                >
                  {t("tab.path")}
                </button>
              </div>

              {/* Input */}
              {inputMode === "project" ? (
                <div className="space-y-4">
                  <input
                    className="w-full bg-white/[0.03] border border-white/[0.06] rounded-xl px-5 py-3.5 text-sm text-foreground placeholder-foreground/20 outline-none focus:border-primary/30 transition-colors"
                    placeholder={t("input.project.placeholder")}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && e.currentTarget.value) {
                        handleProjectSelect(e.currentTarget.value);
                      }
                    }}
                  />
                  {/* Discovered Projects */}
                  {projectsLoading ? (
                    <div className="flex items-center justify-center py-4">
                      <div className="w-4 h-4 border-2 border-primary/30 border-t-primary rounded-full animate-spin" />
                    </div>
                  ) : discoveredProjects.length > 0 ? (
                    <div className="space-y-2">
                      <p className="text-[11px] text-foreground/25 uppercase tracking-wider px-1">{t("landing.discovered")}</p>
                      <div className="grid gap-2 max-h-48 overflow-y-auto">
                        {discoveredProjects.map((p) => (
                          <button
                            key={p.name}
                            onClick={() => handleProjectSelect(p.name)}
                            className={`project-card glass-subtle rounded-xl px-4 py-3 flex items-center justify-between group text-left ${
                              p.node_count === 0 ? "opacity-40" : ""
                            }`}
                          >
                            <div className="flex items-center gap-3 min-w-0">
                              <div className={`w-2 h-2 rounded-full shrink-0 ${
                                p.node_count > 0 ? "bg-primary/60" : "bg-foreground/15"
                              }`} />
                              <span className="text-sm text-foreground/70 group-hover:text-foreground/90 truncate transition-colors">
                                {p.name}
                              </span>
                            </div>
                            <div className="flex items-center gap-3 shrink-0">
                              <span className="text-[10px] text-foreground/20 font-mono">
                                {p.node_count > 0
                                  ? `${p.node_count} ${t("landing.nodes")}`
                                  : t("landing.empty")}
                              </span>
                              <svg
                                className="w-3.5 h-3.5 text-foreground/15 group-hover:text-primary/50 transition-colors"
                                viewBox="0 0 16 16"
                                fill="none"
                                stroke="currentColor"
                                strokeWidth="2"
                                strokeLinecap="round"
                              >
                                <path d="M6 4l4 4-4 4" />
                              </svg>
                            </div>
                          </button>
                        ))}
                      </div>
                    </div>
                  ) : null}
                </div>
              ) : (
                <div className="space-y-3">
                  <input
                    className="w-full bg-white/[0.03] border border-white/[0.06] rounded-xl px-5 py-3.5 text-sm text-foreground placeholder-foreground/20 outline-none focus:border-primary/30 transition-colors font-mono"
                    placeholder={t("input.path.placeholder")}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && e.currentTarget.value) {
                        handlePathSubmit(e.currentTarget.value);
                      }
                    }}
                  />
                  <p className="text-[11px] text-foreground/25 leading-relaxed">
                    {t("input.path.hint")}
                  </p>
                </div>
              )}

              {/* Demo mode */}
              <div className="flex items-center gap-3 pt-2 border-t border-white/[0.04]">
                <div className="h-px flex-1 bg-white/[0.03]" />
                <Button variant="outline" size="sm" onClick={enterDemoMode} className="text-xs">
                  {t("landing.demo")}
                </Button>
                <div className="h-px flex-1 bg-white/[0.03]" />
              </div>
            </div>

            {/* Footer hint */}
            <p className="text-center text-[11px] text-foreground/15">
              {t("landing.hint")}
            </p>
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
            <p className="text-sm text-foreground/30 font-mono">{lbugPath ?? project}</p>
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
            <Button variant="outline" size="sm" onClick={() => fetchData(project, maxNodes, undefined, lbugPath ?? undefined)}>
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
          {/* 项目切换下拉框 */}
          <div>
            <button
              ref={dropdownBtnRef}
              onClick={() => { if (showProjectDropdown) { setShowProjectDropdown(false); } else { openDropdown(); } }}
              className="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-white/[0.04] border border-white/[0.08] hover:border-primary/30 transition-colors"
            >
              <span className="text-sm text-foreground/40">{t("header.db")}</span>
              <span className="text-sm text-primary font-medium max-w-40 truncate">
                {lbugPath ? lbugPath.split("/").pop() : project}
              </span>
              <svg className="w-4 h-4 text-foreground/30" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                <path d="M4 6l4 4 4-4" />
              </svg>
            </button>
          </div>
          {/* 下拉面板 — 用 fixed 定位脱离 header 层叠上下文 */}
          {showProjectDropdown && (
            <div
              ref={dropdownPanelRef}
              className="fixed w-64 rounded-xl bg-background/95 backdrop-blur-xl border border-border/50 shadow-2xl z-50 overflow-hidden"
              style={{ top: dropdownPos.top, left: dropdownPos.left }}
            >
                <div className="p-2 border-b border-border/30">
                  <input
                    className="w-full bg-white/[0.04] border border-white/[0.06] rounded-lg px-3 py-2 text-sm text-foreground placeholder-foreground/25 outline-none focus:border-primary/30 transition-colors"
                    placeholder={t("filter.projectPlaceholder")}
                    value={projectSearch}
                    onChange={(e) => setProjectSearch(e.target.value)}
                    autoFocus
                  />
                </div>
                <div className="max-h-56 overflow-y-auto">
                  {discoveredProjects
                    .filter((p) => p.name.toLowerCase().includes(projectSearch.toLowerCase()))
                    .map((p) => (
                      <button
                        key={p.name}
                        onClick={() => {
                          setProject(p.name);
                          setLbugPath(null);
                          setShowProjectDropdown(false);
                        }}
                        className={`w-full flex items-center justify-between px-3 py-2.5 text-sm hover:bg-white/[0.04] transition-colors ${
                          p.name === project ? "text-primary bg-primary/5" : "text-foreground/70"
                        }`}
                      >
                        <span className="truncate">{p.name}</span>
                        <span className="text-xs text-foreground/25 font-mono shrink-0 ml-2">
                          {p.node_count > 0 ? `${(p.node_count / 1000).toFixed(0)}k` : "—"}
                        </span>
                      </button>
                    ))}
                </div>
            </div>
          )}
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
                {/* 自动刷新指示器 */}
                <div className="flex items-center gap-1.5 px-2.5 py-1 rounded-lg bg-white/[0.03] border border-white/[0.05]">
                  <div className="w-1.5 h-1.5 rounded-full bg-primary/50 animate-pulse" />
                  <span className="text-xs text-foreground/25">{t("header.auto")}</span>
                </div>
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
