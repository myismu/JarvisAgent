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
import { extractCommandErrorMessage } from "../../utils/commandError";
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

  /** 编辑重发二选一弹窗状态（仅当消息存在关联代码变更 `rollbackMode === 'both'` 时打开） */
  const editResendConfirm = ref<{
    snapshotId: string;
    messageId: string | null;
    editedText: string;
    images: string[];
    title: string;
    message: string;
    files: PatchSummary[];
    warning: string;
  } | null>(null);

  const rollbackPreview = ref<RollbackPreviewState>(createEmptyPreview());
  const rollbackLoading = ref(false);
  const rollbackError = ref("");

  const normalizeRollbackError = (err: unknown) => {
    const raw = extractCommandErrorMessage(err) || t("common.unknownError");
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

  /**
   * 底层撤回执行器：供「普通撤回」与「原地编辑后撤回重发」共用，确保后端调用、
   * 视图重载、方案文档刷新与恢复警告处理保持唯一口径。
   */
  const performRollbackCore = async (params: {
    mode: "both" | "session";
    snapshotId: string;
    fallbackSnapshotId: string;
    messageId: string | null;
    fillInputWithRecalled: boolean;
  }): Promise<{ ok: boolean; restoreWarnings: string[] }> => {
    rollbackLoading.value = true;
    rollbackError.value = "";
    let restoreWarnings: string[] = [];
    try {
      const sessionId = session.activeSessionId;
      if (!sessionId) {
        alert(t("rollback.noSession"));
        return { ok: false, restoreWarnings: [] };
      }

      let recalledText: string | null = null;
      if (params.snapshotId || params.fallbackSnapshotId || params.mode === "both") {
        const result = await invoke<RollbackRecallResult>("rollback_to_checkpoint_with_recall", {
          sessionId,
          checkpointId: params.snapshotId || params.fallbackSnapshotId || "",
          rollbackFiles: params.mode === "both",
          messageId: params.messageId,
        });
        recalledText = result.recalledText;
        restoreWarnings = result.restoreWarnings ?? [];
      } else if (params.messageId) {
        recalledText = await invoke<string | null>("recall_message", {
          sessionId,
          messageId: params.messageId,
        });
      } else {
        recalledText = await invoke<string | null>("recall_last_message", { sessionId });
      }

      rollbackPreview.value = createEmptyPreview();
      closeRollbackMenu();

      if (params.fillInputWithRecalled && recalledText) {
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

      return { ok: true, restoreWarnings };
    } catch (err) {
      const message = normalizeRollbackError(err);
      console.error("回滚失败:", err);
      rollbackError.value = message;
      return { ok: false, restoreWarnings: [] };
    } finally {
      rollbackLoading.value = false;
    }
  };

  const confirmRollback = async () => {
    if (!rollbackConfirm.value) return;
    if (rollbackConfirm.value.noticeOnly) {
      rollbackConfirm.value = null;
      return;
    }

    const { ok, restoreWarnings } = await performRollbackCore({
      mode: rollbackConfirm.value.mode,
      snapshotId: rollbackConfirm.value.snapshotId,
      fallbackSnapshotId: rollbackConfirm.value.fallbackSnapshotId,
      messageId: rollbackConfirm.value.messageId,
      fillInputWithRecalled: true,
    });
    if (!ok) return;

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
  };

  /**
   * 提交原地编辑后的用户消息：
   * - 无关联代码变更（`rollbackMode !== 'both'`）：直接执行仅会话撤回并重新发送；
   * - 有关联代码变更（`rollbackMode === 'both'`）：先查受影响文件预览，弹出二选一确认框，
   *   由用户决定「仅撤回会话并重发」还是「撤回会话与代码并重发」。
   *
   * 返回 `true` 表示已经直接完成撤回并触发重发（调用方可退出编辑态）；
   * 返回 `false` 表示已打开弹窗等待用户选择或发生错误（调用方应保持编辑态）。
   */
  const submitEditedMessage = async (
    message: any,
    editedText: string,
    onDone?: () => void,
  ): Promise<boolean> => {
    const rollbackMode = message.rollbackMode === "both" ? "both" : "session";
    const snapshotId = message.rollbackCheckpointId || "";
    const messageId = message.messageId || null;
    const images: string[] = Array.isArray(message.images) ? [...message.images] : [];

    rollbackError.value = "";

    // 无代码改动：按需求直接撤回会话并重发，不弹二次确认
    if (rollbackMode !== "both") {
      const { ok } = await performRollbackCore({
        mode: "session",
        snapshotId,
        fallbackSnapshotId: "",
        messageId,
        fillInputWithRecalled: false,
      });
      if (!ok) {
        if (rollbackError.value) alert(rollbackError.value);
        return false;
      }
      onDone?.();
      void chat.sendToJarvis(editedText, undefined, images.length > 0 ? images : undefined);
      return true;
    }

    // 有关联代码改动：先拉取文件变更预览，再弹二选一确认框
    const sessionId = session.activeSessionId;
    if (!sessionId) {
      alert(t("rollback.noSession"));
      return false;
    }

    rollbackLoading.value = true;
    try {
      const result = await invoke<RollbackPreviewResult>(
        "preview_rollback_to_checkpoint_with_recall",
        {
          sessionId,
          checkpointId: snapshotId,
          messageId,
        },
      );
      const previewMessage = result.initialState
        ? t("rollback.editResendBothInitialMessage", { count: result.files.length })
        : t("rollback.editResendBothRangeMessage", {
            round: result.roundNumber,
            rounds: result.affectedRounds,
            count: result.files.length,
          });
      editResendConfirm.value = {
        snapshotId,
        messageId,
        editedText,
        images,
        title: t("rollback.editResendTitle"),
        message: previewMessage,
        files: result.files,
        warning: t("rollback.warningBoth"),
      };
      return false;
    } catch (err) {
      const msg = normalizeRollbackError(err);
      rollbackError.value = msg;
      alert(msg);
      return false;
    } finally {
      rollbackLoading.value = false;
    }
  };

  /**
   * 确认二选一弹窗中的编辑重发模式（`'session'` 或 `'both'`）。
   * 若代码回滚返回了 `restoreWarnings`，则先弹出警告并把修改后的文本暂存回底部输入框，
   * 不盲目自动重发，避免在不一致的工作区状态下继续执行。
   */
  const confirmEditResend = async (mode: "session" | "both", onDone?: () => void) => {
    const pending = editResendConfirm.value;
    if (!pending) return;

    const { ok, restoreWarnings } = await performRollbackCore({
      mode,
      snapshotId: pending.snapshotId,
      fallbackSnapshotId: "",
      messageId: pending.messageId,
      fillInputWithRecalled: false,
    });
    if (!ok) return;

    editResendConfirm.value = null;
    onDone?.();

    if (restoreWarnings.length > 0) {
      // 有文件未能恢复：暂停自动重发，把用户刚改好的文本放进底部输入框方便确认后手动发送
      chat.rollbackRecalledMessage = pending.editedText;
      rollbackConfirm.value = {
        mode: "session",
        snapshotId: "",
        fallbackSnapshotId: "",
        messageId: null,
        title: t("rollback.restoreWarningsTitle"),
        message: t("rollback.restoreWarningsMessage"),
        files: [],
        warning: restoreWarnings.join("\n"),
        noticeOnly: true,
      };
      return;
    }

    void chat.sendToJarvis(
      pending.editedText,
      undefined,
      pending.images.length > 0 ? pending.images : undefined,
    );
  };

  const cancelEditResend = () => {
    if (rollbackLoading.value) return;
    editResendConfirm.value = null;
    rollbackError.value = "";
  };

  return {
    rollbackMenu,
    rollbackConfirm,
    editResendConfirm,
    rollbackPreview,
    rollbackLoading,
    rollbackError,
    closeRollbackMenu,
    handleRollbackClickVue,
    executeRollback,
    confirmRollback,
    submitEditedMessage,
    confirmEditResend,
    cancelEditResend,
  };
}
