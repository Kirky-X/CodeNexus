// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

/* WASM 图数据客户端 — 通过 Web Worker 驱动同步引擎
 *
 * WASM 初始化、文件写入 MEMFS、数据库打开与查询全部在 worker 线程
 * 执行，大库加载期间主线程保持响应。本文件是主线程侧的 RPC 管道，
 * 对上层暴露与旧同步客户端相同的接口。 */

import type { GraphData } from "../lib/types";
import type { LoadBudget } from "../lib/memoryBudget";
import { resolveLocale, translate } from "../lib/i18nCatalog";
import LbugWorker from "../lib/lbug.worker?worker";

/* 模块级状态 */
let worker: Worker | null = null;
let requestSeq = 0;
const pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
let currentFileName = "";
let currentBudget: LoadBudget | null = null;
let stageListener: ((stage: "copy" | "open") => void) | null = null;

/* RPC 不活动看门狗 — worker 被静默销毁时 onerror 不保证触发，pending 条目
 * 会永久悬挂（加载转圈不止）。以"收到任意消息即存活信号"兜底：超时则全部
 * 在途 RPC 失败并丢弃实例，下次调用重建新 worker。
 * 60s 远大于单条 stage/result 间隔；长文件加载期间 stage 消息持续续约。 */
const RPC_INACTIVITY_TIMEOUT_MS = 60_000;
let inactivityTimer: ReturnType<typeof setTimeout> | null = null;

function armInactivityTimer(): void {
  if (inactivityTimer) clearTimeout(inactivityTimer);
  inactivityTimer = setTimeout(onWorkerSilence, RPC_INACTIVITY_TIMEOUT_MS);
}

function disarmInactivityTimer(): void {
  if (inactivityTimer) {
    clearTimeout(inactivityTimer);
    inactivityTimer = null;
  }
}

function failAllPending(err: Error): void {
  for (const [, entry] of pending) entry.reject(err);
  pending.clear();
  disarmInactivityTimer();
}

function recycleWorker(): void {
  disarmInactivityTimer();
  worker?.terminate();
  worker = null;
}

function onWorkerSilence(): void {
  inactivityTimer = null;
  failAllPending(new Error(translate(resolveLocale(), "error.workerTimeout")));
  recycleWorker();
}

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
    /* 任意消息（stage/result/error）都是存活信号，重新起跑看门狗 */
    armInactivityTimer();
    const msg = event.data;
    if (msg.type === "stage") {
      stageListener?.(msg.stage ?? "copy");
      return;
    }
    const entry = msg.requestId != null ? pending.get(msg.requestId) : undefined;
    if (!entry) return;
    pending.delete(msg.requestId!);
    if (pending.size === 0) disarmInactivityTimer();
    if (msg.type === "error") entry.reject(new Error(msg.message ?? translate(resolveLocale(), "error.worker")));
    else entry.resolve(msg);
  };
  worker.onerror = (event) => {
    console.error("[worker onerror]", event.message, event.filename, event.lineno, event.colno);
    failAllPending(new Error(`${translate(resolveLocale(), "error.worker")}: ${event.message || "unknown"}`));
    recycleWorker();
  };
  worker.onmessageerror = () => console.error(translate(resolveLocale(), "log.workerDeserializeFailed"));
  armInactivityTimer();
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
    /* locale 随消息下发 — worker 线程无 localStorage，读不到主线程持久化偏好 */
    w.postMessage({ ...message, locale: resolveLocale(), requestId });
    armInactivityTimer();
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
