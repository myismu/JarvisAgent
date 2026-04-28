<script setup lang="ts">
import { computed } from 'vue';
import { renderMarkdown, renderToolDetails, renderTokenUsage } from '../../utils/markdown';

const props = defineProps<{
  role: 'user' | 'assistant';
  content: string;
  thinkingContent?: string;
  inputTokens?: number;
  outputTokens?: number;
  snapshotId?: string;
  id?: string;
}>();

const roleClass = computed(() => props.role === 'user' ? 'user-message' : 'agent-message');

const renderedContent = computed(() => renderMarkdown(props.content));

const renderedThinking = computed(() => {
  if (!props.thinkingContent) return '';
  return renderToolDetails(props.thinkingContent, 'done');
});

const tokenInfo = computed(() => {
  if (props.inputTokens !== undefined && props.outputTokens !== undefined) {
    return renderTokenUsage(props.inputTokens, props.outputTokens);
  }
  return '';
});
</script>

<template>
  <div class="chat-message" :class="roleClass" :data-msg-id="id" :data-snapshot-id="snapshotId || ''">
    <div class="message-content">
      <div v-if="renderedThinking" v-html="renderedThinking"></div>
      <div v-html="renderedContent"></div>
      <div v-if="tokenInfo" v-html="tokenInfo"></div>
    </div>
  </div>
</template>

<style scoped>
.chat-message {
  margin-bottom: var(--space-md);
  animation: fadeIn var(--transition-normal);
}
.user-message {
  text-align: right;
}
.agent-message {
  text-align: left;
}
@keyframes fadeIn {
  from { opacity: 0; transform: translateY(4px); }
  to { opacity: 1; transform: translateY(0); }
}
</style>
