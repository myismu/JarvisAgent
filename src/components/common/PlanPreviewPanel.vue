<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { usePermissionStore } from '../../stores/permission';
import { useSessionStore } from '../../stores/session';
import { useChatStore } from '../../stores/chat';
import StreamingMarkdown from './StreamingMarkdown.vue';

const { t } = useI18n();

const perm = usePermissionStore();
const session = useSessionStore();
const chat = useChatStore();

const isEditing = ref(false);
const editedContent = ref('');
const revisionFeedback = ref('');
const isRequestingRevision = ref(false);
const isMinimized = ref(true);
const isResolving = ref(false);
const selectedPlanId = ref<string | null>(null);
const bodyRef = ref<HTMLElement | null>(null);
const isAtBottom = ref(true);

function checkAtBottom() {
  const el = bodyRef.value;
  if (!el) return;
  isAtBottom.value = el.scrollHeight - el.scrollTop - el.clientHeight <= 35;
}

function scrollToBottom() {
  const el = bodyRef.value;
  if (!el) return;
  el.scrollTop = el.scrollHeight;
}

let lastSessionId: string | null = null;

watch(() => session.activeSessionId, (newSid) => {
  if (newSid && newSid !== lastSessionId) {
    lastSessionId = newSid;
    // 切换会话：最小化
    isMinimized.value = true;
    selectedPlanId.value = null;
  }
});

watch(() => perm.planProposal, (newVal, oldVal) => {
  if (newVal) {
    // Agent 触发生成新方案：自动展开
    editedContent.value = newVal.content;
    isEditing.value = false;
    if (!oldVal || oldVal.id !== newVal.id) {
      isMinimized.value = false;
      selectedPlanId.value = null;
    }
  }
});

const activeProposal = computed(() => perm.planProposal);
const planDocs = computed(() => perm.currentPlanDocuments);

const viewedDocument = computed(() => {
  if (!selectedPlanId.value) return null;
  return planDocs.value.find(d => d.id === selectedPlanId.value) ?? null;
});

const activeContent = computed(() => {
  if (activeProposal.value) {
    return isEditing.value ? editedContent.value : activeProposal.value.content;
  }
  if (viewedDocument.value) {
    return viewedDocument.value.content;
  }
  return '';
});

const activeTitle = computed(() => {
  if (activeProposal.value) return activeProposal.value.title;
  if (viewedDocument.value) return viewedDocument.value.title;
  return '';
});

const activeStatus = computed(() => {
  if (activeProposal.value) return 'pending';
  if (viewedDocument.value) return viewedDocument.value.status;
  return null;
});

const showPanel = computed(() => !!activeProposal.value || isMinimized.value === false);
const hasPlanHistory = computed(() => planDocs.value.length > 0);

// 如果没有活跃提案但存在历史计划，自动选中最近的一个用于展示
watch([activeProposal, planDocs], ([proposal, docs]) => {
  if (!proposal && docs.length > 0 && !selectedPlanId.value) {
    selectedPlanId.value = docs[0].id;
  }
}, { immediate: true });



watch(activeContent, () => {
  if (isStreaming.value && isAtBottom.value) {
    scrollToBottom();
  }
});

