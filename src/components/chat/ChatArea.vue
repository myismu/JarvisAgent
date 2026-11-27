<!--
# ChatArea.vue — 主对话消息列表与交互区

渲染历史与当前流式轮次消息，处理切会话锚定、上滑懒加载、消息原地编辑、代码/表格复制与回滚菜单交互。

## Key Exports
- `ChatArea`: 主对话区域组件

## Dependencies
- Internal: `@/stores/session`, `@/stores/chat`, `@/stores/permission`, `@/stores/agent`, `@/composables/chat/*`
-->
<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { useSessionStore } from '../../stores/session';
import { useChatStore } from '../../stores/chat';
import { usePermissionStore } from '../../stores/permission';
import { useAgentStore } from '../../stores/agent';
import { usePreferences } from '../../composables/usePreferences';
import { useRunElapsed } from '../../composables/useRunElapsed';
import { getRollbackMenuPosition, useChatRollback } from '../../composables/chat/useChatRollback';
import { useChatClipboard } from '../../composables/chat/useChatClipboard';
import { useChatScroll } from '../../composables/chat/useChatScroll';
import ConfirmModal from '../common/ConfirmModal.vue';
import AgentTurn from './AgentTurn.vue';
import AgentTurnNotice from './AgentTurnNotice.vue';
import TodoPanel from './TodoPanel.vue';
import WelcomeScreen from './WelcomeScreen.vue';
import MessageRail from './MessageRail.vue';
import UserMessageBubble from './UserMessageBubble.vue';
import { noticeTextFromBackend } from '../../utils/agentTurnState';
import type { AgentTurnSnapshot, AgentCurrentTurn } from '../../types';

const { t } = useI18n();

const session = useSessionStore();
const chat = useChatStore();
const perm = usePermissionStore();
const agent = useAgentStore();
const prefs = usePreferences();
const {
  rollbackMenu,
  rollbackConfirm,
  editResendConfirm,
  rollbackLoading,
  rollbackError,
  closeRollbackMenu,
  handleRollbackClickVue,
  executeRollback,
  confirmRollback,
  submitEditedMessage,
  confirmEditResend,
  cancelEditResend,
} = useChatRollback();

/**
 * 将后端 PatchSummary.operation 映射为撤回预览弹窗里的操作标签、样式类与悬停说明。
 */
function rollbackOpMeta(operation: string): {
  kind: 'create' | 'delete' | 'update' | 'rename';
  label: string;
  hint: string;
} {
  switch (operation) {
    case 'create':
      return { kind: 'create', label: t('rollback.opCreate'), hint: t('rollback.opCreateHint') };
    case 'delete':
      return { kind: 'delete', label: t('rollback.opDelete'), hint: t('rollback.opDeleteHint') };
    case 'rename':
      return { kind: 'rename', label: t('rollback.opRename'), hint: t('rollback.opRenameHint') };
    default:
      return { kind: 'update', label: t('rollback.opUpdate'), hint: t('rollback.opUpdateHint') };
  }
}
const { copyCodeBlock, copyTable, copyPlainText } = useChatClipboard();
const currentTurn = computed(() => session.currentSessionView.currentTurn);
const currentTurnRevision = computed(() => currentTurn.value.revision);
const hasCurrentTurnContent = computed(() => {
  const turn = currentTurn.value;
  return Boolean(
    turn.textBlocks.some((block) => block.content.trim()) ||
      turn.thinkingBlocks.some((block) => block.content.trim()) ||
      turn.toolCalls.length > 0 ||
      turn.logs.some((log) => log.content.trim()) ||
      // 状态标注本身也是要展示的内容：若只统计正文，出现"只有提示没有正文"
      // （如上游静默零产出）时整轮会被判定为空而不渲染，提示随之消失。
      (turn.notice || "").trim()
  );
});

/**
 * 气泡里是否有**真内容**（不含 notice）。
 *
 * 与 `hasCurrentTurnContent` 的区别：后者把 notice 也算作内容，那是"整轮要不要渲染"的判断；
 * 而 notice 现在渲染在气泡**外面**，所以决定"气泡要不要出现"必须用这个 ——
 * 否则"只有提示没有正文"的轮次会渲染出一个空 padding 盒子（空气泡）。
 * 历史快照同理：后端 `AgentTurnSnapshot::is_empty()` 特意保留了只有 notice 的轮次。
 */
function hasBubblePayload(
  turn: Pick<AgentCurrentTurn, "textBlocks" | "thinkingBlocks" | "toolCalls" | "logs">,
): boolean {
  return Boolean(
    turn.textBlocks.some((block) => block.content.trim()) ||
      turn.thinkingBlocks.some((block) => block.content.trim()) ||
      turn.toolCalls.length > 0 ||
      turn.logs.some((log) => log.content.trim())
  );
}

// 控制 Live Turn 显示/隐藏
const showAgentTurn = ref(false);

watch(
  hasCurrentTurnContent,
  (shouldShow) => {
    if (shouldShow) {
      showAgentTurn.value = true;
    } else if (showAgentTurn.value) {
      // 立即隐藏：agent 快照已通过 messages 数组的 v-for 渲染，无需延迟
      showAgentTurn.value = false;
    }
  },
  { immediate: true }
);

/**
 * 「已发出、后端还没回包」的空窗期。
 *
 * watch(hasCurrentTurnContent) 在首个事件到达前不会打开 showAgentTurn，
 * 于是在「消息已发出」与「首个正文 / 工具事件」之间，界面是完全没有内容的——
 * 二阶段计数耗时长时这段空窗被放大，用户会以为消息没发出去。
 * status 在 send() 里就已置 RUNNING，比任何内容事件都早，用它把这段窗口托住。
 *
 * 两种视图都放开：开发者视图靠折叠头显示「执行中 · 1 步」，
 * 普通视图靠 ExecutionPanel 的摘要行显示「处理中」。
 * 两者在无内容时都不会出空气泡（ExecutionPanel 已同步支持空态）。
 */
