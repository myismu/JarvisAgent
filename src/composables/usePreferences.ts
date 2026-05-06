import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AgentDisplayMode } from "../types";
import { DEFAULT_LOCALE, normalizeLocale, type AppLocale } from "../i18n";

export type AgentPanelPosition = "left" | "right";

interface UiPreferences {
  fontSize: number;
  codeFontSize: number;
  autoScroll: boolean;
  defaultExpandThinking: boolean;
  agentPanelPosition: AgentPanelPosition;
  compactMode: boolean;
  sidebarCollapsed: boolean;
  agentPanelVisible: boolean;
  agentDisplayMode: AgentDisplayMode;
  locale: AppLocale;
}

const defaults: UiPreferences = {
  fontSize: 15,
  codeFontSize: 13,
  autoScroll: true,
  defaultExpandThinking: false,
  agentPanelPosition: "right",
  compactMode: false,
  sidebarCollapsed: false,
  agentPanelVisible: false,
  agentDisplayMode: "user",
  locale: DEFAULT_LOCALE,
};

function normalizePrefs(value: Partial<UiPreferences>): UiPreferences {
  const mode = value.agentDisplayMode === "developer" ? "developer" : "user";
  const pos = value.agentPanelPosition === "left" ? "left" : "right";
  return { ...defaults, ...value, agentDisplayMode: mode, locale: normalizeLocale(value.locale), agentPanelPosition: pos };
}

const prefs = ref<UiPreferences>({ ...defaults });
let loaded = false;
let watchersInitialized = false;

// ── DOM 应用 ──

function applyFontSize(size: number) {
  document.documentElement.style.fontSize = `${size}px`;
}

function applyCodeFontSize(size: number) {
  document.documentElement.style.setProperty("--code-font-size", `${size}px`);
}

function applyCompactMode(compact: boolean) {
  document.documentElement.classList.toggle("compact-mode", compact);
}

function applyAll(p: UiPreferences) {
  applyFontSize(p.fontSize);
  applyCodeFontSize(p.codeFontSize);
  applyCompactMode(p.compactMode);
}

// ── 持久化（Rust 后端 → data/window-state.json） ──

async function loadFromBackend() {
  try {
    const saved = await invoke<UiPreferences>("get_ui_preferences");
    prefs.value = normalizePrefs(saved);
  } catch {
    prefs.value = { ...defaults };
  }
  applyAll(prefs.value);
  loaded = true;
}

let saveTimer: ReturnType<typeof setTimeout> | null = null;
function scheduleSave() {
  if (!loaded) return;
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = setTimeout(async () => {
    try {
      await invoke("save_ui_preferences", { preferences: prefs.value });
    } catch {
      // ignore save errors
    }
  }, 200);
}

function startWatchers() {
  if (watchersInitialized) return;
  watchersInitialized = true;

  watch(() => prefs.value.fontSize, (val) => { applyFontSize(val); scheduleSave(); });
  watch(() => prefs.value.codeFontSize, (val) => { applyCodeFontSize(val); scheduleSave(); });
  watch(() => prefs.value.compactMode, (val) => { applyCompactMode(val); scheduleSave(); });
  watch(() => prefs.value.agentPanelPosition, () => scheduleSave());
  watch(() => prefs.value.sidebarCollapsed, () => scheduleSave());
  watch(() => prefs.value.agentPanelVisible, () => scheduleSave());
  watch(() => prefs.value.agentDisplayMode, () => scheduleSave());
  watch(() => prefs.value.locale, () => scheduleSave());
  watch(() => prefs.value.defaultExpandThinking, () => scheduleSave());
  watch(() => prefs.value.autoScroll, () => scheduleSave());
}

// ── 自动初始化（usePreferences 首次调用时触发，无需顶层 await） ──

let initStarted = false;

function ensureInit() {
  if (initStarted) return;
  initStarted = true;

  // 先用默认值渲染，避免阻塞 UI
  applyAll(defaults);

  // 异步加载后端数据
  loadFromBackend().catch((e) => {
    console.error("[Preferences] 加载失败，使用默认值:", e);
    prefs.value = { ...defaults };
    applyAll(prefs.value);
    loaded = true;
  });

  // 注册跨窗口同步
  listen("ui-preferences-changed", async () => {
    try {
      await loadFromBackend();
    } catch {
      // ignore
    }
  }).catch((e) => {
    console.error("[Preferences] 注册跨窗口同步监听失败:", e);
  });

  startWatchers();
}

// ── 同步 API ──

export function usePreferences() {
  ensureInit();
  const agentDisplayMode = computed<AgentDisplayMode>({
    get: () => prefs.value.agentDisplayMode,
    set: (val) => { prefs.value.agentDisplayMode = val; },
  });

  const locale = computed<AppLocale>({
    get: () => prefs.value.locale,
    set: (val) => { prefs.value.locale = normalizeLocale(val); },
  });

  return {
    get sidebarCollapsed() { return prefs.value.sidebarCollapsed; },
    setSidebarCollapsed: (val: boolean) => { prefs.value.sidebarCollapsed = val; },
    get agentPanelVisible() { return prefs.value.agentPanelVisible; },
    setAgentPanelVisible: (val: boolean) => { prefs.value.agentPanelVisible = val; },
    get fontSize() { return prefs.value.fontSize; },
    setFontSize: (val: number) => { prefs.value.fontSize = val; },
    get codeFontSize() { return prefs.value.codeFontSize; },
    setCodeFontSize: (val: number) => { prefs.value.codeFontSize = val; },
    agentDisplayMode,
    setAgentDisplayMode: (val: AgentDisplayMode) => { prefs.value.agentDisplayMode = val; },
    locale,
    setLocale: (val: AppLocale) => { prefs.value.locale = normalizeLocale(val); },
    get defaultExpandThinking() { return prefs.value.defaultExpandThinking; },
    setDefaultExpandThinking: (val: boolean) => { prefs.value.defaultExpandThinking = val; },
    get autoScroll() { return prefs.value.autoScroll; },
    setAutoScroll: (val: boolean) => { prefs.value.autoScroll = val; },
    get agentPanelPosition() { return prefs.value.agentPanelPosition; },
    setAgentPanelPosition: (val: AgentPanelPosition) => { prefs.value.agentPanelPosition = val; },
    get compactMode() { return prefs.value.compactMode; },
    setCompactMode: (val: boolean) => { prefs.value.compactMode = val; },
  };
}
