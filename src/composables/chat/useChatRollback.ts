/**
 * # useChatRollback.ts — 聊天消息撤回与检查点回滚交互状态机
 *
 * 管理用户消息回滚触发菜单定位、代码变更预览查询（`preview_rollback_to_checkpoint_with_recall`）、
 * 二次确认弹窗状态、回滚执行（含 `restoreWarnings` 结果通知）以及回滚后的历史/方案文档重载。
 *
 * ## Key Exports
 * - `useChatRollback()`: 暴露回滚菜单/弹窗状态与操作方法
 *
 * ## Dependencies
 * - Internal: `@/stores/session`, `@/stores/chat`, `@/stores/permission`, `@/types`
 * - External: `@tauri-apps/api/core`, `vue`, `vue-i18n`
 */
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import { invoke } from "@tauri-apps/api/core";
import { useSessionStore } from "../../stores/session";
import { useChatStore } from "../../stores/chat";
import { usePermissionStore } from "../../stores/permission";
import type {
  PatchSummary,
  PlanDocument,
  RollbackPreviewResult,
  RollbackRecallResult,
} from "../../types";

export interface RollbackPreviewState {
  loading: boolean;
  checkpointId: string;
  files: PatchSummary[];
  targetCheckpointId: string;
  checkpointLabel: string;
  error: string;
  initialState: boolean;
  roundNumber: number;
  affectedRounds: number;
}

const createEmptyPreview = (): RollbackPreviewState => ({
  loading: false,
  checkpointId: "",
  files: [],
  targetCheckpointId: "",
  checkpointLabel: "",
  error: "",
  initialState: false,
  roundNumber: 0,
  affectedRounds: 0,
});

export function getRollbackMenuPosition(x: number, y: number, rollbackMode: "both" | "session") {
  const menuWidth = 220;
  const menuHeight = rollbackMode === "both" ? 118 : 74;
  const margin = 12;

  let left = x;
  let top = y;
  if (left + menuWidth > window.innerWidth - margin) {
    left = window.innerWidth - menuWidth - margin;
  }
  if (top + menuHeight > window.innerHeight - margin) {
    top = window.innerHeight - menuHeight - margin;
  }
  return {
    left: Math.max(margin, left),
    top: Math.max(margin, top),
  };
}