const awaitingFirstEvent = computed(
  () =>
    !showAgentTurn.value &&
    session.currentSessionView.status === "RUNNING",
);

/**
 * 渲染用的消息列表 = 历史消息 + 当前 live 轮次。
 *
 * 为什么要把两者合并成一个 v-for：live 轮次原先走独立的 `v-if(showAgentTurn)` 块，
 * 收尾时「append 历史消息 + 清空 live 缓冲区」让两棵树分属不同分支，
 * Vue 只能卸载旧树、挂载新树 —— 整段 Markdown 重新解析
 * （StreamingMarkdown 的 frozenBoundary / frozenTokens 是实例内局部缓存），
 * 表现为内容闪一下、口径跳变。
 *
 * 合并后同一轮在列表里用同一个 key（`view.liveTurnId`，收尾时由 chat.ts
 * 的 agentMessageId 沿用），Vue 会 patch 复用组件实例，DOM 与缓存都保住。
 *
 * 去重是必需的：append 与 showAgentTurn 的更新不在同一个同步调用里，
 * 若正式消息已出现在 messages 中，虚拟项必须立刻退场，否则 key 重复。
 */
const renderMessages = computed(() => {
  const stored = chat.messages;
  const liveId = session.currentSessionView.liveTurnId;
  if ((!showAgentTurn.value && !awaitingFirstEvent.value) || !liveId) return stored;
  if (stored.some((message: any) => message.id === liveId)) return stored;
  return [...stored, { id: liveId, role: "agent", live: true }];
});

/** 消息 → 本轮 turn：live 项用内存 currentTurn，历史项用快照转换（带 WeakMap 缓存） */
function turnOfMessage(message: any): AgentCurrentTurn {
  return message.live ? currentTurn.value : convertSnapshotToTurn(message.snapshot);
}

// 运行中用时的唯一事实源：后端 `agent_runs.started_at`。
// 不再自己维护开始时刻与累计等待量（旧实现里那套状态跨会话、跨轮次，
// 一旦某条路径漏重置就会接着上一轮的数字读）。
const runningAgentRun = computed(
  () => agent.currentAgentRuns.find((run) => run.status === 'running') ?? null,
);
// 权限卡弹出期间冻结读秒：等待用户决策的时间不属于执行用时。
// 方案审批（planProposal）不参与冻结——本轮在提交方案时已结束，不会在读秒。
const awaitingDecision = computed(
  () => Boolean(perm.permissionRequest || perm.userQuestion),
);
const { elapsed: runElapsed } = useRunElapsed(runningAgentRun, awaitingDecision);

// 将 AgentTurnSnapshot 转换为 AgentCurrentTurn 格式（供 AgentTurn 组件渲染历史消息）
// 注意：**新增快照字段时必须同步到这里**，否则该字段在界面永远不生效
// （notice 就曾因为漏拷而完全不显示）。
//
// 缓存：同一个 snapshot 对象 → 同一个 turn 对象。
// 动机：历史消息在模板里写的是 :turn="convertSnapshotToTurn(...)"。若每次调用都新建对象，
// 则 ChatArea 每次重渲染（运行期约 1 Hz，节拍来自 useRunElapsed 的读秒）都会让全部历史
// AgentTurn 的 turn prop 引用变化，使 shouldUpdateComponent 判定「props 变了」并整块
// 重渲染这些子树（代价 O(历史条数)）。缓存后同一 snapshot 恒返回同一引用，该开销归零。
// 用 WeakMap 而非 Map：snapshot 被回收时条目自动消失，无需手动清理。
// 声明在 <script setup> 顶层 = 每组件实例一份；ChatArea 仅挂载一次，够用。
//
// ⚠️ 不变量：本缓存依赖 snapshot 不可变。
// 若将来有代码需要原地改写 snapshot（改 textBlocks / notice，其中 textBlocks 若是整个数组
// 被替换，引用会变），必须先让它脱离本缓存，否则会拿到陈旧值。
// 历史上唯一的原地改写者是 utils/agentTurnRender.ts 的 extractInterruptNotice（旧 HTML
// 渲染路径），该路径已随死代码清理删除，当前无人在写 snapshot。
const turnCache = new WeakMap<AgentTurnSnapshot, AgentCurrentTurn>();

function convertSnapshotToTurn(snapshot: AgentTurnSnapshot): AgentCurrentTurn {
  const cached = turnCache.get(snapshot);
  if (cached) return cached;

  const turn: AgentCurrentTurn = {
    id: snapshot.createdAt.toString(),
    loop: 1,
    revision: 1,
    isRunning: false,
    hasToolActivity: snapshot.toolCalls.length > 0,
    activeTextBlockId: null,
    activeThinkingBlockId: null,
    textBlocks: snapshot.textBlocks,
    thinkingBlocks: snapshot.thinkingBlocks,
    toolCalls: snapshot.toolCalls,
    logs: snapshot.logs,
    // 状态标注（气泡下方小字）：中断/取消/等待说明。
    // 后端只给 i18n key，这里用语言包成句 —— 切语言时这条标注会跟着变。
    notice: noticeTextFromBackend(snapshot.noticeI18nKey, snapshot.noticeDetail),
    startedAt: snapshot.createdAt,
    // 轮次真实用时（毫秒）：折叠头「用时 N 秒」直接读它，不再用首尾时间戳差推算
    durationMs: snapshot.durationMs,
  };

  turnCache.set(snapshot, turn);
  return turn;
}

const editingMsgId = ref<string | null>(null);

function startEditUserMsg(message: any) {
  if (session.isCurrentSessionRunning || rollbackLoading.value) return;
  closeRollbackMenu();
  editingMsgId.value = message.id;
}

function cancelEditUserMsg() {
  if (rollbackLoading.value) return;
  editingMsgId.value = null;
}

async function handleConfirmEditUserMsg(message: any, nextText: string) {
  if (rollbackLoading.value || session.isCurrentSessionRunning) return;
  await submitEditedMessage(message, nextText, () => {
    editingMsgId.value = null;
  });
}

