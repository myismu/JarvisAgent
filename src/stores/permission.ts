import { defineStore } from "pinia";
import { ref, computed } from "vue";
import type { PermissionRequest, PlanProposal, PlanDocument } from "../types";
import { useSessionStore } from "./session";

export const usePermissionStore = defineStore("permission", () => {
  const permissionRequests = ref<Record<string, PermissionRequest>>({});
  const planProposals = ref<Record<string, PlanProposal>>({});
  const planDocumentsBySession = ref<Record<string, PlanDocument[]>>({});

  const permissionRequest = computed(() => {
    const session = useSessionStore();
    if (!session.activeSessionId) return null;
    return permissionRequests.value[session.activeSessionId] ?? null;
  });

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

  return {
    permissionRequests,
    planProposals,
    planDocumentsBySession,
    permissionRequest,
    planProposal,
    currentPlanDocuments,
    upsertPlanDocument,
    updatePlanProposalContent,
  };
});
