<script setup lang="ts">
import { ref, onMounted, nextTick, computed, watch, onUnmounted } from 'vue';
import { useI18n } from 'vue-i18n';
import { useSessionStore } from '../../stores/session';
import { useChatStore } from '../../stores/chat';
import { usePermissionStore } from '../../stores/permission';
import { usePreferences } from '../../composables/usePreferences';
import { invoke } from '@tauri-apps/api/core';
import ConfirmModal from '../common/ConfirmModal.vue';
import AgentTurn from './AgentTurn.vue';
import AgentTurnNotice from './AgentTurnNotice.vue';
import ThinkingStatus from './ThinkingStatus.vue';
import SessionTaskBoard from './SessionTaskBoard.vue';
import TodoPanel from './TodoPanel.vue';
import WelcomeScreen from './WelcomeScreen.vue';
import MessageRail from './MessageRail.vue';
import { noticeTextFromBackend } from '../../utils/agentTurnState';
import type { PlanDocument, AgentTurnSnapshot, AgentCurrentTurn } from '../../types';

interface RollbackPreviewFile {
  path: string;
  linesAdded: number;
  linesRemoved: number;
}

interface RollbackPreviewResult {
  targetCheckpointId: string;
  checkpointLabel: string;
  files: RollbackPreviewFile[];
}

interface RollbackRecallResult {
  restoredFiles: string[];
  recalledText: string;
  /**
   * 回滚后**没能恢复**的对象（快照无内容 且 回收站副本缺失）。
   * 非空 = 工作区与目标检查点不完全一致，必须提示用户。
   */
  restoreWarnings?: string[];
}

interface RollbackPreviewState {
  loading: boolean;
  checkpointId: string;
  files: RollbackPreviewFile[];
  targetCheckpointId: string;
  checkpointLabel: string;
  error: string;
}

const { t } = useI18n();

const session = useSessionStore();
const chat = useChatStore();
const perm = usePermissionStore();
const prefs = usePreferences();
const responseAreaRef = ref<HTMLElement | null>(null);
/** 视口是否贴底：滚动事件与程序化落底共同维护的统一事实源。 */
const atBottom = ref(true);
/**
 * 流式跟随开关 = 「自动跟随滚动」偏好开 且 视口贴底。
 *
 * 偏好是总闸：关闭时流式过程不做程序化吸底（renderTick / store 流式回调路径全部拦下），
 * 只保留手动「滚动到底」按钮；开跑瞬间与切会话的定位（force 路径）是导航行为，不受此约束。
 * 之前这里只是挂载时读了一次偏好的 ref，随后被滚动事件覆写、被开跑强制落底无条件置回 true，
 * 设置开关形同虚设——根因是「跟随」与「定位」两个概念混在同一个变量里。
 */
const shouldFollowStream = computed(() => prefs.autoScroll && atBottom.value);
const currentTurn = computed(() => session.currentSessionView.currentTurn);
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

const hasBubbleContent = computed(() => hasBubblePayload(currentTurn.value));
const isWaitingForUser = computed(() => {
  return Boolean(perm.planProposal || perm.permissionRequest);
});

const showInlineStatus = computed(() => {
  if (isWaitingForUser.value) return false;
  const view = session.currentSessionView;
  return Boolean(
    view.runStartTime &&
    (view.streamActive || (session.isCurrentSessionRunning && !hasCurrentTurnContent.value))
  );
});

// 控制 Live Turn 显示/隐藏
const showAgentTurn = ref(false);

watch(
  () => hasCurrentTurnContent.value || showInlineStatus.value,
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

const thinkingElapsed = ref(0);
let thinkingTimer: ReturnType<typeof setInterval> | null = null;
let waitStartMs = 0;
let accumulatedWaitMs = 0;

// 将 AgentTurnSnapshot 转换为 AgentCurrentTurn 格式（供 AgentTurn 组件渲染历史消息）
// 注意：**新增快照字段时必须同步到这里**，否则该字段在界面永远不生效
// （notice 就曾因为漏拷而完全不显示）。
//
// 缓存：同一个 snapshot 对象 → 同一个 turn 对象。
// 动机：历史消息在模板里写的是 :turn="convertSnapshotToTurn(...)"。若每次调用都新建对象，
// 则 ChatArea 每次重渲染（运行期约 1 Hz，节拍来自 thinkingElapsed）都会让全部历史
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
  };

  turnCache.set(snapshot, turn);
  return turn;
}