const planStats = computed(() => {
  const source = activeContent.value.trim();
  if (!source) return { lines: 0, sections: 0 };

  const lines = source.split(/\r?\n/).filter((line) => line.trim().length > 0).length;
  const sections = source.match(/^#{1,3}\s+/gm)?.length ?? 0;
  return { lines, sections };
});

const isStreaming = computed(() => {
  const proposal = activeProposal.value;
  if (!proposal) return false;
  return proposal.title === '方案生成中...' || proposal.id?.startsWith('plan_stream_');
});

const toggleEdit = () => {
  if (!activeProposal.value) return;

  if (isEditing.value) {
    perm.updatePlanProposalContent(editedContent.value);
  } else {
    editedContent.value = activeProposal.value.content;
  }

  isEditing.value = !isEditing.value;
};

const cancelEdit = () => {
  if (activeProposal.value) {
    editedContent.value = activeProposal.value.content;
  }
  isEditing.value = false;
};

const handleApprove = async () => {
  const proposal = activeProposal.value;
  const doc = viewedDocument.value;
  if (isResolving.value || (!proposal && !doc)) return;
  isResolving.value = true;
  const planId = proposal?.id || doc!.id;
  const title = proposal?.title || doc!.title;
  const content = isEditing.value ? editedContent.value : (proposal?.content || doc!.content);
  try {
    await chat.resolvePlan('allow', content, proposal ? undefined : (doc ?? undefined));
    perm.upsertPlanDocument({
      id: planId,
      sessionId: proposal?.sessionId || doc!.sessionId || '',
      title,
      content,
      status: 'approved',
      createdAt: doc?.createdAt || Date.now(),
      updatedAt: Date.now(),
      decidedAt: Date.now(),
    });
    selectedPlanId.value = planId;
    isEditing.value = false;
    isRequestingRevision.value = false;
    revisionFeedback.value = '';
    void chat.continueFromApprovedPlan(title, content);
  } finally {
    isResolving.value = false;
  }
};

const handleReject = async () => {
  const proposal = activeProposal.value;
  const doc = viewedDocument.value;
  if (isResolving.value || (!proposal && !doc)) return;
  if (!isRequestingRevision.value) {
    isRequestingRevision.value = true;
    return;
  }
  const feedback = revisionFeedback.value.trim();
  if (!feedback) return;

  isResolving.value = true;
  const planId = proposal?.id || doc!.id;
  const title = proposal?.title || doc!.title;
  const content = proposal?.content || doc!.content;
  try {
    await chat.resolvePlan('reject', feedback, proposal ? undefined : (doc ?? undefined));
    perm.upsertPlanDocument({
      id: planId,
      sessionId: proposal?.sessionId || doc!.sessionId || '',
      title,
      content,
      status: 'revision_requested',
      createdAt: doc?.createdAt || Date.now(),
      updatedAt: Date.now(),
      decidedAt: Date.now(),
    });
    selectedPlanId.value = planId;
    isEditing.value = false;
    isRequestingRevision.value = false;
    revisionFeedback.value = '';
    void chat.requestPlanRevision(title, feedback);
  } finally {
    isResolving.value = false;
  }
};

const selectPlan = (id: string) => {
  selectedPlanId.value = id;
  isEditing.value = false;
  isRequestingRevision.value = false;
  revisionFeedback.value = '';
};

const toggleMinimize = () => {
  const next = !isMinimized.value;
  isMinimized.value = next;
  // 如果展开但没有活跃提案，查看最新的历史计划
  if (!next && !activeProposal.value && planDocs.value.length > 0) {
    selectedPlanId.value = planDocs.value[0].id;
  }
};
</script>

<template>
  <Transition name="plan-slide">
    <div v-if="showPanel || hasPlanHistory" class="plan-overlay" :class="{ minimized: isMinimized }" role="dialog" aria-modal="true" aria-labelledby="plan-review-heading">
      <aside v-if="!isMinimized" class="plan-panel">
        <header class="plan-header">
          <div class="plan-header-main">
            <div class="plan-icon-shell" aria-hidden="true">
              <svg viewBox="0 0 24 24" width="18" height="18" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <path d="M14 2H7a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7z"></path>
                <polyline points="14 2 14 7 19 7"></polyline>
                <line x1="9" y1="13" x2="15" y2="13"></line>
                <line x1="9" y1="17" x2="13" y2="17"></line>
              </svg>
            </div>
            <div>
              <span class="plan-kicker">{{ t('plan.kicker') }}</span>
              <h2 id="plan-review-heading">{{ activeStatus === 'approved' ? t('plan.approvedTitle') : activeStatus === 'rejected' ? t('plan.rejectedTitle') : t('plan.title') }}</h2>
            </div>
          </div>

          <div class="plan-header-actions">
            <button class="plan-icon-btn plan-minimize-btn" @click="toggleMinimize" :title="t('plan.minimize')" :aria-label="t('plan.minimize')">
              <svg viewBox="0 0 24 24" width="15" height="15" stroke="currentColor" stroke-width="2.2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <line x1="5" y1="12" x2="19" y2="12"></line>
              </svg>
            </button>
          </div>
        </header>

        <section class="plan-summary">
          <div class="plan-summary-copy">
            <span class="plan-label">{{ activeStatus === 'pending' ? t('plan.pendingPlan') : activeStatus === 'approved' ? t('plan.approvedPlan') : t('plan.rejectedPlan') }}</span>
            <h3 class="plan-title">{{ activeTitle }}</h3>
          </div>
          <div class="plan-status-stack">
            <span class="plan-status-pill" :class="{ streaming: isStreaming, approved: activeStatus === 'approved', rejected: activeStatus === 'rejected' }">
              <span class="plan-status-dot"></span>
              {{ isStreaming ? t('plan.generating') : activeStatus === 'approved' ? t('plan.approved') : activeStatus === 'rejected' ? t('plan.rejected') : t('plan.waiting') }}
            </span>
            <span v-if="!isStreaming" class="plan-stats">{{ t('plan.stats', { sections: planStats.sections, lines: planStats.lines }) }}</span>
            <span v-else class="plan-stats">{{ t('plan.generatingHint') }}</span>
          </div>
        </section>

        <div v-if="planDocs.length > 1" class="plan-doc-selector">
          <button
            v-for="doc in planDocs"
            :key="doc.id"
            class="plan-doc-chip"
            :class="{ active: (selectedPlanId || planDocs[0]?.id) === doc.id }"
            :title="doc.title"
            @click="selectPlan(doc.id)"
          >
            <span class="plan-doc-chip-status" :class="doc.status"></span>
            {{ doc.title.length > 40 ? doc.title.slice(0, 40) + '...' : doc.title }}
          </button>
        </div>

        <div class="plan-toolbar">
          <div class="plan-toolbar-copy">
            <span class="plan-toolbar-title">{{ isEditing ? t('plan.editMode') : t('plan.previewMode') }}</span>
            <span class="plan-toolbar-subtitle">{{ isEditing ? t('plan.editSubtitle') : t('plan.previewSubtitle') }}</span>
          </div>

          <div class="plan-edit-actions">
            <template v-if="!activeProposal">
              <button class="plan-mini-btn" @click="isMinimized = true; selectedPlanId = null" :title="t('plan.close')">
                {{ t('plan.close') }}
              </button>
            </template>
            <template v-else>
              <button
                v-if="!isEditing && !isStreaming"
                class="plan-mini-btn"
                @click="toggleEdit"
                :title="t('plan.edit')"
              >
                <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M12 20h9"></path>
                  <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L8 18l-4 1 1-4Z"></path>
                </svg>
                {{ t('plan.edit') }}
              </button>
              <template v-else-if="isEditing">
                <button class="plan-mini-btn plan-mini-btn-muted" @click="cancelEdit" :title="t('plan.cancelEdit')">
                  {{ t('plan.cancel') }}
                </button>
                <button class="plan-mini-btn plan-mini-btn-primary" @click="toggleEdit" :title="t('plan.saveEdit')">
                  {{ t('plan.save') }}
                </button>
              </template>
            </template>
          </div>
        </div>

        <main
          ref="bodyRef"
          class="plan-body"
          :class="{ 'is-editing': isEditing }"
          @scroll="checkAtBottom"
        >
          <textarea
            v-if="isEditing"
            v-model="editedContent"
            class="plan-editor"
            :placeholder="t('plan.placeholder')"
          ></textarea>
          <StreamingMarkdown v-else class="plan-markdown" :content="activeContent" />
          <div v-if="isRequestingRevision" class="plan-revision-box">
            <label class="plan-revision-label">修改意见</label>
            <textarea
              v-model="revisionFeedback"
              class="plan-revision-input"
              placeholder="说明你希望 Agent 如何修改方案，例如：改技术栈、删减范围、重新拆分任务..."
            ></textarea>
          </div>
          <Transition name="scroll-btn">
            <button
              v-if="isStreaming && !isAtBottom"
              class="plan-scroll-bottom"
              @click="scrollToBottom"
              :title="t('plan.scrollToBottom')"
            >
              <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="6 9 12 15 18 9"></polyline>
              </svg>
            </button>
          </Transition>
        </main>

        <footer v-if="(activeProposal || activeStatus === 'pending') && !isStreaming" class="plan-actions">
          <button class="plan-btn plan-btn-reject" @click="handleReject" :disabled="isResolving || (isRequestingRevision && !revisionFeedback.trim())">
            <svg viewBox="0 0 24 24" width="15" height="15" stroke="currentColor" stroke-width="2.2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="10"></circle>
              <line x1="15" y1="9" x2="9" y2="15"></line>
              <line x1="9" y1="9" x2="15" y2="15"></line>
            </svg>
            {{ isRequestingRevision ? '提交修改意见' : t('plan.reject') }}
          </button>
          <button class="plan-btn plan-btn-approve" @click="handleApprove" :disabled="isResolving">
            <svg viewBox="0 0 24 24" width="15" height="15" stroke="currentColor" stroke-width="2.2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <path d="M20 6 9 17l-5-5"></path>
            </svg>
            {{ t('plan.approve') }}
          </button>
        </footer>
        <footer v-else-if="activeProposal && isStreaming" class="plan-actions plan-actions-streaming">
          <span class="streaming-hint">
            <span class="streaming-dot"></span>
            {{ t('plan.streamingHint') }}
          </span>
        </footer>
      </aside>

      <!-- 最小化悬浮按钮 -->
      <button v-else class="plan-minimized-float" @click="toggleMinimize" :title="t('plan.restore')">
        <div class="plan-minimized-icon">
          <svg viewBox="0 0 24 24" width="20" height="20" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <path d="M14 2H7a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7z"></path>
            <polyline points="14 2 14 7 19 7"></polyline>
          </svg>
        </div>
        <span class="plan-minimized-label">{{ activeProposal?.title || viewedDocument?.title || planDocs[0]?.title || t('plan.title') }}</span>
        <span v-if="isStreaming" class="streaming-indicator-mini">
          <span class="streaming-dot-mini"></span>
        </span>
      </button>
    </div>
  </Transition>
</template>

<style scoped>
.plan-overlay {
  position: absolute;
  inset: 0;
  display: flex;
  justify-content: flex-end;
  padding: 10px;
  pointer-events: none;
  z-index: 200;
}

.plan-overlay.minimized {
  justify-content: flex-end;
  align-items: flex-end;
  padding: 16px;
  pointer-events: none;
}

.plan-panel {
  pointer-events: auto;
  position: relative;
  isolation: isolate;
  width: min(640px, 92vw);
  height: 100%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  color: var(--text-main);
  background: var(--glass-bg-heavy);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-xl);
  box-shadow: var(--glass-shadow), -18px 0 56px rgba(0, 0, 0, 0.15);
  backdrop-filter: blur(var(--glass-blur-heavy)) saturate(1.28);
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy)) saturate(1.28);
}

