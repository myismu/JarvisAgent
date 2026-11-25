/**
 * # useProfileSwitcher.ts — 输入框预设切换、模型能力探测与深度思考联动
 *
 * 管理 `AppConfig` 加载、模型能力（视觉/深度思考/退役状态）探测、切换预设时的
 * 退役模型阻断与缓存失效二次确认，以及预设下拉菜单的动态可视高度计算。
 *
 * ## Key Exports
 * - `useProfileSwitcher()`: 暴露配置状态、预设切换流程、思考开关投影与菜单布局辅助
 *
 * ## Dependencies
 * - Internal: `@/stores/session`, `@/stores/agent`, `@/composables/useThinkingMode`, `@/utils/thinking`, `@/types`
 * - External: `@tauri-apps/api/core`, `vue`, `vue-i18n`
 */
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { invoke } from "@tauri-apps/api/core";
import { useSessionStore } from "../../stores/session";
import { useAgentStore } from "../../stores/agent";
import { useThinkingMode } from "../useThinkingMode";
import { isThinkingToggleDisabled, type ThinkingCaps } from "../../utils/thinking";
import type { AppConfig, ModelCapabilities, ModelProfile } from "../../types";

const MENU_GAP = 12;
const MENU_SAFE_MARGIN = 12;
const MENU_MAX_HEIGHT = 340;

