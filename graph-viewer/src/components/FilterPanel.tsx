/* 筛选面板 — 按节点类型、边类型、文件路径筛选 */

import { useMemo } from "react";
import { ScrollArea } from "./ui/scroll-area";
import { Input } from "./ui/input";
import { colorForLabel, colorForEdgeType } from "../lib/colors";
import { useI18n } from "../lib/i18n";
import { NODE_LABEL_GROUPS } from "../lib/types";
import type { GraphData } from "../lib/types";

interface FilterPanelProps {
  data: GraphData | null;
  enabledLabels: Set<string>;
  enabledEdgeTypes: Set<string>;
  fileFilter: string;
  showLabels: boolean;
  onToggleLabel: (label: string) => void;
  onToggleEdgeType: (type: string) => void;
  onFileFilterChange: (value: string) => void;
  onToggleShowLabels: () => void;
  onEnableAll: () => void;
  onDisableAll: () => void;
}

export function FilterPanel({
  data, enabledLabels, enabledEdgeTypes, fileFilter,
  showLabels, onToggleLabel, onToggleEdgeType,
  onFileFilterChange, onToggleShowLabels,
  onEnableAll, onDisableAll,
}: FilterPanelProps) {
  const { t } = useI18n();

  /* 节点分组名称翻译映射 */
  const groupNames: Record<string, string> = {
    "\u7ed3\u6784": t("group.structure"),
    "\u7c7b\u578b\u5b9a\u4e49": t("group.typeDef"),
    "\u53ef\u8c03\u7528": t("group.callable"),
    "\u53d8\u91cf": t("group.variable"),
    "\u5143\u4fe1\u606f": t("group.meta"),
    "\u6a21\u677f": t("group.template"),
    "\u8fd0\u884c\u65f6": t("group.runtime"),
    "\u57fa\u7840\u8bbe\u65bd": t("group.infra"),
    "\u8d28\u91cf/\u6587\u6863": t("group.quality"),
    "\u6269\u5c55": t("group.extension"),
  };
  const { labelCounts, edgeTypeCounts } = useMemo(() => {
    const lc = new Map<string, number>();
    for (const n of data?.nodes ?? []) lc.set(n.label, (lc.get(n.label) ?? 0) + 1);
    const ec = new Map<string, number>();
    for (const e of data?.edges ?? []) ec.set(e.edge_type, (ec.get(e.edge_type) ?? 0) + 1);
    return {
      labelCounts: [...lc.entries()].sort((a, b) => b[1] - a[1]),
      edgeTypeCounts: [...ec.entries()].sort((a, b) => b[1] - a[1]),
    };
  }, [data]);

  return (
    <div className="flex flex-col h-full">
      {/* 文件路径筛选 */}
      <div className="px-3 pt-3 pb-2 border-b border-border/30 shrink-0">
        <div>
          <p className="text-xs font-medium text-foreground/40 mb-1.5 uppercase tracking-wider">{t("filter.filePath")}</p>
          <Input
            placeholder={t("filter.filePlaceholder")}
            value={fileFilter}
            onChange={(e) => onFileFilterChange(e.target.value)}
          />
        </div>
      </div>

      {/* 节点类型和边类型筛选 */}
      <ScrollArea className="flex-1 min-h-0">
        <div className="px-3 py-2 space-y-3">
          {/* 节点类型（按分组） */}
          <div>
            <div className="flex items-center justify-between mb-1.5">
              <p className="text-xs font-medium text-foreground/40 uppercase tracking-wider">{t("filter.nodeTypes")}</p>
              <div className="flex items-center gap-1.5">
                <button onClick={onEnableAll} className="text-xs text-primary/70 hover:text-primary transition-colors">{t("filter.selectAll")}</button>
                <span className="text-foreground/10">|</span>
                <button onClick={onDisableAll} className="text-xs text-primary/70 hover:text-primary transition-colors">{t("filter.selectNone")}</button>
              </div>
            </div>
            {Object.entries(NODE_LABEL_GROUPS).map(([group, labels]) => {
              const groupLabels = labels.filter((l) => labelCounts.some(([lc]) => lc === l));
              if (groupLabels.length === 0) return null;
              return (
                <div key={group} className="mb-1.5">
                  <p className="text-xs text-foreground/25 mb-1 font-medium">{groupNames[group] ?? group}</p>
                  <div className="flex flex-wrap gap-1">
                    {groupLabels.map((label) => {
                      const count = labelCounts.find(([lc]) => lc === label)?.[1] ?? 0;
                      const on = enabledLabels.has(label);
                      const c = colorForLabel(label);
                      return (
                        <button
                          key={label}
                          onClick={() => onToggleLabel(label)}
                          className={`inline-flex items-center gap-1.5 px-2 py-0.5 rounded text-xs font-medium transition-all border ${
                            on ? "border-white/[0.08] bg-white/[0.04]" : "border-transparent opacity-25"
                          }`}
                        >
                          <span className="w-[5px] h-[5px] rounded-full" style={{ backgroundColor: on ? c : "#444" }} />
                          <span style={{ color: on ? c : "#555" }}>{label}</span>
                          <span className="text-foreground/15 tabular-nums">{count}</span>
                        </button>
                      );
                    })}
                  </div>
                </div>
              );
            })}
          </div>

          {/* 关系类型 */}
          <div>
            <p className="text-xs font-medium text-foreground/40 mb-1.5 uppercase tracking-wider">{t("filter.edgeTypes")}</p>
            <div className="flex flex-wrap gap-1">
              {edgeTypeCounts.map(([type, count]) => {
                const on = enabledEdgeTypes.has(type);
                const c = colorForEdgeType(type);
                return (
                  <button
                    key={type}
                    onClick={() => onToggleEdgeType(type)}
                    className={`inline-flex items-center gap-1.5 px-2 py-0.5 rounded text-xs font-medium transition-all border ${
                      on ? "border-white/[0.06] bg-white/[0.03] text-foreground/60" : "border-transparent opacity-20 text-foreground/30"
                    }`}
                  >
                    <span className="w-[5px] h-[5px] rounded-full" style={{ backgroundColor: on ? c : "#444" }} />
                    {type.replace(/_/g, " ").toLowerCase()}
                    <span className="text-foreground/15 tabular-nums">{count}</span>
                  </button>
                );
              })}
            </div>
          </div>
        </div>
      </ScrollArea>

      {/* 底部显示选项 */}
      <div className="px-3 py-2 border-t border-border/20 shrink-0">
        <button
          onClick={onToggleShowLabels}
          className={`inline-flex items-center gap-1.5 text-sm font-medium transition-all ${
            showLabels ? "text-primary" : "text-foreground/30"
          }`}
        >
          <span className={`w-3.5 h-3.5 rounded border flex items-center justify-center transition-all ${
            showLabels ? "border-primary bg-primary/20" : "border-foreground/15"
          }`}>
            {showLabels && <span className="text-primary text-[9px]">✓</span>}
          </span>
          {t("filter.showLabels")}
        </button>
      </div>
    </div>
  );
}
