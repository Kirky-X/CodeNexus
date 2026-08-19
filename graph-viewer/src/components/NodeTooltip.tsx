/* 节点悬浮提示 */

import { Html } from "@react-three/drei";
import type { GraphNode } from "../lib/types";
import { useI18n } from "../lib/i18n";
import { colorForLabel } from "../lib/colors";

interface NodeTooltipProps {
  node: GraphNode;
}

export function NodeTooltip({ node }: NodeTooltipProps) {
  const { t } = useI18n();
  return (
    <Html position={[node.x, node.y + 15, node.z]} center distanceFactor={400} style={{ pointerEvents: "none" }}>
      <div className="bg-background/95 border border-border/50 rounded-lg px-3 py-2 backdrop-blur-md shadow-xl min-w-[160px] pointer-events-none">
        <div className="flex items-center gap-1.5 mb-1">
          <span className="w-2 h-2 rounded-full" style={{ backgroundColor: colorForLabel(node.label) }} />
          <span className="text-[11px] font-semibold text-foreground truncate">{node.name}</span>
        </div>
        <span
          className="inline-block px-1.5 py-0.5 rounded text-[9px] font-medium mb-1"
          style={{ backgroundColor: colorForLabel(node.label) + "20", color: colorForLabel(node.label) }}
        >
          {node.label}
        </span>
        {node.file_path && (
          <p className="text-[9px] text-foreground/40 font-mono truncate mt-1">{node.file_path}</p>
        )}
        {node.project && (
          <p className="text-[9px] text-foreground/30 mt-0.5">{t("tooltip.project")} {node.project}</p>
        )}
      </div>
    </Html>
  );
}
