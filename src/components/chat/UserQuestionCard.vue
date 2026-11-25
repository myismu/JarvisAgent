<!--
# UserQuestionCard.vue — 运行期用户咨询卡片（AskUser 工具）

挂在输入区上方（与 TerminalInput 完全等宽），当 Agent 运行期间需要澄清需求或确认方案分叉时弹出，
支持一键折叠/展开、单题/多题切换、单选/多选选项卡片、内置「其他」自填输入框，以及「提交回复 / 跳过」决策出口。

## Key Exports
- 默认导出组件：运行期咨询卡片

## Dependencies
- Internal: `@/stores/permission`, `@/types`
- External: `@tauri-apps/api/core`, `vue`
-->
<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { usePermissionStore } from '../../stores/permission';
import type { UserQuestionAnswerItem, UserQuestionItem } from '../../types';

const perm = usePermissionStore();
const activeRequest = computed(() => perm.userQuestion);

/** 多题场景下当前查看的题目索引 */
const currentIndex = ref(0);
/** 折叠态：允许用户临时收起卡片翻看上方聊天记录 */
const isCollapsed = ref(false);
const isSubmitting = ref(false);

/** 每道题的作答草稿（按 question.id 索引） */
const selectedMap = reactive<Record<string, string[]>>({});
const customTextMap = reactive<Record<string, string>>({});
/** 是否勾选了“其他（自定义输入）” */
const customEnabledMap = reactive<Record<string, boolean>>({});

function resetDrafts(questions: UserQuestionItem[]) {
  Object.keys(selectedMap).forEach((k) => delete selectedMap[k]);
  Object.keys(customTextMap).forEach((k) => delete customTextMap[k]);
  Object.keys(customEnabledMap).forEach((k) => delete customEnabledMap[k]);
  for (const q of questions) {
    selectedMap[q.id] = [];
    customTextMap[q.id] = '';
    customEnabledMap[q.id] = false;
  }
  currentIndex.value = 0;
  isCollapsed.value = false;
  isSubmitting.value = false;
}

watch(
  () => activeRequest.value?.id,
  () => {
    if (activeRequest.value) {
      resetDrafts(activeRequest.value.questions);
    }
  },
  { immediate: true },
);

const questions = computed(() => activeRequest.value?.questions ?? []);
const currentQuestion = computed<UserQuestionItem | null>(
  () => questions.value[currentIndex.value] ?? null,
);

function isQuestionAnswered(q: UserQuestionItem): boolean {
  const picks = selectedMap[q.id] ?? [];
  const custom = (customTextMap[q.id] ?? '').trim();
  return picks.length > 0 || (customEnabledMap[q.id] && custom.length > 0);
}

const answeredCount = computed(
  () => questions.value.filter((q) => isQuestionAnswered(q)).length,
);

/** 多题场景下：仅在最后一题或全部题目均已作答时才显示「提交回复」，前面题目只引导「下一题」 */
const showSubmitButton = computed(() => {
  if (questions.value.length <= 1) return true;
  return (
    currentIndex.value === questions.value.length - 1 ||
    answeredCount.value === questions.value.length
  );
});

const canSubmit = computed(() => {
  if (!activeRequest.value || isSubmitting.value) return false;
  return answeredCount.value > 0;
});

function toggleOption(q: UserQuestionItem, label: string) {
  const current = selectedMap[q.id] ?? [];
  if (q.multiSelect) {
    if (current.includes(label)) {
      selectedMap[q.id] = current.filter((item) => item !== label);
    } else {
      selectedMap[q.id] = [...current, label];
    }
  } else {
    selectedMap[q.id] = [label];
    customEnabledMap[q.id] = false;
  }
}

function toggleCustomOption(q: UserQuestionItem) {
  const next = !customEnabledMap[q.id];
  customEnabledMap[q.id] = next;
  if (next && !q.multiSelect) {
    selectedMap[q.id] = [];
  }
}

function onCustomInput(q: UserQuestionItem, value: string) {
  customTextMap[q.id] = value;
  if (value.trim().length > 0) {
    customEnabledMap[q.id] = true;
    if (!q.multiSelect) {
      selectedMap[q.id] = [];
    }
  }
}

