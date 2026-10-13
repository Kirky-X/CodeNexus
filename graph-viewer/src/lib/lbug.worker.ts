// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

/* LadybugDB Worker — 在 Web Worker 线程运行同步 WASM 引擎
 *
 * 主线程只做 RPC，文件切片/MEMFS 写入/DB 打开/关系扫描全部离线程，
 * 大库加载期间 UI 不再冻结。File 对象经结构化克隆传入（零拷贝语义由
 * 浏览器保证按需读取）。
 *
 * 协议（requestId 关联请求/响应）：
 *   { type:"load", file, bufferPoolSize, locale, requestId }
 *       → 阶段消息 { type:"stage", stage:"copy"|"open" }
 *       → { type:"loaded", requestId, fileName } | { type:"error", requestId, message }
 *   { type:"queryGraph", maxNodes, scanLimit, projectName, locale, requestId }
 *       → { type:"result", requestId, data } | { type:"error", ... }
 *   { type:"close", locale, requestId } → { type:"closed", requestId }
 *
 * locale：主线程下发的生效语言（zh/en）。WorkerGlobalScope 不暴露
 * localStorage，worker 读不到持久化偏好，只能由消息携带。
 *
 * 消息串行：onmessage 是 async，load 的 WASM 初始化/MEMFS 写入期间
 * 插入的后续消息会先改写 activeLocale 与 db——先到消息失败时 wt() 已
 * 读到新语言（文案错配），queryGraph 也可能查到正在被替换的旧 db。
 * 以 Promise 链保证同一 worker 内消息逐条处理。
 */

import { LbugDatabase, setActiveLocale, wt } from "./lbugWasm";
import { queryGraph } from "./graphQuery";
import { parseLocale } from "./i18nCatalog";
import type { GraphData } from "./types";

let db: LbugDatabase | null = null;
let messageQueue: Promise<void> = Promise.resolve();

const post = (msg: unknown) => self.postMessage(msg);

type WorkerRequest = {
  type: string;
  requestId: number;
  file?: File;
  bufferPoolSize?: number;
  maxNodes?: number;
  scanLimit?: number;
  projectName?: string;
  locale?: string;
};

async function handleMessage(msg: WorkerRequest): Promise<void> {
  setActiveLocale(parseLocale(msg.locale));

  try {
    switch (msg.type) {
      case "load": {
        if (!msg.file) throw new Error(wt("error.missingFile"));
        post({ type: "stage", requestId: msg.requestId, stage: "copy" });
        db?.close();
        db = await LbugDatabase.fromFile(msg.file, msg.bufferPoolSize ?? 0, (stage) =>
          post({ type: "stage", requestId: msg.requestId, stage }),
        );
        post({ type: "loaded", requestId: msg.requestId, fileName: db.fileName });
        break;
      }
      case "queryGraph": {
        if (!db) throw new Error(wt("error.dbNotLoaded"));
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
        throw new Error(wt("error.unknownMessageType", { type: msg.type }));
    }
  } catch (err) {
    post({
      type: "error",
      requestId: msg.requestId,
      message: err instanceof Error ? err.message : String(err),
    });
  }
}

self.onmessage = (event: MessageEvent) => {
  const msg = event.data as WorkerRequest;
  /* 队列尾追加；catch 仅兜底 postMessage 通道本身失效（响应已无法回传），
   * 保证后续消息不因单条处理异常而断链 */
  messageQueue = messageQueue.then(() => handleMessage(msg)).catch((err) => {
    console.error(wt("log.workerMessageHandlingFailed"), err);
  });
};
