/**
 * # useThinkingMode.ts — 深度思考档位的会话级状态
 *
 * 把「深度思考」作为**会话级状态**，与「模型预设」「工作模式」「权限档位」对称：
 * 切会话即切档位。
 *
 * ## 数据流（单向，前端只渲染）
 * ```
 * 后端 sessions.thinking_mode（唯一真相，布尔）
 *   → switch_session / get_session_thinking 带回 thinkingEnabled
 *   → 本 composable 缓存「当前会话」的档位
 *   → 开关样式直接由该布尔（+ 能力夹紧）投影出来
 *   → 用户点击 → 只改本地界面值；发送消息时由 chat.flushPendingSessionPrefs 落库
 * ```
 *
 * ## 职责边界（重要）
 * 前端**不解析「跟随全局」**、**不读设置默认**、**不推导档位**。
 * - 新建会话（无 id）：初值由后端 `get_pending_thinking_enabled` 给出；
 * - 已有会话：值取后端快照；
 * - 拨动只改本地界面值，**发送消息才落库**（与工作模式/权限档位同一套三值分治）。
 *
 * ## 关键约束
 * - **发送消息不携带 `thinkingOverride`**：后端按 sessionId 自行裁决
 * - 响应/事件按 `sessionId` + 代次过滤，丢弃乱序回包（快速 A→B→A 切会话）
 */

import { computed, ref, watch, type Ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useSessionStore } from "../stores/session";
import {
  decideThinking,
  isThinkingToggleDisabled,
  type ThinkingCaps,
  type ThinkingDecision,
  type ThinkingSnapshot,
} from "../utils/thinking";

/** 每个会话各自的档位缓存：`sessionId -> 布尔` */
const modeBySession = ref<Record<string, boolean>>({});
/** 写入进行中：期间忽略重复点击 */
const saving = ref(false);
/** 最近一次 IPC 失败原因（诊断用） */
const lastError = ref<string | null>(null);

let listening = false;
/**
 * **读**序列号：只由"切会话/重新拉取"推进，用于丢弃过期的 `get_session_thinking` 回包。
 */
let loadGeneration = 0;

/**
 * "尚无会话"（新建但未发首条消息）期间用户拨过的档位。
 *
 * 与工作模式/权限档位同构：**只记界面值，不写库**——发送消息时由
 * `chat.flushPendingSessionPrefs` 统一转交。`null` = 用户没拨过。
 */
let pendingMode: boolean | null = null;

/**
 * "尚无会话"这一状态在 `modeBySession` 里的键。
 *
 * 会话 id 是 8 位 hex（见 `create_session`），不可能等于该字面量。
 */
const NO_SESSION_KEY = "__no_session__";

/** 拉取指定会话的档位快照（幂等，可安全重复调用）。 */
async function loadSessionThinkingInto(sessionId: string | null) {
  if (!sessionId) return;
  const myGen = ++loadGeneration;
  try {
    const snapshot = await invoke<ThinkingSnapshot>("get_session_thinking", { id: sessionId });
    if (myGen !== loadGeneration) return; // 期间又切了会话，丢弃
    if (useSessionStore().activeSessionId !== sessionId) return; // 会话已不是当前会话
    ingestSnapshot(snapshot, sessionId);
    lastError.value = null;
  } catch (e) {
    // 不写入任何档位：宁可不显示，也不猜测
    lastError.value = String(e);
  }
}

/**
 * 模块级导出：供 `Sidebar` 在"切会话 + 预设已落库"之后重新对齐一次，
 * 以及其他非组件路径使用。
 */
export async function loadSessionThinking(sessionId: string | null) {
  return loadSessionThinkingInto(sessionId);
}

/**
 * 向后端要"尚无会话时的初值"（设置默认 + 当前模型能力，由后端解析）。
 *
 * 只影响首屏显示；真正入库的值在首条消息时由后端重新解析。
 */
