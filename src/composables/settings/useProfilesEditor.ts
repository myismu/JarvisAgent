/**
 * # useProfilesEditor.ts — 设置面板模型预设管理、生命周期探测与拖拽排序
 *
 * 封装预设草稿（`draftConfig`）与已保存态（`savedConfig`）的同步、模型能力与
 * 生命周期（retired / deprecated / unverifiable / alias）缓存探测、新引入退役模型
 * 保存阻断、预设增删复制/拖拽排序以及关窗前未保存新预设的自动落盘逻辑。
 *
 * ## Key Exports
 * - `RetiredHit`: 阻断保存的单条退役模型命中记录
 * - `useProfilesEditor()`: 提供预设管理页所需的全部响应式状态与操作方法
 *
 * ## Dependencies
 * - Internal: `@/types`
 * - External: `@tauri-apps/api/core`, `vue`, `vue-i18n`
 *
 * ## Constraints
 * - 退役阻断判据为“草稿相比已保存态**新引入**的 retired 模型”，避免老默认预设卡死保存按钮
 */
import { computed, onMounted, onUnmounted, ref, watch, type Ref } from "vue";
import { useI18n } from "vue-i18n";
import { invoke } from "@tauri-apps/api/core";
import type { AgentConfig, AppConfig, ModelCapabilities, ModelProfile } from "../../types";
import { extractCommandErrorMessage } from "../../utils/commandError";

export interface RetiredHit {
  profileName: string;
  field: "mainModel" | "utilityModel";
  modelId: string;
}

interface ModelLifecycle {
  status: string;
}

interface ProfilesEditorOptions {
  open: () => boolean;
  activeTab: Ref<"general" | "presets" | "prompts" | "tools" | "about">;
  onClose: () => void;
}

const DRAG_THRESHOLD = 5;

const createEmptyConfig = (): AppConfig => ({
  activeProfileId: "default",
  globalProfileId: "default",
  profiles: [],
});

const cloneConfig = <T>(value: T): T => JSON.parse(JSON.stringify(value));

const normalizeProfileConfig = (config: AppConfig): AppConfig => {
  config.profiles.forEach((p) => {
    p.config.temperature = p.config.temperature == null ? null : Number(p.config.temperature);
    p.config.topP = p.config.topP == null ? null : Number(p.config.topP);
    p.config.topK = p.config.topK == null ? null : Number(p.config.topK);
    p.config.maxTokens = p.config.maxTokens == null ? null : Number(p.config.maxTokens);
  });
  return config;
};

const hasMeaningfulDraftValue = (value: unknown): boolean => {
  if (value === null || value === undefined || value === false) return false;
  if (typeof value === "number") return Number.isFinite(value);
  return String(value).trim().length > 0;
};