function isRollbackMenuActiveFor(message: any): boolean {
  if (!rollbackMenu.value.visible) return false;
  if (message.messageId && rollbackMenu.value.messageId === message.messageId) return true;
  if (message.rollbackCheckpointId && rollbackMenu.value.snapshotId === message.rollbackCheckpointId) {
    return true;
  }
  return false;
}

const {
  responseAreaRef,
  initialAnchoring,
  showScrollToBottom,
  showHistoryLoader,
  historyLoaderText,
  scrollToBottom,
  handleResponseScroll,
} = useChatScroll({
  hasCurrentTurnContent,
  currentTurnRevision,
  onSessionChange: () => {
    // 切换会话时退出可能停留的原地编辑态，避免跨会话误操作
    editingMsgId.value = null;
  },
});

const handleContextMenu = (e: MouseEvent) => {
  const target = e.target as HTMLElement;
  const messageEl = target.closest('.chat-message.agent-message');
  if (!messageEl) return;

  const snapshotId = messageEl.getAttribute('data-snapshot-id');
  if (!snapshotId) return;

  e.preventDefault();
  const position = getRollbackMenuPosition(e.clientX, e.clientY, 'session');
  rollbackMenu.value = {
    visible: true,
    x: position.left,
    y: position.top,
    snapshotId,
    rollbackMode: 'session',
    messageId: null,
    fallbackSnapshotId: '',
  };
};

const handleResponseClick = (e: MouseEvent) => {
  const target = e.target as HTMLElement;
  const codeCopyButton = target.closest<HTMLButtonElement>('.code-copy-btn');
  if (codeCopyButton) {
    e.preventDefault();
    e.stopPropagation();
    void copyCodeBlock(codeCopyButton);
    return;
  }

  const tableCopyButton = target.closest<HTMLButtonElement>('.table-copy-btn');
  if (tableCopyButton) {
    e.preventDefault();
    e.stopPropagation();
    void copyTable(tableCopyButton);
    return;
  }
};

const handleDocumentClick = (e: MouseEvent) => {
  const target = e.target as HTMLElement | null;
  if (!target) return;
  if (target.closest('.rollback-trigger')) return;
  if (!target.closest('.rollback-menu')) {
    closeRollbackMenu();
  }
};

onMounted(() => {
  document.addEventListener('click', handleDocumentClick);
});

onUnmounted(() => {
  document.removeEventListener('click', handleDocumentClick);
});
</script>

