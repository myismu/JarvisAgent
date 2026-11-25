import { defineStore } from "pinia";
import { ref, computed } from "vue";
import type {
  PermissionRequest,
  PlanProposal,
  PlanDocument,
  UserQuestionRequestEvent,
} from "../types";
import { useSessionStore } from "./session";

export const usePermissionStore = defineStore("permission", () => {
  /**
   * 待决策权限请求：按会话分组、按到达顺序排队。
   * 旧实现每会话只有一个槽位，多个子代理同时要权限时会互相覆盖，
   * 被覆盖的请求会一直等不到回答（而界面上根本看不到它）。
   */
  const permissionRequests = ref<Record<string, PermissionRequest[]>>({});
  /** 待用户回复的运行期咨询请求队列（按 sessionId 隔离，与权限队列同构） */
  const userQuestions = ref<Record<string, UserQuestionRequestEvent[]>>({});
  const planProposals = ref<Record<string, PlanProposal>>({});
  const planDocumentsBySession = ref<Record<string, PlanDocument[]>>({});

  const permissionQueue = computed<PermissionRequest[]>(() => {
    const session = useSessionStore();
    if (!session.activeSessionId) return [];
    return permissionRequests.value[session.activeSessionId] ?? [];
  });

  /// 当前会话最早的一个待决策请求（内联卡片展示这个）
  const permissionRequest = computed<PermissionRequest | null>(
    () => permissionQueue.value[0] ?? null
  );

  function enqueuePermission(request: PermissionRequest) {
    const sid = request.sessionId || useSessionStore().activeSessionId;
    if (!sid) return;
    const existing = permissionRequests.value[sid] ?? [];
    if (existing.some((item) => item.id === request.id)) return;
    permissionRequests.value = {
      ...permissionRequests.value,
      [sid]: [...existing, request],
    };
  }

  function removePermission(sessionId: string | null | undefined, requestId: string) {
    if (!sessionId) return;
    const existing = permissionRequests.value[sessionId] ?? [];
    const next = existing.filter((item) => item.id !== requestId);
    const map = { ...permissionRequests.value };
    if (next.length === 0) {
      delete map[sessionId];
    } else {
      map[sessionId] = next;
    }
    permissionRequests.value = map;
  }

  function clearPermissions(sessionId: string) {
    const map = { ...permissionRequests.value };
    delete map[sessionId];
    permissionRequests.value = map;
  }

  const userQuestionQueue = computed<UserQuestionRequestEvent[]>(() => {
    const session = useSessionStore();
    if (!session.activeSessionId) return [];
    return userQuestions.value[session.activeSessionId] ?? [];
  });

  /** 当前会话最早的一条待回复咨询请求 */
  const userQuestion = computed<UserQuestionRequestEvent | null>(
    () => userQuestionQueue.value[0] ?? null
  );

  function enqueueUserQuestion(request: UserQuestionRequestEvent) {
    const sid = request.sessionId || useSessionStore().activeSessionId;
    if (!sid) return;
    const existing = userQuestions.value[sid] ?? [];
    if (existing.some((item) => item.id === request.id)) return;
    userQuestions.value = {
      ...userQuestions.value,
      [sid]: [...existing, request],
    };
  }

  function removeUserQuestion(sessionId: string | null | undefined, requestId: string) {
    if (!sessionId) return;
    const existing = userQuestions.value[sessionId] ?? [];
    const next = existing.filter((item) => item.id !== requestId);
    const map = { ...userQuestions.value };
    if (next.length === 0) {
      delete map[sessionId];
    } else {
      map[sessionId] = next;
    }
    userQuestions.value = map;
  }

  function clearUserQuestions(sessionId: string) {
    const map = { ...userQuestions.value };
    delete map[sessionId];
    userQuestions.value = map;
  }

  /** 指定会话是否正在等待用户交互（权限审批或运行期咨询） */
  function isSessionWaitingForUser(sessionId: string | null | undefined): boolean {
    if (!sessionId) return false;
    return (
      (permissionRequests.value[sessionId]?.length ?? 0) > 0 ||
      (userQuestions.value[sessionId]?.length ?? 0) > 0
    );
  }

  const planProposal = computed(() => {
    const session = useSessionStore();
    if (!session.activeSessionId) return null;
    return planProposals.value[session.activeSessionId] ?? null;
  });

  const currentPlanDocuments = computed(() => {
    const session = useSessionStore();
    if (!session.activeSessionId) return [];
    return planDocumentsBySession.value[session.activeSessionId] ?? [];
  });

  function upsertPlanDocument(
    document: PlanDocument,
    fallbackSessionId?: string | null
  ) {
    const session = useSessionStore();
    const sessionId = document.sessionId || fallbackSessionId || session.activeSessionId;
    if (!sessionId) return;
    const existing = planDocumentsBySession.value[sessionId] ?? [];
    const next = [
      document,
      ...existing.filter((item) => item.id !== document.id),
    ].sort((a, b) => b.updatedAt - a.updatedAt);
    planDocumentsBySession.value = {
      ...planDocumentsBySession.value,
      [sessionId]: next,
    };
  }

  function updatePlanProposalContent(newContent: string) {
    const session = useSessionStore();
    const sid = session.activeSessionId;
    if (sid && planProposals.value[sid]) {
      planProposals.value[sid] = { ...planProposals.value[sid], content: newContent };
    }
  }

  function updatePlanProposalStreamingContent(sessionId: string, chunk: string) {
    if (!sessionId) return;
    const existing = planProposals.value[sessionId];
    if (existing) {
      planProposals.value = {
        ...planProposals.value,
        [sessionId]: { ...existing, content: existing.content + chunk },
      };
    } else {
      planProposals.value = {
        ...planProposals.value,
        [sessionId]: {
          id: `plan_stream_${Date.now()}`,
          title: "方案生成中...",
          content: chunk,
          sessionId,
        },
      };
    }
  }

  function finalizePlanProposal(sessionId: string, proposal: PlanProposal) {
    if (!sessionId) return;
    planProposals.value[sessionId] = proposal;
  }

  // 清理流式态方案预览（id = plan_stream_*）。
  // run 以任何方式结束（完成 / 看门狗截胡 / 取消 / 中断 / 错误）时，若方案
  // 始终没走到 plan-proposal 正式提交，"生成中..." 面板会永久卡死——
  // run 收口（applyAgentRunState）处统一调用此函数兜底。
  // 正式方案（plan_xxx）不受影响，只有流式占位会被清掉。
  function clearStreamingPlanProposal(sessionId: string) {
    if (!sessionId) return;
    const existing = planProposals.value[sessionId];
    if (existing && (existing.id ?? "").startsWith("plan_stream_")) {
      const next = { ...planProposals.value };
      delete next[sessionId];
      planProposals.value = next;
    }
  }

  return {
    permissionRequests,
    userQuestions,
    planProposals,
    planDocumentsBySession,
    permissionQueue,
    permissionRequest,
    userQuestionQueue,
    userQuestion,
    planProposal,
    currentPlanDocuments,
    enqueuePermission,
    removePermission,
    clearPermissions,
    enqueueUserQuestion,
    removeUserQuestion,
    clearUserQuestions,
    isSessionWaitingForUser,
    upsertPlanDocument,
    updatePlanProposalContent,
    updatePlanProposalStreamingContent,
    finalizePlanProposal,
    clearStreamingPlanProposal,
  };
});
