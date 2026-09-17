import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AgentApprovalMode, AgentAudience, AgentUserMode, AgentWorkMode } from "../types";
import { DEFAULT_LOCALE, normalizeLocale, type AppLocale } from "../i18n";

export type AgentPanelPosition = "left" | "right";

/**
 * 上传图片的压缩档位（三档）。
 *
 * 档位是**用户的选择意图**，`IMAGE_COMPRESS_TIERS` 里的宽高数值是它的**实现细节**，
 * 两者都落库（后端 `UiPreferences` 里是两个独立字段），因此任何一处改动都要同步另一处。
 * 之所以不直接暴露数值输入框：普通用户无法判断 1568 和 1920 的差别。
 */
export type ImageCompressTier = "eco" | "standard" | "hd";

/** 档位 → 压缩参数。数值与后端 `app_config.rs` 的默认值同源，改这里要同步改后端。 */
export const IMAGE_COMPRESS_TIERS: Record<ImageCompressTier, { maxWidth: number; maxHeight: number; quality: number }> = {
  // 约 1229 token/张（Anthropic 官方文档给出的安全档）
  eco: { maxWidth: 1280, maxHeight: 720, quality: 0.8 },
  // 约 1874 token/张；1568 是 Anthropic 服务端长边硬上限，超过也会被压到 1568
  standard: { maxWidth: 1568, maxHeight: 896, quality: 0.8 },
  // 约 2765 token/张；仅在小字/密集截图需要保真时用
  hd: { maxWidth: 1920, maxHeight: 1080, quality: 0.8 },
};

/**
 * 把档位对应的数值写进偏好对象。
 *
 * 存在的意义是收口"档位→数值"这一次映射，避免各处重复写 `Object.assign`：
 * 档位表的键（`maxWidth/maxHeight/quality`）与偏好字段名（`imageMaxWidth/...`）
 * **不同名**，直接 assign 只会多出废键、真正的字段一个都不改。这里显式逐字段赋值。
 */
function applyImageTier(prefs: UiPreferences, tier: ImageCompressTier) {
  const t = IMAGE_COMPRESS_TIERS[tier] ?? IMAGE_COMPRESS_TIERS.standard;
  prefs.imageMaxWidth = t.maxWidth;
  prefs.imageMaxHeight = t.maxHeight;
  prefs.imageQuality = t.quality;
}

interface UiPreferences {
  fontSize: number;
  codeFontSize: number;
  autoScroll: boolean;
  defaultExpandThinking: boolean;
  agentPanelPosition: AgentPanelPosition;
  compactMode: boolean;
  sidebarCollapsed: boolean;
  agentPanelVisible: boolean;
  agentAudience: AgentAudience;
  agentWorkMode: AgentWorkMode;
  /** 权限档位：请求审批（默认）/ 帮我批准 */
  agentApprovalMode: AgentApprovalMode;
  locale: AppLocale;
  agentMessageOpacity: number;
  userMessageOpacity: number;
  reflectionMode: "always" | "smart" | "off";
  /**
   * 图片压缩档位。
   *
   * ⚠️ 这三个字段必须在前端声明齐全：`save_ui_preferences` 是全量替换结构体，
   * 少一个字段，后端就会把它当成"未提供"回落到默认值。早期没有 UI 绑定时没暴露问题，
   * 现在有了档位选择器，漏掉就会被下一次偏好保存悄悄冲掉。
   */
  imageCompressTier: ImageCompressTier;
  imageMaxWidth: number;
  imageMaxHeight: number;
  imageQuality: number;
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
  agentAudience: "developer",
  agentWorkMode: "edit",
  agentApprovalMode: "request_approval",
  locale: DEFAULT_LOCALE,
  agentMessageOpacity: 0,
  userMessageOpacity: 0,
  reflectionMode: "smart",
  imageCompressTier: "standard",
  imageMaxWidth: IMAGE_COMPRESS_TIERS.standard.maxWidth,
  imageMaxHeight: IMAGE_COMPRESS_TIERS.standard.maxHeight,
  imageQuality: IMAGE_COMPRESS_TIERS.standard.quality,
};

function normalizePrefs(
  value: Partial<UiPreferences> & { agentDisplayMode?: string; agentReadOnly?: boolean },
): UiPreferences {
  const result = { ...defaults, ...value };
  // 向后兼容：旧 agentDisplayMode 值自动迁移
  if (value.agentDisplayMode !== undefined && !value.agentAudience) {
    result.agentAudience = value.agentDisplayMode === "developer" ? "developer" : "user";
    result.agentWorkMode = "edit";
  }
  result.agentAudience = result.agentAudience === "user" ? "user" : "developer";
  // 兼容旧版本：chat（只读保护）与 agentReadOnly 都已取消，
  // 统一迁移成"编辑模式 + 请求审批档"——安全等价：改动都会先问用户
  if ((result.agentWorkMode as string) === "chat") {
    result.agentWorkMode = "edit";
  }
  result.agentWorkMode = result.agentWorkMode === "plan" ? "plan" : "edit";
  if (value.agentReadOnly) {
    result.agentApprovalMode = "request_approval";
  }
  result.agentApprovalMode =
    result.agentApprovalMode === "auto_approve" ? "auto_approve" : "request_approval";
  result.agentPanelPosition = result.agentPanelPosition === "left" ? "left" : "right";
  result.locale = normalizeLocale(result.locale);
  normalizeImageCompress(result);
  return result;
}

