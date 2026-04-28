<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { usePermissionStore } from '../../stores/permission';
import { useChatStore } from '../../stores/chat';
import { marked } from 'marked';

const perm = usePermissionStore();
const chat = useChatStore();

const isEditing = ref(false);
const editedContent = ref('');

watch(() => perm.planProposal, (newVal) => {
  if (newVal) {
    editedContent.value = newVal.content;
    isEditing.value = false;
  }
});

const activeContent = computed(() => {
  if (!perm.planProposal) return '';
  return isEditing.value ? editedContent.value : perm.planProposal.content;
});

const renderedContent = computed(() => {
  if (!perm.planProposal) return '';
  return marked.parse(activeContent.value) as string;
});

const planStats = computed(() => {
  const source = activeContent.value.trim();
  if (!source) return { lines: 0, sections: 0 };

  const lines = source.split(/\r?\n/).filter((line) => line.trim().length > 0).length;
  const sections = source.match(/^#{1,3}\s+/gm)?.length ?? 0;
  return { lines, sections };
});

const toggleEdit = () => {
  if (!perm.planProposal) return;

  if (isEditing.value) {
    perm.updatePlanProposalContent(editedContent.value);
  } else {
    editedContent.value = perm.planProposal.content;
  }

  isEditing.value = !isEditing.value;
};

const cancelEdit = () => {
  if (perm.planProposal) {
    editedContent.value = perm.planProposal.content;
  }
  isEditing.value = false;
};

const handleApprove = () => {
  const contentToUse = isEditing.value ? editedContent.value : perm.planProposal?.content;
  chat.resolvePlan('allow', contentToUse);
};

const handleReject = () => {
  chat.resolvePlan('reject');
};
</script>

<template>
  <Transition name="plan-slide">
    <div v-if="perm.planProposal" class="plan-overlay" role="dialog" aria-modal="true" aria-labelledby="plan-review-heading">
      <aside class="plan-panel">
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
              <span class="plan-kicker">Plan Review</span>
              <h2 id="plan-review-heading">方案审批</h2>
            </div>
          </div>

          <button class="plan-icon-btn plan-close-btn" @click="handleReject" title="关闭并拒绝" aria-label="关闭并拒绝">
            <svg viewBox="0 0 24 24" width="15" height="15" stroke="currentColor" stroke-width="2.2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <line x1="18" y1="6" x2="6" y2="18"></line>
              <line x1="6" y1="6" x2="18" y2="18"></line>
            </svg>
          </button>
        </header>

        <section class="plan-summary">
          <div class="plan-summary-copy">
            <span class="plan-label">待批阅方案</span>
            <h3 class="plan-title">{{ perm.planProposal.title }}</h3>
          </div>
          <div class="plan-status-stack">
            <span class="plan-status-pill">
              <span class="plan-status-dot"></span>
              等待确认
            </span>
            <span class="plan-stats">{{ planStats.sections }} 章节 / {{ planStats.lines }} 行</span>
          </div>
        </section>

        <div class="plan-toolbar">
          <div class="plan-toolbar-copy">
            <span class="plan-toolbar-title">{{ isEditing ? 'Markdown 编辑' : 'Markdown 预览' }}</span>
            <span class="plan-toolbar-subtitle">{{ isEditing ? '修改后保存或直接同意执行' : '请确认计划内容后再执行' }}</span>
          </div>

          <div class="plan-edit-actions">
            <button
              v-if="!isEditing"
              class="plan-mini-btn"
              @click="toggleEdit"
              title="编辑方案"
            >
              <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <path d="M12 20h9"></path>
                <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L8 18l-4 1 1-4Z"></path>
              </svg>
              编辑
            </button>
            <template v-else>
              <button class="plan-mini-btn plan-mini-btn-muted" @click="cancelEdit" title="取消编辑">
                取消
              </button>
              <button class="plan-mini-btn plan-mini-btn-primary" @click="toggleEdit" title="保存修改">
                保存
              </button>
            </template>
          </div>
        </div>

        <main class="plan-body" :class="{ 'is-editing': isEditing }">
          <textarea
            v-if="isEditing"
            v-model="editedContent"
            class="plan-editor"
            placeholder="在此编辑方案内容..."
          ></textarea>
          <article v-else class="plan-markdown" v-html="renderedContent"></article>
        </main>

        <footer class="plan-actions">
          <button class="plan-btn plan-btn-reject" @click="handleReject">
            <svg viewBox="0 0 24 24" width="15" height="15" stroke="currentColor" stroke-width="2.2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="10"></circle>
              <line x1="15" y1="9" x2="9" y2="15"></line>
              <line x1="9" y1="9" x2="15" y2="15"></line>
            </svg>
            拒绝
          </button>
          <button class="plan-btn plan-btn-approve" @click="handleApprove">
            <svg viewBox="0 0 24 24" width="15" height="15" stroke="currentColor" stroke-width="2.2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <path d="M20 6 9 17l-5-5"></path>
            </svg>
            同意
          </button>
        </footer>
      </aside>
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
  background:
    linear-gradient(90deg, rgba(15, 23, 42, 0.04), rgba(15, 23, 42, 0.34)),
    rgba(2, 6, 23, 0.18);
  backdrop-filter: blur(10px) saturate(1.08);
  -webkit-backdrop-filter: blur(10px) saturate(1.08);
  z-index: 200;
}