export function useProfilesEditor(options: ProfilesEditorOptions) {
  const { open, activeTab, onClose } = options;
  const { t } = useI18n();

  const createBlankProfile = (id: string): ModelProfile => ({
    id,
    name: t("settings.profiles.newName"),
    config: {
      apiFormat: "openai",
      apiKey: "",
      baseUrl: "",
      mainModel: "",
      utilityModel: "",
      reflectionMode: "smart",
      temperature: null,
      topP: null,
      topK: null,
      maxTokens: null,
    },
  });

  const savedConfig = ref<AppConfig>(createEmptyConfig());
  const draftConfig = ref<AppConfig>(createEmptyConfig());
  const selectedProfileId = ref("default");
  const showAdvanced = ref(false);
  const showApiKey = ref(false);
  const isSaving = ref(false);
  const actionLoading = ref(false);
  const statusMsg = ref("");
  const isError = ref(false);
  const isSuccess = ref(false);
  const deleteConfirm = ref<{ id: string; title: string; message: string; warning: string } | null>(
    null,
  );

  const ensureValidSelection = (config: AppConfig, preferredId?: string) => {
    if (config.profiles.length === 0) {
      config.activeProfileId = "default";
      config.globalProfileId = "default";
      selectedProfileId.value = "default";
      return;
    }

    const fallbackId =
      preferredId && config.profiles.find((p) => p.id === preferredId)
        ? preferredId
        : config.profiles[0].id;

    if (!config.profiles.find((p) => p.id === config.activeProfileId)) {
      config.activeProfileId = config.profiles[0].id;
    }
    if (!config.profiles.find((p) => p.id === config.globalProfileId)) {
      config.globalProfileId = config.profiles[0].id;
    }
    if (!config.profiles.find((p) => p.id === selectedProfileId.value)) {
      selectedProfileId.value = fallbackId;
    }
  };

  const syncConfigs = (config: AppConfig, preferredId?: string) => {
    const normalized = normalizeProfileConfig(cloneConfig(config));
    ensureValidSelection(normalized, preferredId);
    savedConfig.value = cloneConfig(normalized);
    draftConfig.value = cloneConfig(normalized);
  };

  const editingProfile = computed(() =>
    draftConfig.value.profiles.find((p) => p.id === selectedProfileId.value),
  );

  // ── 模型能力与生命周期状态缓存 ──
  const mainModelCaps = ref<ModelCapabilities | null | undefined>(undefined);
  let capQueryTimer: ReturnType<typeof setTimeout> | null = null;
  const modelLifecycleCache = ref<Record<string, ModelLifecycle>>({});

  const lifecycleOf = (modelId: string): ModelLifecycle =>
    modelLifecycleCache.value[modelId] ?? { status: "" };

  const cacheLifecycle = (modelId: string, caps: ModelCapabilities | null) => {
    modelLifecycleCache.value = {
      ...modelLifecycleCache.value,
      [modelId]: { status: caps?.status ?? "" },
    };
  };

  const probeModelLifecycles = async (ids: string[]) => {
    const missing = [...new Set(ids.map((id) => id.trim()).filter(Boolean))].filter(
      (id) => !(id in modelLifecycleCache.value),
    );
    if (missing.length === 0) return;

    const results = await Promise.all(
      missing.map(async (id) => {
        try {
          const caps = await invoke<ModelCapabilities | null>("get_model_capabilities", {
            modelId: id,
          });
          return [id, caps] as const;
        } catch {
          return [id, null] as const;
        }
      }),
    );

    const next = { ...modelLifecycleCache.value };
    for (const [id, caps] of results) {
      next[id] = { status: caps?.status ?? "" };
    }
    modelLifecycleCache.value = next;
  };

  const draftModelIds = computed(() => {
    const ids: string[] = [];
    for (const p of draftConfig.value.profiles) {
      const main = p.config.mainModel?.trim();
      const utility = p.config.utilityModel?.trim();
      if (main) ids.push(main);
      if (utility) ids.push(utility);
    }
    return ids;
  });

  const retiredHitsIn = (profiles: ModelProfile[]): RetiredHit[] => {
    const hits: RetiredHit[] = [];
    for (const p of profiles) {
      const saved = savedConfig.value.profiles.find((s) => s.id === p.id);
      for (const field of ["mainModel", "utilityModel"] as const) {
        const id = p.config[field]?.trim();
        if (!id || lifecycleOf(id).status !== "retired") continue;
        // 与已保存值一致 → 既有状态，放行
        if (saved?.config?.[field]?.trim() === id) continue;
        hits.push({ profileName: p.name, field, modelId: id });
      }
    }
    return hits;
  };

  const retiredBlockMessage = (hit: RetiredHit) => {
    const key =
      hit.field === "mainModel"
        ? "settings.validation.mainModelRetired"
        : "settings.validation.utilityModelRetired";
    return t(key, { name: hit.profileName, model: hit.modelId });
  };

  const retiredBlockers = computed(() => retiredHitsIn(draftConfig.value.profiles));
  const retiredBlockLines = computed(() =>
    retiredBlockers.value.map((hit) => retiredBlockMessage(hit)),
  );

  let lifecycleProbeTimer: ReturnType<typeof setTimeout> | null = null;
  watch(
    draftModelIds,
    (ids) => {
      if (lifecycleProbeTimer) clearTimeout(lifecycleProbeTimer);
      lifecycleProbeTimer = setTimeout(() => {
        void probeModelLifecycles(ids);
      }, 300);
    },
    { immediate: true },
  );

  const editingMainStatus = computed(() => {
    const id = editingProfile.value?.config.mainModel?.trim();
    return id ? lifecycleOf(id).status : "";
  });
  const editingUtilityStatus = computed(() => {
    const id = editingProfile.value?.config.utilityModel?.trim();
    return id ? lifecycleOf(id).status : "";
  });

  const statusBadgeClass = (status: string) => {
    switch (status) {
      case "retired":
        return "badge-retired";
      case "deprecated":
        return "badge-deprecated";
      case "unverifiable":
        return "badge-unverifiable";
      case "alias":
        return "badge-info";
      default:
        return "";
    }
  };

  const statusLabel = (status: string) => {
    switch (status) {
      case "retired":
        return t("settings.profileEditor.statusRetired");
      case "deprecated":
        return t("settings.profileEditor.statusDeprecated");
      case "unverifiable":
        return t("settings.profileEditor.statusUnverifiable");
      case "alias":
        return t("settings.profileEditor.statusAlias");
      default:
        return "";
    }
  };

  const statusHint = (status: string) => {
    switch (status) {
      case "retired":
        return t("settings.profileEditor.retiredHint");
      case "deprecated":
        return t("settings.profileEditor.deprecatedHint");
      case "unverifiable":
        return t("settings.profileEditor.unverifiableHint");
      case "alias":
        return t("settings.profileEditor.aliasHint");
      default:
        return "";
    }
  };

  const resetStatus = () => {
    statusMsg.value = "";
    isError.value = false;
    isSuccess.value = false;
  };

  const setErrorStatus = (message: string) => {
    statusMsg.value = message;
    isError.value = true;
    isSuccess.value = false;
  };

  const setSuccessStatus = (message: string) => {
    statusMsg.value = message;
    isError.value = false;
    isSuccess.value = true;
  };

  const onMainModelInput = () => {
    if (capQueryTimer) clearTimeout(capQueryTimer);
    mainModelCaps.value = undefined;
    capQueryTimer = setTimeout(async () => {
      const modelId = editingProfile.value?.config.mainModel?.trim();
      if (!modelId) {
        mainModelCaps.value = undefined;
        return;
      }
      try {
        const caps = await invoke<ModelCapabilities | null>("get_model_capabilities", { modelId });
        mainModelCaps.value = caps;
        cacheLifecycle(modelId, caps);
      } catch {
        mainModelCaps.value = null;
        cacheLifecycle(modelId, null);
      }
    }, 400);
  };

  watch(selectedProfileId, () => {
    showApiKey.value = false;
    const modelId = editingProfile.value?.config.mainModel?.trim();
    if (modelId) onMainModelInput();
    else mainModelCaps.value = undefined;
  });

  const loadConfig = async () => {
    try {
      const res = await invoke<AppConfig>("get_config");
      syncConfigs(res, res.activeProfileId || res.profiles[0]?.id);
    } catch (e) {
      console.error("Failed to load config:", e);
    }
  };

  watch(open, (newVal) => {
    if (newVal) {
      resetStatus();
      deleteConfirm.value = null;
      void loadConfig();
    }
  });

  const selectProfile = (id: string) => {
    selectedProfileId.value = id;
    activeTab.value = "presets";
  };

  const addProfile = () => {
    resetStatus();
    const newId = `profile_${Date.now()}`;
    draftConfig.value.profiles.push(createBlankProfile(newId));
    selectedProfileId.value = newId;
    activeTab.value = "presets";
  };

  const copyProfile = (id: string) => {
    resetStatus();
    const source = draftConfig.value.profiles.find((p) => p.id === id);
    if (!source) return;
    const newId = `profile_${Date.now()}`;
    const copy = cloneConfig(source);
    copy.id = newId;
    copy.name = `${copy.name} - ${t("settings.profiles.copySuffix")}`;
    draftConfig.value.profiles.push(copy);
    selectedProfileId.value = newId;
    activeTab.value = "presets";
  };

  // ── 拖拽排序 ──
  const dragIndex = ref<number | null>(null);
  const dragOverIndex = ref<number | null>(null);
  const dragOffsetY = ref(0);
  const dragMoved = ref(false);
  let dragStartY = 0;
  let dragStartIndex = 0;
  let dragItemHeight = 0;

  const onMouseDown = (e: MouseEvent, index: number) => {
    if (actionLoading.value) return;
    dragStartY = e.clientY;
    dragStartIndex = index;
    dragItemHeight = (e.currentTarget as HTMLElement).offsetHeight;
    dragMoved.value = false;
    dragIndex.value = index;
  };

  const onMouseMove = (e: MouseEvent) => {
    if (dragIndex.value === null) return;
    const deltaY = e.clientY - dragStartY;
    if (!dragMoved.value && Math.abs(deltaY) > DRAG_THRESHOLD) {
      dragMoved.value = true;
      document.body.style.userSelect = "none";
    }
    if (!dragMoved.value) return;
    dragOffsetY.value = deltaY;
    const profiles = draftConfig.value.profiles;
    const relativeIdx = Math.round(deltaY / dragItemHeight);
    const targetIdx = Math.max(0, Math.min(profiles.length - 1, dragStartIndex + relativeIdx));
    if (targetIdx !== dragStartIndex || dragOverIndex.value === null) {
      dragOverIndex.value = targetIdx;
    }
  };

  const onMouseUp = (_e: MouseEvent, _index: number) => {
    if (dragIndex.value === null) return;
    document.body.style.userSelect = "";
    if (!dragMoved.value) {
      selectProfile(draftConfig.value.profiles[dragStartIndex].id);
    } else if (dragOverIndex.value !== null && dragOverIndex.value !== dragStartIndex) {
      const profiles = draftConfig.value.profiles;
      const [moved] = profiles.splice(dragStartIndex, 1);
      profiles.splice(dragOverIndex.value, 0, moved);
    }
    dragIndex.value = null;
    dragOverIndex.value = null;
    dragOffsetY.value = 0;
    dragMoved.value = false;
  };

  const onWindowMouseUp = () => {
    if (dragIndex.value !== null) {
      document.body.style.userSelect = "";
      dragIndex.value = null;
      dragOverIndex.value = null;
      dragOffsetY.value = 0;
      dragMoved.value = false;
    }
  };

  onMounted(() => window.addEventListener("mouseup", onWindowMouseUp));
  onUnmounted(() => window.removeEventListener("mouseup", onWindowMouseUp));

  const toggleGlobalProfile = async (profileId: string) => {
    if (actionLoading.value || savedConfig.value.globalProfileId === profileId) return;

    actionLoading.value = true;
    resetStatus();
    try {
      const nextConfig = cloneConfig(savedConfig.value);
      nextConfig.globalProfileId = profileId;
      ensureValidSelection(nextConfig, selectedProfileId.value);
      await invoke("save_config_cmd", { newConfig: nextConfig });
      syncConfigs(nextConfig, selectedProfileId.value);
      setSuccessStatus(t("settings.profiles.globalUpdated"));
    } catch (e) {
      console.error("保存全局预设失败:", e);
      setErrorStatus(t("settings.status.globalSaveError", { error: extractCommandErrorMessage(e) }));
    } finally {
      actionLoading.value = false;
    }
  };

  const requestDeleteProfile = (id: string) => {
    if (draftConfig.value.profiles.length <= 1) return;
    const profile = draftConfig.value.profiles.find((p) => p.id === id);
    if (!profile) return;

    deleteConfirm.value = {
      id,
      title: t("settings.profiles.deleteTitle"),
      message: t("settings.profiles.deleteMessage", { name: profile.name }),
      warning: t("settings.profiles.deleteWarning"),
    };
  };

  const confirmDeleteProfile = async () => {
    if (!deleteConfirm.value || actionLoading.value) return;

    actionLoading.value = true;
    resetStatus();
    try {
      const targetId = deleteConfirm.value.id;
      const nextConfig = cloneConfig(savedConfig.value);
      const index = nextConfig.profiles.findIndex((p) => p.id === targetId);
      if (index === -1) {
        deleteConfirm.value = null;
        return;
      }

      nextConfig.profiles.splice(index, 1);
      if (nextConfig.profiles.length === 0) {
        throw new Error(t("settings.validation.keepOneProfile"));
      }

      const fallbackId = nextConfig.profiles[0].id;
      if (nextConfig.activeProfileId === targetId) nextConfig.activeProfileId = fallbackId;
      if (nextConfig.globalProfileId === targetId) nextConfig.globalProfileId = fallbackId;

      const nextSelectedId =
        selectedProfileId.value === targetId ? fallbackId : selectedProfileId.value;
      ensureValidSelection(nextConfig, nextSelectedId);
      await invoke("save_config_cmd", { newConfig: nextConfig });
      syncConfigs(nextConfig, nextSelectedId);
      deleteConfirm.value = null;
      setSuccessStatus(t("settings.profiles.deleteSuccess"));
    } catch (e) {
      console.error("删除配置预设失败:", e);
      setErrorStatus(t("settings.status.deleteError", { error: extractCommandErrorMessage(e) }));
    } finally {
      actionLoading.value = false;
    }
  };

  const hasNewProfileContent = (profile: ModelProfile): boolean => {
    const blank = createBlankProfile(profile.id);
    if (profile.name.trim() && profile.name.trim() !== blank.name) return true;
    if (profile.config.apiFormat !== blank.config.apiFormat) return true;

    const contentKeys: Array<keyof AgentConfig> = [
      "apiKey",
      "baseUrl",
      "mainModel",
      "utilityModel",
      "temperature",
      "topP",
      "topK",
      "maxTokens",
    ];
    return contentKeys.some((key) => hasMeaningfulDraftValue(profile.config[key]));
  };

  const persistFilledNewProfilesBeforeClose = async () => {
    const savedIds = new Set(savedConfig.value.profiles.map((profile) => profile.id));
    const filledNewProfiles = draftConfig.value.profiles.filter(
      (profile) => !savedIds.has(profile.id) && hasNewProfileContent(profile),
    );
    if (filledNewProfiles.length === 0) return;

    await probeModelLifecycles(
      filledNewProfiles.flatMap((p) => [p.config.mainModel ?? "", p.config.utilityModel ?? ""]),
    );
    const hits = retiredHitsIn(filledNewProfiles);
    if (hits.length > 0) {
      throw new Error(retiredBlockMessage(hits[0]));
    }

    const nextConfig = cloneConfig(savedConfig.value);
    nextConfig.profiles.push(...filledNewProfiles.map((profile) => cloneConfig(profile)));
    ensureValidSelection(nextConfig, selectedProfileId.value);
    const normalized = normalizeProfileConfig(nextConfig);

    await invoke("save_config_cmd", { newConfig: normalized });
    syncConfigs(normalized, selectedProfileId.value);
  };

  const close = async () => {
    if (isSaving.value || actionLoading.value) return;
    try {
      await persistFilledNewProfilesBeforeClose();
    } catch (e) {
      console.error("保存已填写的新预设失败:", e);
      setErrorStatus(t("settings.status.filledProfileSaveError", { error: extractCommandErrorMessage(e) }));
      return;
    }

    draftConfig.value = cloneConfig(savedConfig.value);
    ensureValidSelection(draftConfig.value, selectedProfileId.value);
    deleteConfirm.value = null;
    resetStatus();
    onClose();
  };

  const save = async () => {
    if (!draftConfig.value.profiles || draftConfig.value.profiles.length === 0) return;
    ensureValidSelection(draftConfig.value, selectedProfileId.value);

    for (const p of draftConfig.value.profiles) {
      if (!p.name || !p.name.trim()) {
        setErrorStatus(t("settings.validation.nameRequired"));
        return;
      }
      if (!p.config.baseUrl || !p.config.baseUrl.trim()) {
        setErrorStatus(t("settings.validation.baseUrlRequired", { name: p.name }));
        return;
      }
      if (!p.config.mainModel || !p.config.mainModel.trim()) {
        setErrorStatus(t("settings.validation.mainModelRequired", { name: p.name }));
        return;
      }
      if (!p.config.utilityModel || !p.config.utilityModel.trim()) {
        setErrorStatus(t("settings.validation.utilityModelRequired", { name: p.name }));
        return;
      }
    }

    await probeModelLifecycles(draftModelIds.value);
    if (retiredBlockers.value.length > 0) {
      setErrorStatus(retiredBlockMessage(retiredBlockers.value[0]));
      return;
    }

    isSaving.value = true;
    resetStatus();
    try {
      const nextConfig = normalizeProfileConfig(cloneConfig(draftConfig.value));
      ensureValidSelection(nextConfig, selectedProfileId.value);
      await invoke("save_config_cmd", { newConfig: nextConfig });
      syncConfigs(nextConfig, selectedProfileId.value);
      setSuccessStatus(t("settings.status.saveSuccess"));
      setTimeout(() => {
        void close();
      }, 800);
    } catch (e) {
      setErrorStatus(t("settings.status.saveError", { error: extractCommandErrorMessage(e) }));
    } finally {
      isSaving.value = false;
    }
  };

  return {
    savedConfig,
    draftConfig,
    selectedProfileId,
    showAdvanced,
    showApiKey,
    isSaving,
    actionLoading,
    statusMsg,
    isError,
    isSuccess,
    deleteConfirm,
    editingProfile,
    mainModelCaps,
    retiredBlockers,
    retiredBlockLines,
    editingMainStatus,
    editingUtilityStatus,
    dragIndex,
    dragOverIndex,
    dragOffsetY,
    dragMoved,
    statusBadgeClass,
    statusLabel,
    statusHint,
    resetStatus,
    setErrorStatus,
    setSuccessStatus,
    onMainModelInput,
    selectProfile,
    addProfile,
    copyProfile,
    onMouseDown,
    onMouseMove,
    onMouseUp,
    toggleGlobalProfile,
    requestDeleteProfile,
    confirmDeleteProfile,
    close,
    save,
  };
}