/**
 * 归一图片压缩字段，保持「档位 ↔ 宽高数值」一致。
 *
 * 档位与数值是一份数据的两种视图：档位是用户的**选择意图**，数值是它的实现细节，
 * 两者都会落库（后端 `UiPreferences` 是两个独立字段），因此读取时要防它们对不上。
 * **以数值为准反推档位** —— 数值才是压缩算法实际用的东西，反推出的档位才是事实。
 *
 * 后端 C 侧有同构的 `normalize_image_tier()`：那边兜住"前端漏送字段"，
 * 这边兜住"后端数据被手改/旧版本写入"。两处逻辑必须保持一致。
 */
function normalizeImageCompress(result: UiPreferences) {
  const tiers = Object.entries(IMAGE_COMPRESS_TIERS) as [
    ImageCompressTier,
    { maxWidth: number; maxHeight: number; quality: number },
  ][];
  const matched = tiers.find(
    ([, t]) => t.maxWidth === result.imageMaxWidth && t.maxHeight === result.imageMaxHeight,
  );
  if (matched) {
    result.imageCompressTier = matched[0];
    return;
  }
  // 数值不在任何档上（档位功能上线前存下的自定义值）：
  // 收敛到标准档，并把三个数值一起校正，避免"显示一档、实际压另一套参数"
  result.imageCompressTier = "standard";
  applyImageTier(result, "standard");
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

function applyMessageOpacity() {
  document.documentElement.style.setProperty("--agent-message-opacity", String(prefs.value.agentMessageOpacity ?? 0));
  document.documentElement.style.setProperty("--user-message-opacity", String(prefs.value.userMessageOpacity ?? 100));
}

function applyAll(p: UiPreferences) {
  applyFontSize(p.fontSize);
  applyCodeFontSize(p.codeFontSize);
  applyCompactMode(p.compactMode);
  applyMessageOpacity();
}

// ── 持久化（Rust 后端 → data/app-config.json） ──

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
  watch(() => prefs.value.agentAudience, () => scheduleSave());
  watch(() => prefs.value.agentWorkMode, () => scheduleSave());
  watch(() => prefs.value.agentApprovalMode, () => scheduleSave());
  watch(() => prefs.value.locale, () => scheduleSave());
  watch(() => prefs.value.defaultExpandThinking, () => scheduleSave());
  watch(() => prefs.value.autoScroll, () => scheduleSave());
  watch(() => prefs.value.agentMessageOpacity, () => { applyMessageOpacity(); scheduleSave(); });
  watch(() => prefs.value.userMessageOpacity, () => { applyMessageOpacity(); scheduleSave(); });
  // 图片压缩档位是唯一"档位→数值"的写入点：改档位必须同时把三组数值写进去，
  // 否则后端拿到的是旧数值、界面显示的是新档位。
  // 档位与数值是两个独立字段，两者都会随 prefs 整体落库。
  watch(() => prefs.value.imageCompressTier, (tier) => {
    applyImageTier(prefs.value, tier);
    scheduleSave();
  });
  // 数值被外部改动（例如后端广播回来的数据）时反向回填档位，维持两个字段一致
  watch(
    () => [prefs.value.imageMaxWidth, prefs.value.imageMaxHeight] as const,
    () => {
      const matched = (Object.entries(IMAGE_COMPRESS_TIERS) as [
        ImageCompressTier,
        { maxWidth: number; maxHeight: number },
      ][]).find(
        ([, t]) => t.maxWidth === prefs.value.imageMaxWidth && t.maxHeight === prefs.value.imageMaxHeight,
      );
      if (matched && prefs.value.imageCompressTier !== matched[0]) {
        prefs.value.imageCompressTier = matched[0];
      }
    },
  );
}

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
  const agentAudience = computed<AgentAudience>({
    get: () => prefs.value.agentAudience,
    set: (val) => { prefs.value.agentAudience = val; },
  });
  const agentWorkMode = computed<AgentUserMode>({
    get: () => (prefs.value.agentWorkMode === "plan" ? "plan" : "edit"),
    set: (val) => { prefs.value.agentWorkMode = val; },
  });
  const agentApprovalMode = computed<AgentApprovalMode>({
    get: () => (prefs.value.agentApprovalMode === "auto_approve" ? "auto_approve" : "request_approval"),
    set: (val) => { prefs.value.agentApprovalMode = val; },
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
    agentAudience,
    setAgentAudience: (val: AgentAudience) => { prefs.value.agentAudience = val; },
    agentWorkMode,
    setAgentWorkMode: (val: AgentUserMode) => { prefs.value.agentWorkMode = val; },
    agentApprovalMode,
    setAgentApprovalMode: (val: AgentApprovalMode) => { prefs.value.agentApprovalMode = val; },
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
    get agentMessageOpacity() { return prefs.value.agentMessageOpacity; },
    setAgentMessageOpacity: (val: number) => { prefs.value.agentMessageOpacity = Math.round(val); },
    get userMessageOpacity() { return prefs.value.userMessageOpacity; },
    setUserMessageOpacity: (val: number) => { prefs.value.userMessageOpacity = Math.round(val); },
    get reflectionMode() { return prefs.value.reflectionMode; },
    setReflectionMode: (val: "always" | "smart" | "off") => { prefs.value.reflectionMode = val; },
    get imageCompressTier() { return prefs.value.imageCompressTier; },
    setImageCompressTier: (val: ImageCompressTier) => { prefs.value.imageCompressTier = val; },
    // 数值只读：唯一写入途径是上面的档位 setter（由 watcher 统一换算），
    // 避免出现"档位是省流、数值却是高清"的错位状态
    get imageMaxWidth() { return prefs.value.imageMaxWidth; },
    get imageMaxHeight() { return prefs.value.imageMaxHeight; },
    get imageQuality() { return prefs.value.imageQuality; },
  };
}
