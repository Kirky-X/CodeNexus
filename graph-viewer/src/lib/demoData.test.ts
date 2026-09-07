import { describe, it, expect } from "vitest";
import { generateDemoData } from "./demoData";

describe("generateDemoData", () => {
  const data = generateDemoData();

  it("返回正确数量的节点", () => {
    expect(data.nodes).toHaveLength(25);
    expect(data.total_nodes).toBe(25);
  });

  it("返回正确数量的边", () => {
    expect(data.edges).toHaveLength(25);
    expect(data.total_edges).toBe(25);
  });

  it("所有节点都有必需的字段", () => {
    for (const node of data.nodes) {
      expect(node.id).toBeTruthy();
      expect(node.label).toBeTruthy();
      expect(node.name).toBeTruthy();
      expect(node.project).toBe("demo");
      expect(typeof node.x).toBe("number");
      expect(typeof node.y).toBe("number");
      expect(typeof node.z).toBe("number");
    }
  });

  it("所有边的 source 和 target 都指向存在的节点", () => {
    const nodeIds = new Set(data.nodes.map((n) => n.id));
    for (const edge of data.edges) {
      expect(nodeIds.has(edge.source)).toBe(true);
      expect(nodeIds.has(edge.target)).toBe(true);
      expect(edge.confidence).toBe(1.0);
      expect(edge.project).toBe("demo");
    }
  });

  it("包含多种节点类型", () => {
    const labels = new Set(data.nodes.map((n) => n.label));
    expect(labels.has("Function")).toBe(true);
    expect(labels.has("Class")).toBe(true);
    expect(labels.has("Variable")).toBe(true);
    expect(labels.has("Trait")).toBe(true);
    expect(labels.has("Module")).toBe(true);
    expect(labels.has("File")).toBe(true);
    expect(labels.size).toBeGreaterThanOrEqual(6);
  });

  it("包含多种边类型", () => {
    const types = new Set(data.edges.map((e) => e.edge_type));
    expect(types.has("CALLS")).toBe(true);
    expect(types.has("USAGE")).toBe(true);
    expect(types.has("IMPLEMENTS")).toBe(true);
    expect(types.has("CONTAINS")).toBe(true);
    expect(types.has("IMPORTS")).toBe(true);
    expect(types.has("READS")).toBe(true);
    expect(types.has("WRITES")).toBe(true);
  });

  it("节点坐标不全为零（球面螺旋分布生效）", () => {
    const nonZeroCount = data.nodes.filter(
      (n) => n.x !== 0 || n.y !== 0 || n.z !== 0,
    ).length;
    expect(nonZeroCount).toBe(data.nodes.length);
  });

  it("节点坐标在合理范围内", () => {
    for (const node of data.nodes) {
      expect(Math.abs(node.x)).toBeLessThan(100);
      expect(Math.abs(node.y)).toBeLessThan(100);
      expect(Math.abs(node.z)).toBeLessThan(100);
    }
  });
});
