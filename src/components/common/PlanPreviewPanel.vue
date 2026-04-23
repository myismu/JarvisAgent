<script setup lang="ts">
import { computed } from 'vue';
import { useJarvis } from '../../composables/useJarvis';
import { marked } from 'marked';

const { planProposal, resolvePlan } = useJarvis();

// 将方案 Markdown 渲染为 HTML
const renderedContent = computed(() => {
  if (!planProposal.value) return '';
  return marked.parse(planProposal.value.content) as string;
});
</script>

<template>
  <!-- 方案预览面板：从右侧滑入的侧边栏 -->
  <Transition name="plan-slide">
    <div v-if="planProposal" class="plan-overlay">
      <div class="plan-panel">
        <!-- 面板头部 -->
        <div class="plan-header">
          <div class="plan-header-left">
            <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path>
              <polyline points="14 2 14 8 20 8"></polyline>
              <line x1="16" y1="13" x2="8" y2="13"></line>
              <line x1="16" y1="17" x2="8" y2="17"></line>
              <polyline points="10 9 9 9 8 9"></polyline>
            </svg>
            <span class="plan-header-title">方案审阅</span>
          </div>
          <button class="plan-close-btn" @click="resolvePlan('reject')" title="关闭并拒绝">
            <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <line x1="18" y1="6" x2="6" y2="18"></line>
              <line x1="6" y1="6" x2="18" y2="18"></line>
            </svg>
          </button>
        </div>

        <!-- 方案标题 -->
        <div class="plan-title-bar">
          <h2 class="plan-title">{{ planProposal.title }}</h2>
          <span class="plan-badge">PENDING REVIEW</span>
        </div>

        <!-- 方案正文（Markdown 渲染） -->
        <div class="plan-body" v-html="renderedContent"></div>

        <!-- 操作按钮 -->
        <div class="plan-actions">
          <button class="plan-btn plan-btn-reject" @click="resolvePlan('reject')">
            <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <circle cx="12" cy="12" r="10"></circle>
              <line x1="15" y1="9" x2="9" y2="15"></line>
              <line x1="9" y1="9" x2="15" y2="15"></line>
            </svg>
            拒绝方案
          </button>
          <button class="plan-btn plan-btn-approve" @click="resolvePlan('allow')">
            <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
              <polyline points="20 6 9 17 4 12"></polyline>
            </svg>
            同意执行
          </button>
        </div>
      </div>
    </div>
  </Transition>
</template>

<style scoped>
/* --- 遮罩层 --- */
.plan-overlay {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background: rgba(0, 0, 0, 0.3);
  display: flex;
  justify-content: flex-end;
  z-index: 200;
  backdrop-filter: blur(2px);
}

/* --- 侧边栏面板 --- */
.plan-panel {
  width: min(600px, 90%);
  height: 100%;
  background-color: var(--bg-panel);
  border-left: 1px solid var(--border-color);
  display: flex;
  flex-direction: column;
  box-shadow: -8px 0 30px rgba(0, 0, 0, 0.1);
  overflow: hidden;
}

/* --- 面板头部 --- */
.plan-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 16px;
  background: linear-gradient(135deg, var(--accent-blue), #4a90d9);
  color: #ffffff;
  flex-shrink: 0;
}

.plan-header-left {
  display: flex;
  align-items: center;
  gap: 8px;
  font-weight: 600;
  font-size: 0.9rem;
}

.plan-close-btn {
  background: rgba(255, 255, 255, 0.15);
  border: none;
  color: #ffffff;
  width: 28px;
  height: 28px;
  border-radius: 4px;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: background 0.2s;
}

.plan-close-btn:hover {
  background: rgba(255, 255, 255, 0.3);
}

/* --- 标题栏 --- */
.plan-title-bar {
  padding: 16px 20px 12px;
  border-bottom: 1px solid var(--border-color);
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-shrink: 0;
}

.plan-title {
  font-size: 1.1rem;
  font-weight: 700;
  color: var(--text-main);
  margin: 0;
}