.plan-panel::after {
  content: "";
  position: absolute;
  top: 12px;
  bottom: 12px;
  left: 0;
  width: 1px;
  pointer-events: none;
  background: linear-gradient(180deg, transparent, var(--accent-blue), transparent);
  opacity: 0.5;
}

.plan-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 14px;
  padding: 16px 18px 14px;
  border-bottom: 1px solid var(--glass-border-subtle);
  flex-shrink: 0;
}

.plan-header-main {
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 12px;
}

.plan-header-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}

.plan-icon-shell {
  width: 38px;
  height: 38px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  color: var(--accent-blue);
  border: 1px solid var(--glass-border);
  border-radius: 12px;
  background: var(--glass-bg-light);
  box-shadow: var(--shadow-sm);
}

.plan-kicker {
  display: block;
  margin-bottom: 1px;
  color: var(--text-muted);
  font-size: 0.68rem;
  font-weight: 700;
  line-height: 1.1;
  letter-spacing: 0;
}

.plan-header h2 {
  margin: 0;
  color: var(--text-main);
  font-size: 1rem;
  font-weight: 720;
  line-height: 1.25;
}

.plan-icon-btn {
  width: 32px;
  height: 32px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  color: var(--text-muted);
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  background: var(--glass-bg-light);
  cursor: pointer;
  transition:
    color var(--transition-fast),
    background var(--transition-fast),
    border-color var(--transition-fast),
    transform var(--transition-fast);
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
}

