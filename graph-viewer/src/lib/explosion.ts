/* 共享爆炸动画进度 — 节点与边线共用同一时间轴，保证同步展开 */

export const EXPLODE_DURATION = 1.2;

/* 模块级共享爆炸缓动值（0→1，easeOutCubic），由挂载组件每帧更新 */
export const sharedExplodeEased = { current: 0 };

/* easeOutCubic 缓动 */
export function easeOutCubic(t: number): number {
  return 1 - Math.pow(1 - t, 3);
}
