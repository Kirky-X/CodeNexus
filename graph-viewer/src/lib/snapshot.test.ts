// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

import { describe, it, expect } from "vitest";
import {
  readSnapshotFromUrl,
  snapshotToGraphData,
  SnapshotError,
  MAX_SNAPSHOT_BYTES,
  type ViewerSnapshot,
} from "./snapshot";

const validSnapshot: ViewerSnapshot = {
  version: 1,
  nodes: [
    { id: "src-api", label: "/src/api", group: "Controller", change: "added" },
    { id: "src-db", label: "/src/db", group: "storage" },
  ],
  edges: [{ source: "src-api", target: "src-db", change: "none" }],
};

describe("readSnapshotFromUrl", () => {
  it("无 snapshot 参数返回 null（走原加载路径）", () => {
    expect(readSnapshotFromUrl("?foo=bar")).toBeNull();
    expect(readSnapshotFromUrl("")).toBeNull();
  });

  it("解析 data URI 内嵌的 JSON 快照", () => {
    const payload = encodeURIComponent(
      "data:application/json;charset=utf-8," + JSON.stringify(validSnapshot),
    );
    const snap = readSnapshotFromUrl(`?snapshot=${payload}`);
    expect(snap).not.toBeNull();
    expect(snap!.nodes).toHaveLength(2);
    expect(snap!.nodes[0].id).toBe("src-api");
    expect(snap!.nodes[0].change).toBe("added");
  });

  it("解析直接传入的 JSON（URLSearchParams 已解码）", () => {
    const snap = readSnapshotFromUrl(`?snapshot=${encodeURIComponent(JSON.stringify(validSnapshot))}`);
    expect(snap!.edges[0].source).toBe("src-api");
  });

  it("坏 JSON 抛出 SnapshotError", () => {
    const bad = encodeURIComponent("data:application/json,{not json");
    expect(() => readSnapshotFromUrl(`?snapshot=${bad}`)).toThrow(SnapshotError);
  });

  it("超过大小上限抛出 SnapshotError", () => {
    const big = JSON.stringify({
      version: 1,
      nodes: [{ id: "x".repeat(MAX_SNAPSHOT_BYTES), label: "n", group: "g" }],
      edges: [],
    });
    const encoded = encodeURIComponent("data:application/json," + big);
    expect(() => readSnapshotFromUrl(`?snapshot=${encoded}`)).toThrow(SnapshotError);
  });

  it("未知版本与缺失字段拒绝", () => {
    const v2 = encodeURIComponent(JSON.stringify({ version: 2, nodes: [], edges: [] }));
    expect(() => readSnapshotFromUrl(`?snapshot=${v2}`)).toThrow(/版本/);
    const noNodes = encodeURIComponent(JSON.stringify({ version: 1, edges: [] }));
    expect(() => readSnapshotFromUrl(`?snapshot=${noNodes}`)).toThrow(/nodes/);
  });

  it("边引用不存在的节点拒绝", () => {
    const dangling = {
      version: 1,
      nodes: [{ id: "a", label: "A", group: "g" }],
      edges: [{ source: "a", target: "ghost" }],
    };
    const encoded = encodeURIComponent(JSON.stringify(dangling));
    expect(() => readSnapshotFromUrl(`?snapshot=${encoded}`)).toThrow(/不存在/);
  });
});

describe("snapshotToGraphData", () => {
  it("映射为内部图模型并保留 change 标注", () => {
    const data = snapshotToGraphData(validSnapshot);
    expect(data.total_nodes).toBe(2);
    expect(data.total_edges).toBe(1);
    expect(data.nodes[0].name).toBe("/src/api");
    expect(data.nodes[0].change).toBe("added");
    expect(data.nodes[1].change).toBe("none");
    expect(data.edges[0].edge_type).toBe("CALLS");
    expect(data.nodes.every((n) => Number.isFinite(n.x) && Number.isFinite(n.y))).toBe(true);
  });
});
