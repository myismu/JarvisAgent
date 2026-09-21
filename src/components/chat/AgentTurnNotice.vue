<!--
# AgentTurnNotice.vue — 一轮对话的状态标注

渲染在 assistant 气泡**之外**。notice（中断 / 取消 / 等待说明）是"这一轮的运行状态"，
不是模型回复的正文 —— 放进气泡会跟着气泡的 `padding: 14px 22px` 一起缩进，
看起来像模型自己说的话。所以由调用方放在 `.message-content` 外面。

与正文的区分靠**留白 + 对比度档位**（`--text-soft`），不靠底色 —— 曾经用过色块，
实测偏重、不够简洁。

## Key Exports
- `AgentTurnNotice`: 渲染 notice；notice 为空时不渲染任何节点

## Dependencies
- Internal: 无（纯展示组件）

## Constraints
- 必须放在 `.message-content`（气泡）外面，否则失去"脱离气泡"的意义
-->

<script setup lang="ts">
defineProps<{
  /** 状态标注：中断 / 取消 / 等待说明 */
  notice?: string | null;
  /**
   * 过程态（等待提示 / 重试进度）：加脉冲点表示"还在进行中"，并让宽度自适应内容。
   * 由调用方给 —— 历史快照恒为 false，只有当前轮才可能为 true。
   */
  running?: boolean;
}>();
</script>

<template>
  <div v-if="notice" class="turn-notice" :class="{ running }">
    {{ notice }}
  </div>
</template>

<style scoped>
/* 与气泡左对齐（调用方给 align-items: flex-start）。
   上边距是"从气泡边界算起"的距离 —— 气泡自身还有 padding-bottom，
   两者相加才是视觉间距，故这里只给 10px。
   宽度由 flex 布局的 fit-content 决定：短文案自成一行，长文案撑到 85% 后换行，
   与气泡同口径，不会比气泡更宽。 */
.turn-notice {
  margin-top: 10px;
  max-width: 85%;
  font-size: 0.78rem;
  line-height: 1.55;
  color: var(--text-soft);
  word-break: break-word;
}

/* 过程态：脉冲点 + 内容宽度，扫一眼就知道这条会变，不必等结果 */
.turn-notice.running {
  display: inline-flex;
  align-items: center;
  gap: 8px;
}

.turn-notice.running::before {
  content: "";
  width: 6px;
  height: 6px;
  border-radius: 50%;
  flex-shrink: 0;
  background: var(--text-muted);
  animation: agent-pulse 1.5s ease-in-out infinite;
}

/* 与 AgentTurn.vue 里开发者模式状态点同款节拍（scoped 隔离，无法跨组件复用） */
@keyframes agent-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.4; }
}
</style>