<template>
  <div class="response-area" ref="responseAreaRef" @scroll="handleResponseScroll" @contextmenu="handleContextMenu" @click="handleResponseClick">
    <MessageRail :container="responseAreaRef" />
    <TodoPanel />
    <WelcomeScreen v-if="!chat.messages.length && !showAgentTurn && !session.currentSessionView.loadError" />
    <div class="response-text markdown-body" v-else :class="{ 'pre-anchor-hidden': initialAnchoring }">
      <!-- 懒加载：更早历史提示（顶部小字，无边框；加载中带旋转图标） -->
      <div v-if="showHistoryLoader" class="history-loader">
        <svg
          v-if="session.currentSessionView.pagedLoadingOlder"
          class="history-loader-spinner"
          viewBox="0 0 24 24"
          width="12"
          height="12"
          aria-hidden="true"
        >
          <circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" stroke-width="3" stroke-dasharray="42 21" stroke-linecap="round" />
        </svg>
        {{ historyLoaderText }}
      </div>
      <!-- 会话消息加载失败：取代原先那条 HTML 兜底。
           兜底渲染的是另一套已不被渲染的 HTML，出问题时既不是同一套样式、
           文案还是写死的中文；改为一句可翻译的提示。 -->
      <div v-if="session.currentSessionView.loadError" class="session-load-error">
        {{ t('chat.loadFailed') }}
      </div>
      <!-- 结构化消息列表（Vue 组件渲染） -->
      <template v-for="(message, index) in renderMessages" :key="message.id">
        <!-- 用户消息 -->
        <UserMessageBubble
          v-if="message.role === 'user'"
          :message="message"
          :index="index"
          :is-editing="editingMsgId === message.id"
          :menu-open="isRollbackMenuActiveFor(message)"
          :disabled="session.isCurrentSessionRunning || rollbackLoading"
          :rollback-loading="rollbackLoading"
          :copy-plain-text="copyPlainText"
          @start-edit="startEditUserMsg"
          @cancel-edit="cancelEditUserMsg"
          @confirm-edit="handleConfirmEditUserMsg"
          @open-rollback="handleRollbackClickVue"
        />
        <!-- Agent 消息：历史快照与 live 轮次走**同一条**渲染语句。
             live 项由 renderMessages 追加在列表末尾，key 与收尾后的正式消息一致，
             所以收尾时 Vue 会 patch 复用这棵子树，而不是卸载再挂载（那会闪一下）。 -->
        <div v-else-if="message.role === 'agent' && (message.live || message.snapshot)" class="chat-message agent-message" :data-msg-id="message.id">
          <!-- 只有状态标注、没有正文的轮次不渲染气泡：notice 在气泡外，气泡会是个空盒子。
               例外：空窗期（已发出、后端未回包）要靠气泡里的状态行给用户反馈。 -->
          <div v-if="hasBubblePayload(turnOfMessage(message)) || (message.live && awaitingFirstEvent)" class="message-content current-turn-content">
            <AgentTurn
              :turn="turnOfMessage(message)"
              :display-mode="prefs.agentAudience.value"
              :elapsed="message.live ? runElapsed : 0"
              :status="message.live ? session.currentSessionView.status : message.snapshot.status"
              :awaiting="message.live && awaitingFirstEvent"
            />
          </div>
          <!-- 状态标注刻意放在气泡**外面**：它是这一轮的运行状态，不是模型说的话。
               放气泡内会跟着 padding 缩进，看起来像模型正文的一部分。
               live 时才传 running，历史轮次没有「运行中」这个概念。 -->
          <AgentTurnNotice
            :notice="turnOfMessage(message).notice"
            :tone="message.snapshot?.status === 'ERROR' || message.snapshot?.status === 'FAILED' ? 'error' : undefined"
            :running="message.live ? currentTurn.isRunning || awaitingFirstEvent : undefined"
          />
        </div>
      </template>
      <Transition name="notice-fade">
        <div v-if="chat.memoryNotice" class="memory-notice" @click="chat.memoryNotice = null">
          <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>
          {{ chat.memoryNotice }}
        </div>
      </Transition>
    </div>

    <Teleport to="body">
      <div
        v-if="rollbackMenu.visible"
        class="rollback-menu"
        :style="{ left: rollbackMenu.x + 'px', top: rollbackMenu.y + 'px' }"
      >
        <div class="rollback-menu-title">{{ t('rollback.selectMode') }}</div>
        <button v-if="rollbackMenu.rollbackMode === 'both'" class="rollback-menu-item" @click="executeRollback('both')" :disabled="rollbackLoading">
          <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><polyline points="1 4 1 10 7 10"></polyline><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"></path></svg>
          {{ t('rollback.sessionAndCode') }}
        </button>
        <button class="rollback-menu-item" @click="executeRollback('session')" :disabled="rollbackLoading">
          <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path></svg>
          {{ t('rollback.sessionOnly') }}
        </button>
      </div>
    </Teleport>

    <ConfirmModal
      :open="!!rollbackConfirm"
      :title="rollbackConfirm?.title || ''"
      :message="rollbackConfirm?.message || ''"
      :warning="rollbackError || rollbackConfirm?.warning || ''"
      :confirm-text="rollbackConfirm?.noticeOnly ? t('common.confirm') : t('rollback.confirm')"
      :cancel-text="rollbackConfirm?.noticeOnly ? t('rollback.closeNotice') : t('common.cancel')"
      :confirm-kind="rollbackConfirm?.noticeOnly ? 'primary' : 'danger'"
      :loading="rollbackLoading"
      @cancel="rollbackConfirm = null"
      @confirm="confirmRollback"
    >
      <template v-if="rollbackConfirm?.mode === 'both'" #message>
        <div class="rollback-preview-modal">
          <p class="rollback-preview-message">{{ rollbackConfirm.message }}</p>
          <div v-if="rollbackConfirm.files.length > 0" class="rollback-preview-files">
            <div class="rollback-preview-summary">{{ t('rollback.previewSummary', { count: rollbackConfirm.files.length }) }}</div>
            <div class="rollback-preview-file-list">
              <div v-for="file in rollbackConfirm.files" :key="file.path" class="rollback-preview-file">
                <span
                  class="rollback-preview-op"
                  :class="`op-${rollbackOpMeta(file.operation).kind}`"
                  :title="rollbackOpMeta(file.operation).hint"
                >{{ rollbackOpMeta(file.operation).label }}</span>
                <span class="rollback-preview-path" :title="file.path">{{ file.path }}</span>
                <span v-if="file.linesAdded > 0 || file.linesRemoved > 0" class="rollback-preview-stats">
                  <span v-if="file.linesAdded > 0" class="rollback-preview-added">+{{ file.linesAdded }}</span>
                  <span v-if="file.linesRemoved > 0" class="rollback-preview-removed">-{{ file.linesRemoved }}</span>
                </span>
              </div>
            </div>
          </div>
          <p v-else class="rollback-preview-empty">{{ t('rollback.previewEmpty') }}</p>
        </div>
      </template>
    </ConfirmModal>

    <!-- 原地编辑后存在关联代码变更时的二选一确认弹窗 -->
    <ConfirmModal
      :open="!!editResendConfirm"
      :title="editResendConfirm?.title || ''"
      :message="editResendConfirm?.message || ''"
      :warning="rollbackError || editResendConfirm?.warning || ''"
      confirm-kind="danger"
      :loading="rollbackLoading"
      @cancel="cancelEditResend"
      @confirm="confirmEditResend('session', () => { editingMsgId = null; })"
    >
      <template #message>
        <div v-if="editResendConfirm" class="rollback-preview-modal">
          <p class="rollback-preview-message">{{ editResendConfirm.message }}</p>
          <div v-if="editResendConfirm.files.length > 0" class="rollback-preview-files">
            <div class="rollback-preview-summary">{{ t('rollback.previewSummary', { count: editResendConfirm.files.length }) }}</div>
            <div class="rollback-preview-file-list">
              <div v-for="file in editResendConfirm.files" :key="file.path" class="rollback-preview-file">
                <span
                  class="rollback-preview-op"
                  :class="`op-${rollbackOpMeta(file.operation).kind}`"
                  :title="rollbackOpMeta(file.operation).hint"
                >{{ rollbackOpMeta(file.operation).label }}</span>
                <span class="rollback-preview-path" :title="file.path">{{ file.path }}</span>
                <span v-if="file.linesAdded > 0 || file.linesRemoved > 0" class="rollback-preview-stats">
                  <span v-if="file.linesAdded > 0" class="rollback-preview-added">+{{ file.linesAdded }}</span>
                  <span v-if="file.linesRemoved > 0" class="rollback-preview-removed">-{{ file.linesRemoved }}</span>
                </span>
              </div>
            </div>
          </div>
          <p v-else class="rollback-preview-empty">{{ t('rollback.previewEmpty') }}</p>
        </div>
      </template>
      <template #actions>
        <div class="edit-resend-actions">
          <button
            type="button"
            class="edit-resend-btn ghost"
            :disabled="rollbackLoading"
            @click="cancelEditResend"
          >
            {{ t('common.cancel') }}
          </button>
          <div class="edit-resend-primary-group">
            <button
              type="button"
              class="edit-resend-btn secondary"
              :disabled="rollbackLoading"
              @click="confirmEditResend('session', () => { editingMsgId = null; })"
            >
              {{ rollbackLoading ? t('common.processing') : t('rollback.editResendSessionOnly') }}
            </button>
            <button
              type="button"
              class="edit-resend-btn danger"
              :disabled="rollbackLoading"
              @click="confirmEditResend('both', () => { editingMsgId = null; })"
            >
              {{ rollbackLoading ? t('common.processing') : t('rollback.editResendSessionAndCode') }}
            </button>
          </div>
        </div>
      </template>
    </ConfirmModal>

    <Transition name="scroll-btn">
      <button
        v-if="showScrollToBottom"
        class="scroll-to-bottom-btn"
        @click="scrollToBottom(true)"
        :title="t('rollback.scrollToBottom')"
      >
        <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="6 9 12 15 18 9"></polyline>
        </svg>
      </button>
    </Transition>
  </div>