const updateThinkingElapsed = () => {
  const view = session.getSessionView(session.activeSessionId);
  if (!view.runStartTime) { thinkingElapsed.value = 0; return; }
  const elapsed = Date.now() - view.runStartTime - accumulatedWaitMs;
  thinkingElapsed.value = Math.max(0, Math.floor(elapsed / 1000));
};

watch(showInlineStatus, (running) => {
  if (running) {
    updateThinkingElapsed();
    thinkingTimer = setInterval(updateThinkingElapsed, 1000);
  } else {
    if (thinkingTimer) { clearInterval(thinkingTimer); thinkingTimer = null; }
    thinkingElapsed.value = 0;
    waitStartMs = 0;
    accumulatedWaitMs = 0;
  }
});

// 等待用户决策期间暂停读秒
watch(isWaitingForUser, (waiting) => {
  if (waiting) {
    waitStartMs = Date.now();
  } else if (waitStartMs > 0) {
    accumulatedWaitMs += Date.now() - waitStartMs;
    waitStartMs = 0;
  }
});

watch(() => session.activeSessionId, () => {
  if (showInlineStatus.value) {
    updateThinkingElapsed();
    if (!thinkingTimer) {
      thinkingTimer = setInterval(updateThinkingElapsed, 1000);
    }
  } else {
    if (thinkingTimer) { clearInterval(thinkingTimer); thinkingTimer = null; }
    thinkingElapsed.value = 0;
  }
});

onUnmounted(() => {
  if (thinkingTimer) clearInterval(thinkingTimer);
});

const expandedUserMsgs = ref(new Set<string>());

function getUserMsgText(message: any): string {
  const raw = message.text ?? message.userContent ?? message.content ?? '';
  return String(raw).replace(/<[^>]*>/g, '');
}

function isUserMsgLong(text: string): boolean {
  if (!text) return false;
  const plain = text.replace(/<[^>]*>/g, '').replace(/\n{3,}/g, '\n\n');
  return plain.split('\n').length > 6 || plain.length > 500;
}

function toggleUserMsgExpand(msgId: string) {
  const next = new Set(expandedUserMsgs.value);
  if (next.has(msgId)) {
    next.delete(msgId);
  } else {
    next.add(msgId);
  }
  expandedUserMsgs.value = next;
}

const rollbackMenu = ref<{
  visible: boolean;
  x: number;
  y: number;
  snapshotId: string | null;
  rollbackMode: 'both' | 'session';
  userMessageIndex: number | null;
  messageId: string | null;
  fallbackSnapshotId: string;
}>({
  visible: false,
  x: 0,
  y: 0,
  snapshotId: null,
  rollbackMode: 'session',
  userMessageIndex: null,
  fallbackSnapshotId: '',
  messageId: null,
});

const rollbackConfirm = ref<{
  mode: 'both' | 'session';
  snapshotId: string;
  fallbackSnapshotId: string;
  userMessageIndex: number | null;
  messageId: string | null;
  title: string;
  message: string;
  files: RollbackPreviewFile[];
  warning?: string;
  /** 仅提示态：弹窗只用于告知结果（如"有对象未能恢复"），确认/关闭都只是关掉它 */
  noticeOnly?: boolean;
} | null>(null);

const rollbackPreview = ref<RollbackPreviewState>({
  loading: false,
  checkpointId: '',
  files: [],
  targetCheckpointId: '',
  checkpointLabel: '',
  error: '',
});
const rollbackLoading = ref(false);
const rollbackError = ref('');

const isResponseAtBottom = () => {
  if (!responseAreaRef.value) return false;
  const { scrollTop, scrollHeight, clientHeight } = responseAreaRef.value;
  return scrollHeight - scrollTop - clientHeight <= 35;
};

const setResponseScrollToBottom = () => {
  if (!responseAreaRef.value) return;
  responseAreaRef.value.scrollTop = responseAreaRef.value.scrollHeight;
};