.plan-icon-btn:hover {
  color: var(--accent-blue);
  border-color: var(--accent-blue);
  background: color-mix(in srgb, var(--accent-blue) 10%, transparent);
  transform: translateY(-1px);
}

.plan-summary {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
  padding: 18px 20px 16px;
  border-bottom: 1px solid var(--glass-border-subtle);
  background: var(--glass-bg-light);
  flex-shrink: 0;
}

.plan-summary-copy {
  min-width: 0;
}

.plan-label {
  display: block;
  margin-bottom: 5px;
  color: var(--text-muted);
  font-size: 0.72rem;
  font-weight: 700;
  line-height: 1.2;
}

.plan-title {
  margin: 0;
  color: var(--text-main);
  font-size: 1.18rem;
  font-weight: 760;
  line-height: 1.35;
  overflow-wrap: anywhere;
}

.plan-status-stack {
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 7px;
  flex-shrink: 0;
}

.plan-status-pill {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  min-height: 28px;
  padding: 4px 10px;
  color: var(--text-warning);
  font-size: 0.76rem;
  font-weight: 700;
  white-space: nowrap;
  border: 1px solid var(--border-warning);
  border-radius: 999px;
  background: color-mix(in srgb, var(--surface-warning) 74%, transparent);
}

.plan-status-pill.streaming {
  color: var(--accent-blue);
  border-color: var(--accent-blue);
  background: color-mix(in srgb, var(--accent-blue) 10%, transparent);
}
.plan-status-pill.approved {
  color: var(--accent-green);
  border-color: var(--accent-green);
  background: color-mix(in srgb, var(--accent-green) 10%, transparent);
}
.plan-status-pill.rejected {
  color: var(--accent-red);
  border-color: var(--accent-red);
  background: color-mix(in srgb, var(--accent-red) 10%, transparent);
}

