/* LadybugDB Worker — 在 Web Worker 线程运行同步 WASM 引擎
 *
 * 主线程只做 RPC，文件切片/MEMFS 写入/DB 打开/关系扫描全部离线程，
 * 大库加载期间 UI 不再冻结。File 对象经结构化克隆传入（零拷贝语义由
 * 浏览器保证按需读取）。
 *
 * 协议（requestId 关联请求/响应）：
 *   { type:"load", file, bufferPoolSize, requestId }
 *       → 阶段消息 { type:"stage", stage:"copy"|"open" }
 *       → { type:"loaded", requestId, fileName } | { type:"error", requestId, message }
 *   { type:"queryGraph", maxNodes, scanLimit, projectName, requestId }
 *       → { type:"result", requestId, data } | { type:"error", ... }
 *   { type:"close", requestId } → { type:"closed", requestId }
 */

import { LbugDatabase } from "./lbugWasm";
import { queryGraph } from "./graphQuery";
import type { GraphData } from "./types";

let db: LbugDatabase | null = null;

const post = (msg: unknown) => self.postMessage(msg);

self.onmessage = async (event: MessageEvent) => {
  const msg = event.data as {
    type: string;
    requestId: number;
    file?: File;
    bufferPoolSize?: number;
    maxNodes?: number;
    scanLimit?: number;
    projectName?: string;
  };

  try {
    switch (msg.type) {
      case "load": {
        if (!msg.file) throw new Error("缺少文件");
        post({ type: "stage", requestId: msg.requestId, stage: "copy" });
        db?.close();
        db = await LbugDatabase.fromFile(msg.file, msg.bufferPoolSize ?? 0, (stage) =>
          post({ type: "stage", requestId: msg.requestId, stage }),
        );
        post({ type: "loaded", requestId: msg.requestId, fileName: db.fileName });
        break;
      }
      case "queryGraph": {
        if (!db) throw new Error("未加载数据库");
        const data: GraphData = queryGraph(db, msg.projectName ?? db.fileName, msg.maxNodes ?? 100, msg.scanLimit);
        post({ type: "result", requestId: msg.requestId, data });
        break;
      }
      case "close": {
        db?.close();
        db = null;
        post({ type: "closed", requestId: msg.requestId });
        break;
      }
      default:
        throw new Error(`未知消息类型: ${msg.type}`);
    }
  } catch (err) {
    post({
      type: "error",
      requestId: msg.requestId,
      message: err instanceof Error ? err.message : String(err),
    });
  }
};
