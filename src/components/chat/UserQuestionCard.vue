<!--
# UserQuestionCard.vue — 运行期用户咨询卡片（AskUser 工具）

挂在输入区上方（与 PermissionCard 同位），当 Agent 运行期间需要澄清需求或确认方案分叉时弹出，
支持单题/多题切换、单选/多选选项卡片、内置「其他」自填输入框，以及「提交回答 / 跳过」决策出口。

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
  // 至少有一道题作答即可提交（未填题后端会自动提示按推荐方案处理）
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
    // 单选模式下选了固定选项自动收起“其他”勾选态，避免语义冲突
    customEnabledMap[q.id] = false;
  }
}

function toggleCustomOption(q: UserQuestionItem) {
  const next = !customEnabledMap[q.id];
  customEnabledMap[q.id] = next;
  if (next && !q.multiSelect) {
    // 单选模式切到“其他”时清空固定选项
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
    // 立即在本地移出队列，减少等待回环事件的闪烁
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
    <section v-if="activeRequest && currentQuestion" class="uq-card" aria-label="Agent 咨询">
      <header class="uq-header">
        <div class="uq-header-left">
          <span class="uq-badge">咨询</span>
          <span class="uq-title">{{ activeRequest.title || '需要你确认以下问题' }}</span>
        </div>
        <span class="uq-progress">已答 {{ answeredCount }} / {{ questions.length }}</span>
      </header>

      <!-- 多题场景下的顶部切换标签 -->
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
          <span class="uq-mode-tag">{{ currentQuestion.multiSelect ? '可多选' : '单选' }}</span>
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
              <span class="uq-option-label" @click="toggleCustomOption(currentQuestion)">其他（自定义输入）</span>
              <input
                type="text"
                class="uq-custom-input"
                :value="customTextMap[currentQuestion.id] ?? ''"
                placeholder="输入你的具体想法或补充要求..."
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
            class="uq-btn uq-btn-skip"
            :disabled="isSubmitting"
            @click="submitDecision('skipped')"
          >
            跳过（按推荐继续）
          </button>
        </div>
        <div class="uq-footer-right">
          <button
            v-if="questions.length > 1 && currentIndex > 0"
            type="button"
            class="uq-btn uq-btn-nav"
            @click="currentIndex -= 1"
          >
            上一题
          </button>
          <button
            v-if="questions.length > 1 && currentIndex < questions.length - 1"
            type="button"
            class="uq-btn uq-btn-nav"
            @click="currentIndex += 1"
          >
            下一题
          </button>
          <button
            v-if="showSubmitButton"
            type="button"
            class="uq-btn uq-btn-submit"
            :disabled="!canSubmit"
            @click="submitDecision('answered')"
          >
            {{ isSubmitting ? '提交中...' : '提交回复' }}
          </button>
        </div>
      </footer>
    </section>
  </Transition>
</template>

<style scoped>
.uq-card {
  width: 100%;
  max-width: min(85%, 960px); /* 与 TerminalInput(.chat-input-wrapper) 保持完全等宽 */
  margin: 0 auto 8px;
  box-sizing: border-box;
  padding: 12px 14px;
  border-radius: var(--radius-lg, 14px);
  background: var(--glass-bg-heavy, #fff);
  border: 1px solid var(--glass-border, rgba(0, 0, 0, 0.1));
  box-shadow: var(--shadow-md, 0 8px 24px rgba(0, 0, 0, 0.08));
  color: var(--text-main);
}

.uq-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 8px;
}

.uq-header-left {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.uq-badge {
  flex-shrink: 0;
  font-size: 0.7rem;
  font-weight: 600;
  padding: 2px 7px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text-main) 10%, transparent);
  color: var(--text-main);
}

.uq-title {
  font-size: 0.84rem;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.uq-progress {
  flex-shrink: 0;
  font-size: 0.72rem;
  color: var(--text-muted);
}

.uq-tabs {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-bottom: 10px;
}

.uq-tab {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 4px 10px;
  border-radius: 999px;
  border: 1px solid var(--glass-border-subtle, rgba(0, 0, 0, 0.08));
  background: transparent;
  color: var(--text-muted);
  font-size: 0.75rem;
  cursor: pointer;
}

.uq-tab.active {
  background: var(--glass-bg-light);
  border-color: var(--glass-border);
  color: var(--text-main);
  font-weight: 600;
}

.uq-tab.answered .uq-tab-index {
  background: var(--text-main);
  color: var(--bg-dark, #fff);
}

.uq-tab-index {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  font-size: 0.66rem;
  background: color-mix(in srgb, var(--text-muted) 18%, transparent);
}

.uq-question-line {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 8px;
}

.uq-question-text {
  margin: 0;
  font-size: 0.84rem;
  font-weight: 500;
  line-height: 1.45;
}

.uq-mode-tag {
  flex-shrink: 0;
  font-size: 0.68rem;
  color: var(--text-muted);
}

.uq-options {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-height: 220px;
  overflow-y: auto;
  margin-bottom: 10px;
}

.uq-option {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  width: 100%;
  text-align: left;
  padding: 8px 10px;
  border-radius: var(--radius-md, 10px);
  border: 1px solid var(--glass-border-subtle, rgba(0, 0, 0, 0.08));
  background: var(--glass-bg-light, transparent);
  color: var(--text-main);
  cursor: pointer;
  transition: border-color 0.15s ease, background 0.15s ease;
}

.uq-option:hover {
  border-color: var(--glass-border);
}

.uq-option.selected {
  border-color: var(--text-main);
  background: color-mix(in srgb, var(--text-main) 6%, transparent);
}

.uq-indicator {
  flex-shrink: 0;
  width: 14px;
  height: 14px;
  margin-top: 2px;
  border-radius: 50%;
  border: 1.5px solid var(--text-muted);
}

.uq-indicator.multi {
  border-radius: 4px;
}

.uq-option.selected .uq-indicator {
  border-color: var(--text-main);
  background: var(--text-main);
  box-shadow: inset 0 0 0 2.5px var(--glass-bg-heavy, #fff);
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
}

.uq-rec-pill {
  font-size: 0.66rem;
  padding: 1px 6px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text-main) 10%, transparent);
  color: var(--text-soft, var(--text-muted));
}

.uq-option-desc {
  font-size: 0.74rem;
  color: var(--text-muted);
  line-height: 1.35;
}

.uq-custom-body {
  display: flex;
  flex-direction: column;
  gap: 5px;
  flex: 1;
}

.uq-custom-input {
  width: 100%;
  padding: 5px 8px;
  border-radius: var(--radius-sm, 6px);
  border: 1px solid var(--glass-border-subtle, rgba(0, 0, 0, 0.12));
  background: var(--glass-bg, #fff);
  color: var(--text-main);
  font-size: 0.78rem;
  outline: none;
}

.uq-custom-input:focus {
  border-color: var(--text-main);
}

.uq-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.uq-footer-right {
  display: flex;
  align-items: center;
  gap: 6px;
}

.uq-btn {
  padding: 6px 12px;
  border-radius: var(--radius-md, 8px);
  border: 1px solid var(--glass-border-subtle, rgba(0, 0, 0, 0.12));
  background: var(--glass-bg-light, transparent);
  color: var(--text-main);
  font-size: 0.76rem;
  font-weight: 500;
  cursor: pointer;
}

.uq-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.uq-btn-skip {
  color: var(--text-muted);
}

.uq-btn-submit {
  background: var(--btn-primary-bg, var(--text-main));
  border-color: var(--btn-primary-bg, var(--text-main));
  color: var(--btn-primary-fg, #fff);
}

.uq-slide-enter-active { transition: all 0.2s ease-out; }
.uq-slide-leave-active { transition: all 0.14s ease-in; }
.uq-slide-enter-from { opacity: 0; transform: translateY(8px); }
.uq-slide-leave-to { opacity: 0; transform: translateY(4px); }
</style>