.plan-panel {
  position: relative;
  isolation: isolate;
  width: min(640px, 92vw);
  height: 100%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  color: var(--text-main);
  background:
    linear-gradient(150deg, rgba(255, 255, 255, 0.82), rgba(255, 255, 255, 0.48) 48%, rgba(255, 255, 255, 0.62)),
    var(--glass-bg-heavy);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-xl);
  box-shadow:
    -18px 0 56px rgba(15, 23, 42, 0.18),
    inset 1px 1px 0 rgba(255, 255, 255, 0.78),
    inset -1px -1px 0 rgba(255, 255, 255, 0.18);
  backdrop-filter: blur(28px) saturate(1.28);
  -webkit-backdrop-filter: blur(28px) saturate(1.28);
}

.plan-panel::before {
  content: "";
  position: absolute;
  inset: 0;
  z-index: -1;
  pointer-events: none;
  background:
    linear-gradient(115deg, rgba(255, 255, 255, 0.62), transparent 34%),
    linear-gradient(180deg, rgba(59, 130, 246, 0.08), transparent 30%);
  opacity: 0.9;
}

.plan-panel::after {
  content: "";
  position: absolute;
  top: 12px;
  bottom: 12px;
  left: 0;
  width: 1px;
  pointer-events: none;
  background: linear-gradient(180deg, transparent, rgba(96, 165, 250, 0.7), transparent);
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

.plan-icon-shell {
  width: 38px;
  height: 38px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  color: var(--accent-blue);
  border: 1px solid rgba(59, 130, 246, 0.24);
  border-radius: 12px;
  background:
    linear-gradient(145deg, rgba(255, 255, 255, 0.55), rgba(59, 130, 246, 0.08)),
    var(--glass-bg-light);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.7),
    0 10px 24px rgba(59, 130, 246, 0.12);
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
  color: var(--accent-red);
  border-color: rgba(239, 68, 68, 0.28);
  background: rgba(239, 68, 68, 0.08);
  transform: translateY(-1px);
}

.plan-summary {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
  padding: 18px 20px 16px;
  border-bottom: 1px solid var(--glass-border-subtle);
  background: linear-gradient(180deg, rgba(255, 255, 255, 0.24), rgba(255, 255, 255, 0.08));
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
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.42);
}

.plan-status-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--accent-yellow);
  box-shadow: 0 0 0 3px rgba(245, 158, 11, 0.13), 0 0 12px rgba(245, 158, 11, 0.34);
}

.plan-stats {
  color: var(--text-muted);
  font-size: 0.72rem;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.plan-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 12px 20px;
  border-bottom: 1px solid var(--glass-border-subtle);
  background: rgba(255, 255, 255, 0.12);
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
  border-color: rgba(59, 130, 246, 0.32);
  background: rgba(59, 130, 246, 0.08);
  transform: translateY(-1px);
}

.plan-mini-btn-muted:hover {
  color: var(--accent-red);
  border-color: rgba(239, 68, 68, 0.28);
  background: rgba(239, 68, 68, 0.08);
}

.plan-mini-btn-primary {
  color: #ffffff;
  border-color: rgba(59, 130, 246, 0.56);
  background: linear-gradient(180deg, var(--accent-blue), var(--accent-blue-hover));
  box-shadow: 0 8px 18px rgba(59, 130, 246, 0.22);
}

.plan-mini-btn-primary:hover {
  color: #ffffff;
  background: linear-gradient(180deg, var(--accent-blue-hover), #1d4ed8);
}

.plan-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 18px 20px 20px;
  color: var(--text-main);
}

.plan-body.is-editing {
  padding: 14px;
}

