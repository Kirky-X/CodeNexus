/* 节点详情侧边栏 — 点击节点后右侧滑出的信息面板 */

import { useMemo } from "react";
import { colorForLabel, colorForEdgeType } from "../lib/colors";
import { useI18n } from "../lib/i18n";
import type { GraphNode, GraphEdge } from "../lib/types";

interface NodeModalProps {
  node: GraphNode;
  allNodes: GraphNode[];
  allEdges: GraphEdge[];
  onClose: () => void;
  onNavigate: (node: GraphNode) => void;
}

interface Connection {
  node: GraphNode;
  edgeType: string;
  direction: "inbound" | "outbound";
}

export function NodeModal({ node, allNodes, allEdges, onClose, onNavigate }: NodeModalProps) {
  const { t } = useI18n();
  const connections = useMemo(() => {
    const nodeMap = new Map<string, GraphNode>();
    for (const n of allNodes) nodeMap.set(n.id, n);
    const conns: Connection[] = [];
    for (const edge of allEdges) {
      if (edge.source === node.id) {
        const t = nodeMap.get(edge.target);
        if (t) conns.push({ node: t, edgeType: edge.edge_type, direction: "outbound" });
      }
      if (edge.target === node.id) {
        const s = nodeMap.get(edge.source);
        if (s) conns.push({ node: s, edgeType: edge.edge_type, direction: "inbound" });
      }
    }
    return conns;
  }, [node, allNodes, allEdges]);

  const outbound = connections.filter((c) => c.direction === "outbound");
  const inbound = connections.filter((c) => c.direction === "inbound");

  return (
    <div className="w-[340px] border-l border-border/30 flex flex-col h-full bg-background/95 backdrop-blur-md shrink-0 animate-slide-in-right">
      {/* Header */}
      <div className="px-5 pt-4 pb-3 border-b border-border/30 shrink-0">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2.5 mb-2">
              <span
                className="w-3 h-3 rounded-full shrink-0 shadow-lg"
                style={{ backgroundColor: colorForLabel(node.label), boxShadow: `0 0 8px ${colorForLabel(node.label)}40` }}
              />
              <h2 className="text-base font-semibold text-foreground truncate">{node.name || "Unnamed"}</h2>
            </div>
            <span
              className="inline-block px-2.5 py-1 rounded-lg text-xs font-medium"
              style={{ backgroundColor: colorForLabel(node.label) + "18", color: colorForLabel(node.label) }}
            >
              {node.label}
            </span>
          </div>
          <button
            onClick={onClose}
            className="text-foreground/30 hover:text-foreground/70 transition-colors p-1.5 rounded-lg hover:bg-white/[0.05]"
          >
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
              <path d="M4 4l8 8M12 4l-8 8" />
            </svg>
          </button>
        </div>

        {/* Properties */}
        <div className="mt-3 space-y-1.5">
          {node.file_path && (
            <div className="flex items-start gap-2">
              <span className="text-[11px] text-foreground/30 uppercase tracking-wider w-14 shrink-0 pt-0.5">{t("modal.path")}</span>
              <span className="text-[13px] text-foreground/70 font-mono break-all leading-relaxed">
                {node.file_path}
                {node.start_line ? ` :${node.start_line}${node.end_line && node.end_line !== node.start_line ? `-${node.end_line}` : ""}` : ""}
              </span>
            </div>
          )}
          {node.qualified_name && (
            <div className="flex items-start gap-2">
              <span className="text-[11px] text-foreground/30 uppercase tracking-wider w-14 shrink-0 pt-0.5">{t("modal.qn")}</span>
              <span className="text-[13px] text-foreground/60 font-mono break-all leading-relaxed">{node.qualified_name}</span>
            </div>
          )}
          {node.project && (
            <div className="flex items-start gap-2">
              <span className="text-[11px] text-foreground/30 uppercase tracking-wider w-14 shrink-0 pt-0.5">{t("modal.project")}</span>
              <span className="text-[13px] text-foreground/60">{node.project}</span>
            </div>
          )}
        </div>

        {/* Connection stats */}
        <div className="flex gap-5 mt-3 pt-2.5 border-t border-border/20">
          {[
            { label: t("modal.outbound"), value: outbound.length, color: "text-primary" },
            { label: t("modal.inbound"), value: inbound.length, color: "text-accent" },
            { label: t("modal.total"), value: connections.length, color: "text-foreground" },
          ].map((s) => (
            <div key={s.label}>
              <p className="text-[10px] text-foreground/25 uppercase tracking-widest mb-0.5">{s.label}</p>
              <p className={`text-lg font-semibold tabular-nums ${s.color}`}>{s.value}</p>
            </div>
          ))}
        </div>
      </div>

      {/* Connections list */}
      <div className="flex-1 overflow-y-auto px-5 py-3 min-h-0">
        {outbound.length > 0 && (
          <ConnectionGroup title={t("modal.references")} icon="->" connections={outbound} onNavigate={onNavigate} />
        )}
        {inbound.length > 0 && (
          <ConnectionGroup title={t("modal.referencedBy")} icon="<-" connections={inbound} onNavigate={onNavigate} />
        )}
        {connections.length === 0 && (
          <p className="text-[13px] text-foreground/25 text-center py-12">{t("modal.noConnections")}</p>
        )}
      </div>
    </div>
  );
}

function ConnectionGroup({
  title,
  icon,
  connections,
  onNavigate,
}: {
  title: string;
  icon: string;
  connections: Connection[];
  onNavigate: (n: GraphNode) => void;
}) {
  const grouped = useMemo(() => {
    const g = new Map<string, Connection[]>();
    for (const c of connections) g.set(c.edgeType, [...(g.get(c.edgeType) ?? []), c]);
    return [...g.entries()].sort((a, b) => b[1].length - a[1].length);
  }, [connections]);

  return (
    <div className="mb-4">
      <p className="text-[13px] font-medium text-foreground/40 mb-2.5 flex items-center gap-1.5">
        <span className="text-foreground/20">{icon}</span>
        {title}
        <span className="text-foreground/15 text-[11px]">({connections.length})</span>
      </p>
      <div className="space-y-2">
        {grouped.map(([type, conns]) => (
          <div key={type}>
            <p className="text-[11px] text-foreground/25 uppercase tracking-wider mb-1.5 flex items-center gap-1.5">
              <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: colorForEdgeType(type) }} />
              {type.replace(/_/g, " ").toLowerCase()}
            </p>
            <div className="space-y-0.5">
              {conns.map((c, i) => (
                <button
                  key={`${c.node.id}-${i}`}
                  onClick={() => onNavigate(c.node)}
                  className="flex items-center gap-2 w-full text-left px-3 py-2 rounded-lg hover:bg-white/[0.04] transition-colors group"
                >
                  <span
                    className="w-2 h-2 rounded-full shrink-0"
                    style={{ backgroundColor: colorForLabel(c.node.label) }}
                  />
                  <span className="text-[13px] text-foreground/60 group-hover:text-foreground/90 truncate transition-colors">
                    {c.node.name}
                  </span>
                  <span className="text-[11px] text-foreground/15 ml-auto shrink-0">{c.node.label}</span>
                </button>
              ))}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
