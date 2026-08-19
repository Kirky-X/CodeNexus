/* 演示用模拟数据 — 无后端时展示 UI 效果 */

import type { GraphData, GraphNode, GraphEdge } from "./types";

/** 球面螺旋分布 — 生成均匀 3D 坐标 */
function sphereCoords(index: number, total: number, radius = 40): { x: number; y: number; z: number } {
  const phi = Math.acos(1 - (2 * (index + 0.5)) / total);
  const theta = Math.PI * (1 + Math.sqrt(5)) * index;
  return {
    x: radius * Math.sin(phi) * Math.cos(theta),
    y: radius * Math.sin(phi) * Math.sin(theta),
    z: radius * Math.cos(phi),
  };
}

function n(
  id: string,
  label: GraphNode["label"],
  name: string,
  file_path: string,
  idx: number,
  total: number,
  start_line?: number,
): GraphNode {
  const c = sphereCoords(idx, total);
  return { id, label, name, file_path, project: "demo", start_line, ...c };
}

function e(
  id: string,
  source: string,
  target: string,
  edge_type: GraphEdge["edge_type"],
): GraphEdge {
  return { id, source, target, edge_type, confidence: 1.0, project: "demo" };
}

/** 生成一组模拟图数据，用于演示模式 */
export function generateDemoData(): GraphData {
  const TOTAL = 25;
  const nodes: GraphNode[] = [
    n("fn:main", "Function", "main", "src/main.rs", 0, TOTAL, 10),
    n("fn:parse_file", "Function", "parse_file", "src/parse/mod.rs", 1, TOTAL, 25),
    n("fn:build_graph", "Function", "build_graph", "src/index/graph.rs", 2, TOTAL, 42),
    n("fn:query_nodes", "Function", "query_nodes", "src/query/engine.rs", 3, TOTAL, 88),
    n("fn:resolve_imports", "Function", "resolve_imports", "src/resolve/mod.rs", 4, TOTAL, 15),
    n("fn:analyze_complexity", "Function", "analyze_complexity", "src/analysis/complexity.rs", 5, TOTAL, 33),
    n("fn:store_node", "Function", "store_node", "src/storage/db.rs", 6, TOTAL, 100),
    n("fn:emit_diagnostic", "Function", "emit_diagnostic", "src/service/lsp.rs", 7, TOTAL, 55),
    n("cls:GraphIndex", "Class", "GraphIndex", "src/index/graph.rs", 8, TOTAL, 1),
    n("cls:QueryEngine", "Class", "QueryEngine", "src/query/engine.rs", 9, TOTAL, 1),
    n("cls:StorageDb", "Class", "StorageDb", "src/storage/db.rs", 10, TOTAL, 1),
    n("cls:Resolver", "Class", "Resolver", "src/resolve/mod.rs", 11, TOTAL, 1),
    n("cls:Parser", "Class", "Parser", "src/parse/mod.rs", 12, TOTAL, 1),
    n("var:node_count", "Variable", "node_count", "src/index/graph.rs", 13, TOTAL, 43),
    n("var:edge_count", "Variable", "edge_count", "src/index/graph.rs", 14, TOTAL, 44),
    n("var:query_result", "Variable", "query_result", "src/query/engine.rs", 15, TOTAL, 89),
    n("var:config", "Variable", "config", "src/main.rs", 16, TOTAL, 11),
    n("var:ast", "Variable", "ast", "src/parse/mod.rs", 17, TOTAL, 26),
    n("trait:Queryable", "Trait", "Queryable", "src/query/mod.rs", 18, TOTAL, 1),
    n("trait:Storable", "Trait", "Storable", "src/storage/mod.rs", 19, TOTAL, 1),
    n("trait:Resolvable", "Trait", "Resolvable", "src/resolve/mod.rs", 20, TOTAL, 10),
    n("mod:analysis", "Module", "analysis", "src/analysis/mod.rs", 21, TOTAL, 1),
    n("mod:index", "Module", "index", "src/index/mod.rs", 22, TOTAL, 1),
    n("file:main.rs", "File", "main.rs", "src/main.rs", 23, TOTAL, 1),
    n("file:graph.rs", "File", "graph.rs", "src/index/graph.rs", 24, TOTAL, 1),
  ];

  const edges: GraphEdge[] = [
    // CALLS
    e("e1", "fn:main", "fn:parse_file", "CALLS"),
    e("e2", "fn:main", "fn:build_graph", "CALLS"),
    e("e3", "fn:build_graph", "fn:store_node", "CALLS"),
    e("e4", "fn:query_nodes", "fn:resolve_imports", "CALLS"),
    e("e5", "fn:main", "fn:query_nodes", "CALLS"),
    e("e6", "fn:parse_file", "fn:analyze_complexity", "CALLS"),
    e("e7", "fn:emit_diagnostic", "fn:query_nodes", "CALLS"),
    // USAGE (uses)
    e("e8", "fn:build_graph", "var:node_count", "USAGE"),
    e("e9", "fn:build_graph", "var:edge_count", "USAGE"),
    e("e10", "fn:query_nodes", "var:query_result", "USAGE"),
    e("e11", "fn:main", "var:config", "USAGE"),
    e("e12", "fn:parse_file", "var:ast", "USAGE"),
    // IMPLEMENTS
    e("e13", "cls:QueryEngine", "trait:Queryable", "IMPLEMENTS"),
    e("e14", "cls:StorageDb", "trait:Storable", "IMPLEMENTS"),
    e("e15", "cls:Resolver", "trait:Resolvable", "IMPLEMENTS"),
    // CONTAINS
    e("e16", "cls:GraphIndex", "fn:build_graph", "CONTAINS"),
    e("e17", "cls:QueryEngine", "fn:query_nodes", "CONTAINS"),
    e("e18", "cls:StorageDb", "fn:store_node", "CONTAINS"),
    e("e19", "cls:Parser", "fn:parse_file", "CONTAINS"),
    e("e20", "cls:Resolver", "fn:resolve_imports", "CONTAINS"),
    // IMPORTS
    e("e21", "file:main.rs", "mod:index", "IMPORTS"),
    e("e22", "file:main.rs", "mod:analysis", "IMPORTS"),
    e("e23", "file:graph.rs", "cls:GraphIndex", "IMPORTS"),
    // READS / WRITES
    e("e24", "fn:query_nodes", "var:query_result", "READS"),
    e("e25", "fn:build_graph", "var:node_count", "WRITES"),
  ];

  return { nodes, edges, total_nodes: nodes.length, total_edges: edges.length };
}
