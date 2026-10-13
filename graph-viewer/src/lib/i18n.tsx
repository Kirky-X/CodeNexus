// Copyright (c) 2026 Kirky.X🌠
// SPDX-License-Identifier: MIT

/* 国际化 — React 侧上下文与 hook
 *
 * 语言包与取值函数在 React-free 的 ./i18nCatalog（worker 与非组件模块直接
 * 引用它，避免把 React 拖进 worker 包）；本文件只保留 Provider、useI18n
 * 与文档级语言元数据同步。 */

import { createContext, useContext, useState, useCallback, useEffect, type ReactNode } from "react";
import { resolveLocale, translate, type Locale } from "./i18nCatalog";

interface I18nContextValue {
  locale: Locale;
  setLocale: (l: Locale) => void;
  t: (key: string) => string;
}

const I18nContext = createContext<I18nContextValue>({
  locale: "en",
  setLocale: () => {},
  t: (k) => k,
});

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocale] = useState<Locale>(resolveLocale);

  const t = useCallback(
    (key: string): string => translate(locale, key),
    [locale],
  );

  /* 持久化用户选择 */
  useEffect(() => {
    try { localStorage.setItem("codenexus-locale", locale); } catch {}
  }, [locale]);

  /* 同步文档级语言元数据 — index.html 的静态 lang/title 只覆盖 JS 执行前的
   * 首屏，挂载后改由生效 locale 驱动，屏幕阅读器与浏览器翻译按真实界面语言处理 */
  useEffect(() => {
    document.documentElement.lang = locale === "zh" ? "zh-CN" : "en";
    document.title = translate(locale, "webui.documentTitle");
  }, [locale]);

  return (
    <I18nContext.Provider value={{ locale, setLocale, t }}>
      {children}
    </I18nContext.Provider>
  );
}

export function useI18n() {
  return useContext(I18nContext);
}

export type { Locale };