.plan-markdown {
  min-height: 100%;
  padding: 18px 18px 26px;
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-lg);
  background:
    linear-gradient(180deg, rgba(255, 255, 255, 0.42), rgba(255, 255, 255, 0.2)),
    rgba(255, 255, 255, 0.18);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.55),
    0 12px 32px rgba(15, 23, 42, 0.06);
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
  background:
    linear-gradient(180deg, rgba(255, 255, 255, 0.42), rgba(255, 255, 255, 0.18)),
    var(--glass-bg-light);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.48),
    inset 0 0 0 1px rgba(255, 255, 255, 0.06);
  backdrop-filter: blur(14px) saturate(1.12);
  -webkit-backdrop-filter: blur(14px) saturate(1.12);
}

.plan-editor::placeholder {
  color: var(--text-muted);
}

.plan-editor:focus {
  border-color: rgba(59, 130, 246, 0.56);
  box-shadow:
    0 0 0 3px rgba(59, 130, 246, 0.11),
    inset 0 1px 0 rgba(255, 255, 255, 0.48);
}

.plan-markdown :deep(h1) {
  margin: 0 0 14px;
  padding-bottom: 10px;
  color: var(--text-main);
  font-size: 1.32rem;
  font-weight: 760;
  line-height: 1.32;
  border-bottom: 1px solid rgba(59, 130, 246, 0.24);
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
  background: rgba(59, 130, 246, 0.08);
}

.plan-markdown :deep(pre) {
  margin: 12px 0;
  padding: 14px 16px;
  overflow-x: auto;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
  background:
    linear-gradient(180deg, rgba(15, 23, 42, 0.88), rgba(15, 23, 42, 0.78));
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.08);
}

.plan-markdown :deep(pre code) {
  padding: 0;
  color: #dbeafe;
  border: 0;
  background: transparent;
}

.plan-markdown :deep(blockquote) {
  margin: 12px 0;
  padding: 10px 14px;
  color: var(--text-main);
  border-left: 3px solid var(--accent-blue);
  border-radius: 0 var(--radius-md) var(--radius-md) 0;
  background: rgba(59, 130, 246, 0.08);
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
}

.plan-markdown :deep(th) {
  color: var(--text-main);
  font-weight: 730;
  background: rgba(59, 130, 246, 0.08);
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
  background:
    linear-gradient(180deg, rgba(255, 255, 255, 0.08), rgba(255, 255, 255, 0.18)),
    var(--glass-bg-light);
  backdrop-filter: blur(18px) saturate(1.15);
  -webkit-backdrop-filter: blur(18px) saturate(1.15);
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
  color: var(--text-muted);
  border-color: var(--glass-border-subtle);
  background: rgba(255, 255, 255, 0.08);
}

.plan-btn-reject:hover {
  color: var(--accent-red);
  border-color: rgba(239, 68, 68, 0.28);
  background: rgba(239, 68, 68, 0.08);
  box-shadow: none;
}

.plan-btn-approve {
  color: #ffffff;
  border-color: rgba(16, 185, 129, 0.42);
  background: rgba(16, 185, 129, 0.92);
  box-shadow: 0 8px 18px rgba(16, 185, 129, 0.18);
}

.plan-btn-approve:hover {
  border-color: rgba(16, 185, 129, 0.58);
  background: rgba(5, 150, 105, 0.96);
  box-shadow: 0 10px 20px rgba(16, 185, 129, 0.2);
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

:global(body.dark-mode) .plan-overlay {
  background:
    linear-gradient(90deg, rgba(2, 6, 23, 0.12), rgba(2, 6, 23, 0.52)),
    rgba(2, 6, 23, 0.38);
}

:global(body.dark-mode) .plan-panel {
  background:
    linear-gradient(150deg, rgba(22, 27, 41, 0.82), rgba(9, 12, 22, 0.62) 52%, rgba(18, 24, 38, 0.72)),
    var(--glass-bg-heavy);
  box-shadow:
    -22px 0 64px rgba(0, 0, 0, 0.36),
    inset 1px 1px 0 rgba(255, 255, 255, 0.08),
    inset -1px -1px 0 rgba(255, 255, 255, 0.04);
}

:global(body.dark-mode) .plan-panel::before {
  background:
    linear-gradient(115deg, rgba(255, 255, 255, 0.1), transparent 34%),
    linear-gradient(180deg, rgba(96, 165, 250, 0.1), transparent 32%);
}

:global(body.dark-mode) .plan-summary,
:global(body.dark-mode) .plan-toolbar,
:global(body.dark-mode) .plan-actions {
  background: rgba(15, 23, 42, 0.18);
}

:global(body.dark-mode) .plan-markdown,
:global(body.dark-mode) .plan-editor {
  background:
    linear-gradient(180deg, rgba(15, 23, 42, 0.5), rgba(15, 23, 42, 0.24)),
    rgba(15, 23, 42, 0.2);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.06),
    0 14px 36px rgba(0, 0, 0, 0.2);
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
}
</style>