export function useChatRollback() {
  const { t } = useI18n();
  const session = useSessionStore();
  const chat = useChatStore();
  const perm = usePermissionStore();

  const rollbackMenu = ref<{
    visible: boolean;
    x: number;
    y: number;
    snapshotId: string | null;
    rollbackMode: "both" | "session";
    messageId: string | null;
    fallbackSnapshotId: string;
  }>({
    visible: false,
    x: 0,
    y: 0,
    snapshotId: null,
    rollbackMode: "session",
    fallbackSnapshotId: "",
    messageId: null,
  });

  const rollbackConfirm = ref<{
    mode: "both" | "session";
    snapshotId: string;
    fallbackSnapshotId: string;
    messageId: string | null;
    title: string;
    message: string;
    files: PatchSummary[];
    warning?: string;
    /** 仅提示态：弹窗只用于告知结果（如"有对象未能恢复"），确认/关闭都只是关掉它 */
    noticeOnly?: boolean;
  } | null>(null);

  const rollbackPreview = ref<RollbackPreviewState>(createEmptyPreview());
  const rollbackLoading = ref(false);
  const rollbackError = ref("");

  const normalizeRollbackError = (err: unknown) => {
    const raw =
      typeof err === "string"
        ? err
        : err instanceof Error
          ? err.message
          : String(err || t("common.unknownError"));
    if (raw.includes(t("rollback.historyKept"))) {
      return raw;
    }
    return t("rollback.historyKeptSuffix", { error: raw });
  };

  const closeRollbackMenu = () => {
    rollbackMenu.value.visible = false;
  };

  const handleRollbackClickVue = (message: any, event: MouseEvent) => {
    const rollbackMode = message.rollbackMode === "both" ? "both" : "session";
    const rollbackCheckpointId = message.rollbackCheckpointId || "";
    const position = getRollbackMenuPosition(event.clientX, event.clientY, rollbackMode);

    rollbackMenu.value = {
      visible: true,
      x: position.left,
      y: position.top,
      snapshotId: rollbackCheckpointId,
      rollbackMode,
      messageId: message.messageId || null,
      fallbackSnapshotId: "",
    };
  };

  const executeRollback = async (mode: "both" | "session") => {
    rollbackError.value = "";
    rollbackPreview.value.error = "";
    if (mode === "both") {
      rollbackLoading.value = true;
      rollbackPreview.value.loading = true;
      try {
        const sessionId = session.activeSessionId;
        if (!sessionId) {
          alert(t("rollback.noSession"));
          return;
        }
        const result = await invoke<RollbackPreviewResult>(
          "preview_rollback_to_checkpoint_with_recall",
          {
            sessionId,
            checkpointId:
              rollbackMenu.value.snapshotId || rollbackMenu.value.fallbackSnapshotId || "",
            messageId: rollbackMenu.value.messageId,
          },
        );
        rollbackPreview.value = {
          loading: false,
          checkpointId: result.targetCheckpointId,
          files: result.files,
          targetCheckpointId: result.targetCheckpointId,
          checkpointLabel: result.checkpointLabel,
          error: "",
          initialState: result.initialState,
          roundNumber: result.roundNumber,
          affectedRounds: result.affectedRounds,
        };
      } catch (err) {
        const message = normalizeRollbackError(err);
        rollbackPreview.value.loading = false;
        rollbackPreview.value.error = message;
        rollbackError.value = message;
        alert(message);
        return;
      } finally {
        rollbackPreview.value.loading = false;
        rollbackLoading.value = false;
      }
    }

    const previewMessage =
      mode === "both"
        ? rollbackPreview.value.initialState
          ? t("rollback.confirmBothInitialMessage", { count: rollbackPreview.value.files.length })
          : t("rollback.confirmBothRangeMessage", {
              round: rollbackPreview.value.roundNumber,
              rounds: rollbackPreview.value.affectedRounds,
              count: rollbackPreview.value.files.length,
            })
        : t("rollback.confirmSessionMessage");

    rollbackConfirm.value = {
      mode,
      snapshotId: rollbackMenu.value.snapshotId || "",
      fallbackSnapshotId: rollbackMenu.value.fallbackSnapshotId,
      messageId: rollbackMenu.value.messageId,
      title: mode === "both" ? t("rollback.confirmBothTitle") : t("rollback.confirmSessionTitle"),
      message: previewMessage,
      files: mode === "both" ? rollbackPreview.value.files : [],
      warning: mode === "both" ? t("rollback.warningBoth") : t("rollback.warningSession"),
    };
  };

  const confirmRollback = async () => {
    if (!rollbackConfirm.value) return;
    if (rollbackConfirm.value.noticeOnly) {
      rollbackConfirm.value = null;
      return;
    }

    rollbackLoading.value = true;
    rollbackError.value = "";
    let restoreWarnings: string[] = [];
    try {
      const sessionId = session.activeSessionId;
      if (!sessionId) {
        alert(t("rollback.noSession"));
        return;
      }

      let recalledText: string | null = null;
      const rollbackMessageId = rollbackConfirm.value.messageId;
      if (
        rollbackConfirm.value.snapshotId ||
        rollbackConfirm.value.fallbackSnapshotId ||
        rollbackConfirm.value.mode === "both"
      ) {
        const result = await invoke<RollbackRecallResult>("rollback_to_checkpoint_with_recall", {
          sessionId,
          checkpointId:
            rollbackConfirm.value.snapshotId || rollbackConfirm.value.fallbackSnapshotId || "",
          rollbackFiles: rollbackConfirm.value.mode === "both",
          messageId: rollbackMessageId,
        });
        recalledText = result.recalledText;
        restoreWarnings = result.restoreWarnings ?? [];
      } else if (rollbackMessageId) {
        recalledText = await invoke<string | null>("recall_message", {
          sessionId,
          messageId: rollbackMessageId,
        });
      } else {
        recalledText = await invoke<string | null>("recall_last_message", { sessionId });
      }

      rollbackConfirm.value = restoreWarnings.length
        ? {
            mode: "session",
            snapshotId: "",
            fallbackSnapshotId: "",
            messageId: null,
            title: t("rollback.restoreWarningsTitle"),
            message: t("rollback.restoreWarningsMessage"),
            files: [],
            warning: restoreWarnings.join("\n"),
            noticeOnly: true,
          }
        : null;
      rollbackPreview.value = createEmptyPreview();
      closeRollbackMenu();

      if (recalledText) {
        chat.rollbackRecalledMessage = recalledText;
      }

      try {
        try {
          await chat.reloadSessionView(sessionId, "rollback");
        } catch (err) {
          console.error("加载会话消息失败:", err);
          session.setSessionLoadError(sessionId, true);
        }
        const planDocuments = await invoke<PlanDocument[]>("list_plan_documents", { sessionId });
        perm.planDocumentsBySession = {
          ...perm.planDocumentsBySession,
          [sessionId]: planDocuments,
        };
        delete perm.planProposals[sessionId];
        chat.triggerRender();
      } catch {
        session.resetSessionView(sessionId);
        chat.triggerRender();
      }
    } catch (err) {
      const message = normalizeRollbackError(err);
      console.error("回滚失败:", err);
      rollbackError.value = message;
    } finally {
      rollbackLoading.value = false;
    }
  };

  return {
    rollbackMenu,
    rollbackConfirm,
    rollbackPreview,
    rollbackLoading,
    rollbackError,
    closeRollbackMenu,
    handleRollbackClickVue,
    executeRollback,
    confirmRollback,
  };
}
