import { describe, it, expect } from "vitest";
import { computeForceLayout } from "./layout";
import type { GraphNode, GraphEdge } from "./types";

function makeNode(id: string, x = 0, y = 0, z = 0): GraphNode {
  return { id, label: "Function", name: id, project: "test", x, y, z };
}

function makeEdge(id: string, source: string, target: string): GraphEdge {
  return { id, source, target, edge_type: "CALLS", confidence: 1.0, project: "test" };
}

describe("computeForceLayout", () => {
  it("已有坐标的节点直接返回（不重新计算）", () => {
    const nodes = [
      makeNode("a", 10, 20, 30),
      makeNode("b", 40, 50, 60),
    ];
    const edges: GraphEdge[] = [];
    const result = computeForceLayout(nodes, edges);
    expect(result).toBe(nodes); // 同一引用
  });

  it("零坐标节点均匀分布在球面上（爆炸的球）", () => {
    const nodes = [makeNode("a"), makeNode("b"), makeNode("c")];
    const edges = [makeEdge("e1", "a", "b")];
    const result = computeForceLayout(nodes, edges);

    expect(result).toHaveLength(3);
    /* 所有节点都应离开原点 */
    const hasMovement = result.some((n) => n.x !== 0 || n.y !== 0 || n.z !== 0);
    expect(hasMovement).toBe(true);
    /* 全部落在同一半径的球面上 */
    const radii = result.map((n) => Math.sqrt(n.x ** 2 + n.y ** 2 + n.z ** 2));
    const maxDiff = Math.max(...radii) - Math.min(...radii);
    expect(maxDiff).toBeLessThan(1e-6);
  });

  it("结果保留原始节点属性", () => {
    const nodes = [
      makeNode("a"),
      makeNode("b"),
    ];
    nodes[0].name = "func_a";
    nodes[0].file_path = "src/a.rs";
    const result = computeForceLayout(nodes, [], { iterations: 5 });

    expect(result[0].name).toBe("func_a");
    expect(result[0].file_path).toBe("src/a.rs");
    expect(result[0].label).toBe("Function");
  });

  it("空节点列表返回空", () => {
    const result = computeForceLayout([], []);
    expect(result).toHaveLength(0);
  });

  it("边引用不存在的节点不报错", () => {
    const nodes = [makeNode("a"), makeNode("b")];
    const edges = [makeEdge("e1", "a", "nonexistent")];
    expect(() => computeForceLayout(nodes, edges)).not.toThrow();
  });
});
