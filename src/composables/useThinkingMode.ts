/**
 * # useThinkingMode.ts — 深度思考档位的会话级状态
 *
 * 把「深度思考」从输入框的局部 `ref` 提升为**会话级状态**，与「模型预设」对称：
 * 切会话即切档位。
 *
 * ## 数据流（单向）
 * ```
 * 后端 sessions.thinking_mode（唯一真相）
 *   → switch_session / get_session_thinking 带回 thinking_mode
 *   → 本 composable 缓存「当前会话」的档位
 *   → decideThinking() 同步投影出开关样式（只读）
 *   → 用户点击 → set_session_thinking_mode 写后端 → 事件/返回值刷新
 * ```
 *
 * ## 关键约束
 * - **发送消息不再携带 `thinkingOverride`**：后端按 sessionId 自行裁决，
 *   因此本 composable 的本地投影即使短暂过期，也**污染不到真实请求**
 * - 写入以**后端返回的权威快照**为准（服务端 last-write-wins）；
 *   本地只做即时乐观显示，失败即回滚
 * - 响应/事件按 `sessionId` + 代次过滤，丢弃乱序回包（快速 A→B→A 切会话）
 * - **拉取失败不猜测档位**：宁可显示为 auto，也不把未知状态写成一个具体档位
 */

import { computed, ref, watch, type Ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useSessionStore } from "../stores/session";
import { usePreferences } from "../composables/usePreferences";
import {
  decideThinking,
  type ThinkingCaps,
  type ThinkingDecision,
  type ThinkingMode,
  type ThinkingSnapshot,
} from "../utils/thinking";

/** 每个会话各自的档位缓存：`sessionId -> ThinkingMode` */
const modeBySession = ref<Record<string, ThinkingMode>>({});
/** 每个会话"当时"的预设默认解析值（后端随快照下发，前端不重复解析预设链） */
const profileDefaultBySession = ref<Record<string, boolean>>({});
/** 写入进行中：期间忽略重复点击 */
const saving = ref(false);
/** 最近一次 IPC 失败原因（诊断用） */
const lastError = ref<string | null>(null);

let listening = false;
/**
 * **读**序列号：只由"切会话/重新拉取"推进，用于丢弃过期的 `get_session_thinking` 回包。
 *
 * 刻意与用户点击的写入序列分离——早期版本读、写共用一个计数器，
 * 导致"点击后紧跟着一次 watcher 触发的拉取"会把写入结果作废
 * （表现为开关点了又弹回去，即用户报告的"永远开着、点不动"）。
 */
let loadGeneration = 0;
/**
 * **写**序列号：只由用户点击推进，用于丢弃过期的 `set_session_thinking_mode` 回包。
 */
let writeGeneration = 0;

/**
 * 尚无会话时用户表达的档位意愿（`activeSessionId` 为 `null` 的新会话态）。
 *
 * 新建会话在**发送首条消息前并不存在**（`ensureActiveSessionForSend` 才创建），
 * 若此时点击开关直接返回，用户就会遇到"点了没反应、发了消息才能点"的假故障。
 * 这里先把意愿记下来，等会话一出现立刻落库（见 `useThinkingMode` 里的 null→id watcher）。
 */
let pendingMode: ThinkingMode | null = null;

/**
 * "尚无会话"这一状态在 `modeBySession` 里的键。
 *
 * 刻意用不可能与会话 id 冲突的字面量：会话 id 是 8 位 hex（见 `create_session`），
 * 不可能等于该字符串。这样"无会话期间的档位"能与真实会话档位共存于同一张表，
 * 且切到任意会话后不会被误读。
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
 *
 * 与内部调用走同一个 `loadGeneration` 序列，避免两种来源互相作废。
 */
export async function loadSessionThinking(sessionId: string | null) {
  return loadSessionThinkingInto(sessionId);
}

function ingestSnapshot(snapshot: ThinkingSnapshot, expectedSessionId: string) {
  if (snapshot.sessionId !== expectedSessionId) return;
  modeBySession.value = { ...modeBySession.value, [snapshot.sessionId]: snapshot.thinkingMode };
  if (typeof snapshot.profileResolvedDefault === "boolean") {
    profileDefaultBySession.value = {
      ...profileDefaultBySession.value,
      [snapshot.sessionId]: snapshot.profileResolvedDefault,
    };
  }
}