.plan-status-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--accent-yellow);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent-yellow) 20%, transparent);
}

.plan-status-pill.streaming .plan-status-dot {
  background: var(--accent-blue);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent-blue) 20%, transparent);
  animation: pulse-dot 1.5s ease-in-out infinite;
}

@keyframes pulse-dot {
  0%, 100% { opacity: 1; transform: scale(1); }
  50% { opacity: 0.5; transform: scale(0.8); }
}

.plan-stats {
  color: var(--text-muted);
  font-size: 0.72rem;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.plan-doc-selector {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 10px 20px;
  border-bottom: 1px solid var(--glass-border-subtle);
  background: color-mix(in srgb, var(--bg-panel) 30%, transparent);
  flex-shrink: 0;
}

.plan-doc-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px;
  border: 1px solid var(--glass-border-subtle);
  border-radius: 999px;
  background: var(--glass-bg-light);
  color: var(--text-muted);
  font-size: 0.72rem;
  cursor: pointer;
  transition: all var(--transition-fast);
  max-width: 280px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.plan-doc-chip:hover {
  border-color: var(--accent-blue);
  color: var(--text-main);
}
.plan-doc-chip.active {
  border-color: var(--accent-blue);
  color: var(--accent-blue);
  background: color-mix(in srgb, var(--accent-blue) 10%, transparent);
  font-weight: 600;
}

.plan-doc-chip-status {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  flex-shrink: 0;
}
.plan-doc-chip-status.approved { background: var(--accent-green); }
.plan-doc-chip-status.rejected { background: var(--accent-red); }
.plan-doc-chip-status.pending  { background: var(--accent-yellow); }

.plan-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 12px 20px;
  border-bottom: 1px solid var(--glass-border-subtle);
  background: color-mix(in srgb, var(--bg-panel) 40%, transparent);
  flex-shrink: 0;
}

