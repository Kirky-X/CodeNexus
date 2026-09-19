// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

/* 全局错误边界 — WASM OOM、渲染异常等灾难态的白屏兜底 */

import { Component, type ReactNode } from "react";
import { useI18n } from "../lib/i18n";

interface State {
  error: Error | null;
}

/** WASM 内存耗尽等运行时灾难的识别文案（t 由调用方注入） */
function describe(error: Error, t: (key: string) => string): string {
  const msg = `${error.name}: ${error.message}`;
  if (/out of memory|OOM|memory access out of bounds|RangeError/i.test(msg)) {
    return t("error.boundaryOOM");
  }
  if (/WebGL|webglcontext/i.test(msg)) {
    return t("error.boundaryWebGL");
  }
  return msg;
}

/* 兜底 UI 本体：函数组件以经 useI18n 取当前语言的 t()
 * （ErrorBoundary 是类组件不能直接调 Hook；Provider 挂载于其外层） */
function ErrorFallback({ error }: { error: Error }) {
  const { t } = useI18n();
  const detail = describe(error, t);
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
          <p className="text-sm text-foreground/80 font-medium">{t("error.boundaryTitle")}</p>
          <p className="text-xs text-foreground/60 leading-relaxed">{detail}</p>
          <p className="text-[10px] text-fg-subtle font-mono break-all">
            {error.name}: {error.message.slice(0, 200)}
          </p>
        </div>
        <button
          onClick={() => window.location.reload()}
          className="px-4 py-1.5 rounded-lg bg-primary text-primary-foreground text-xs font-medium hover:bg-primary/90 transition-colors"
        >
          {t("error.boundaryReload")}
        </button>
      </div>
    </div>
  );
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
      return <ErrorFallback error={this.state.error} />;
    }
    return this.props.children;
  }
}
