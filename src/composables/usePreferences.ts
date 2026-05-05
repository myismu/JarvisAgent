import { computed, ref, watch } from "vue";
import type { AgentDisplayMode } from "../types";
import { DEFAULT_LOCALE, normalizeLocale, type AppLocale } from "../i18n";

const STORAGE_KEY = "jarvis_ui_prefs";

interface UiPreferences {
  sidebarCollapsed: boolean;
  agentPanelVisible: boolean;
  fontSize: number;
  agentDisplayMode: AgentDisplayMode;
  locale: AppLocale;
}

const defaults: UiPreferences = {
  sidebarCollapsed: false,
  agentPanelVisible: false,
  fontSize: 15,
  agentDisplayMode: "user",
  locale: DEFAULT_LOCALE,
};

function normalizePrefs(value: Partial<UiPreferences>): UiPreferences {
  const mode = value.agentDisplayMode === "developer" ? "developer" : "user";
  return { ...defaults, ...value, agentDisplayMode: mode, locale: normalizeLocale(value.locale) };
}

function loadPrefs(): UiPreferences {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      return normalizePrefs(JSON.parse(raw));
    }
  } catch {
    // ignore parse errors
  }
  return { ...defaults };
}

function savePrefs(prefs: UiPreferences) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs));
  } catch {
    // ignore storage errors
  }
}

const prefs = ref<UiPreferences>(loadPrefs());
let watchersInitialized = false;

function applyFontSize(size: number) {
  document.documentElement.style.fontSize = `${size}px`;
}

function ensureWatchers() {
  if (watchersInitialized) return;
  watchersInitialized = true;

  watch(
    () => prefs.value.sidebarCollapsed,
    () => savePrefs(prefs.value),
  );

  watch(
    () => prefs.value.agentPanelVisible,
    () => savePrefs(prefs.value),
  );

  watch(
    () => prefs.value.fontSize,
    (val) => {
      applyFontSize(val);
      savePrefs(prefs.value);
    },
  );

  watch(
    () => prefs.value.agentDisplayMode,
    () => savePrefs(prefs.value),
  );

  watch(
    () => prefs.value.locale,
    () => savePrefs(prefs.value),
  );
}

export function usePreferences() {
  applyFontSize(prefs.value.fontSize);
  ensureWatchers();

  const agentDisplayMode = computed<AgentDisplayMode>({
    get: () => prefs.value.agentDisplayMode,
    set: (val) => {
      prefs.value.agentDisplayMode = val;
    },
  });

  const locale = computed<AppLocale>({
    get: () => prefs.value.locale,
    set: (val) => {
      prefs.value.locale = normalizeLocale(val);
    },
  });

  return {
    get sidebarCollapsed() {
      return prefs.value.sidebarCollapsed;
    },
    setSidebarCollapsed: (val: boolean) => {
      prefs.value.sidebarCollapsed = val;
    },
    get agentPanelVisible() {
      return prefs.value.agentPanelVisible;
    },
    setAgentPanelVisible: (val: boolean) => {
      prefs.value.agentPanelVisible = val;
    },
    get fontSize() {
      return prefs.value.fontSize;
    },
    setFontSize: (val: number) => {
      prefs.value.fontSize = val;
    },
    agentDisplayMode,
    setAgentDisplayMode: (val: AgentDisplayMode) => {
      prefs.value.agentDisplayMode = val;
    },
    locale,
    setLocale: (val: AppLocale) => {
      prefs.value.locale = normalizeLocale(val);
    },
  };
}
