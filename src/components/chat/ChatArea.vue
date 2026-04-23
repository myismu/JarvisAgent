<script setup lang="ts">
import { ref, onMounted, nextTick } from 'vue';
import { useJarvis, registerScrollCb } from '../../composables/useJarvis';

const { parsedHistory, parsedCurrentTurnHtml } = useJarvis();
const responseAreaRef = ref<HTMLElement | null>(null);

const scrollToBottom = async (force = false) => {
  if (!responseAreaRef.value) return;
  const { scrollTop, scrollHeight, clientHeight } = responseAreaRef.value;
  const isAtBottom = scrollHeight - scrollTop - clientHeight <= 100;
  
  await nextTick();
  if (responseAreaRef.value && (isAtBottom || force)) {
    responseAreaRef.value.scrollTop = responseAreaRef.value.scrollHeight;
  }
};

onMounted(() => {
  registerScrollCb(scrollToBottom);
});
</script>

<template>
  <div class="response-area" ref="responseAreaRef">
    <div class="welcome-screen" v-if="!parsedHistory || parsedHistory === '<p>Ready for input...</p>\n'">
      <pre class="ascii-art">
       + ---------------- +
     .'  _.-~ ~ ~ ~ ~-._  '.
    /  .'               '.  \
   /  /    .- ~ ~ ~ -.    \  \
  |  |    /           \    |  |
  |  |   |    ( O )    |   |  |
  |  |    \           /    |  |
   \  \    '- _ _ _ -'    /  /
    \  '.               .'  /
     '.  `'-.......-'`  .'
       + ---------------- +
       
      J . A . R . V . I . S
      </pre>
      <div class="welcome-text">随时准备为您效劳，先生。</div>
    </div>
    <div class="response-text markdown-body" v-else>
      <div v-html="parsedHistory"></div>
      <div v-if="parsedCurrentTurnHtml" class="chat-message agent-message">
        <div class="message-content" v-html="parsedCurrentTurnHtml"></div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.response-area {
  flex: 1;
  display: flex;
  padding: 10px 0;
  overflow-y: auto;
  overflow-x: hidden;
  font-size: 0.9rem;
  line-height: 1.4;
  min-width: 0;
  min-height: 0;
}

.welcome-screen {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  color: var(--accent-blue);
  opacity: 0.8;
  padding: 20px;
}

.ascii-art {
  font-family: var(--font-mono);
  font-size: 14px;
  line-height: 1.2;
  white-space: pre;
  text-align: center;
  margin-bottom: 20px;
  color: #00e5ff;
  text-shadow: 0 0 5px rgba(0, 229, 255, 0.5), 0 0 15px rgba(0, 229, 255, 0.3), 0 0 30px rgba(0, 229, 255, 0.2);
  animation: reactorPulse 3s infinite alternate ease-in-out;
}

@keyframes reactorPulse {
  0% { 
    opacity: 0.8; 
    filter: drop-shadow(0 0 4px rgba(0, 229, 255, 0.4));
    transform: scale(0.98);
  }
  100% { 
    opacity: 1; 
    filter: drop-shadow(0 0 15px rgba(0, 229, 255, 0.8));
    transform: scale(1.02);
  }
}

.welcome-text {
  font-size: 16px;
  letter-spacing: 2px;
  font-weight: 500;
  color: var(--text-muted);
}

.response-text {
  flex: 1;
  padding: 0 10px;
  display: flex;
  flex-direction: column;
  gap: 16px;
}

:deep(.chat-message) {
  display: flex;
  width: 100%;
  margin-bottom: 16px;
  animation: slideIn 0.3s cubic-bezier(0.2, 0.8, 0.2, 1) forwards;
}

@keyframes slideIn {
  from { opacity: 0; transform: translateY(10px) scale(0.98); }
  to { opacity: 1; transform: translateY(0) scale(1); }
}

:deep(.user-message) {
  justify-content: flex-end;
}

:deep(.agent-message) {
  justify-content: flex-start;
}

:deep(.message-content) {
  max-width: 85%;
  padding: 14px 20px;
  border-radius: 18px;
  font-size: 0.95rem;
  line-height: 1.6;
  letter-spacing: 0.2px;
  transition: transform 0.2s cubic-bezier(0.2, 0.8, 0.2, 1), box-shadow 0.2s ease;
}

:deep(.message-content:hover) {
  transform: translateY(-1px);
}

:deep(.user-message .message-content) {
  background: linear-gradient(135deg, var(--accent-blue) 0%, #0052a3 100%);
  color: white;
  border-bottom-right-radius: 4px;
  box-shadow: 0 4px 15px rgba(0, 102, 204, 0.15);
}

:deep(.user-message .message-content:hover) {
  box-shadow: 0 6px 20px rgba(0, 102, 204, 0.25);
}

:deep(.agent-message .message-content) {
  background-color: var(--bg-sidebar);
  border: 1px solid var(--border-color);
  border-bottom-left-radius: 4px;
  box-shadow: 0 2px 10px rgba(0, 0, 0, 0.02);
}

:deep(.agent-message .message-content:hover) {
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.05);
}

/* User message specific markdown adjustments */
:deep(.user-message .message-content p) {
  margin: 0;
}
:deep(.user-message .message-content a) {
  color: rgba(255, 255, 255, 0.9);
  text-decoration: underline;
  text-underline-offset: 3px;
}
:deep(.user-message .message-content a:hover) {
  color: white;
}

/* Markdown Styles */
.response-text :deep(p) {
  margin-top: 0;
  margin-bottom: 0.3em;
}

.response-text :deep(details) {
  margin: 5px 0;
  padding: 4px 10px;
  background-color: rgba(0, 0, 0, 0.02);
  border-left: 2px solid var(--border-color);
  border-radius: 4px;
}

.response-text :deep(summary) {
  cursor: pointer;
  font-size: 0.8rem;
  color: var(--text-muted);
  user-select: none;
  outline: none;
}

.response-text :deep(summary:hover) {
  color: var(--accent-blue);
}

.response-text :deep(details[open]) {
  background-color: rgba(0, 0, 0, 0.03);
}

.response-text :deep(strong) {
  color: var(--accent-blue);
  font-weight: 600;
}
.response-text :deep(code) {
  background-color: rgba(0, 0, 0, 0.04);
  padding: 0.1em 0.3em;
  border-radius: 3px;
  font-family: var(--font-mono);
  font-size: 0.9em;
}
.response-text :deep(pre) {
  background-color: var(--bg-sidebar);
  padding: 0.5em 0.8em;
  border-radius: 6px;
  overflow-x: auto;
  border: 1px solid var(--border-color);
  margin-bottom: 0.2em;
}
.response-text :deep(pre code) {
  background-color: transparent;
  padding: 0;
}
.response-text :deep(ul), .response-text :deep(ol) {
  padding-left: 1.5em;
  margin-bottom: 0.2em;
}
.response-text :deep(li) {
  margin-bottom: 0;
}
</style>