.plan-toolbar-copy {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.plan-toolbar-title {
  color: var(--text-main);
  font-size: 0.84rem;
  font-weight: 700;
}

.plan-toolbar-subtitle {
  color: var(--text-muted);
  font-size: 0.72rem;
  line-height: 1.35;
}

.plan-edit-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  flex-shrink: 0;
}

.plan-mini-btn {
  min-height: 30px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 5px 11px;
  color: var(--text-main);
  font-size: 0.78rem;
  font-weight: 700;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
  background: var(--glass-bg-light);
  cursor: pointer;
  transition:
    color var(--transition-fast),
    background var(--transition-fast),
    border-color var(--transition-fast),
    transform var(--transition-fast),
    box-shadow var(--transition-fast);
  backdrop-filter: blur(10px);
  -webkit-backdrop-filter: blur(10px);
}

.plan-mini-btn:hover {
  color: var(--accent-blue);
  border-color: var(--accent-blue);
  background: color-mix(in srgb, var(--accent-blue) 10%, transparent);
  transform: translateY(-1px);
}

.plan-mini-btn-muted:hover {
  color: var(--accent-red);
  border-color: var(--border-danger);
  background: color-mix(in srgb, var(--accent-red) 10%, transparent);
}

.plan-mini-btn-primary {
  color: var(--text-inverse);
  border-color: var(--accent-blue);
  background: var(--accent-blue);
  box-shadow: 0 4px 12px color-mix(in srgb, var(--accent-blue) 30%, transparent);
}

.plan-mini-btn-primary:hover {
  color: var(--text-inverse);
  background: var(--accent-blue-hover);
}

.plan-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 18px 20px 20px;
  color: var(--text-main);
  position: relative;
}

.plan-scroll-bottom {
  position: absolute;
  bottom: 24px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 10;
  width: 36px;
  height: 36px;
  border-radius: 50%;
  border: 1px solid var(--glass-border);
  background: var(--glass-bg-heavy);
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  color: var(--accent-blue);
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  box-shadow: var(--shadow-md);
  transition: all var(--transition-fast);
}
.plan-scroll-bottom:hover {
  background: var(--glass-bg);
  border-color: var(--accent-blue);
  box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.2);
  transform: translateX(-50%) scale(1.08);
}
.plan-scroll-bottom:active {
  transform: translateX(-50%) scale(0.95);
}

.plan-body.is-editing {
  padding: 14px;
}

.plan-markdown {
  min-height: 100%;
  padding: 18px 18px 26px;
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-lg);
  background: var(--glass-bg-light);
  box-shadow: var(--shadow-sm);
}

.plan-revision-box {
  margin-top: 14px;
  padding: 14px;
  border: 1px solid var(--border-danger);
  border-radius: var(--radius-lg);
  background: color-mix(in srgb, var(--accent-red) 8%, transparent);
}

.plan-revision-label {
  display: block;
  margin-bottom: 8px;
  color: var(--text-main);
  font-size: 0.82rem;
  font-weight: 760;
}

.plan-revision-input {
  width: 100%;
  min-height: 96px;
  resize: vertical;
  outline: none;
  color: var(--text-main);
  font-family: var(--font-sans);
  font-size: 0.88rem;
  line-height: 1.6;
  padding: 12px;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
  background: var(--glass-bg-light);
}

.plan-revision-input:focus {
  border-color: var(--accent-red);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent-red) 16%, transparent);
}

.plan-editor {
  width: 100%;
  height: 100%;
  min-height: 360px;
  resize: none;
  outline: none;
  color: var(--text-main);
  font-family: var(--font-mono);
  font-size: 0.88rem;
  line-height: 1.7;
  padding: 16px;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
  background: var(--glass-bg-light);
  box-shadow: inset 0 1px 3px rgba(0, 0, 0, 0.05);
  backdrop-filter: blur(14px) saturate(1.12);
  -webkit-backdrop-filter: blur(14px) saturate(1.12);
}