</template>

<style scoped>
.response-area {
  flex: 1;
  display: flex;
  flex-direction: column;
  /* 底部留白 = 浮动输入区的实际高度 + 24px 呼吸位（由 TerminalInput 测量的
     --input-area-height 提供；兜底 200px 只在首帧变量就绪前生效） */
  padding: 16px 0 calc(var(--input-area-height, 200px) + 24px);
  overflow-y: auto;
  overflow-x: hidden;
  font-size: 0.95rem;
  line-height: 1.6;
  min-width: 0;
  min-height: 0;
  /* 禁用 smooth：程序化 scrollTop 赋值必须是瞬时跳变——smooth 会让每次
     赋值都变成动画（进入会话"从中间滑到底"、上滑锚定"回弹"的根源），
     且动画期间反复触发 scroll 事件与状态竞争。手动滚动不受影响。 */
}



.response-text {
  flex: 1;
  width: 100%;
  max-width: min(85%, 960px); /* 与输入框同一套宽度口径，超宽屏 960px 封顶 */
  margin: 0 auto; /* 居中 */
  display: flex;
  flex-direction: column;
  gap: 24px;
}

:deep(.chat-message) {
  display: flex;
  width: 100%;
  margin-bottom: 20px;
  animation: slideIn var(--transition-normal) forwards;
}

.history-html :deep(.chat-message) {
  animation: none;
}

@keyframes slideIn {
  from { opacity: 0; transform: translateY(8px) scale(0.98); }
  to { opacity: 1; transform: translateY(0) scale(1); }
}

:deep(.user-message) {
  flex-direction: column;
  align-items: flex-end;
  position: relative;
  gap: 4px;
}

:deep(.agent-message) {
  justify-content: flex-start;
  /* 纵向排列：气泡在上，状态标注（AgentTurnNotice）在下。
     加这两条是为了让 notice **脱离气泡**渲染 —— 它有独立的宽度与左边界，
     不再跟着气泡的 padding: 14px 22px 一起缩进。
     align-items: flex-start 让每个子元素宽度各自 fit-content：
     长文本下 fit-content 会撑到容器宽度再换行，与原先 flex row 下
     "气泡宽度由内容决定（上限 85%）"的行为一致。 */
  flex-direction: column;
  align-items: flex-start;
}

:deep(.message-content) {
  max-width: 85%; /* 自适应宽度，留出边距 */
  padding: 14px 22px;
  border-radius: var(--radius-xl);
  font-size: 0.95rem;
  line-height: 1.6;
  letter-spacing: 0.01em;
  transition: transform var(--transition-fast), box-shadow var(--transition-fast);
  word-wrap: break-word;
}

:deep(.message-content:hover) {
  transform: translateY(-1px);
}

:deep(.user-message .message-content) {
  color: var(--text-main);
  border-bottom-right-radius: 4px;
}

:deep(.user-message .message-content:hover) {
  box-shadow: var(--shadow-md);
}

:deep(.agent-message .message-content) {
  max-width: 100%; /* agent 回复放开满宽，长内容（代码块/表格）不再被挤到一侧 */
  border-bottom-left-radius: 4px;
}

:deep(.agent-message .message-content:hover) {
  /* transparent, no hover effect needed */
}

:deep(.user-message .message-content p) {
  margin: 0;
}
:deep(.user-message .message-content a) {
  color: var(--accent-blue);
  text-decoration: underline;
  text-underline-offset: 3px;
}
:deep(.user-message .message-content a:hover) {
  color: var(--accent-blue-hover);
}

/* 用户图片 */
:deep(.user-images) {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-bottom: 8px;
}
:deep(.user-image) {
  max-width: 200px;
  max-height: 200px;
  border-radius: 8px;
  display: inline-block;
  vertical-align: middle;
}

:deep(.user-text) {
  white-space: pre-wrap;
  word-break: break-word;
}

/* 长消息折叠 */
:deep(.user-text.collapsed) {
  max-height: 180px;
  overflow: hidden;
  position: relative;
}
:deep(.user-text.collapsed::after) {
  content: '';
  position: absolute;
  bottom: 0;
  left: 0;
  right: 0;
  height: 48px;
  background: linear-gradient(transparent, var(--glass-bg-heavy));
  pointer-events: none;
}
:deep(.user-msg-toggle) {
  display: block;
  margin-top: 6px;
  padding: 2px 0;
  background: none;
  border: none;
  color: var(--accent-blue);
  font-size: 0.8rem;
  cursor: pointer;
  font-family: inherit;
}
:deep(.user-msg-toggle:hover) {
  color: var(--accent-blue-hover);
  text-decoration: underline;
}

.response-text :deep(p) {
  margin-top: 0;
  margin-bottom: 0.75em;
}

/* 以下通用样式只服务 Markdown 内容（StreamingMarkdown 渲染出来的 details /
   summary / code）。必须限定在 .streaming-markdown 内：执行时间线的每一行也是
   details / summary / code，如果不限定，会给时间线行加上 padding、外边距和
   灰色胶囊底色（实测曾把 22px 的行撑到 37px，并顶掉组件自己的样式）。 */
.response-text :deep(.streaming-markdown details) {
  margin: 12px 0;
  padding: 8px 12px;
  background: color-mix(in srgb, var(--glass-bg-light) calc(var(--agent-message-opacity, 0) * 1%), transparent);
  border-radius: var(--radius-md);
  transition: background-color var(--transition-fast);
}

