/* 文件树侧边栏 — 按文件路径组织节点 */

import { useMemo, useState } from "react";
import { ScrollArea } from "./ui/scroll-area";
import { Input } from "./ui/input";
import type { GraphNode } from "../lib/types";
import { useI18n } from "../lib/i18n";
import { colorForLabel } from "../lib/colors";

interface SidebarProps {
  nodes: GraphNode[];
  onSelectPath: (path: string, nodeIds: Set<string>) => void;
  selectedPath: string | null;
}

interface DirNode {
  name: string;
  fullPath: string;
  children: Map<string, DirNode>;
  nodeIds: Set<string>;
  directNodes: GraphNode[];
}

function buildFileTree(nodes: GraphNode[]): DirNode {
  const root: DirNode = { name: "/", fullPath: "", children: new Map(), nodeIds: new Set(), directNodes: [] };
  for (const node of nodes) {
    if (!node.file_path) continue;
    const parts = node.file_path.split("/");
    let cur = root;
    for (let i = 0; i < parts.length - 1; i++) {
      if (!parts[i]) continue;
      let child = cur.children.get(parts[i]);
      if (!child) {
        const prefix = parts.slice(0, i + 1).join("/");
        child = { name: parts[i], fullPath: prefix, children: new Map(), nodeIds: new Set(), directNodes: [] };
        cur.children.set(parts[i], child);
      }
      cur = child;
    }
    cur.directNodes.push(node);
  }
  function collect(d: DirNode): Set<string> {
    const ids = new Set<string>();
    for (const n of d.directNodes) ids.add(n.id);
    for (const c of d.children.values()) for (const id of collect(c)) ids.add(id);
    d.nodeIds = ids;
    return ids;
  }
  collect(root);
  return root;
}

function TreeItem({ dir, depth, onSelect, selectedPath }: {
  dir: DirNode; depth: number;
  onSelect: (path: string, ids: Set<string>) => void;
  selectedPath: string | null;
}) {
  const [expanded, setExpanded] = useState(false);
  const isSelected = selectedPath === dir.fullPath;
  const sorted = useMemo(() => [...dir.children.values()].sort((a, b) => a.name.localeCompare(b.name)), [dir.children]);
  const sortedNodes = useMemo(() => [...dir.directNodes].sort((a, b) => a.name.localeCompare(b.name)), [dir.directNodes]);

  return (
    <div>
      <button
        onClick={() => { setExpanded(!expanded); onSelect(dir.fullPath, dir.nodeIds); }}
        className={`flex items-center gap-1.5 w-full text-left px-3 py-[5px] text-[13px] transition-colors ${
          isSelected ? "bg-primary/10 text-primary" : "text-foreground/60 hover:text-foreground/80 hover:bg-white/[0.03]"
        }`}
        style={{ paddingLeft: `${depth * 16 + 12}px` }}
      >
        <span className="text-foreground/20 w-3 text-center text-[11px] shrink-0">
          {(dir.children.size > 0 || dir.directNodes.length > 0) ? (expanded ? "▾" : "▸") : ""}
        </span>
        <span className="truncate font-medium">{dir.name}</span>
        <span className="text-foreground/15 ml-auto text-[11px] tabular-nums shrink-0">{dir.nodeIds.size}</span>
      </button>
      {expanded && (
        <>
          {sorted.map((c) => <TreeItem key={c.fullPath} dir={c} depth={depth + 1} onSelect={onSelect} selectedPath={selectedPath} />)}
          {sortedNodes.map((gn) => (
            <button
              key={gn.id}
              onClick={() => onSelect(dir.fullPath + "/" + gn.name, new Set([gn.id]))}
              className="flex items-center gap-1.5 w-full text-left px-3 py-[3px] text-[12px] text-foreground/40 hover:text-foreground/60 hover:bg-white/[0.02] transition-colors"
              style={{ paddingLeft: `${(depth + 1) * 16 + 12}px` }}
            >
              <span className="w-[5px] h-[5px] rounded-full shrink-0" style={{ backgroundColor: colorForLabel(gn.label) }} />
              <span className="truncate font-mono">{gn.name}</span>
              <span className="text-foreground/10 ml-auto text-[11px] shrink-0">{gn.label}</span>
            </button>
          ))}
        </>
      )}
    </div>
  );
}

export function Sidebar({ nodes, onSelectPath, selectedPath }: SidebarProps) {
  const { t } = useI18n();
  const [search, setSearch] = useState("");
  const tree = useMemo(() => buildFileTree(nodes), [nodes]);

  const filtered = useMemo(() => {
    if (!search) return null;
    const q = search.toLowerCase();
    return nodes.filter((n) => n.name.toLowerCase().includes(q) || (n.file_path ?? "").toLowerCase().includes(q)).slice(0, 50);
  }, [nodes, search]);

  const topLevel = useMemo(() => [...tree.children.values()].sort((a, b) => a.name.localeCompare(b.name)), [tree.children]);

  return (
    <div className="flex flex-col flex-1 min-h-0 border-t border-border/30">
      <div className="px-3 pt-3 pb-2 shrink-0">
        <span className="text-[12px] font-medium text-foreground/50 uppercase tracking-widest">{t("sidebar.title")}</span>
      </div>
      <div className="px-3 pb-2.5 shrink-0">
        <Input
          placeholder={t("sidebar.searchPlaceholder")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
      </div>

      <ScrollArea className="flex-1 min-h-0">
        <div className="py-1">
          {filtered ? (
            filtered.length === 0 ? (
              <p className="text-foreground/20 text-[13px] px-4 py-6 text-center">{t("sidebar.noResults")}</p>
            ) : (
              filtered.map((n) => (
                <button
                  key={n.id}
                  onClick={() => onSelectPath(n.file_path ?? "", new Set([n.id]))}
                  className="flex items-center gap-2 w-full text-left px-4 py-1.5 text-[12px] hover:bg-white/[0.03] transition-colors"
                >
                  <span className="w-[5px] h-[5px] rounded-full shrink-0" style={{ backgroundColor: colorForLabel(n.label) }} />
                  <span className="text-foreground/60 truncate">{n.name}</span>
                  <span className="text-foreground/15 ml-auto text-[11px] font-mono truncate max-w-[100px]">{n.file_path}</span>
                </button>
              ))
            )
          ) : (
            topLevel.map((c) => <TreeItem key={c.fullPath} dir={c} depth={0} onSelect={onSelectPath} selectedPath={selectedPath} />)
          )}
        </div>
      </ScrollArea>

      {selectedPath && (
        <div className="px-3 py-2 border-t border-border/30">
          <button
            onClick={() => onSelectPath("", new Set())}
            className="w-full px-3 py-1.5 rounded-lg bg-white/[0.04] hover:bg-white/[0.07] text-[12px] text-foreground/40 font-medium transition-all"
          >
            {t("sidebar.clearSelection")}
          </button>
        </div>
      )}
    </div>
  );
}
