import { describe, it, expect } from "vitest";
import { colorForLabel, colorForEdgeType, TRACE_COLORS } from "./colors";

describe("colorForLabel", () => {
  it("返回已知标签的颜色", () => {
    expect(colorForLabel("Function")).toBe("#22d3ee");
    expect(colorForLabel("Class")).toBe("#c084fc");
    expect(colorForLabel("Variable")).toBe("#94a3b8");
    expect(colorForLabel("File")).toBe("#60a5fa");
    expect(colorForLabel("Trait")).toBe("#a78bfa");
    expect(colorForLabel("Module")).toBe("#fb923c");
  });

  it("未知标签返回默认颜色", () => {
    expect(colorForLabel("UnknownLabel")).toBe("#94a3b8");
    expect(colorForLabel("")).toBe("#94a3b8");
  });

  it("所有 44 种 NodeLabel 都有对应颜色", () => {
    const labels = [
      "Project", "Folder", "File", "Module",
      "Class", "Struct", "Enum", "Trait", "Impl", "Interface", "Union", "Variant", "Field", "Record", "Typedef",
      "Function", "Method", "Constructor",
      "Variable", "GlobalVar", "Parameter", "Const", "Static", "Property",
      "Macro", "TypeAlias", "Namespace", "Delegate", "Annotation",
      "Template",
      "Event", "Handler", "Middleware", "Service", "Endpoint", "Route", "Process",
      "Database", "Config",
      "Test", "Section",
      "Community", "Tool", "Embedding",
    ];
    for (const label of labels) {
      const color = colorForLabel(label);
      expect(color).toMatch(/^#[0-9a-f]{6}$/i);
    }
  });
});

describe("colorForEdgeType", () => {
  it("返回已知边类型的颜色", () => {
    expect(colorForEdgeType("CALLS")).toBe("#22d3ee");
    expect(colorForEdgeType("CONTAINS")).toBe("#34d399");
    expect(colorForEdgeType("IMPLEMENTS")).toBe("#c084fc");
    expect(colorForEdgeType("IMPORTS")).toBe("#34d399");
    expect(colorForEdgeType("READS")).toBe("#94a3b8");
    expect(colorForEdgeType("WRITES")).toBe("#f87171");
  });

  it("未知边类型返回默认颜色", () => {
    expect(colorForEdgeType("UNKNOWN_EDGE")).toBe("#64748b");
    expect(colorForEdgeType("")).toBe("#64748b");
  });
});

describe("TRACE_COLORS", () => {
  it("包含所有追踪颜色常量", () => {
    expect(TRACE_COLORS.callChain).toBeDefined();
    expect(TRACE_COLORS.variableUse).toBeDefined();
    expect(TRACE_COLORS.origin).toBeDefined();
    expect(TRACE_COLORS.highlight).toBeDefined();
  });
});