.response-text :deep(.streaming-markdown details:hover) {
  background: color-mix(in srgb, var(--glass-bg) calc(var(--agent-message-opacity, 0) * 1%), transparent);
}

.response-text :deep(.streaming-markdown summary) {
  cursor: pointer;
  font-size: 0.85rem;
  font-weight: 500;
  color: var(--text-muted);
  user-select: none;
  outline: none;
  transition: color var(--transition-fast);
}

.response-text :deep(.streaming-markdown summary:hover) {
  color: var(--text-main);
}

.response-text :deep(.streaming-markdown details[open]) {
  background: color-mix(in srgb, var(--glass-bg) calc(var(--agent-message-opacity, 0) * 1%), transparent);
}

.current-turn-content,
.response-text :deep(.current-turn-content) {
  position: relative;
  width: 100%;
  max-width: 100%; /* agent 满宽（user 气泡走 .message-content 的 85%，不含此类） */
  overflow: hidden;
}

.current-turn-content.waiting-only {
  min-width: auto;
  min-height: 34px;
  display: inline-flex;
  align-items: center;
  justify-content: flex-start;
}

.current-turn-content :deep(details:first-child),
.response-text :deep(.current-turn-content details:first-child) {
  margin-top: 0;
}

.current-turn-content :deep(summary),
.response-text :deep(.current-turn-content summary) {
  padding-right: 0;
}

.response-text :deep(a) {
  color: var(--accent-blue);
  text-decoration: none;
}

.response-text :deep(a:hover) {
  text-decoration: underline;
}

.response-text :deep(strong) {
  color: var(--text-main);
  font-weight: var(--fw-bold);
}

.response-text :deep(em) {
  font-style: italic;
}

/* 行内代码胶囊的样式统一在 StreamingMarkdown.vue 里维护，全局只留那一份。
   这里曾有一份同值拷贝（padding 2px 6px / border #ececec / radius 5px / 文字灰 15% 底色），
   当初加它只是为了把胶囊限制在 .streaming-markdown 内、不污染执行时间线；
   组件内的 :not(pre) > code 已经排除了代码块，且作用域天然跟组件走，
   再留一份只会让两处取值永远漂移（本次改动前就漂移了两次）。 */

.response-text :deep(.markdown-code-block),
.response-text :deep(.md-code-block),
.response-text :deep(.markdown-table-wrap) {
  margin: 12px 0 16px;
  overflow: hidden;
  border: 1px solid color-mix(in srgb, var(--text-muted) 18%, transparent);
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--surface-strong) 72%, var(--glass-bg-heavy));
  box-shadow: var(--shadow-sm);
}

/* 代码块与表格的头部容器：几何与配色共用，但**字号不共用** ——
   代码头部走代码口径（--code-font-size），表格头部是正文内容、跟正文口径。 */
.response-text :deep(.markdown-code-header),
.response-text :deep(.md-code-header),
.response-text :deep(.markdown-table-header) {
  min-height: 34px;
  padding: 6px 8px 6px 12px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  border-bottom: 1px solid color-mix(in srgb, var(--text-muted) 14%, transparent);
  background: color-mix(in srgb, var(--glass-bg-heavy) 72%, transparent);
  color: var(--text-muted);
  font-weight: var(--fw-semibold);
}

.response-text :deep(.markdown-table-header) {
  font-size: 0.76rem;
}

/* 代码头部按代码字号排版：它内部的 .md-code-lang / .markdown-copy-btn 用 em 相对这里，
   于是语言标签与复制按钮会随代码字号一起缩放。之前它们是固定 rem ——
   界面字号 19 + 代码字号「小」时，标签(14px)会比代码本身(11px)还大。 */
.response-text :deep(.markdown-code-header),
.response-text :deep(.md-code-header) {
  font-size: var(--code-font-size);
}