.plan-editor::placeholder {
  color: var(--text-muted);
}

.plan-editor:focus {
  border-color: var(--accent-blue);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent-blue) 20%, transparent);
}

.plan-markdown :deep(h1) {
  margin: 0 0 14px;
  padding-bottom: 10px;
  color: var(--text-main);
  font-size: 1.32rem;
  font-weight: 760;
  line-height: 1.32;
  border-bottom: 1px solid var(--glass-border-subtle);
}

.plan-markdown :deep(h2) {
  margin: 22px 0 10px;
  color: var(--accent-blue);
  font-size: 1.06rem;
  font-weight: 740;
  line-height: 1.35;
}

.plan-markdown :deep(h3) {
  margin: 18px 0 8px;
  color: var(--text-main);
  font-size: 0.96rem;
  font-weight: 720;
  line-height: 1.4;
}

.plan-markdown :deep(p) {
  margin: 9px 0;
  color: var(--text-main);
  font-size: 0.9rem;
  line-height: 1.72;
}

.plan-markdown :deep(ul),
.plan-markdown :deep(ol) {
  margin: 9px 0;
  padding-left: 24px;
}

.plan-markdown :deep(li) {
  margin: 5px 0;
  color: var(--text-main);
  line-height: 1.68;
}

.plan-markdown :deep(li::marker) {
  color: var(--accent-blue);
}

.plan-markdown :deep(a) {
  color: var(--accent-blue);
  text-decoration: none;
}

.plan-markdown :deep(a:hover) {
  text-decoration: underline;
}

.plan-markdown :deep(code) {
  padding: 2px 6px;
  color: var(--text-main);
  font-family: var(--font-mono);
  font-size: 0.85em;
  border: 1px solid var(--glass-border-subtle);
  border-radius: 5px;
  background: color-mix(in srgb, var(--text-muted) 15%, transparent);
}

.plan-markdown :deep(pre) {
  margin: 12px 0;
  padding: 14px 16px;
  overflow-x: auto;
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--bg-dark) 80%, var(--bg-sidebar));
  box-shadow: inset 0 1px 3px rgba(0, 0, 0, 0.1);
}

.plan-markdown :deep(pre code) {
  padding: 0;
  color: var(--text-main);
  border: 0;
  background: transparent;
}

.plan-markdown :deep(blockquote) {
  margin: 12px 0;
  padding: 10px 14px;
  color: var(--text-muted);
  border-left: 3px solid var(--accent-blue);
  border-radius: 0 var(--radius-md) var(--radius-md) 0;
  background: color-mix(in srgb, var(--accent-blue) 10%, transparent);
}

.plan-markdown :deep(strong) {
  color: var(--text-main);
  font-weight: 760;
}

.plan-markdown :deep(table) {
  width: 100%;
  margin: 12px 0;
  border-collapse: separate;
  border-spacing: 0;
  overflow: hidden;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
}

.plan-markdown :deep(th),
.plan-markdown :deep(td) {
  padding: 9px 12px;
  text-align: left;
  border-bottom: 1px solid var(--glass-border-subtle);
  color: var(--text-main);
}

.plan-markdown :deep(th) {
  font-weight: 730;
  background: color-mix(in srgb, var(--bg-panel) 50%, transparent);
}

.plan-markdown :deep(tr:last-child td) {
  border-bottom: 0;
}

.plan-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 18px;
  flex-shrink: 0;
  border-top: 1px solid var(--glass-border-subtle);
  background: var(--glass-bg-light);
  backdrop-filter: blur(18px) saturate(1.15);
  -webkit-backdrop-filter: blur(18px) saturate(1.15);
}

.plan-actions-streaming {
  justify-content: center;
}

.streaming-hint {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  color: var(--accent-blue);
  font-size: 0.82rem;
  font-weight: 700;
}

.streaming-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--accent-blue);
  animation: pulse-dot 1.5s ease-in-out infinite;
}

