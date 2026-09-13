import { defineStore } from "pinia";
import { ref, computed } from "vue";
import type { PermissionRequest, PlanProposal, PlanDocument } from "../types";
import { useSessionStore } from "./session";

export const usePermissionStore = defineStore("permission", () => {
  /**
   * 待决策权限请求：按会话分组、按到达顺序排队。
   * 旧实现每会话只有一个槽位，多个子代理同时要权限时会互相覆盖，
   * 被覆盖的请求会一直等不到回答（而界面上根本看不到它）。
   */
  const permissionRequests = ref<Record<string, PermissionRequest[]>>({});
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

  return {
    permissionRequests,
    planProposals,
    planDocumentsBySession,
    permissionQueue,
    permissionRequest,
    planProposal,
    currentPlanDocuments,
    enqueuePermission,
    removePermission,
    clearPermissions,
    upsertPlanDocument,
    updatePlanProposalContent,
    updatePlanProposalStreamingContent,
    finalizePlanProposal,
  };
});