.response-text :deep(.markdown-code-language),
.response-text :deep(.md-code-lang),
.response-text :deep(.markdown-table-header span) {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.response-text :deep(.markdown-copy-btn) {
  /* 盒子也按 em 走：只用固定 24px / 48px 的话，特大字号下文字会顶出按钮 */
  height: 2.2em;
  min-width: 4.4em;
  padding: 0 9px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid color-mix(in srgb, var(--text-muted) 18%, transparent);
  border-radius: 6px;
  background: color-mix(in srgb, var(--surface-strong) 82%, transparent);
  color: var(--text-muted);
  font: inherit;
  font-size: 0.85em;
  font-weight: var(--fw-semibold);
  cursor: pointer;
  transition: color var(--transition-fast), border-color var(--transition-fast), background var(--transition-fast);
}

.response-text :deep(.markdown-copy-btn:hover) {
  color: var(--accent-blue);
  border-color: color-mix(in srgb, var(--accent-blue) 38%, transparent);
  background: color-mix(in srgb, var(--accent-blue) 9%, var(--surface-strong));
}

.response-text :deep(.markdown-copy-btn.copied) {
  color: var(--accent-green);
  border-color: color-mix(in srgb, var(--accent-green) 42%, transparent);
  background: color-mix(in srgb, var(--accent-green) 10%, var(--surface-strong));
}

.response-text :deep(pre) {
  background: var(--bg-dark);
  padding: 12px;
  border-radius: var(--radius-md);
  overflow-x: auto;
  border: 1px solid var(--glass-border);
  margin-bottom: 0.75em;
  box-shadow: inset 0 2px 4px rgba(0,0,0,0.05);
}

.response-text :deep(.markdown-code-block pre),
.response-text :deep(.md-code-block pre) {
  margin: 0;
  padding: 14px 16px;
  border: 0;
  border-radius: 0;
  background: color-mix(in srgb, var(--bg-dark) 82%, var(--surface-strong));
  box-shadow: none;
}

.response-text :deep(pre code) {
  background-color: transparent;
  padding: 0;
  color: inherit;
  font-size: var(--code-font-size);
  border: none;
}

.response-text :deep(.markdown-table-scroll),
.response-text :deep(.md-table-wrap) {
  overflow-x: auto;
}

.response-text :deep(.md-table-wrap) {
  margin: 12px 0;
  overflow-x: auto;
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
}

.response-text :deep(table),
.response-text :deep(.md-table) {
  width: 100%;
  border-collapse: separate;
  border-spacing: 0;
  /* 与单元格、段落、列表统一到同一个正文口径（原 0.88rem 会和单元格的
     0.9rem 打架，表格内外差 0.3px）。这里用 --md-body-font-size 而不是直接写
     --content-font-size：工具输出容器会把这个变量改写成代码字号。 */
  font-size: var(--md-body-font-size);
}

.response-text :deep(th),
.response-text :deep(td),
.response-text :deep(.md-table th),
.response-text :deep(.md-table td) {
  padding: 9px 12px;
  text-align: left;
  vertical-align: top;
  border-right: 1px solid color-mix(in srgb, var(--text-muted) 13%, transparent);
  border-bottom: 1px solid color-mix(in srgb, var(--text-muted) 13%, transparent);
}

.response-text :deep(th:last-child),
.response-text :deep(td:last-child),
.response-text :deep(.md-table th:last-child),
.response-text :deep(.md-table td:last-child) {
  border-right: 0;
}

.response-text :deep(tr:last-child td),
.response-text :deep(.md-table tr:last-child td) {
  border-bottom: 0;
}

.response-text :deep(th),
.response-text :deep(.md-table th) {
  color: var(--text-main);
  font-weight: 700;
  /* 表头去掉蓝底：这里与 StreamingMarkdown 的 .md-table th 是同名规则、
     特异性也相同，谁后加载谁生效。两边写成同一个中性底色才没有隐性依赖 */
  background: color-mix(in srgb, var(--bg-panel) 50%, transparent);
}

.response-text :deep(td code),
.response-text :deep(.md-table td code) {
  white-space: nowrap;
}

.response-text :deep(del) {
  text-decoration: line-through;
  color: var(--text-muted);
}

.response-text :deep(ul), .response-text :deep(ol) {
  padding-left: 1.5em;
  margin-bottom: 0.75em;
}

.response-text :deep(li) {
  margin-bottom: 0.25em;
}

/* 约束：:deep() 是 Vue 的编译期指令，只能写在组件 scoped 样式里；
   写进 global.css 会被浏览器当非法选择器整条丢弃（曾踩过这个坑）。 */

.rollback-menu {
  position: fixed;
  z-index: 10000;
  background: var(--glass-bg-heavy);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  padding: 8px;
  min-width: 180px;
  animation: popIn 0.15s ease-out;
}

@keyframes popIn {
  from { opacity: 0; transform: scale(0.95); }
  to { opacity: 1; transform: scale(1); }
}

.rollback-menu-title {
  font-size: 0.75rem;
  font-weight: 600;
  color: var(--text-soft);
  padding: 6px 10px;
  border-bottom: 1px solid var(--glass-border-subtle);
  margin-bottom: 4px;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.rollback-menu-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 8px 12px;
  background: transparent;
  border: none;
  border-radius: var(--radius-md);
  color: var(--text-main);
  font-size: 0.85rem;
  cursor: pointer;
  transition: all var(--transition-fast);
  text-align: left;
}

.rollback-menu-item:hover:not(:disabled) {
  background: var(--glass-bg-light);
}

.rollback-menu-item:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.rollback-menu-item.cancel {
  margin-top: 4px;
  border-top: 1px solid var(--glass-border-subtle);
  color: var(--text-muted);
}

.rollback-menu-item.cancel:hover {
  color: var(--accent-red);
  background: color-mix(in srgb, var(--accent-red) 10%, transparent);
}

.rollback-preview-modal {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.rollback-preview-message,
.rollback-preview-empty {
  margin: 0;
  color: var(--text-soft);
  line-height: 1.65;
  font-size: 0.86rem;
}

.rollback-preview-empty {
  color: var(--text-muted);
}

.rollback-preview-files {
  border: 1px solid color-mix(in srgb, var(--text-muted) 18%, transparent);
  border-radius: var(--radius-md);
  background: var(--msg-block-tint);
  overflow: hidden;
}

.rollback-preview-summary {
  padding: 7px 12px;
  color: var(--text-muted);
  font-size: 0.76rem;
  font-weight: 600;
  background: color-mix(in srgb, var(--text-main) 2%, transparent);
  border-bottom: 1px solid color-mix(in srgb, var(--text-muted) 14%, transparent);
}

.rollback-preview-file-list {
  max-height: 220px;
  overflow-y: auto;
}

.rollback-preview-file {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  font-family: var(--font-mono);
  font-size: 0.82rem;
  border-bottom: 1px solid var(--glass-border-subtle);
}

.rollback-preview-file:last-child {
  border-bottom: none;
}

.rollback-preview-op {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
  padding: 2px 6px;
  border-radius: 5px;
  font-family: var(--font-sans);
  font-size: 0.72rem;
  font-weight: 600;
  line-height: 1.3;
}

.rollback-preview-op.op-create {
  color: var(--accent-green);
  background: color-mix(in srgb, var(--accent-green) 12%, transparent);
}

.rollback-preview-op.op-delete {
  color: var(--accent-red);
  background: color-mix(in srgb, var(--accent-red) 12%, transparent);
}

.rollback-preview-op.op-update,
.rollback-preview-op.op-rename {
  color: var(--text-muted);
  background: color-mix(in srgb, var(--text-muted) 12%, transparent);
}

.rollback-preview-path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-main);
}

.rollback-preview-stats {
  display: inline-flex;
  gap: 8px;
  flex-shrink: 0;
  font-weight: 700;
}

.rollback-preview-added {
  color: var(--accent-green);
}

.rollback-preview-removed {
  color: var(--accent-red);
}

/* 用户消息操作栏：位于气泡下方右侧，无外框纯幽灵图标组 */
:deep(.user-msg-actions) {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  height: 26px;
  padding-right: 2px;
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.15s ease;
  -webkit-app-region: no-drag;
}

