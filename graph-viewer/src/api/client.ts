/* WASM 图数据客户端 — 通过 Web Worker 驱动同步引擎
 *
 * WASM 初始化、文件写入 MEMFS、数据库打开与查询全部在 worker 线程
 * 执行，大库加载期间主线程保持响应。本文件是主线程侧的 RPC 管道，
 * 对上层暴露与旧同步客户端相同的接口。 */

import type { GraphData } from "../lib/types";
import type { LoadBudget } from "../lib/memoryBudget";
import LbugWorker from "../lib/lbug.worker?worker";

/* 模块级状态 */
let worker: Worker | null = null;
let requestSeq = 0;
const pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
let currentFileName = "";
let currentBudget: LoadBudget | null = null;
let stageListener: ((stage: "copy" | "open") => void) | null = null;

type WorkerResponse = {
  type: "loaded" | "result" | "closed" | "stage" | "error";
  requestId?: number;
  fileName?: string;
  data?: GraphData;
  stage?: "copy" | "open";
  message?: string;
};

function ensureWorker(): Worker {
  if (worker) return worker;
  /* ?worker&inline：worker 连同 WASM 依赖在构建期内联为 Blob，
   * 规避 module worker 的依赖 URL/加载失败问题 */
  worker = new LbugWorker();
  worker.onmessage = (event: MessageEvent<WorkerResponse>) => {
    const msg = event.data;
    if (msg.type === "stage") {
      stageListener?.(msg.stage ?? "copy");
      return;
    }
    const entry = msg.requestId != null ? pending.get(msg.requestId) : undefined;
    if (!entry) return;
    pending.delete(msg.requestId!);
    if (msg.type === "error") entry.reject(new Error(msg.message ?? "Worker 错误"));
    else entry.resolve(msg);
  };
  worker.onerror = (event) => {
    console.error("[worker onerror]", event.message, event.filename, event.lineno, event.colno);
    const err = new Error(`Worker 错误: ${event.message || "unknown"}`);
    for (const [, entry] of pending) entry.reject(err);
    pending.clear();
  };
  worker.onmessageerror = () => console.error("[worker onmessageerror] 反序列化失败");
  return worker;
}

function call<T>(message: Record<string, unknown>): Promise<T> {
  const w = ensureWorker();
  const requestId = ++requestSeq;
  return new Promise<T>((resolve, reject) => {
    pending.set(requestId, {
      resolve: (v) => resolve(v as T),
      reject,
    });
    w.postMessage({ ...message, requestId });
  });
}

/**
 * 加载 .lbug 文件到 worker 内的数据库
 * @param budget 内存预算（bufferPool 封顶 DB 缓冲）
 * @param onStage 加载阶段回调（copy=写本地引擎，open=打开数据库）
 */
export async function loadLbugFile(
  file: File,
  budget?: LoadBudget,
  onStage?: (stage: "copy" | "open") => void,
): Promise<void> {
  if (onStage) stageListener = onStage;
  await call({
    type: "load",
    file,
    bufferPoolSize: budget?.bufferPoolBytes ?? 0,
  });
  currentFileName = file.name;
  currentBudget = budget ?? null;
  stageListener = null;
}

/** 当前打开的文件名 */
export function getCurrentFileName(): string {
  return currentFileName;
}

/** 关闭当前数据库（worker 内同步释放 MEMFS 与引擎资源） */
export async function closeDatabase(): Promise<void> {
  if (!worker) return;
  await call({ type: "close" });
  currentFileName = "";
}

/**
 * 获取图数据（节点 + 边）
 * @param scanLimit 关系扫描行数上限 — 内存预算模块传入
 */
export async function fetchGraphData(
  _project: string,
  maxNodes = 100,
  _fileFilter?: string,
  _lbugPath?: string,
  scanLimit?: number,
): Promise<GraphData> {
  const res = await call<{ data: GraphData }>({
    type: "queryGraph",
    maxNodes,
    scanLimit: scanLimit ?? currentBudget?.scanLimit,
    projectName: _project || currentFileName,
  });
  return res.data;
}
