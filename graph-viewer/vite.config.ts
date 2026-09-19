// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

/// <reference types="vitest" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "path";

export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    include: ["src/**/*.test.{ts,tsx}"],
  },
  plugins: [react(), tailwindcss()],
  /* worker 用 ES 格式：lbug.worker 内含动态导入（WASM 引擎依赖），
   * 默认 iife 不支持代码分割 */
  worker: {
    format: "es",
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  build: {
    outDir: "dist",
    sourcemap: false,
  },
  server: {
    port: 5174,
    strictPort: true,
  },
});