:deep(.user-message:hover .user-msg-actions),
:deep(.user-message:focus-within .user-msg-actions),
:deep(.user-message.menu-open .user-msg-actions) {
  opacity: 1;
  pointer-events: auto;
}

:deep(.user-action-btn) {
  width: 26px;
  height: 26px;
  padding: 0;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  transition: background 0.15s ease, color 0.15s ease, opacity 0.15s ease;
}

:deep(.user-action-btn:hover:not(:disabled)) {
  background: color-mix(in srgb, var(--text-main) 8%, transparent);
  color: var(--text-main);
}

:deep(.user-action-btn:active:not(:disabled)) {
  background: color-mix(in srgb, var(--text-main) 13%, transparent);
}

:deep(.user-action-btn:disabled) {
  opacity: 0.35;
  cursor: not-allowed;
}

:deep(.user-action-btn.is-copied) {
  color: var(--accent-green);
}

/* 原地编辑态：气泡平滑切换为聚焦输入框，内部输入与按钮样式归属 UserMessageBubble.vue */
:deep(.user-message .message-content.is-editing) {
  width: min(560px, 85%);
  border: 1px solid color-mix(in srgb, var(--accent-blue) 65%, transparent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent-blue) 12%, transparent);
  transform: none;
}

/* 编辑重发二选一弹窗底部操作栏：左幽灵取消，右紧凑双操作键 */
.edit-resend-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  width: 100%;
}

.edit-resend-primary-group {
  display: flex;
  align-items: center;
  gap: 8px;
}

.edit-resend-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  height: 32px;
  padding: 0 12px;
  border-radius: 8px;
  font-size: 0.8rem;
  font-weight: 500;
  cursor: pointer;
  white-space: nowrap;
  border: 1px solid transparent;
  transition: background 0.14s ease, border-color 0.14s ease, color 0.14s ease, opacity 0.14s ease;
}

.edit-resend-btn.ghost {
  padding: 0 6px;
  background: transparent;
  color: var(--text-muted);
}

.edit-resend-btn.ghost:hover:not(:disabled) {
  color: var(--text-main);
}

.edit-resend-btn.secondary {
  background: transparent;
  color: var(--text-main);
  border-color: color-mix(in srgb, var(--text-main) 14%, transparent);
}

.edit-resend-btn.secondary:hover:not(:disabled) {
  background: color-mix(in srgb, var(--text-main) 5%, transparent);
  border-color: color-mix(in srgb, var(--text-main) 22%, transparent);
}

.edit-resend-btn.danger {
  background: var(--surface-danger);
  color: var(--text-on-danger);
}

.edit-resend-btn.danger:hover:not(:disabled) {
  background: var(--surface-danger-hover);
}

.edit-resend-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.thinking-timer {
  font-family: var(--font-mono);
  font-size: 0.8rem;
  font-weight: 600;
  color: var(--text-muted);
  opacity: 1 !important;
  animation: none !important;
  font-variant-numeric: tabular-nums;
  letter-spacing: 0.02em;
}

/* 滚动到底部按钮：贴在浮动输入区上沿 12px，随输入框高度自适应 */
.scroll-to-bottom-btn {
  position: absolute;
  bottom: calc(var(--input-area-height, 180px) + 12px);
  left: 50%;
  transform: translateX(-50%);
  z-index: 10;
  width: 36px;
  height: 36px;
  border-radius: 50%;
  border: 1px solid var(--glass-border);
  background: var(--glass-bg-heavy);
  color: var(--accent-blue);
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  box-shadow: var(--shadow-md);
  transition: all var(--transition-fast);
}
.scroll-to-bottom-btn:hover {
  background: var(--glass-bg);
  border-color: var(--accent-blue);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent-blue) 20%, transparent);
  transform: translateX(-50%) scale(1.08);
}
.scroll-to-bottom-btn:active {
  transform: translateX(-50%) scale(0.95);
}

.scroll-btn-enter-active,
.scroll-btn-leave-active {
  transition: opacity 0.2s ease, transform 0.2s ease;
}
.scroll-btn-enter-from,
.scroll-btn-leave-to {
  opacity: 0;
  transform: translateX(-50%) translateY(8px);
}

/* 全局记忆更新提示（仅内存，不持久化） */
.memory-notice {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin: 0 16px 8px;
  padding: 6px 12px;
  border-radius: 6px;
  background: var(--glass-bg-light);
  color: var(--text-muted);
  font-size: 0.73rem;
  cursor: pointer;
  user-select: none;
  max-width: fit-content;
  transition: background 0.15s;
}
.memory-notice:hover {
  background: var(--glass-bg);
  color: var(--text-main);
}
.memory-notice svg {
  flex-shrink: 0;
  opacity: 0.85;
}

/* 懒加载：更早历史提示（顶部小字，无边框，与通知条同风格） */
.history-loader {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 8px 0 4px;
  color: var(--text-muted);
  font-size: 0.73rem;
  user-select: none;
}

/* 会话消息加载失败提示：与 history-loader 同一档视觉重量（不抢眼，但要看得见） */
.session-load-error {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 10px 12px;
  margin: 8px 0;
  border: 1px dashed var(--glass-border-subtle);
  border-radius: var(--radius-md);
  color: var(--text-muted);
  font-size: 0.76rem;
}

/* 切会话锚定期：列表不可见但保留布局（滚动定位照常计算），落底后一次性显现 */
.pre-anchor-hidden {
  visibility: hidden;
}

.history-loader-spinner {
  animation: history-loader-spin 0.8s linear infinite;
  opacity: 0.7;
}

@keyframes history-loader-spin {
  to { transform: rotate(360deg); }
}

.notice-fade-enter-active { transition: all 0.3s ease; }
.notice-fade-leave-active { transition: all 0.5s ease; }
.notice-fade-enter-from { opacity: 0; transform: translateY(-6px); }
.notice-fade-leave-to { opacity: 0; transform: translateY(-6px); }
</style>