export function useProfileSwitcher() {
  const { t } = useI18n();
  const session = useSessionStore();
  const agent = useAgentStore();

  const appConfig = ref<AppConfig | null>(null);
  const agentModel = computed(() => {
    const active = appConfig.value?.profiles.find(
      (p) => p.id === appConfig.value?.activeProfileId,
    );
    return active?.config.mainModel || "—";
  });
  const showProfileMenu = ref(false);
  const profileMenuRef = ref<HTMLElement | null>(null);
  const showProfileCacheWarning = ref(false);
  const pendingProfileId = ref<string | null>(null);

  /** 切预设时撞到退役模型的阻断提示 */
  const retiredSwitchNotice = ref<{
    profileName: string;
    field: "mainModel" | "utilityModel";
    modelId: string;
  } | null>(null);

  const retiredSwitchMessage = computed(() => {
    const n = retiredSwitchNotice.value;
    if (!n) return "";
    const key =
      n.field === "mainModel"
        ? "settings.profiles.switchBlockedRetired"
        : "settings.profiles.switchBlockedRetiredUtility";
    return t(key, { name: n.profileName, model: n.modelId });
  });

  /** 模型思考与视觉能力（只影响 UI 投影，不决定后端下发参数） */
  const modelThinkingCaps = ref<ThinkingCaps | null>(null);
  const isThinkingForced = computed(() => modelThinkingCaps.value?.thinking_forced ?? false);
  const canModelThink = computed(() => modelThinkingCaps.value?.thinking ?? true);
  const canModelVision = ref(true);

  const {
    isThinkingActive,
    saving: thinkingSaving,
    toggleThinking: toggleThinkingMode,
  } = useThinkingMode(modelThinkingCaps);

  const toggleThinking = () => {
    toggleThinkingMode();
    if (!session.activeSessionId) {
      session.pendingThinkingEnabled = isThinkingActive.value;
    }
  };

  const thinkingToggleDisabled = computed(() => isThinkingToggleDisabled(modelThinkingCaps.value));
  const thinkingLocked = computed(
    () => isThinkingForced.value && canModelThink.value && !thinkingSaving.value,
  );

  const checkModelCapabilities = async (modelId: string) => {
    if (!modelId) return;
    try {
      const caps = await invoke<ModelCapabilities | null>("get_model_capabilities", { modelId });
      if (caps) {
        canModelVision.value = caps.vision ?? true;
        modelThinkingCaps.value = {
          thinking: caps.thinking ?? true,
          thinking_forced: caps.thinkingForced ?? false,
        };
      } else {
        canModelVision.value = true;
        modelThinkingCaps.value = { thinking: true, thinking_forced: false };
      }
    } catch (e) {
      console.error("Failed to check model capabilities:", e);
      canModelVision.value = true;
      modelThinkingCaps.value = { thinking: true, thinking_forced: false };
    }
  };

  const loadConfig = async () => {
    try {
      appConfig.value = await invoke<AppConfig>("get_config");
      const activeProfile = appConfig.value.profiles.find(
        (p) => p.id === appConfig.value?.activeProfileId,
      );
      if (activeProfile) {
        await checkModelCapabilities(activeProfile.config.mainModel);
      }
    } catch (e) {
      console.error("Failed to load config for input box:", e);
    }
  };

  watch(
    () => appConfig.value?.activeProfileId,
    (profileId, prevId) => {
      if (!profileId || profileId === prevId || !appConfig.value) return;
      const activeProfile = appConfig.value.profiles?.find((p) => p.id === profileId);
      if (activeProfile) {
        void checkModelCapabilities(activeProfile.config.mainModel);
      }
    },
  );

  const findRetiredModel = async (profile: ModelProfile) => {
    for (const field of ["mainModel", "utilityModel"] as const) {
      const modelId = profile?.config?.[field];
      if (!modelId) continue;
      try {
        const caps = await invoke<ModelCapabilities | null>("get_model_capabilities", { modelId });
        if (caps?.status === "retired") {
          return { field, modelId };
        }
      } catch {
        // 探测失败按未知处理，不阻断切换
      }
    }
    return null;
  };

  const doSwitchProfile = async (id: string) => {
    if (!appConfig.value) return;
    const currentSessionId = await invoke<string | null>("get_active_session_id").catch(() => null);
    appConfig.value.activeProfileId = id;
    try {
      await invoke("save_config_cmd", { newConfig: appConfig.value });
      if (currentSessionId) {
        await invoke("update_session_profile", { id: currentSessionId, profileId: id });
      }
      showProfileMenu.value = false;
      const activeProfile = appConfig.value.profiles.find((p) => p.id === id);
      if (activeProfile) {
        await checkModelCapabilities(activeProfile.config.mainModel);
      }
    } catch (e) {
      console.error("Failed to switch profile:", e);
    }
  };

  const switchProfile = async (id: string) => {
    if (!appConfig.value) return;

    // 退役模型拦截优先级高于 prompt cache 失效确认
    const targetProfile = appConfig.value.profiles?.find((p) => p.id === id);
    if (targetProfile) {
      const hit = await findRetiredModel(targetProfile);
      if (hit) {
        retiredSwitchNotice.value = { profileName: targetProfile.name, ...hit };
        return;
      }
    }

    if (session.activeSessionId) {
      const snap = agent.currentContextSnapshot;
      if ((snap?.messageCount ?? 0) > 0) {
        pendingProfileId.value = id;
        showProfileCacheWarning.value = true;
        return;
      }
    }
    await doSwitchProfile(id);
  };

  const confirmSwitchProfile = async () => {
    showProfileCacheWarning.value = false;
    if (pendingProfileId.value) {
      await doSwitchProfile(pendingProfileId.value);
      pendingProfileId.value = null;
    }
  };

  const openProfileMenuLayout = async () => {
    await nextTick();
    const menu = profileMenuRef.value;
    if (!menu) return;

    const btn = menu.parentElement?.querySelector<HTMLElement>(".profile-btn");
    if (btn) {
      const spaceAbove = btn.getBoundingClientRect().top - MENU_GAP - MENU_SAFE_MARGIN;
      const height = Math.max(120, Math.min(MENU_MAX_HEIGHT, Math.floor(spaceAbove)));
      menu.style.maxHeight = `${height}px`;
    }

    const activeItem = menu.querySelector<HTMLElement>(".profile-menu-item.active");
    activeItem?.scrollIntoView({ block: "nearest" });
  };

  return {
    appConfig,
    agentModel,
    showProfileMenu,
    profileMenuRef,
    showProfileCacheWarning,
    pendingProfileId,
    retiredSwitchNotice,
    retiredSwitchMessage,
    isThinkingForced,
    canModelThink,
    canModelVision,
    isThinkingActive,
    thinkingSaving,
    thinkingToggleDisabled,
    thinkingLocked,
    toggleThinking,
    loadConfig,
    switchProfile,
    confirmSwitchProfile,
    openProfileMenuLayout,
  };
}
