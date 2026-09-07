# graph-viewer/server — 已弃用

此目录包含原有的 Rust 后端服务，用于提供图数据 REST API。

**状态**：已被 WASM 方案替代。前端现在通过 `@ladybugdb/wasm-core` 直接在浏览器内加载和查询 `.lbug` 文件，不再需要此后端服务。

**替代方案**：参见 `src/lib/lbugWasm.ts` 和 `src/lib/graphQuery.ts`。

**保留原因**：作为 Cypher 查询逻辑的参考实现。