async function submitDecision(decision: 'answered' | 'skipped') {
  const req = activeRequest.value;
  if (!req || isSubmitting.value) return;
  isSubmitting.value = true;

  const answers: UserQuestionAnswerItem[] =
    decision === 'answered'
      ? req.questions.map((q) => {
          const custom = customEnabledMap[q.id] ? (customTextMap[q.id] ?? '').trim() : '';
          return {
            questionId: q.id,
            selectedOptions: [...(selectedMap[q.id] ?? [])],
            ...(custom ? { customText: custom } : {}),
          };
        })
      : [];

  try {
    await invoke('resolve_user_question', {
      requestId: req.id,
      sessionId: req.sessionId,
      decision,
      answers: decision === 'answered' ? answers : null,
    });
    perm.removeUserQuestion(req.sessionId, req.id);
  } catch (err) {
    console.error('[AskUser] 提交咨询回复失败:', err);
  } finally {
    isSubmitting.value = false;
  }
}
</script>

<template>
  <Transition name="uq-slide">
    <section
      v-if="activeRequest && currentQuestion"
      class="uq-card"
      :class="{ collapsed: isCollapsed }"
      aria-label="Agent 咨询"
    >
      <header class="uq-header" @click="isCollapsed = !isCollapsed">
        <div class="uq-header-left">
          <span class="uq-dot" aria-hidden="true"></span>
          <span class="uq-title">{{ activeRequest.title || '需要你确认以下问题' }}</span>
        </div>
        <div class="uq-header-right">
          <span class="uq-progress">{{ answeredCount }}/{{ questions.length }}</span>
          <button
            type="button"
            class="uq-collapse-btn"
            :aria-expanded="!isCollapsed"
            :title="isCollapsed ? '展开咨询' : '收起咨询'"
            @click.stop="isCollapsed = !isCollapsed"
          >
            <svg
              class="uq-chevron"
              :class="{ rotated: isCollapsed }"
              viewBox="0 0 24 24"
              width="14"
              height="14"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <polyline points="6 9 12 15 18 9"></polyline>
            </svg>
          </button>
        </div>
      </header>

      <div v-if="!isCollapsed" class="uq-collapsible">
        <!-- 多题场景下的顶部轻量标签栏 -->
        <div v-if="questions.length > 1" class="uq-tabs" role="tablist">
          <button
            v-for="(q, idx) in questions"
            :key="q.id"
            type="button"
            class="uq-tab"
            :class="{ active: idx === currentIndex, answered: isQuestionAnswered(q) }"
            @click="currentIndex = idx"
          >
            <span class="uq-tab-index">{{ idx + 1 }}</span>
            <span class="uq-tab-label">{{ q.header || `问题 ${idx + 1}` }}</span>
          </button>
        </div>

        <div class="uq-body">
          <div class="uq-question-line">
            <p class="uq-question-text">{{ currentQuestion.question }}</p>
            <span class="uq-mode-tag">{{ currentQuestion.multiSelect ? '多选' : '单选' }}</span>
          </div>

          <div class="uq-options">
            <button
              v-for="opt in currentQuestion.options"
              :key="opt.label"
              type="button"
              class="uq-option"
              :class="{ selected: (selectedMap[currentQuestion.id] ?? []).includes(opt.label) }"
              @click="toggleOption(currentQuestion, opt.label)"
            >
              <span class="uq-indicator" :class="{ multi: currentQuestion.multiSelect }"></span>
              <span class="uq-option-content">
                <span class="uq-option-head">
                  <span class="uq-option-label">{{ opt.label }}</span>
                  <span v-if="opt.recommended" class="uq-rec-pill">推荐</span>
                </span>
                <span v-if="opt.description" class="uq-option-desc">{{ opt.description }}</span>
              </span>
            </button>

            <!-- 内置「其他」自填选项 -->
            <div
              v-if="currentQuestion.allowCustom"
              class="uq-option uq-custom-option"
              :class="{ selected: customEnabledMap[currentQuestion.id] }"
              @click="toggleCustomOption(currentQuestion)"
            >
              <span class="uq-indicator" :class="{ multi: currentQuestion.multiSelect }"></span>
              <div class="uq-custom-body" @click.stop>
                <span class="uq-option-label uq-custom-title" @click="toggleCustomOption(currentQuestion)">
                  其他
                </span>
                <input
                  type="text"
                  class="uq-custom-input"
                  :value="customTextMap[currentQuestion.id] ?? ''"
                  placeholder="输入你的自定义想法或补充说明..."
                  @focus="customEnabledMap[currentQuestion.id] = true"
                  @input="onCustomInput(currentQuestion, ($event.target as HTMLInputElement).value)"
                />
              </div>
            </div>
          </div>
        </div>

        <footer class="uq-footer">
          <div class="uq-footer-left">
            <button
              type="button"
              class="uq-btn-ghost"
              :disabled="isSubmitting"
              @click="submitDecision('skipped')"
            >
              跳过，按推荐继续
            </button>
          </div>
          <div class="uq-footer-right">
            <button
              v-if="questions.length > 1 && currentIndex > 0"
              type="button"
              class="uq-btn uq-btn-secondary"
              @click="currentIndex -= 1"
            >
              上一题
            </button>
            <button
              v-if="questions.length > 1 && currentIndex < questions.length - 1"
              type="button"
              class="uq-btn uq-btn-secondary"
              @click="currentIndex += 1"
            >
              下一题
            </button>
            <button
              v-if="showSubmitButton"
              type="button"
              class="uq-btn uq-btn-primary"
              :disabled="!canSubmit"
              @click="submitDecision('answered')"
            >
              {{ isSubmitting ? '提交中...' : '提交回复' }}
            </button>
          </div>
        </footer>
      </div>
    </section>
  </Transition>
