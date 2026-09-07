import { describe, it, expect } from "vitest";
import { CALL_EDGE_TYPES, VARIABLE_EDGE_TYPES } from "./types";

describe("CALL_EDGE_TYPES", () => {
  it("包含所有调用类边类型", () => {
    expect(CALL_EDGE_TYPES).toContain("CALLS");
    expect(CALL_EDGE_TYPES).toContain("FFI_CALLS");
    expect(CALL_EDGE_TYPES).toContain("HTTP_CALLS");
    expect(CALL_EDGE_TYPES).toContain("ASYNC_CALLS");
  });

  it("不包含非调用类边类型", () => {
    expect(CALL_EDGE_TYPES).not.toContain("READS");
    expect(CALL_EDGE_TYPES).not.toContain("WRITES");
    expect(CALL_EDGE_TYPES).not.toContain("IMPLEMENTS");
    expect(CALL_EDGE_TYPES).not.toContain("CONTAINS");
  });

  it("有 4 种调用边类型", () => {
    expect(CALL_EDGE_TYPES).toHaveLength(4);
  });
});

describe("VARIABLE_EDGE_TYPES", () => {
  it("包含所有变量使用类边类型", () => {
    expect(VARIABLE_EDGE_TYPES).toContain("READS");
    expect(VARIABLE_EDGE_TYPES).toContain("WRITES");
    expect(VARIABLE_EDGE_TYPES).toContain("ACCESSES");
    expect(VARIABLE_EDGE_TYPES).toContain("DATAFLOWS");
  });

  it("不包含非变量类边类型", () => {
    expect(VARIABLE_EDGE_TYPES).not.toContain("CALLS");
    expect(VARIABLE_EDGE_TYPES).not.toContain("IMPLEMENTS");
    expect(VARIABLE_EDGE_TYPES).not.toContain("CONTAINS");
  });

  it("有 4 种变量边类型", () => {
    expect(VARIABLE_EDGE_TYPES).toHaveLength(4);
  });
});
