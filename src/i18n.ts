import { createI18n } from "vue-i18n";
import zhCN from "./locales/zh-CN.json";
import enUS from "./locales/en-US.json";

export type AppLocale = "zh-CN" | "en-US";

export const SUPPORTED_LOCALES: AppLocale[] = ["zh-CN", "en-US"];
export const DEFAULT_LOCALE: AppLocale = "zh-CN";

export function normalizeLocale(locale: unknown): AppLocale {
  return SUPPORTED_LOCALES.includes(locale as AppLocale) ? (locale as AppLocale) : DEFAULT_LOCALE;
}

function loadInitialLocale(): AppLocale {
  try {
    const raw = localStorage.getItem("jarvis_ui_prefs");
    if (!raw) return DEFAULT_LOCALE;
    const parsed = JSON.parse(raw) as { locale?: unknown };
    return normalizeLocale(parsed.locale);
  } catch {
    return DEFAULT_LOCALE;
  }
}

export const i18n = createI18n({
  legacy: false,
  locale: loadInitialLocale(),
  fallbackLocale: DEFAULT_LOCALE,
  messages: {
    "zh-CN": zhCN,
    "en-US": enUS,
  },
});
