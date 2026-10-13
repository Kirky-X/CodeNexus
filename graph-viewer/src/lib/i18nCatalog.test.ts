// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

import { describe, it, expect } from "vitest";
import { translate, parseLocale, resolveLocale } from "./i18nCatalog";

describe("translate", () => {
  it("vars 替换消息模板占位符", () => {
    expect(translate("en", "error.wasmInit", { detail: "boom" })).toBe(
      "WASM engine initialization failed: boom",
    );
    expect(translate("zh", "error.cypherQuery", { detail: "语法错误" })).toBe(
      "Cypher 查询失败: 语法错误",
    );
  });

  it("未提供的占位符原样保留", () => {
    expect(translate("en", "error.wasmInit", {})).toBe("WASM engine initialization failed: {detail}");
  });

  it("键缺失时回落键名本身", () => {
    expect(translate("zh", "no.such.key")).toBe("no.such.key");
    expect(translate("en", "no.such.key", { x: 1 })).toBe("no.such.key");
  });
});

describe("parseLocale", () => {
  it("接受 zh/en 原值", () => {
    expect(parseLocale("zh")).toBe("zh");
    expect(parseLocale("en")).toBe("en");
  });

  it("其余值回落 resolveLocale（含 undefined 与非字符串）", () => {
    const expected = resolveLocale();
    expect(parseLocale("fr")).toBe(expected);
    expect(parseLocale(undefined)).toBe(expected);
    expect(parseLocale(42)).toBe(expected);
    expect(parseLocale("ZH")).toBe(expected);
  });
});