.plan-badge {
  font-size: 0.7rem;
  font-weight: 600;
  padding: 3px 8px;
  border-radius: 10px;
  background: rgba(176, 136, 0, 0.1);
  color: var(--accent-yellow);
  letter-spacing: 0.5px;
  flex-shrink: 0;
}

/* --- 方案正文 --- */
.plan-body {
  flex: 1;
  overflow-y: auto;
  padding: 20px;
  font-size: 0.88rem;
  line-height: 1.7;
  color: var(--text-main);
}

/* Markdown 内容样式 */
.plan-body :deep(h1) {
  font-size: 1.3rem;
  font-weight: 700;
  margin: 0 0 12px 0;
  padding-bottom: 8px;
  border-bottom: 2px solid var(--accent-blue);
  color: var(--text-main);
}

.plan-body :deep(h2) {
  font-size: 1.1rem;
  font-weight: 600;
  margin: 20px 0 8px 0;
  color: var(--accent-blue);
}

.plan-body :deep(h3) {
  font-size: 0.95rem;
  font-weight: 600;
  margin: 16px 0 6px 0;
  color: var(--text-main);
}

.plan-body :deep(p) {
  margin: 8px 0;
}

.plan-body :deep(ul),
.plan-body :deep(ol) {
  margin: 8px 0;
  padding-left: 24px;
}

.plan-body :deep(li) {
  margin: 4px 0;
}

.plan-body :deep(code) {
  background: var(--bg-sidebar);
  padding: 2px 6px;
  border-radius: 3px;
  font-family: var(--font-mono);
  font-size: 0.85em;
}

.plan-body :deep(pre) {
  background: var(--bg-sidebar);
  padding: 12px 16px;
  border-radius: 6px;
  overflow-x: auto;
  margin: 10px 0;
}

.plan-body :deep(pre code) {
  background: none;
  padding: 0;
}

.plan-body :deep(blockquote) {
  border-left: 3px solid var(--accent-blue);
  padding: 8px 16px;
  margin: 10px 0;
  background: rgba(0, 102, 204, 0.04);
  color: var(--text-muted);
}

.plan-body :deep(strong) {
  color: var(--text-main);
  font-weight: 600;
}

.plan-body :deep(table) {
  width: 100%;
  border-collapse: collapse;
  margin: 10px 0;
}

.plan-body :deep(th),
.plan-body :deep(td) {
  border: 1px solid var(--border-color);
  padding: 8px 12px;
  text-align: left;
}

.plan-body :deep(th) {
  background: var(--bg-sidebar);
  font-weight: 600;
}

/* --- 操作按钮区 --- */
.plan-actions {
  padding: 16px 20px;
  border-top: 1px solid var(--border-color);
  display: flex;
  gap: 12px;
  justify-content: flex-end;
  flex-shrink: 0;
  background: var(--bg-panel);
}

.plan-btn {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 20px;
  border: 1px solid var(--border-color);
  border-radius: 6px;
  font-family: var(--font-mono);
  font-size: 0.85rem;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.2s ease;
}

.plan-btn-reject {
  background: transparent;
  color: var(--accent-red);
  border-color: rgba(215, 58, 73, 0.3);
}

.plan-btn-reject:hover {
  background: rgba(215, 58, 73, 0.08);
  border-color: var(--accent-red);
}

.plan-btn-approve {
  background: var(--accent-blue);
  color: #ffffff;
  border-color: var(--accent-blue);
}

.plan-btn-approve:hover {
  background: #0055aa;
  box-shadow: 0 2px 8px rgba(0, 102, 204, 0.3);
}

/* --- 滑入动画 --- */
.plan-slide-enter-active {
  transition: all 0.3s ease-out;
}

.plan-slide-leave-active {
  transition: all 0.2s ease-in;
}

.plan-slide-enter-from .plan-panel,
.plan-slide-leave-to .plan-panel {
  transform: translateX(100%);
}

.plan-slide-enter-from,
.plan-slide-leave-to {
  opacity: 0;
}
</style>
