/* 语言切换器 */

import { useI18n, type Locale } from "../lib/i18n";

const OPTIONS: { value: Locale; label: string }[] = [
  { value: "zh", label: "中" },
  { value: "en", label: "EN" },
];

export function LanguageSwitcher() {
  const { locale, setLocale } = useI18n();

  return (
    <div className="flex items-center gap-0.5 p-0.5 rounded-lg bg-white/[0.03] border border-white/[0.05]">
      {OPTIONS.map((opt) => (
        <button
          key={opt.value}
          onClick={() => setLocale(opt.value)}
          className={`px-2 py-1 rounded-md text-[11px] font-medium transition-all duration-200 ${
            locale === opt.value
              ? "bg-primary/10 text-primary"
              : "text-foreground/30 hover:text-foreground/50"
          }`}
        >
          {opt.label}
        </button>
      ))}
    </div>
  );
}
