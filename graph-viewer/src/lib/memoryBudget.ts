/* 内存预算 — 依据文件大小与设备可用内存，自适应控制加载参数
 *
 * 大库的内存大头（按占比排序）：
 * 1. .lbug 全量载入 MEMFS（WASM 堆内一份完整副本）≈ fileSize
 * 2. LadybugDB 存储列缓冲 + buffer pool ≈ 0.5~1 × fileSize（bufferPoolSize 可控）
 * 3. 关系扫描物化的 JS 行对象 ≈ scanLimit × 每行数百字节（scanLimit 可控）
 *
 * 因此预算输出三件事：bufferPoolBytes（封顶 DB 缓冲）、
 * scanLimit（封顶关系扫描行数）、maxNodesCap（节点上限，
 * 对结果集与几何体做二线保护）。 */

export interface MemoryProfile {
  /** navigator.deviceMemory（GB，Chrome 系，上限 8）；不可用为 null */
  deviceMemoryGB: number | null;
  /** performance.memory.jsHeapSizeLimit（Chrome 系）；不可用为 null */
  heapLimitBytes: number | null;
}

export interface LoadBudget {
  /** 传给 Database 构造的 bufferPoolSize（字节） */
  bufferPoolBytes: number;
  /** 关系扫描行数上限 */
  scanLimit: number;
  /** 节点数上限（钳制 UI 的 maxNodes 选择） */
  maxNodesCap: number;
  /** 文件偏大，已启用省内存档 */
  warnLowMemory: boolean;
}

/** 低内存设备的判定阈值（GB） */
const LOW_MEMORY_GB = 4;
/** 非低内存设备的基准可用份额 */
const USABLE_PER_GB = 160 * 1024 * 1024;
/** 文件峰值系数：MEMFS 副本 + DB 存储 ≈ 2.2 × fileSize */
const PEAK_FACTOR = 2.2;

/** 采集设备内存画像（仅 Chromium 系提供完整信息，缺失时按 4GB 档兜底） */
export function detectMemoryProfile(): MemoryProfile {
  const nav = navigator as Navigator & { deviceMemory?: number };
  const perf = performance as Performance & { memory?: { jsHeapSizeLimit?: number } };
  const deviceMemoryGB = typeof nav.deviceMemory === "number" ? nav.deviceMemory : null;
  const heapLimitBytes =
    typeof perf.memory?.jsHeapSizeLimit === "number" ? perf.memory.jsHeapSizeLimit : null;
  return { deviceMemoryGB, heapLimitBytes };
}

function usableBytes(profile: MemoryProfile): number {
  const gb = profile.deviceMemoryGB ?? LOW_MEMORY_GB;
  const byDevice = gb * USABLE_PER_GB;
  if (profile.heapLimitBytes != null && profile.heapLimitBytes > 0) {
    return Math.min(byDevice, profile.heapLimitBytes * 0.4);
  }
  return byDevice;
}

const MB = 1024 * 1024;

/**
 * 依据文件大小与内存画像计算加载预算。
 * 规则（可从常量直接推导）：
 * - 可用内存 = 设备档位份额（GB×160MB），与 JS 堆上限×0.4 取小
 * - 文件峰值 ≈ fileSize×2.2；峰值超过可用内存 55% → 省内存档
 * - maxNodesCap 按文件大小分档 500/200/100/50，省内存档再减半（下限 50）
 * - scanLimit 基准 200k，省内存档 50k
 * - bufferPool 夹在 [32MB, 192MB]，低内存设备减半
 */
export function computeLoadBudget(fileSizeBytes: number, profile: MemoryProfile): LoadBudget {
  const usable = usableBytes(profile);
  const peak = fileSizeBytes * PEAK_FACTOR;
  const warnLowMemory = peak > usable * 0.55;

  let maxNodesCap: number;
  if (fileSizeBytes <= 64 * MB) maxNodesCap = 500;
  else if (fileSizeBytes <= 192 * MB) maxNodesCap = 200;
  else if (fileSizeBytes <= 384 * MB) maxNodesCap = 100;
  else maxNodesCap = 50;
  if (warnLowMemory) maxNodesCap = Math.max(50, Math.floor(maxNodesCap / 2));

  const scanLimit = warnLowMemory ? 50_000 : 200_000;

  let bufferPoolBytes = Math.min(Math.max(fileSizeBytes, 32 * MB), 192 * MB);
  const gb = profile.deviceMemoryGB ?? LOW_MEMORY_GB;
  if (gb <= LOW_MEMORY_GB) bufferPoolBytes = Math.floor(bufferPoolBytes / 2);

  return { bufferPoolBytes, scanLimit, maxNodesCap, warnLowMemory };
}
