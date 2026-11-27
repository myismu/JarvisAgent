/**
 * # useSessionControlModes.ts — 工作模式、权限档位、只读保护与压缩状态控制
 *
 * 管理输入栏底部的双轴控制项（WorkMode 编辑/规划、权限档位三档、只读保护硬闸门）
 * 与后端会话状态的同步、乐观更新回滚，以及当前会话上下文压缩状态的版本化追踪。
 *
 * ## Key Exports
 * - `useSessionControlModes()`: 返回模式/档位/只读/压缩状态与全部操作方法
 *
 * ## Dependencies
 * - Internal: `@/stores/session`, `@/composables/usePreferences`, `@/types`
 * - External: `@tauri-apps/api/core`, `vue`
 */
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useSessionStore } from "../../stores/session";
import { usePreferences } from "../usePreferences";
import { extractCommandErrorMessage } from "../../utils/commandError";
import type {
  AgentApprovalMode,
  AgentUserMode,
  AgentWorkMode,
  SessionPermissionSettingsView,
} from "../../types";

export function useSessionControlModes() {
  const session = useSessionStore();
  const uiPrefs = usePreferences();

  // 压缩状态按会话维护，并用事件/查询双版本号防旧查询覆盖新事件
  const compactingSessionIds = ref<Set<string>>(new Set());
  const compactingEventVersions = new Map<string, number>();
  const compactingQueryVersions = new Map<string, number>();

  const isCompacting = computed(() => {
    const sessionId = session.activeSessionId;
    return !!sessionId && compactingSessionIds.value.has(sessionId);
  });

  function applyCompactingStatus(sessionId: string, compacting: boolean) {
    compactingEventVersions.set(sessionId, (compactingEventVersions.get(sessionId) ?? 0) + 1);
    const next = new Set(compactingSessionIds.value);
    if (compacting) next.add(sessionId);
    else next.delete(sessionId);
    compactingSessionIds.value = next;
  }

  async function syncCompactingStatus(sessionId: string) {
    const queryVersion = (compactingQueryVersions.get(sessionId) ?? 0) + 1;
    compactingQueryVersions.set(sessionId, queryVersion);
    const eventVersion = compactingEventVersions.get(sessionId) ?? 0;
    try {
      const compacting = await invoke<boolean>("is_session_compacting", { sessionId });
      if (
        compactingQueryVersions.get(sessionId) !== queryVersion ||
        (compactingEventVersions.get(sessionId) ?? 0) !== eventVersion
      ) {
        return;
      }
      const next = new Set(compactingSessionIds.value);
      if (compacting) next.add(sessionId);
      else next.delete(sessionId);
      compactingSessionIds.value = next;
    } catch {
      // 保留最近一次已知状态
    }
  }

  watch(
    () => session.activeSessionId,
    (sessionId) => {
      if (sessionId) void syncCompactingStatus(sessionId);
    },
    { immediate: true },
  );

  // 模式、档位与选项菜单状态
  const showWorkModeMenu = ref(false);
  const showApprovalMenu = ref(false);
  const showOptionsMenu = ref(false);
  const currentWorkMode = ref<AgentWorkMode>(uiPrefs.agentWorkMode.value);
  const currentApprovalMode = ref<AgentApprovalMode>(uiPrefs.agentApprovalMode.value);
  const agentReadOnly = ref(false);
  const workModeWarning = ref("");

  // 设置页修改默认值时，只在无活跃会话（新建态）下同步刷新选择器
  watch(
    () => uiPrefs.agentWorkMode.value,
    (val) => {
      if (session.activeSessionId) return;
      session.pendingWorkMode = null;
      currentWorkMode.value = val;
    },
  );

  watch(
    () => uiPrefs.agentApprovalMode.value,
    (val) => {
      if (session.activeSessionId) return;
      session.pendingApprovalMode = null;
      currentApprovalMode.value = val;
    },
  );

  /** 当前会话是否绑定项目（决定是否开放 full_approve 完全批准档） */
  const projectBound = computed(() => {
    if (session.activeSessionId) return !!session.workingDirectory;
    return !!session.pendingProjectId;
  });

  const approvalModeOptions = computed<AgentApprovalMode[]>(() => {
    const modes: AgentApprovalMode[] = ["request_approval", "auto_approve"];
    if (projectBound.value || currentApprovalMode.value === "full_approve") {
      modes.push("full_approve");
    }
    return modes;
  });

  const toggleReadOnly = async () => {
    const next = !agentReadOnly.value;
    const prev = agentReadOnly.value;
    workModeWarning.value = "";
    try {
      if (session.activeSessionId) {
        await invoke("set_agent_read_only", { sessionId: session.activeSessionId, enabled: next });
      } else {
        session.pendingReadOnly = next;
      }
      agentReadOnly.value = next;
    } catch (e) {
      agentReadOnly.value = prev;
      workModeWarning.value = extractCommandErrorMessage(e);
      console.error("Failed to toggle read-only protection:", e);
    }
  };

  const applyApprovalMode = async (mode: AgentApprovalMode) => {
    showApprovalMenu.value = false;
    if (mode === currentApprovalMode.value) return;
    const prev = currentApprovalMode.value;
    workModeWarning.value = "";
    currentApprovalMode.value = mode;
    try {
      const sid = session.activeSessionId;
      if (sid) {
        await invoke("set_session_approval_mode", { sessionId: sid, mode });
        session.pendingApprovalMode = null;
      } else {
        session.pendingApprovalMode = mode;
      }
    } catch (e) {
      currentApprovalMode.value = prev;
      workModeWarning.value = extractCommandErrorMessage(e);
      console.error("Failed to switch approval mode:", e);
    }
  };

  const applyWorkMode = async (mode: AgentUserMode) => {
    showWorkModeMenu.value = false;
    if (mode === currentWorkMode.value) return;
    const prev = currentWorkMode.value;
    workModeWarning.value = "";
    currentWorkMode.value = mode;
    try {
      const sid = session.activeSessionId;
      if (sid) {
        await invoke("set_session_work_mode", { sessionId: sid, mode });
        session.pendingWorkMode = null;
      } else {
        session.pendingWorkMode = mode;
      }
    } catch (e) {
      currentWorkMode.value = prev;
      workModeWarning.value = extractCommandErrorMessage(e);
      console.error("Failed to switch work mode:", e);
    }
  };

  const syncPermissionFromSession = async () => {
    const sid = session.activeSessionId;
    if (!sid) {
      currentWorkMode.value = session.pendingWorkMode ?? uiPrefs.agentWorkMode.value;
      currentApprovalMode.value = session.pendingApprovalMode ?? uiPrefs.agentApprovalMode.value;
      agentReadOnly.value = session.pendingReadOnly ?? false;
      return;
    }
    session.pendingWorkMode = null;
    session.pendingApprovalMode = null;
    session.pendingReadOnly = null;
    session.pendingThinkingEnabled = null;
    try {
      const mode = await invoke<AgentWorkMode>("get_session_work_mode", { sessionId: sid });
      currentWorkMode.value = mode;
      const settings = await invoke<SessionPermissionSettingsView>(
        "get_session_permission_settings",
        { sessionId: sid },
      );
      currentApprovalMode.value = settings.approvalMode as AgentApprovalMode;
      agentReadOnly.value = settings.readOnly === true;
    } catch (e) {
      console.error("Failed to read session permission settings:", e);
    }
  };

  watch(() => session.activeSessionId, syncPermissionFromSession);

  const hideWorkModeWarning = () => {
    workModeWarning.value = "";
  };

  return {
    isCompacting,
    applyCompactingStatus,
    syncCompactingStatus,
    showWorkModeMenu,
    showApprovalMenu,
    showOptionsMenu,
    currentWorkMode,
    currentApprovalMode,
    agentReadOnly,
    workModeWarning,
    projectBound,
    approvalModeOptions,
    toggleReadOnly,
    applyApprovalMode,
    applyWorkMode,
    syncPermissionFromSession,
    hideWorkModeWarning,
  };
}
