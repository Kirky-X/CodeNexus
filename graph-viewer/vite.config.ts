/// <reference types="vitest" />
import { defineConfig, type ProxyOptions } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "path";

const backendOrigin = "http://127.0.0.1:9800";

const backendProxy = (): ProxyOptions => ({
  target: backendOrigin,
  changeOrigin: true,
  headers: { Origin: backendOrigin },
});

export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    include: ["src/**/*.test.{ts,tsx}"],
  },
  plugins: [react(), tailwindcss()],
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
    proxy: {
      "/api": backendProxy(),
    },
  },
});
