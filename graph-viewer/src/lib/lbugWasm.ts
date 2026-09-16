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

/** Emscripten FS — sync 变体仅类型化导出 createDataFile 等少量 API，
 * 运行时对象通常还带 open/write/close/unlink（用于分块流式与释放，均需 feature-detect） */
interface EmscriptenFS {
  createDataFile(
    parent: string,
    name: string | null,
    data: Uint8Array | string,
    canRead: boolean,
    canWrite: boolean,
    canOwn?: boolean,
  ): unknown;
  open?(path: string, flags: string): { object: unknown };
  write?(stream: { object: unknown }, buffer: Uint8Array, offset: number, length: number, position?: number): number;
  close?(stream: { object: unknown }): void;
  unlink?(path: string): void;
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
  /** MEMFS 内的库文件路径 — close 时尝试 unlink 释放 WASM 堆 */
  private vfsPath: string | null;

  private constructor(db: LbugDB, conn: LbugConn, fileName: string, vfsPath: string | null) {
    this.db = db;
    this.conn = conn;
    this._fileName = fileName;
    this.vfsPath = vfsPath;
  }

  /** 当前打开的文件名 */
  get fileName(): string {
    return this._fileName;
  }

  /**
   * 从 File 对象加载 .lbug 数据库
   *
   * @param file 浏览器 File 对象
   * @param bufferPoolSize DB 缓冲池上限（字节），0 = 引擎默认。
   *        内存受限设备按 {@link computeLoadBudget} 的预算传入
   */
  static async fromFile(file: File, bufferPoolSize = 0): Promise<LbugDatabase> {
    const mod = await getLbugModule();
    const fs = mod.getFS();

    /* 写入 WASM 虚拟文件系统。
     * 优先分块流式写入（8MB/块）：File 按需切片读取，不在 JS 堆持有
     * 全量副本——大文件场景 JS 侧峰值从 ~2×fileSize 降到 ~8MB。
     * FS.open/write 不可用（旧运行时）时退回 createDataFile 全量拷贝。 */
    const vfsName = `${vfsFileCounter++}_${file.name}`;
    const vfsPath = `${VFS_ROOT}${vfsName}`;
    try {
      if (typeof fs.open === "function" && typeof fs.write === "function" && typeof fs.close === "function") {
        const stream = fs.open(vfsPath, "w+");
        const CHUNK = 8 * 1024 * 1024;
        let pos = 0;
        while (pos < file.size) {
          const slice = file.slice(pos, pos + CHUNK);
          const buf = new Uint8Array(await slice.arrayBuffer());
          fs.write(stream, buf, 0, buf.length, pos);
          pos += buf.length;
        }
        fs.close(stream);
      } else {
        const data = new Uint8Array(await file.arrayBuffer());
        fs.createDataFile(VFS_ROOT, vfsName, data, true, false);
      }
    } catch (err) {
      console.error("[lbugWasm] 写入 VFS 失败:", err);
      throw new Error(`写入虚拟文件系统失败: ${err instanceof Error ? err.message : String(err)}`);
    }

    /* 以只读模式打开数据库（bufferPoolSize 封顶 DB 缓冲，保护低内存设备） */
    try {
      const db = new mod.Database(vfsPath, bufferPoolSize, 0, true, true);
      const conn = new mod.Connection(db);
      console.debug(`[lbugWasm] 已加载 ${file.name}（${(file.size / 1048576).toFixed(1)}MB，bufferPool=${bufferPoolSize || "默认"}）`);
      return new LbugDatabase(db, conn, file.name, vfsPath);
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
    /* 释放 MEMFS 副本：99MB 库在 WASM 堆占约 100MB，切换文件若不
     * unlink 会累积。旧运行时无 unlink 时退回原行为（页面卸载释放） */
    if (this.vfsPath && lbugModule) {
      try {
        const fs = lbugModule.getFS();
        if (typeof fs.unlink === "function") fs.unlink(this.vfsPath);
      } catch { /* 已释放或不支持 */ }
      this.vfsPath = null;
    }
  }
}
