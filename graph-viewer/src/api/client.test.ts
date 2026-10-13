// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

/* api/client 的 RPC 管道测试 — FakeWorker 捕获 postMessage 并驱动响应，
 * 覆盖：正常解析、locale 下发、不活动看门狗超时与实例重建、stage 续约、
 * onerror 全量失败。 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

const { FakeWorker } = vi.hoisted(() => {
  class FakeWorker {
    static instances: FakeWorker[] = [];
    onmessage: ((e: { data: unknown }) => void) | null = null;
    onerror: ((e: unknown) => void) | null = null;
    onmessageerror: (() => void) | null = null;
    posted: Record<string, unknown>[] = [];
    terminated = false;
    constructor() {
      FakeWorker.instances.push(this);
    }
    postMessage(msg: Record<string, unknown>) {
      this.posted.push(msg);
    }
    terminate() {
      this.terminated = true;
    }
    emit(data: unknown) {
      this.onmessage?.({ data });
    }
    fail(e: unknown) {
      this.onerror?.(e);
    }
  }
  return { FakeWorker };
});

vi.mock("../lib/lbug.worker?worker", () => ({ default: FakeWorker }));

async function freshClient() {
  vi.resetModules();
  return import("./client");
}

const stubFile = () => new File([new Uint8Array([1, 2, 3])], "t.lbug");

beforeEach(() => {
  FakeWorker.instances = [];
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("loadLbugFile", () => {
  it("成功加载并记录文件名，消息携带生效语言", async () => {
    const client = await freshClient();
    const p = client.loadLbugFile(stubFile());
    const w = FakeWorker.instances[0]!;
    expect(w.posted[0]).toMatchObject({ type: "load", locale: "en" });
    w.emit({ type: "loaded", requestId: w.posted[0].requestId, fileName: "t.lbug" });
    await expect(p).resolves.toBeUndefined();
    expect(client.getCurrentFileName()).toBe("t.lbug");
  });

  it("扩展名校验在 App 层，client 层不拦截（消息照常下发）", async () => {
    const client = await freshClient();
    const p = client.loadLbugFile(new File([new Uint8Array([0])], "a.txt"));
    const w = FakeWorker.instances[0]!;
    expect(w.posted[0]).toMatchObject({ type: "load" });
    w.emit({ type: "loaded", requestId: 1, fileName: "a.txt" });
    await expect(p).resolves.toBeUndefined();
  });
});

describe("fetchGraphData", () => {
  it("解析 result 响应返回图数据", async () => {
    const client = await freshClient();
    const p = client.fetchGraphData("", 10);
    const w = FakeWorker.instances[0]!;
    expect(w.posted[0]).toMatchObject({ type: "queryGraph", maxNodes: 10 });
    const data = { nodes: [], edges: [], total_nodes: 0, total_edges: 0 };
    w.emit({ type: "result", requestId: w.posted[0].requestId, data });
    await expect(p).resolves.toEqual(data);
  });
});

describe("RPC 不活动看门狗", () => {
  it("60s 无任何消息则超时失败，worker 被终止且下次调用重建实例", async () => {
    const client = await freshClient();
    const p = client.fetchGraphData("", 10);
    const w = FakeWorker.instances[0]!;
    vi.advanceTimersByTime(60_000);
    await expect(p).rejects.toThrow(/timed out/i);
    expect(w.terminated).toBe(true);

    const p2 = client.fetchGraphData("", 10);
    expect(FakeWorker.instances.length).toBe(2);
    const w2 = FakeWorker.instances[1]!;
    const data = { nodes: [], edges: [], total_nodes: 0, total_edges: 0 };
    w2.emit({ type: "result", requestId: w2.posted[0].requestId, data });
    await expect(p2).resolves.toEqual(data);
  });

  it("加载期间 stage 消息续约，累计超过 60s 不误超时", async () => {
    const client = await freshClient();
    const p = client.loadLbugFile(stubFile());
    const w = FakeWorker.instances[0]!;
    vi.advanceTimersByTime(59_000);
    w.emit({ type: "stage", requestId: 1, stage: "copy" });
    vi.advanceTimersByTime(59_000);
    w.emit({ type: "stage", requestId: 1, stage: "open" });
    vi.advanceTimersByTime(59_000);
    w.emit({ type: "loaded", requestId: 1, fileName: "t.lbug" });
    await expect(p).resolves.toBeUndefined();
  });

  it("静默超时时全部在途 RPC 一并失败", async () => {
    const client = await freshClient();
    const p1 = client.fetchGraphData("", 10);
    const p2 = client.closeDatabase();
    vi.advanceTimersByTime(60_000);
    await expect(p1).rejects.toThrow(/timed out/i);
    await expect(p2).rejects.toThrow(/timed out/i);
  });
});

describe("worker onerror", () => {
  it("在途 RPC 全部失败且实例被丢弃重建", async () => {
    const client = await freshClient();
    const p = client.fetchGraphData("", 10);
    const w = FakeWorker.instances[0]!;
    w.fail({ message: "boom", filename: "w", lineno: 1, colno: 1 });
    await expect(p).rejects.toThrow(/boom/);
    expect(w.terminated).toBe(true);
    client.fetchGraphData("", 10);
    expect(FakeWorker.instances.length).toBe(2);
  });
});
