// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

import { describe, expect, it } from "vitest";
import { computeLoadBudget, detectMemoryProfile, type MemoryProfile } from "./memoryBudget";

const MB = 1024 * 1024;
const GB = 1024 * MB;

const profiles: Record<string, MemoryProfile> = {
  highEnd: { deviceMemoryGB: 8, heapLimitBytes: 4 * GB },
  lowEnd: { deviceMemoryGB: 2, heapLimitBytes: 1 * GB },
  unknown: { deviceMemoryGB: null, heapLimitBytes: null },
};

describe("detectMemoryProfile", () => {
  it("缺失 API 时返回 null 字段（由预算函数兜底）", () => {
    const p = detectMemoryProfile();
    expect(p.deviceMemoryGB === null || typeof p.deviceMemoryGB === "number").toBe(true);
    expect(p.heapLimitBytes === null || typeof p.heapLimitBytes === "number").toBe(true);
  });
});

describe("computeLoadBudget", () => {
  it("小文件 + 高配机器：全量预算，无省内存档", () => {
    const b = computeLoadBudget(32 * MB, profiles.highEnd);
    expect(b.warnLowMemory).toBe(false);
    expect(b.maxNodesCap).toBe(500);
    expect(b.scanLimit).toBe(200_000);
  });

  it("大文件：节点上限按文件大小分档递减", () => {
    expect(computeLoadBudget(100 * MB, profiles.highEnd).maxNodesCap).toBe(200);
    expect(computeLoadBudget(300 * MB, profiles.highEnd).maxNodesCap).toBe(100);
    expect(computeLoadBudget(512 * MB, profiles.highEnd).maxNodesCap).toBe(50);
  });

  it("低内存设备 + 大文件：进入省内存档，节点上限减半（下限 50），扫描降档", () => {
    const b = computeLoadBudget(300 * MB, profiles.lowEnd);
    expect(b.warnLowMemory).toBe(true);
    expect(b.maxNodesCap).toBe(50);
    expect(b.scanLimit).toBe(50_000);
    /* 峰值 660MB > 可用 320MB×0.95 → 硬上限预警 */
    expect(b.fileTooLarge).toBe(true);
  });

  it("高配设备 + 中等文件：不触发硬上限预警", () => {
    const b = computeLoadBudget(100 * MB, profiles.highEnd);
    expect(b.fileTooLarge).toBe(false);
  });

  it("省内存档下 bufferPool 减半且不超上限", () => {
    const b = computeLoadBudget(512 * MB, profiles.lowEnd);
    expect(b.bufferPoolBytes).toBeLessThanOrEqual(96 * MB);
    expect(b.bufferPoolBytes).toBeGreaterThanOrEqual(16 * MB);
  });

  it("小文件 bufferPool 不低于下限 32MB", () => {
    const b = computeLoadBudget(5 * MB, profiles.highEnd);
    expect(b.bufferPoolBytes).toBe(32 * MB);
  });

  it("信息缺失（unknown 档）不抛错且给出合理预算", () => {
    const b = computeLoadBudget(99606528, profiles.unknown);
    expect(b.maxNodesCap).toBeGreaterThanOrEqual(50);
    expect(b.scanLimit).toBeGreaterThan(0);
    expect(b.bufferPoolBytes).toBeGreaterThan(0);
  });
});