export async function loadPendingThinking() {
  try {
    const snapshot = await invoke<ThinkingSnapshot>("get_pending_thinking_enabled");
    // 只有在用户还没拨过时才用初值覆盖显示，避免把用户的选择顶掉
    if (pendingMode === null) {
      modeBySession.value = {
        ...modeBySession.value,
        [NO_SESSION_KEY]: snapshot.thinkingEnabled,
      };
    }
    lastError.value = null;
  } catch (e) {
    lastError.value = String(e);
  }
}

/**
 * 归一后端回包。
 *
 * 注意：`pipeline::ensure_session_thinking_initialized` 发的
 * `session-thinking-mode-changed` 只带 `{sessionId, thinkingEnabled}`，
 * 因此这里把缺省字段容错处理，不要假定 `resolvedEnabled` 一定存在。
 */
function ingestSnapshot(snapshot: ThinkingSnapshot, expectedSessionId: string) {
  if (snapshot.sessionId !== expectedSessionId) return;
  modeBySession.value = {
    ...modeBySession.value,
    [snapshot.sessionId]: snapshot.thinkingEnabled === true,
  };
}

export function useThinkingMode(modelCaps: Ref<ThinkingCaps | null>) {
  const session = useSessionStore();

  /** 当前档位（布尔）。尚无会话时返回该期间用户拨过的值，或后端给的初值。 */
  const currentEnabled = computed<boolean>(() => {
    const sid = session.activeSessionId;
    if (!sid) {
      if (pendingMode !== null) return pendingMode;
      return modeBySession.value[NO_SESSION_KEY] ?? false;
    }
    return modeBySession.value[sid] ?? false;
  });

  /** 开关应该显示成什么（只读投影，与后端裁决层同规则） */
  const decision = computed<ThinkingDecision>(() =>
    decideThinking(currentEnabled.value, modelCaps.value),
  );

  const isThinkingActive = computed(() => decision.value.enabled);

  /**
   * 用户点击开关。
   *
   * **只改界面值**：写入 session 的时机推迟到"发送消息"那一刻
   * （`chat.ensureActiveSessionForSend` → `flushPendingSessionPrefs`），
   * 与工作模式/权限档位完全一致——拨了不发消息 = 没表态，切走即作废。
   */
  function toggleThinking() {
    // 锁定态不可点：改不了就不给改，也不记账
    if (isThinkingToggleDisabled(modelCaps.value) || saving.value) return;

    const next = !isThinkingActive.value;
    const sid = session.activeSessionId;

    // 尚无会话：记界面值（发送首条消息时落库）
    if (!sid) {
      pendingMode = next;
      modeBySession.value = { ...modeBySession.value, [NO_SESSION_KEY]: next };
      return;
    }

    modeBySession.value = { ...modeBySession.value, [sid]: next };
  }

  /** 会话建立后清掉"无会话"期间的临时状态（值已由 chat 层落库） */
  function clearPendingMode() {
    pendingMode = null;
    const { [NO_SESSION_KEY]: _dropped, ...rest } = modeBySession.value;
    modeBySession.value = rest;
  }

  // 会话切换 → 同步该会话的档位
  watch(
    () => session.activeSessionId,
    (sid, prevSid) => {
      if (sid) {
        // null → id 是"新建会话刚被创建"：清掉临时态，值以后端为准
        if (!prevSid) clearPendingMode();
        void loadSessionThinkingInto(sid);
      } else {
        // 切回"新建会话"态：问后端初值
        void loadPendingThinking();
      }
    },
    { immediate: true },
  );

  // 跨窗口/后端推送：只接受当前会话的
  if (!listening) {
    listening = true;
    listen<ThinkingSnapshot>("session-thinking-mode-changed", (event) => {
      const sid = session.activeSessionId;
      if (!sid || event.payload.sessionId !== sid) return;
      ingestSnapshot(event.payload, sid);
    }).catch(() => {
      listening = false;
    });
  }

  return {
    currentEnabled,
    decision,
    isThinkingActive,
    lastError,
    saving,
    toggleThinking,
    loadSessionThinking,
    loadPendingThinking,
  };
}

/** 供 store / 非组件路径使用：只读当前会话档位 */
export function getSessionThinkingEnabled(sessionId: string | null): boolean {
  if (!sessionId) return false;
  return modeBySession.value[sessionId] ?? false;
}