export function useThinkingMode(modelCaps: Ref<ThinkingCaps | null>) {
  const session = useSessionStore();
  const prefs = usePreferences();

  /**
   * 预设默认（L1）的兜底估算：会话快照尚未到达时使用。
   *
   * 只用于"首次绘制"——真正的值以后端快照里的 `profileResolvedDefault` 为准，
   * 因为后端才知道激活预设的 `thinkingDefault`（前端不读预设链，避免假单源真相）。
   */
  const fallbackProfileDefault = computed(() => prefs.agentAudience.value === "developer");

  /**
   * 当前档位。
   *
   * 尚无会话时（`activeSessionId === null`，即新建但未发首条消息）返回该期间用户
   * 表达的意愿（`NO_SESSION_KEY`），否则返回该会话的权威档位、未取到时按 `auto`。
   */
  const currentMode = computed<ThinkingMode>(() => {
    const sid = session.activeSessionId;
    if (!sid) return modeBySession.value[NO_SESSION_KEY] ?? "auto";
    return modeBySession.value[sid] ?? "auto";
  });

  /** 当前会话生效的预设默认值 */
  const profileResolvedDefault = computed<boolean>(() => {
    const sid = session.activeSessionId;
    if (!sid) return fallbackProfileDefault.value;
    return profileDefaultBySession.value[sid] ?? fallbackProfileDefault.value;
  });

  /** 开关应该显示成什么（只读投影，与后端裁决层同规则） */
  const decision = computed<ThinkingDecision>(() =>
    decideThinking(null, currentMode.value, profileResolvedDefault.value, modelCaps.value),
  );

  const isThinkingActive = computed(() => decision.value.enabled);

  /**
   * 用户点击开关。
   *
   * 两种情况：
   * 1. **已有会话**：写 L2（`sessions.thinking_mode`）→ 以返回的权威快照刷新；
   * 2. **尚无会话**（新建但未发首条消息）：只记录意愿并立即反馈，等会话建立后落库。
   *    **绝不静默 return**——那正是"点了没反应"这种假故障的来源。
   */
  async function toggleThinking() {
    const sid = session.activeSessionId;
    if (saving.value) return;

    const next: ThinkingMode = isThinkingActive.value ? "never" : "always";
    const previous = currentMode.value;

    // 尚无会话：记意愿 + 即时反馈，会话一建立就落库
    if (!sid) {
      pendingMode = next;
      modeBySession.value = { ...modeBySession.value, [NO_SESSION_KEY]: next };
      return;
    }

    // 点击时立刻作废"在途的档位拉取"，否则它会把写入结果覆盖掉
    loadGeneration += 1;

    // 即时反馈（纯 UI，失败回滚）
    modeBySession.value = { ...modeBySession.value, [sid]: next };
    saving.value = true;
    const myGen = ++writeGeneration;
    try {
      const snapshot = await invoke<ThinkingSnapshot>("set_session_thinking_mode", {
        id: sid,
        mode: next,
      });
      if (myGen !== writeGeneration) return;
      if (session.activeSessionId !== sid) return; // 期间切走了会话，不污染新会话
      ingestSnapshot(snapshot, sid);
      lastError.value = null;
    } catch (e) {
      lastError.value = String(e);
      modeBySession.value = { ...modeBySession.value, [sid]: previous };
    } finally {
      saving.value = false;
    }
  }

  /** 把"无会话期间的意愿"落库到刚建立的会话 */
  async function flushPendingMode(sid: string) {
    const mode = pendingMode;
    if (!mode) return;
    pendingMode = null;
    // 清掉无会话态的临时条目，会话档位以刚落库的结果为准
    const { [NO_SESSION_KEY]: _dropped, ...rest } = modeBySession.value;
    modeBySession.value = rest;
    try {
      const snapshot = await invoke<ThinkingSnapshot>("set_session_thinking_mode", {
        id: sid,
        mode,
      });
      if (session.activeSessionId !== sid) return;
      ingestSnapshot(snapshot, sid);
      lastError.value = null;
    } catch (e) {
      lastError.value = String(e);
      // 落库失败：退回到后端当前值，避免 UI 与后端长期不一致
      void loadSessionThinkingInto(sid);
    }
  }

  // 会话切换 → 同步该会话的档位（这一步取代了原来"开关跟着组件走"的行为）
  watch(
    () => session.activeSessionId,
    (sid, prevSid) => {
      if (sid) {
        // null → id 是"新建会话刚被创建"，先把无会话期间的意愿落库
        if (!prevSid && pendingMode) {
          void flushPendingMode(sid);
        }
        void loadSessionThinkingInto(sid);
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
    currentMode,
    decision,
    isThinkingActive,
    lastError,
    saving,
    toggleThinking,
    loadSessionThinking,
  };
}

/** 供 store / 非组件路径使用：只读当前会话档位 */
export function getSessionThinkingMode(sessionId: string | null): ThinkingMode {
  if (!sessionId) return "auto";
  return modeBySession.value[sessionId] ?? "auto";
}
