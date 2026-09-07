/* LadybugDB WASM 管理层 — 使用 sync 变体，直接在主线程加载和查询 .lbug 文件
 *
 * sync 变体无需 Web Worker，直接访问 Emscripten 虚拟文件系统。
 * 所有数据库操作同步执行，适合本地文件查询场景。
 */

import type {
  Database as LbugDB,
  Connection as LbugConn,
  QueryResult as LbugResult,
} from "@ladybugdb/wasm-core/sync";

/** sync 模块默认导出的 API 形状 */
interface LbugSyncModule {
  init(): Promise<void>;
  getVersion(): string;
  getFS(): EmscriptenFS;
  Database: typeof LbugDB;
  Connection: typeof LbugConn;
}

/** Emscripten 虚拟文件系统（sync 变体仅导出 createDataFile 等预加载 API） */
interface EmscriptenFS {
  createDataFile(
    parent: string,
    name: string | null,
    data: Uint8Array | string,
    canRead: boolean,
    canWrite: boolean,
    canOwn?: boolean,
  ): void;
}

/* WASM 模块 — 延迟加载 */
let lbugModule: LbugSyncModule | null = null;

/* 虚拟文件系统路径 — 直接写入根目录，避免需要 mkdir */
const VFS_ROOT = "/";

/* VFS 文件计数器 — createDataFile 不允许覆盖已存在文件，用唯一路径避免冲突 */
let vfsFileCounter = 0;

/**
 * 获取 WASM 模块实例（单例，延迟加载）
 */
async function getLbugModule(): Promise<LbugSyncModule> {
  if (lbugModule) return lbugModule;

  try {
    const ns = await import("@ladybugdb/wasm-core/sync");
    const mod = (ns as unknown as { default: LbugSyncModule }).default;
    await mod.init();
    lbugModule = mod;
    return mod;
  } catch (err) {
    console.error("[lbugWasm] WASM 模块初始化失败:", err);
    throw new Error(`WASM 引擎初始化失败: ${err instanceof Error ? err.message : String(err)}`);
  }
}

/**
 * 浏览器内 LadybugDB 封装（sync 变体 — 同步查询）
 *
 * 使用方式：
 * ```ts
 * const db = await LbugDatabase.fromFile(file);
 * const rows = db.query("MATCH (n:Function) RETURN n.name LIMIT 10");
 * db.close();
 * ```
 */
export class LbugDatabase {
  private db: LbugDB;
  private conn: LbugConn;
  private _fileName: string;

  private constructor(db: LbugDB, conn: LbugConn, fileName: string) {
    this.db = db;
    this.conn = conn;
    this._fileName = fileName;
  }

  /** 当前打开的文件名 */
  get fileName(): string {
    return this._fileName;
  }

  /**
   * 从 File 对象加载 .lbug 数据库
   */
  static async fromFile(file: File): Promise<LbugDatabase> {
    const mod = await getLbugModule();
    const fs = mod.getFS();

    /* 读取文件内容为 Uint8Array */
    let data: Uint8Array;
    try {
      const buffer = await file.arrayBuffer();
      data = new Uint8Array(buffer);
    } catch (err) {
      console.error("[lbugWasm] 文件读取失败:", err);
      throw new Error(`无法读取文件: ${err instanceof Error ? err.message : String(err)}`);
    }

    /* 写入 WASM 虚拟文件系统（使用 createDataFile，sync FS 不导出 writeFile）
     * createDataFile 不允许覆盖已存在文件，因此用计数器生成唯一路径 */
    const vfsName = `${vfsFileCounter++}_${file.name}`;
    const vfsPath = `${VFS_ROOT}${vfsName}`;
    try {
      fs.createDataFile(VFS_ROOT, vfsName, data, true, false);
    } catch (err) {
      console.error("[lbugWasm] 写入 VFS 失败:", err);
      throw new Error(`写入虚拟文件系统失败: ${err instanceof Error ? err.message : String(err)}`);
    }

    /* 以只读模式打开数据库 */
    try {
      const db = new mod.Database(vfsPath, 0, 0, true, true);
      const conn = new mod.Connection(db);
      return new LbugDatabase(db, conn, file.name);
    } catch (err) {
      console.error("[lbugWasm] 打开数据库失败:", err);
      throw new Error(`打开数据库失败: ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  /**
   * 执行 Cypher 查询，返回行数组（同步）
   */
  query(cypher: string): Record<string, unknown>[] {
    let result: LbugResult | null = null;
    try {
      result = this.conn.query(cypher);
      if (!result.isSuccess()) {
        const err = result.getErrorMessage();
        throw new Error(`Cypher 查询失败: ${err}`);
      }
      return result.getAllObjects();
    } finally {
      if (result) result.close();
    }
  }

  /**
   * 获取数据库统计信息（节点总数 + 边总数）
   */
  getStats(): { nodes: number; edges: number } {
    const nodeRows = this.query("MATCH (n) RETURN count(n) AS cnt");
    const edgeRows = this.query("MATCH (r:CodeRelation) RETURN count(*) AS cnt");

    const nodes = Number(nodeRows[0]?.cnt ?? 0);
    const edges = Number(edgeRows[0]?.cnt ?? 0);
    return { nodes, edges };
  }

  /**
   * 释放所有资源（连接、数据库、虚拟文件）
   */
  close(): void {
    try { this.conn.close(); } catch { /* 连接可能已关闭 */ }
    try { this.db.close(); } catch { /* 数据库可能已关闭 */ }
    /* sync FS 不导出 unlink，VFS 文件留在内存中直到页面卸载 */
  }
}
