/* 全局错误边界 — WASM OOM、渲染异常等灾难态的白屏兜底 */

import { Component, type ReactNode } from "react";

interface State {
  error: Error | null;
}

/** WASM 内存耗尽等运行时灾难的识别文案 */
function describe(error: Error): string {
  const msg = `${error.name}: ${error.message}`;
  if (/out of memory|OOM|memory access out of bounds|RangeError/i.test(msg)) {
    return "内存不足：该文件对当前设备而言过大，请关闭其他标签页后重试，或使用更小的数据库文件。";
  }
  if (/WebGL|webglcontext/i.test(msg)) {
    return "图形初始化失败：浏览器或显卡不支持 WebGL2，请更换浏览器或更新显卡驱动。";
  }
  return msg;
}

export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error) {
    console.error("[ErrorBoundary]", error);
  }

  render() {
    if (this.state.error) {
      const detail = describe(this.state.error);
      return (
        <div className="h-screen flex items-center justify-center bg-ambient text-foreground px-6">
          <div className="glass rounded-2xl p-8 max-w-lg text-center space-y-5">
            <div className="w-10 h-10 rounded-full bg-destructive/10 flex items-center justify-center mx-auto">
              <svg className="w-5 h-5 text-destructive" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2">
                <circle cx="8" cy="8" r="6" />
                <path d="M8 5v3M8 10h.01" />
              </svg>
            </div>
            <div className="space-y-2">
              <p className="text-sm text-foreground/80 font-medium">出错了</p>
              <p className="text-xs text-foreground/60 leading-relaxed">{detail}</p>
              <p className="text-[10px] text-foreground/30 font-mono break-all">
                {this.state.error.name}: {this.state.error.message.slice(0, 200)}
              </p>
            </div>
            <button
              onClick={() => window.location.reload()}
              className="px-4 py-1.5 rounded-lg bg-primary text-primary-foreground text-xs font-medium hover:bg-primary/90 transition-colors"
            >
              重新加载
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