</template>

<style scoped>
.uq-card {
  width: 100%;
  max-width: min(85%, 960px); /* 与 TerminalInput(.chat-input-wrapper) 完全等宽 */
  margin: 0 auto 10px;
  box-sizing: border-box;
  padding: 12px 16px 14px;
  border-radius: 16px;
  background: var(--surface-strong, #fff);
  border: 1px solid var(--glass-border, rgba(0, 0, 0, 0.09));
  box-shadow: var(--shadow-md, 0 6px 20px rgba(0, 0, 0, 0.06));
  color: var(--text-main);
  transition: padding 0.16s ease, box-shadow 0.16s ease;
}

.uq-card.collapsed {
  padding: 10px 16px;
}

.uq-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  cursor: pointer;
  user-select: none;
}

.uq-header-left {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.uq-dot {
  flex-shrink: 0;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--text-main);
  opacity: 0.72;
}

.uq-title {
  font-size: 0.84rem;
  font-weight: 600;
  letter-spacing: -0.01em;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.uq-header-right {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}

.uq-progress {
  font-size: 0.74rem;
  font-weight: 500;
  color: var(--text-muted);
  font-variant-numeric: tabular-nums;
}

.uq-collapse-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  padding: 0;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition: background 0.15s ease, color 0.15s ease;
}

.uq-collapse-btn:hover {
  background: color-mix(in srgb, var(--text-main) 6%, transparent);
  color: var(--text-main);
}

.uq-chevron {
  transition: transform 0.18s ease;
}

.uq-chevron.rotated {
  transform: rotate(180deg);
}

.uq-collapsible {
  margin-top: 10px;
}

.uq-tabs {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  margin-bottom: 10px;
  padding-bottom: 8px;
  border-bottom: 1px solid var(--glass-border-subtle, rgba(0, 0, 0, 0.06));
}

.uq-tab {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px;
  border-radius: 8px;
  border: 1px solid transparent;
  background: transparent;
  color: var(--text-muted);
  font-size: 0.75rem;
  font-weight: 500;
  cursor: pointer;
  transition: background 0.15s ease, color 0.15s ease, border-color 0.15s ease;
}

.uq-tab:hover {
  color: var(--text-main);
  background: color-mix(in srgb, var(--text-main) 4%, transparent);
}

.uq-tab.active {
  background: color-mix(in srgb, var(--text-main) 6%, transparent);
  border-color: var(--glass-border-subtle, rgba(0, 0, 0, 0.08));
  color: var(--text-main);
  font-weight: 600;
}

.uq-tab-index {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 15px;
  height: 15px;
  border-radius: 50%;
  font-size: 0.65rem;
  font-variant-numeric: tabular-nums;
  color: var(--text-muted);
  border: 1px solid color-mix(in srgb, var(--text-muted) 35%, transparent);
}

.uq-tab.answered .uq-tab-index {
  background: var(--text-main);
  border-color: var(--text-main);
  color: var(--surface-strong, #fff);
}

.uq-question-line {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 8px;
}

.uq-question-text {
  margin: 0;
  font-size: 0.85rem;
  font-weight: 500;
  line-height: 1.5;
  color: var(--text-main);
}

.uq-mode-tag {
  flex-shrink: 0;
  font-size: 0.7rem;
  color: var(--text-muted);
}

.uq-options {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-height: 280px;
  overflow-y: auto;
  margin-bottom: 12px;
  padding-right: 2px;
}

.uq-option {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  width: 100%;
  text-align: left;
  padding: 9px 12px;
  border-radius: 10px;
  border: 1px solid var(--glass-border-subtle, rgba(0, 0, 0, 0.08));
  background: transparent;
  color: var(--text-main);
  cursor: pointer;
  transition: border-color 0.15s ease, background 0.15s ease, box-shadow 0.15s ease;
}

.uq-option:hover {
  border-color: var(--glass-border, rgba(0, 0, 0, 0.16));
  background: color-mix(in srgb, var(--text-main) 2.5%, transparent);
}

.uq-option.selected {
  border-color: color-mix(in srgb, var(--text-main) 55%, transparent);
  background: color-mix(in srgb, var(--text-main) 3.5%, transparent);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.04);
}