const forceScrollToBottomAfterRender = async () => {
  // 只恢复「贴底」事实，不越过跟随总闸：之后是否持续跟随由 shouldFollowStream
  // （偏好 且 贴底）决定。这里始终要落底——它承担开跑定位与切会话锚定，属导航行为。
  atBottom.value = true;

  await nextTick();
  setResponseScrollToBottom();

  // 渲染是分帧进行的（StreamingMarkdown 逐帧撑高内容）：单次滚动只会停在
  // "当时的底部"，后续帧内容继续变长就把视口顶离底部——表现为进入会话时
  // 先看到中间位置、过一会儿才滑到底。持续跟随到高度连续 2 帧稳定才算落底。
  let lastHeight = -1;
  let stableFrames = 0;
  let totalFrames = 0;
  await new Promise<void>((resolve) => {
    const tick = () => {
      if (!responseAreaRef.value) return resolve();
      setResponseScrollToBottom();
      const h = responseAreaRef.value.scrollHeight;
      stableFrames = h === lastHeight ? stableFrames + 1 : 0;
      lastHeight = h;
      totalFrames += 1;
      if (stableFrames >= 2 || totalFrames > 60) return resolve();
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
};

const scrollToBottom = async (force = false) => {
  if (force) {
    await forceScrollToBottomAfterRender();
    return;
  }

  // 跟随判定收口在 shouldFollowStream（偏好 且 贴底）：
  // 偏好关闭时，流式回调 / renderTick 到这里都会被拦下，不再"贴底就吸"。
  // force（点按钮 / 开跑 / 切会话）是显式定位，不受总闸约束。
  const shouldScroll = force || shouldFollowStream.value;

  await nextTick();
  if (responseAreaRef.value && shouldScroll) {
    responseAreaRef.value.scrollTop = responseAreaRef.value.scrollHeight;
  }
};

const handleResponseScroll = () => {
  atBottom.value = isResponseAtBottom();
  // 懒加载：滚动接近顶部（<60px）时加载更早的历史；防重入与 hasMore 由 store 状态保证
  void maybeLoadOlderMessages();
};

/** 懒加载锚定期标志：头部插入 → 渲染 → 补偿滚动期间屏蔽再次触发（否则会连续加载两页并回弹） */
const historyAnchoring = ref(false);

/** 懒加载：上滑到顶触发加载更早历史，插入后做滚动锚定（补偿 scrollHeight 差值，视口不跳） */
const maybeLoadOlderMessages = async () => {
  const el = responseAreaRef.value;
  if (!el) return;
  const view = session.currentSessionView;
  if (!view.pagedHasMore || view.pagedLoadingOlder || historyAnchoring.value) return;
  if (el.scrollTop > 60) return;
  const sid = session.activeSessionId;
  if (!sid) return;
  const prevHeight = el.scrollHeight;
  const prevTop = el.scrollTop;
  historyAnchoring.value = true;
  const loaded = await chat.loadSessionMessagesOlder(sid);
  if (!loaded) {
    historyAnchoring.value = false;
    return;
  }
  // 双 rAF：等 Vue 渲染与浏览器布局稳定后再补偿——单 nextTick 时流式组件可能
  // 还在继续撑高内容，补偿量不足就会"往回弹"
  await nextTick();
  await new Promise<void>((resolve) =>
    requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
  );
  if (responseAreaRef.value) {
    responseAreaRef.value.scrollTop =
      prevTop + (responseAreaRef.value.scrollHeight - prevHeight);
  }
  historyAnchoring.value = false;
};

const showScrollToBottom = computed(() => {
  // 用贴底事实（atBottom）而非跟随开关判断：跟随关闭（偏好关）但视口仍贴底时，不该显示按钮
  return !atBottom.value && (chat.messages.length > 0 || hasCurrentTurnContent.value);
});

/** 懒加载提示条三态：加载中（转圈）/ 上滑提示 / 已加载全部（翻到最老后常驻小字） */
const showHistoryLoader = computed(() => {
  const v = session.currentSessionView;
  return v.pagedHasMore || v.pagedLoadingOlder || v.pagedOldestSeq !== null;
});
const historyLoaderText = computed(() => {
  const v = session.currentSessionView;
  if (v.pagedLoadingOlder) return t('chat.loadingOlderMessages');
  if (v.pagedHasMore) return t('chat.scrollForOlderMessages');
  return v.pagedOldestSeq !== null ? t('chat.allMessagesLoaded') : '';
});

watch(() => session.isCurrentSessionRunning, (running) => {
  if (running) {
    scrollToBottom(true);
  }
});

const pendingInitialScrollSessionKey = ref<string | null>(null);
const currentSessionKey = computed(() => session.activeSessionId || '__default__');
/**
 * 切会话锚定期：消息列表渲染是分帧撑高的，若渲染过程可见，用户会先看到
 * 中间位置的内容、再"滚"到底——主流聊天应用（微信/Discord）都是直接出现
 * 在最新消息。实现：锚定期列表 `visibility: hidden`（保留布局，滚动定位
 * 照常计算），稳定落底后一次性显现——用户看到的就是"直接在最新消息"。
 */
const initialAnchoring = ref(false);

watch(currentSessionKey, (key) => {
  pendingInitialScrollSessionKey.value = key;
  initialAnchoring.value = true;
}, { immediate: true });

watch(
  () => [
    currentSessionKey.value,
    session.currentSessionView.hydrated,
    chat.messages.length,
    hasCurrentTurnContent.value,
  ],
  async ([key, hydrated, msgCount, hasTurnContent]) => {
    if (pendingInitialScrollSessionKey.value !== key) return;
    if (!hydrated && !msgCount && !hasTurnContent) return;

    pendingInitialScrollSessionKey.value = null;
    // 锚定期：列表不可见状态下稳定落底，完成后一次性显现（直接在最新消息）
    await forceScrollToBottomAfterRender();
    initialAnchoring.value = false;
  },
  { immediate: true, flush: 'post' }
);

watch(() => [chat.renderTick, currentTurn.value.revision], () => {
  if (shouldFollowStream.value) {
    scrollToBottom();
    return;
  }
  // 未在跟随（偏好关或已上滑）时：内容增长只把视口"顶离"底部、不产生 scroll 事件，
  // 贴底状态必须在这里重算，否则视口漂离后「滚动到底」按钮永远不会出现。
  // 顺序不能反：先重算再判断会把"即将跟随"的一帧误判成已离底，跟随会被自己掐断。
  atBottom.value = isResponseAtBottom();
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
    userMessageIndex: null,
    messageId: null,
    fallbackSnapshotId: '',
  };
};

// 使用 Vue 数据驱动的回滚按钮点击
const handleRollbackClickVue = (index: number, message: any, event: MouseEvent) => {
  const rollbackMode = message.rollbackMode === 'both' ? 'both' : 'session';
  const rollbackCheckpointId = message.rollbackCheckpointId || '';
  const position = getRollbackMenuPosition(event.clientX, event.clientY, rollbackMode);

  rollbackMenu.value = {
    visible: true,
    x: position.left,
    y: position.top,
    snapshotId: rollbackCheckpointId,
    rollbackMode,
    userMessageIndex: index,
    messageId: message.messageId || null,
    fallbackSnapshotId: '',
  };
};

// handleRollbackClick 保留用于 @click 委托（旧 DOM 路径兼容）
const handleRollbackClick = (_e: MouseEvent) => {
  /* now handled by handleRollbackClickVue via @click.stop */
};

const copyText = async (text: string, html?: string) => {
  if (html && navigator.clipboard?.write && typeof ClipboardItem !== 'undefined') {
    try {
      await navigator.clipboard.write([
        new ClipboardItem({
          'text/html': new Blob([html], { type: 'text/html' }),
          'text/plain': new Blob([text], { type: 'text/plain' }),
        }),
      ]);
      return;
    } catch {
      // Fall back to plain text below when rich clipboard writes are unavailable.
    }
  }

  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }

  const textarea = document.createElement('textarea');
  textarea.value = text;
  textarea.setAttribute('readonly', '');
  textarea.style.position = 'fixed';
  textarea.style.left = '-9999px';
  textarea.style.opacity = '0';
  document.body.appendChild(textarea);
  textarea.select();
  document.execCommand('copy');
  document.body.removeChild(textarea);
};

const showCopiedState = (button: HTMLButtonElement) => {
  const previousText = button.textContent || t('common.copy');
  button.textContent = t('common.copied');
  button.classList.add('copied');
  window.setTimeout(() => {
    button.textContent = previousText;
    button.classList.remove('copied');
  }, 1200);
};

const copyCodeBlock = async (button: HTMLButtonElement) => {
  const block = button.closest('.markdown-code-block, .md-code-block');
  const code = block?.querySelector('pre code')?.textContent || '';
  if (!code.trim()) return;
  await copyText(code);
  showCopiedState(button);
};

const normalizeMarkdownTableCell = (cell?: HTMLTableCellElement) => {
  if (!cell) return '';
  return cell.innerText
    .replace(/\s+/g, ' ')
    .trim()
    .replace(/\\/g, '\\\\')
    .replace(/\|/g, '\\|');
};

const markdownAlignment = (cell?: HTMLTableCellElement) => {
  const align = (cell?.getAttribute('align') || '').toLowerCase();
  if (align === 'center') return ':---:';
  if (align === 'right') return '---:';
  return '---';
};

const tableToMarkdown = (table: HTMLTableElement) => {
  const rows = Array.from(table.rows);
  if (rows.length === 0) return '';

  const headerRow = table.tHead?.rows[0] || rows[0];
  const bodyRows = rows.filter((row) => row !== headerRow);
  const columnCount = Math.max(...rows.map((row) => row.cells.length), 1);
  const readRow = (row: HTMLTableRowElement) =>
    Array.from({ length: columnCount }, (_, index) => normalizeMarkdownTableCell(row.cells[index]));

  const header = readRow(headerRow);
  const separator = Array.from({ length: columnCount }, (_, index) =>
    markdownAlignment(headerRow.cells[index])
  );
  const body = bodyRows.map((row) => `| ${readRow(row).join(' | ')} |`);

  return [`| ${header.join(' | ')} |`, `| ${separator.join(' | ')} |`, ...body].join('\n');
};

const copyTable = async (button: HTMLButtonElement) => {
  const wrap = button.closest('.markdown-table-wrap, .md-table-wrap');
  const table = wrap?.querySelector('table') as HTMLTableElement | null | undefined;
  if (!table) return;
  await copyText(tableToMarkdown(table), table.outerHTML);
  showCopiedState(button);
};

const handleResponseClick = (e: MouseEvent) => {
  const target = e.target as HTMLElement;
  const codeCopyButton = target.closest<HTMLButtonElement>('.code-copy-btn');
  if (codeCopyButton) {
    e.preventDefault();
    e.stopPropagation();
    copyCodeBlock(codeCopyButton);
    return;
  }

  const tableCopyButton = target.closest<HTMLButtonElement>('.table-copy-btn');
  if (tableCopyButton) {
    e.preventDefault();
    e.stopPropagation();
    copyTable(tableCopyButton);
    return;
  }

  handleRollbackClick(e);
};

const closeRollbackMenu = () => {
  rollbackMenu.value.visible = false;
};

const getRollbackMenuPosition = (x: number, y: number, rollbackMode: 'both' | 'session') => {
  const menuWidth = 220;
  const menuHeight = rollbackMode === 'both' ? 118 : 74;
  const margin = 12;

  let left = x;
  let top = y;

  if (left + menuWidth > window.innerWidth - margin) {
    left = window.innerWidth - menuWidth - margin;
  }
  if (top + menuHeight > window.innerHeight - margin) {
    top = window.innerHeight - menuHeight - margin;
  }

  left = Math.max(margin, left);
  top = Math.max(margin, top);

  return { left, top };
};

const executeRollback = async (mode: 'both' | 'session') => {
  rollbackError.value = '';
  rollbackPreview.value.error = '';
  if (mode === 'both') {
    rollbackLoading.value = true;
    rollbackPreview.value.loading = true;
    try {
      const sessionId = session.activeSessionId;
      if (!sessionId) {
        alert(t('rollback.noSession'));
        return;
      }
      const result = await invoke<RollbackPreviewResult>('preview_rollback_to_checkpoint_with_recall', {
        sessionId,
        checkpointId: rollbackMenu.value.snapshotId || rollbackMenu.value.fallbackSnapshotId || '',
        messageId: rollbackMenu.value.messageId,
        userMessageIndex: rollbackMenu.value.userMessageIndex,
      });
      rollbackPreview.value = {
        loading: false,
        checkpointId: result.targetCheckpointId,
        files: result.files,
        targetCheckpointId: result.targetCheckpointId,
        checkpointLabel: result.checkpointLabel,
        error: '',
      };
    } catch (err) {
      const message = normalizeRollbackError(err);
      rollbackPreview.value.loading = false;
      rollbackPreview.value.error = message;
      rollbackError.value = message;
      return;
    } finally {
      rollbackPreview.value.loading = false;
      rollbackLoading.value = false;
    }
  }

  const previewMessage = mode === 'both'
    ? t('rollback.confirmBothMessage')
    : t('rollback.confirmSessionMessage');

  rollbackConfirm.value = {
    mode,
    snapshotId: rollbackMenu.value.snapshotId || '',
    fallbackSnapshotId: rollbackMenu.value.fallbackSnapshotId,
    userMessageIndex: rollbackMenu.value.userMessageIndex,
    messageId: rollbackMenu.value.messageId,
    title: mode === 'both' ? t('rollback.confirmBothTitle') : t('rollback.confirmSessionTitle'),
    message: previewMessage,
    files: mode === 'both' ? rollbackPreview.value.files : [],
    warning:
      mode === 'both'
        ? t('rollback.warningBoth')
        : t('rollback.warningSession'),
  };
};

const normalizeRollbackError = (err: unknown) => {
  const raw = typeof err === 'string' ? err : err instanceof Error ? err.message : String(err || t('common.unknownError'));
  if (raw.includes(t('rollback.historyKept'))) {
    return raw;
  }
  return t('rollback.historyKeptSuffix', { error: raw });
};

const confirmRollback = async () => {
  if (!rollbackConfirm.value) return;
  // 仅提示态：只是通知，点任何按钮都只关掉弹窗（绝不能重复执行回滚）
  if (rollbackConfirm.value.noticeOnly) {
    rollbackConfirm.value = null;
    return;
  }

  rollbackLoading.value = true;
  rollbackError.value = '';
  let restoreWarnings: string[] = [];
  try {
    const sessionId = session.activeSessionId;
    if (!sessionId) {
      alert(t('rollback.noSession'));
      return;
    }

    let recalledText: string | null = null;
    const rollbackUserMessageIndex = rollbackConfirm.value.userMessageIndex;
    const rollbackMessageId = rollbackConfirm.value.messageId;
    if (rollbackConfirm.value.snapshotId || rollbackConfirm.value.fallbackSnapshotId || rollbackConfirm.value.mode === 'both') {
      // 有 checkpointId 或需要回滚代码（后端会按传入快照恢复文件）
      const result = await invoke<RollbackRecallResult>('rollback_to_checkpoint_with_recall', {
        sessionId,
        checkpointId: rollbackConfirm.value.snapshotId || rollbackConfirm.value.fallbackSnapshotId || '',
        rollbackFiles: rollbackConfirm.value.mode === 'both',
        messageId: rollbackMessageId,
        userMessageIndex: rollbackUserMessageIndex,
      });
      recalledText = result.recalledText;
      restoreWarnings = result.restoreWarnings ?? [];
    } else if (rollbackMessageId || rollbackUserMessageIndex !== null) {
      recalledText = await invoke<string | null>('recall_message', {
        sessionId,
        messageId: rollbackMessageId,
        userMessageIndex: rollbackUserMessageIndex,
      });
    } else {
      recalledText = await invoke<string | null>('recall_last_message', { sessionId });
    }

    // 有对象没能恢复 → 改用"仅提示"态复用这个弹窗告知用户（工作区与目标检查点不一致）；
    // 否则照常关掉。
    rollbackConfirm.value = restoreWarnings.length
      ? {
          mode: 'session',
          snapshotId: '',
          fallbackSnapshotId: '',
          userMessageIndex: null,
          messageId: null,
          title: t('rollback.restoreWarningsTitle'),
          message: t('rollback.restoreWarningsMessage'),
          files: [],
          warning: restoreWarnings.join('\n'),
          noticeOnly: true,
        }
      : null;
    rollbackPreview.value = {
      loading: false,
      checkpointId: '',
      files: [],
      targetCheckpointId: '',
      checkpointLabel: '',
      error: '',
    };
    closeRollbackMenu();

    if (recalledText) {
      chat.rollbackRecalledMessage = recalledText;
    }

    try {
      try {
        // 检查点回滚后重置式懒加载首屏（回滚已截断消息，分页游标一并重置）
        await chat.loadSessionMessagesReset(sessionId);
      } catch (err) {
        // 结构化加载失败，没有 HTML 兜底了（那条通道已删除，见 session store 的说明）：
        // 置一个标记，由界面显示可翻译的提示，而不是让用户对着空白猜
        console.error('加载会话消息失败:', err);
        session.setSessionLoadError(sessionId, true);
      }
      const planDocuments = await invoke<PlanDocument[]>('list_plan_documents', { sessionId });
      perm.planDocumentsBySession = {
        ...perm.planDocumentsBySession,
        [sessionId]: planDocuments,
      };
      delete perm.planProposals[sessionId];
      chat.triggerRender();
    } catch {
      session.resetSessionView(sessionId);
      chat.triggerRender();
    }
  } catch (err) {
    const message = normalizeRollbackError(err);
    console.error('回滚失败:', err);
    rollbackError.value = message;
  } finally {
    rollbackLoading.value = false;
  }
};

onMounted(() => {
  chat.registerScrollCb(scrollToBottom);
  document.addEventListener('click', (e) => {
    const target = e.target as HTMLElement;
    if (target.closest('.rollback-trigger')) return;
    if (!target.closest('.rollback-menu')) {
      closeRollbackMenu();
    }
  });
});
</script>

<template>
  <div class="response-area" ref="responseAreaRef" @scroll="handleResponseScroll" @contextmenu="handleContextMenu" @click="handleResponseClick">
    <MessageRail :container="responseAreaRef" />
    <TodoPanel />
    <SessionTaskBoard />
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
      <template v-for="(message, index) in chat.messages" :key="message.id">
        <!-- 用户消息 -->
        <div
          v-if="message.role === 'user'"
          class="chat-message user-message"
          :data-msg-id="message.id"
        >
          <div
            class="message-content"
            :data-user-message-index="index"
            :data-message-id="message.messageId"
            :data-rollback-checkpoint-id="message.rollbackCheckpointId || ''"
            :data-rollback-mode="message.rollbackMode || 'session'"
          >
            <!-- 图片 -->
            <div v-if="message.images && message.images.length > 0" class="user-images">
              <img
                v-for="(img, imgIdx) in message.images"
                :key="`img-${imgIdx}`"
                :src="img"
                class="user-image"
                alt="用户发送的图片"
              />
            </div>
            <!-- 文本 -->
            <div class="user-text" :class="{ collapsed: isUserMsgLong(getUserMsgText(message)) && !expandedUserMsgs.has(message.id) }">
              {{ getUserMsgText(message) }}
            </div>
            <button
              v-if="isUserMsgLong(getUserMsgText(message))"
              class="user-msg-toggle"
              @click="toggleUserMsgExpand(message.id)"
            >
              {{ expandedUserMsgs.has(message.id) ? '收起' : '展开全部' }}
            </button>
          </div>
          <button
            v-if="message.rollbackCheckpointId || message.messageId"
            class="rollback-trigger"
            :data-cp-id="message.rollbackCheckpointId || ''"
            :title="t('rollback.trigger')"
            :aria-label="t('rollback.trigger')"
            @click.stop="handleRollbackClickVue(index, message, $event)"
          >
            <svg
              class="rollback-trigger-icon"
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
              <path d="M9 14 4 9l5-5" />
              <path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5V20" />
            </svg>
          </button>
        </div>
        <!-- Agent 消息 -->
        <div v-else-if="message.role === 'agent' && message.snapshot" class="chat-message agent-message" :data-msg-id="message.id">
          <!-- 只有状态标注、没有正文的轮次不渲染气泡：notice 在气泡外，气泡会是个空盒子 -->
          <div v-if="hasBubblePayload(message.snapshot)" class="message-content current-turn-content">
            <AgentTurn
              :turn="convertSnapshotToTurn(message.snapshot)"
              :display-mode="prefs.agentAudience.value"
              :show-status="false"
              :elapsed="0"
              :paused="false"
            />
          </div>
          <!-- 状态标注刻意放在气泡**外面**：它是这一轮的运行状态，不是模型说的话。
               放气泡内会跟着 padding 缩进，看起来像模型正文的一部分。 -->
          <AgentTurnNotice :notice="noticeTextFromBackend(message.snapshot.noticeI18nKey, message.snapshot.noticeDetail)" />
        </div>
      </template>

      <!-- Live 当前 Turn -->
      <div v-if="showAgentTurn" class="chat-message agent-message current-turn-message">
        <!-- 气泡只在"有正文"或"运行中要占位"时渲染；
             只有 notice（如上游静默零产出）时不渲染气泡，让它单独出现在外面 -->
        <div
          v-if="hasBubbleContent || showInlineStatus"
          class="message-content current-turn-content"
          :class="{ 'waiting-only': !hasBubbleContent }"
        >
          <AgentTurn
            :turn="currentTurn"
            :display-mode="prefs.agentAudience.value"
            :show-status="showInlineStatus"
            :elapsed="thinkingElapsed"
            :paused="isWaitingForUser"
          />
        </div>
        <!-- running：当前轮才可能为 true，用来区分"还在进行中"的过程态 -->
        <AgentTurnNotice :notice="currentTurn.notice" :running="currentTurn.isRunning" />
      </div>
      <Transition name="notice-fade">
        <div v-if="chat.memoryNotice" class="memory-notice" @click="chat.memoryNotice = null">
          <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>
          {{ chat.memoryNotice }}
        </div>
      </Transition>
      <ThinkingStatus :running="showInlineStatus" :elapsed="thinkingElapsed" :paused="isWaitingForUser" />
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
                <span class="rollback-preview-path" :title="file.path">{{ file.path }}</span>
                <span class="rollback-preview-stats">
                  <span class="rollback-preview-added">+{{ file.linesAdded }}</span>
                  <span class="rollback-preview-removed">-{{ file.linesRemoved }}</span>
                </span>
              </div>
            </div>
          </div>
          <p v-else class="rollback-preview-empty">{{ t('rollback.previewEmpty') }}</p>
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
  justify-content: flex-end;
  align-items: flex-start;
  position: relative;
  gap: 8px;
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

.response-text :deep(details) {
  margin: 12px 0;
  padding: 8px 12px;
  background: color-mix(in srgb, var(--glass-bg-light) calc(var(--agent-message-opacity, 0) * 1%), transparent);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
  border-radius: var(--radius-md);
  transition: background-color var(--transition-fast);
}

.response-text :deep(details:hover) {
  background: color-mix(in srgb, var(--glass-bg) calc(var(--agent-message-opacity, 0) * 1%), transparent);
}

.response-text :deep(summary) {
  cursor: pointer;
  font-size: 0.85rem;
  font-weight: 500;
  color: var(--text-muted);
  user-select: none;
  outline: none;
  transition: color var(--transition-fast);
}

.response-text :deep(summary:hover) {
  color: var(--text-main);
}

.response-text :deep(details[open]) {
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

.response-text :deep(code) {
  padding: 2px 6px;
  color: var(--text-main);
  font-family: var(--font-mono);
  font-size: 0.85em;
  border: 1px solid var(--glass-border-subtle);
  border-radius: 5px;
  background: color-mix(in srgb, var(--text-muted) 15%, transparent);
}

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

/* ── 紧凑模式（类挂在 <html> 上，由「常规设置 → 紧凑模式」驱动）──
   必须放在组件 scoped 样式里：:deep() 是 Vue 的编译期指令，写进全局样式表会被
   浏览器当非法选择器整条丢弃（这些规则曾放在 global.css 导致开关完全无效）；
   放回组件内，多一层 .compact-mode 的特异性也稳定压过本文件的基础规则。 */
.compact-mode .response-text :deep(.chat-message) {
  margin-bottom: 8px;
}
.compact-mode .response-text :deep(.message-content) {
  padding: 8px 14px;
}
.compact-mode .response-area {
  padding-top: 8px;
}
.compact-mode .response-text :deep(p) {
  margin-bottom: 0.35em;
}
.compact-mode .response-text :deep(pre) {
  padding: 8px;
  margin-bottom: 0.35em;
}

.rollback-menu {
  position: fixed;
  z-index: 10000;
  background: var(--glass-bg-heavy);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
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
  color: var(--text-muted);
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
  gap: 12px;
}

.rollback-preview-message,
.rollback-preview-empty {
  margin: 0;
  color: var(--text-main);
  line-height: 1.7;
  font-size: 0.95rem;
}

.rollback-preview-empty {
  color: var(--text-muted);
}

.rollback-preview-files {
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  background: var(--glass-bg-light);
  overflow: hidden;
}

.rollback-preview-summary {
  padding: 9px 12px;
  color: var(--text-muted);
  font-size: 0.82rem;
  font-weight: 600;
  border-bottom: 1px solid var(--glass-border-subtle);
}

.rollback-preview-file-list {
  max-height: 220px;
  overflow-y: auto;
}

.rollback-preview-file {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 8px 12px;
  font-family: var(--font-mono);
  font-size: 0.82rem;
  border-bottom: 1px solid var(--glass-border-subtle);
}

.rollback-preview-file:last-child {
  border-bottom: none;
}

.rollback-preview-path {
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

/* 撤回触发按钮：位于用户消息气泡左侧外部 */
:deep(.rollback-trigger) {
  position: static;
  order: -1;
  align-self: flex-start;
  flex: 0 0 auto;
  width: 24px;
  height: 24px;
  border-radius: 6px;
  border: 1px solid transparent;
  background: transparent;
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
  cursor: pointer;
  padding: 0;
  margin-top: 8px;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: opacity 0.15s ease, transform 0.15s ease, border-color 0.15s ease, background 0.15s ease, box-shadow 0.15s ease;
  z-index: 5;
  -webkit-app-region: no-drag;
  opacity: 0.2;
  pointer-events: auto;
}
:deep(.rollback-trigger-icon) {
  color: color-mix(in srgb, var(--text-muted) 78%, transparent);
  transition: color 0.15s ease;
}
:deep(.user-message:hover .rollback-trigger) {
  opacity: 0.75;
  border-color: var(--glass-border-subtle);
  background: var(--glass-bg);
}
:deep(.rollback-trigger:hover) {
  opacity: 1 !important;
  border-color: var(--accent-blue) !important;
  background: color-mix(in srgb, var(--accent-blue) 12%, transparent);
  box-shadow: 0 0 0 1px color-mix(in srgb, var(--accent-blue) 12%, transparent);
  transform: scale(1.1);
}
:deep(.rollback-trigger:hover) .rollback-trigger-icon {
  color: var(--accent-blue);
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
  opacity: 0.5;
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