.plan-btn {
  min-height: 34px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 7px;
  padding: 7px 14px;
  border: 1px solid var(--glass-border);
  border-radius: 7px;
  font-size: 0.82rem;
  font-weight: 700;
  cursor: pointer;
  transition:
    background var(--transition-fast),
    border-color var(--transition-fast),
    box-shadow var(--transition-fast),
    transform var(--transition-fast),
    color var(--transition-fast);
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
}

.plan-btn:hover {
  transform: translateY(-1px);
}

.plan-btn:active {
  transform: translateY(0);
}

.plan-btn-reject {
  color: var(--text-main);
  border-color: var(--glass-border-subtle);
  background: var(--glass-bg-light);
}

.plan-btn-reject:hover {
  color: var(--accent-red);
  border-color: var(--border-danger);
  background: color-mix(in srgb, var(--accent-red) 10%, transparent);
  box-shadow: none;
}

.plan-btn-approve {
  color: var(--text-inverse);
  border-color: var(--accent-green);
  background: var(--accent-green);
  box-shadow: 0 4px 12px color-mix(in srgb, var(--accent-green) 30%, transparent);
}

.plan-btn-approve:hover {
  background: color-mix(in srgb, var(--accent-green) 85%, #000);
  box-shadow: 0 6px 16px color-mix(in srgb, var(--accent-green) 40%, transparent);
}

/* 最小化悬浮按钮 */
.plan-minimized-float {
  pointer-events: auto;
  display: inline-flex;
  align-items: center;
  gap: 10px;
  padding: 10px 16px;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-xl);
  background: var(--glass-bg-heavy);
  backdrop-filter: blur(var(--glass-blur-heavy)) saturate(1.28);
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy)) saturate(1.28);
  box-shadow: var(--glass-shadow), 0 8px 32px rgba(0, 0, 0, 0.2);
  color: var(--text-main);
  cursor: pointer;
  transition: all var(--transition-fast);
  animation: float-in 300ms ease-out;
}

.plan-minimized-float:hover {
  transform: translateY(-2px);
  box-shadow: var(--glass-shadow), 0 12px 40px rgba(0, 0, 0, 0.25);
  border-color: var(--accent-blue);
}

@keyframes float-in {
  from { opacity: 0; transform: translateY(20px) scale(0.95); }
  to { opacity: 1; transform: translateY(0) scale(1); }
}

.plan-minimized-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--accent-blue);
  flex-shrink: 0;
}

.plan-minimized-label {
  font-size: 0.82rem;
  font-weight: 700;
  max-width: 160px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.streaming-indicator-mini {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
}

.streaming-dot-mini {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--accent-blue);
  animation: pulse-dot 1.5s ease-in-out infinite;
}

.plan-slide-enter-active {
  transition: opacity var(--transition-normal);
}

.plan-slide-leave-active {
  transition: opacity 180ms ease-in;
}

.plan-slide-enter-active .plan-panel {
  transition: transform var(--transition-normal), opacity var(--transition-normal);
}

.plan-slide-leave-active .plan-panel {
  transition: transform 180ms ease-in, opacity 180ms ease-in;
}

.plan-slide-enter-from,
.plan-slide-leave-to {
  opacity: 0;
}

.plan-slide-enter-from .plan-panel,
.plan-slide-leave-to .plan-panel {
  transform: translateX(28px) scale(0.985);
  opacity: 0;
}

@media (max-width: 720px) {
  .plan-overlay {
    padding: 0;
  }

  .plan-panel {
    width: 100%;
    border-radius: 0;
  }

  .plan-header {
    padding: 14px 14px 12px;
  }

  .plan-summary,
  .plan-toolbar {
    align-items: stretch;
    flex-direction: column;
  }

  .plan-status-stack {
    align-items: flex-start;
    flex-direction: row;
    justify-content: space-between;
  }

  .plan-edit-actions {
    justify-content: flex-start;
  }

  .plan-body {
    padding: 12px;
  }

  .plan-markdown {
    padding: 15px;
  }

  .plan-actions {
    flex-direction: column-reverse;
    align-items: stretch;
  }

  .plan-btn {
    width: 100%;
  }

  .plan-minimized-float {
    max-width: 90vw;
  }
}
</style>