.uq-indicator {
  flex-shrink: 0;
  width: 14px;
  height: 14px;
  margin-top: 2px;
  border-radius: 50%;
  border: 1.5px solid color-mix(in srgb, var(--text-muted) 65%, transparent);
  transition: border-color 0.15s ease, background 0.15s ease;
}

.uq-indicator.multi {
  border-radius: 4px;
}

.uq-option.selected .uq-indicator {
  border-color: var(--text-main);
  background: var(--text-main);
  box-shadow: inset 0 0 0 2.5px var(--surface-strong, #fff);
}

.uq-option-content {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.uq-option-head {
  display: flex;
  align-items: center;
  gap: 6px;
}

.uq-option-label {
  font-size: 0.82rem;
  font-weight: 500;
  line-height: 1.4;
}

.uq-rec-pill {
  font-size: 0.65rem;
  font-weight: 500;
  padding: 0 6px;
  line-height: 1.55;
  border-radius: 5px;
  border: 1px solid color-mix(in srgb, var(--text-main) 18%, transparent);
  background: transparent;
  color: var(--text-soft, var(--text-muted));
}

.uq-option-desc {
  font-size: 0.74rem;
  color: var(--text-muted);
  line-height: 1.4;
}

.uq-custom-body {
  display: flex;
  flex-direction: column;
  gap: 6px;
  flex: 1;
}

.uq-custom-title {
  color: var(--text-soft, var(--text-main));
}

.uq-custom-input {
  width: 100%;
  box-sizing: border-box;
  padding: 2px 0;
  border: none;
  border-radius: 0;
  background: transparent;
  color: var(--text-main);
  font-size: 0.8rem;
  line-height: 1.45;
  outline: none;
  box-shadow: none;
}

.uq-custom-input::placeholder {
  color: var(--text-muted);
  opacity: 0.72;
}

.uq-custom-input:focus {
  border: none;
  outline: none;
  box-shadow: none;
}

.uq-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding-top: 8px;
  border-top: 1px solid var(--glass-border-subtle, rgba(0, 0, 0, 0.06));
}

.uq-footer-right {
  display: flex;
  align-items: center;
  gap: 8px;
}

.uq-btn-ghost {
  padding: 5px 6px;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: var(--text-muted);
  font-size: 0.76rem;
  font-weight: 500;
  cursor: pointer;
  transition: color 0.15s ease, background 0.15s ease;
}

.uq-btn-ghost:hover:not(:disabled) {
  color: var(--text-main);
  background: color-mix(in srgb, var(--text-main) 4%, transparent);
}

.uq-btn-ghost:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.uq-btn {
  padding: 6px 14px;
  border-radius: 8px;
  font-size: 0.78rem;
  font-weight: 500;
  cursor: pointer;
  transition: background 0.15s ease, border-color 0.15s ease, opacity 0.15s ease;
}

.uq-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.uq-btn-secondary {
  border: 1px solid var(--glass-border, rgba(0, 0, 0, 0.14));
  background: var(--surface-strong, #fff);
  color: var(--text-main);
}

.uq-btn-secondary:hover:not(:disabled) {
  border-color: color-mix(in srgb, var(--text-main) 35%, transparent);
  background: color-mix(in srgb, var(--text-main) 3%, transparent);
}

.uq-btn-primary {
  border: 1px solid var(--btn-primary-bg, var(--text-main));
  background: var(--btn-primary-bg, var(--text-main));
  color: var(--btn-primary-fg, #fff);
}

.uq-btn-primary:hover:not(:disabled) {
  opacity: 0.9;
}

.uq-slide-enter-active { transition: all 0.18s ease-out; }
.uq-slide-leave-active { transition: all 0.14s ease-in; }
.uq-slide-enter-from { opacity: 0; transform: translateY(8px); }
.uq-slide-leave-to { opacity: 0; transform: translateY(4px); }
</style>
