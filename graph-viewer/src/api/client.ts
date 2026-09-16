/* WASM 图数据客户端 — 在浏览器内直接查询 .lbug 数据库 */

import type { GraphData, SchemaInfo } from "../lib/types";
import { LbugDatabase } from "../lib/lbugWasm";
import { queryGraph, querySchema } from "../lib/graphQuery";
import type { LoadBudget } from "../lib/memoryBudget";

/* 模块级状态 — 当前打开的数据库与其加载预算 */
let currentDb: LbugDatabase | null = null;
let currentBudget: LoadBudget | null = null;

/**
 * 加载 .lbug 文件到浏览器内数据库
 * @param budget 可选内存预算（bufferPool 封顶 DB 缓冲）
 */
export async function loadLbugFile(file: File, budget?: LoadBudget): Promise<void> {
  /* 关闭之前的数据库 */
  if (currentDb) {
    currentDb.close();
    currentDb = null;
  }
  currentDb = await LbugDatabase.fromFile(file, budget?.bufferPoolBytes ?? 0);
  currentBudget = budget ?? null;
}

/**
 * 获取当前数据库实例（内部使用）
 */
function getDb(): LbugDatabase {
  if (!currentDb) {
    throw new Error("未加载数据库，请先选择 .lbug 文件");
  }
  return currentDb;
}

/**
 * 获取当前文件名
 */
export function getCurrentFileName(): string {
  return currentDb?.fileName ?? "";
}

/**
 * 关闭当前数据库
 */
export async function closeDatabase(): Promise<void> {
  if (currentDb) {
    currentDb.close();
    currentDb = null;
  }
}

/**
 * 获取图数据（节点 + 边）
 */
export async function fetchGraphData(
  _project: string,
  maxNodes = 100,
  _fileFilter?: string,
  _lbugPath?: string,
): Promise<GraphData> {
  const db = getDb();
  return queryGraph(db, db.fileName, maxNodes, currentBudget?.scanLimit);
}

/**
 * 获取 schema 统计信息
 */
export async function fetchSchema(
  _project: string,
  _lbugPath?: string,
): Promise<SchemaInfo> {
  const db = getDb();
  return querySchema(db);
}
