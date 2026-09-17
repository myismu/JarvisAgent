<!--
# TerminalInput.vue — 主输入区

消息输入、附件管理、工作模式与审批模式切换、发送/停止按钮。

## Key Exports
- 默认导出组件：会话输入栏

## Constraints
- 停止按钮为中性色（中断非破坏性操作）；模式圆点统一中性灰
-->
<script setup lang="ts">
import { computed, ref, onMounted, nextTick, onUnmounted, onBeforeUnmount, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useSessionStore } from '../../stores/session';
import { useChatStore } from '../../stores/chat';
import { useAgentStore } from '../../stores/agent';
import { usePreferences } from '../../composables/usePreferences';
import { useThinkingMode } from '../../composables/useThinkingMode';
import { isThinkingToggleDisabled, type ThinkingCaps } from '../../utils/thinking';
import { resolveContextTokens } from '../../utils/contextUsage';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { readFile } from '@tauri-apps/plugin-fs';
import ConfirmModal from '../common/ConfirmModal.vue';
import CacheHitTooltip from './CacheHitTooltip.vue';
import type { AgentApprovalMode, AgentUserMode, AgentWorkMode } from '../../types';

const { t } = useI18n();

const userInput = ref("");
const isDragging = ref(false);
const inputRef = ref<HTMLTextAreaElement | null>(null);

/**
 * 待发送的附件。
 *
 * `natural*` 是原图尺寸、`compressed*` 是压缩后尺寸 —— 两者都留着只为了在缩略图上
 * 悬停时告诉用户"这张图被压到多大、大概吃多少 token"。**它们不参与发送**，
 * 发送用的仍是 `base64`；后端也只认 base64。
 *
 * 尺寸为 `null` 表示这张图没能解码（`createImageBitmap` 失败），此时提示里降级显示。
 */
interface MediaFile {
  path: string;
  type: 'image' | 'video';
  url: string;
  base64: string;
  naturalWidth: number | null;
  naturalHeight: number | null;
  compressedWidth: number | null;
  compressedHeight: number | null;
}

const mediaFiles = ref<MediaFile[]>([]);
const showVisionWarning = ref(false);
const showProfileCacheWarning = ref(false);
const pendingProfileId = ref<string | null>(null);

const session = useSessionStore();
const chat = useChatStore();
const agent = useAgentStore();
const uiPrefs = usePreferences();

const isRunning = computed(() =>
  session.getSessionView(session.activeSessionId).status === 'RUNNING'
);

const sessionTokenTotal = computed(() => (session.totalInputTokens || 0) + (session.totalOutputTokens || 0));

/**
 * 当前上下文占用。
 *
 * 口径统一在 `utils/contextUsage.ts`（右侧监控面板 `ContextInspector.vue` 用的是同一个函数）：
 * 优先取 `providerInputTokens`——最近一次请求**实际发出去**的 prompt token 总量，
 * 也是"上下文有多大"唯一权威的答案；厂商没上报才退回本地估算（**系统性偏低**，见该模块注释）。
 * 它同时也是浮层里「命中 X / 共 Y 输入 token」的 Y，两处显示同一个数才不会互相打架。
 */
const contextTokens = computed(() => resolveContextTokens(agent.currentContextSnapshot));

/**
 * 上下文窗口上限（快照的 `maxContextTokens`）。
 *
 * 来源是**编译期内嵌的 `model_registry.json`**（不是 API 上报，见 `registry.rs:64`），
 * 模型没登记窗口时为 null——此时不猜、只报已用量，并给一句「未知上下文窗口」的提示。
 */
const contextWindow = computed(() => agent.currentContextSnapshot?.maxContextTokens ?? null);

/**
 * 上下文占用读数（Codex / WorkBuddy 同款结构）：`70.8% · 212.5K / 300.0K 上下文已使用`。
 *
 * 概览栏不再直接铺这行字，而是收进**进度环的悬停浮层**（环由 `contextRingDash` 画）；
 * 窗口未登记时环不填色，读数里补一句「窗口未知」——不知道分母就不编百分比，
 * 与"未知 ≠ 0"的既有口径一致。
 *
 * 与「本次上下文」**共用同一个分子**（`contextTokens`，provider 实测优先）——同一行里
 * 百分比、已用、窗口三者必须自洽，若这里改用估算值就会出现"已用 / 窗口"与百分比对不上。
 *
 * ⚠️ **未扣输出预算**：环与读数展示的是「已用 / 窗口」，没有为下一次回复的 `max_tokens` 预留。
 * （此前快照里的 `maxOutputTokens` 是硬编码常量、不可信，故无从扣减；2026-09-17 起该字段已与
 * 请求体 `max_tokens` 同源，技术上前提具备。扣不扣是**口径选择**——「占窗口多少」还是「还剩多少可输入」，
 * 前者更贴近各家工具的通行展示，故暂不扣。）
 */
const contextUsageLabel = computed(() => {
  if (contextTokens.value <= 0) return null;
  const used = formatToken(contextTokens.value);
  const max = contextWindow.value;
  if (!max) {
    return `${used} ${t('input.tokenContextUsed')} · ${t('input.tokenWindowUnknown')}`;
  }
  const percent = Math.min(100, Math.round((contextTokens.value / max) * 1000) / 10);
  return `${percent.toFixed(1)}% · ${used} / ${formatToken(max)} ${t('input.tokenContextUsed')}`;
});

/** 进度环的圆周长（r=9，必须与模板里 `<circle r>` 一致，否则环会画不满/溢出） */
const CONTEXT_RING_CIRCUMFERENCE = 2 * Math.PI * 9;

/**
 * 进度环填充长度。窗口未知或无读数时返回 0：只画灰色轨道，**不画进度**——
 * 不知道分母就不能把环填到某个比例，那等于编一个占用率出来。
 */
const contextRingDash = computed(() => {
  const max = contextWindow.value;
  if (!max || contextTokens.value <= 0) return 0;
  const ratio = Math.min(1, contextTokens.value / max);
  return ratio * CONTEXT_RING_CIRCUMFERENCE;
});

/**
 * 单次口径的缓存命中读数（来自最近一次请求的上下文快照，由 `context-snapshot-updated` 事件刷新）。
 *
 * 语义（与后端 `infra/llm/usage.rs` 一致）：
 * - 未报告 → 「缓存未报告」（**不是 0%**，因为"没数据"和"没命中"是两回事）
 * - 报告了但为 0 → 「缓存预热中」（GLM 等档位前 1~2 次请求不命中属正常）
 * - 有命中 → 「缓存命中 N%」（**仅浮层用**；概览栏走下面另一个 computed）
 *
 * 这里**只报当前值**：逐 loop 趋势属于详细视图的内容，统一由右侧上下文监控面板
 * （`ContextInspector.vue` 的趋势柱）承担。原先把它拼进 tooltip，会让这个 hover
 * 表面随会话轮数长期挂着最多 10 个百分比，把「当前命中」这个主信息淹没掉；
 * 而趋势原本要解决的「单看一个数字会被预热期误导」（GLM 前 1~2 轮 0%、第 3 轮才命中）
 * 在面板里有柱状图承担，不需要在概览栏重复一遍。
 *
 * 明细改走自定义浮层 `CacheHitTooltip`：原生 `title` 样式跟随系统、约 1 秒延迟、
 * 单行不换行，塞不下进度条，也没法分组——而浮层里要同时装「本次请求」（当前命中）
 * 与「会话累计」（输入 / 输出 / 合计 + 会话级命中率 / 未命中）两个口径，
 * 分组标题必须能把它们隔开。后两行是解读「合计」的前提：`合计 ≈ 输入`，
 * 而输入是每轮重发整份上下文的累加值，脱离命中率就会被读成"真跑了这么多 token"。
 *
 * 概览栏因此只留两段，且**两段口径不同、各自把口径词写在标签里**：
 * - 「本次上下文」= 最近一次请求的 prompt。它**只能是瞬时值**——上下文不累加，
 *   随对话增长、随压缩缩小，也是"窗口还剩多少"与压缩判断的唯一依据；
 * - 「累计命中」= 整个会话的缓存命中率。比单次更稳、直接对应真实成本，
 *   并且避开了单次口径的已知偏差（多 loop 回合里单次只反映**最后一个 loop**，
 *   而那时前缀最暖 → 系统性偏乐观。实测同一次会话「本次 81%」vs「累计 88%」）。
 *
 * 会话累计的输入 / 输出 / 合计收进浮层；浮层「本次请求」组保留单次命中率，
 * 于是"单次 vs 累计"在界面上构成完整对照：栏上「本次上下文 + 累计命中」，
 * 浮层「本次请求 + 会话累计」。
 *
 * 为什么口径词必须写进标签本身，而不是靠浮层的分组标题承担：
 * 概览栏**没有分组标题**，浮层的口径对照要 hover 之后才出现。实测反馈就是——
 * 裸的「上下文 9.2k · 缓存命中 81%」被读成了会话累计值（尤其它和
 * 「会话累计 输入 16.6k」量级接近）。所以标签自带口径词，让两种口径在
 * **不 hover 时也同时可见**。
 *
 * 单次口径的字段来源：快照的 `cacheHitTokens` / `cacheMissTokens` 由
 * `repository::update_context_snapshot_usage` **覆盖写入**（不是累加），
 * 所以「本次」= 最近一次请求，而不是整轮。
 *
 * 概览栏原先放的是「合计」（会话累计输入 + 输出），换掉它的理由：
 * `合计 ≈ 输入`，而输入是每轮重发整份上下文的累加值——单看会读成"干了 1.8M token 的活"，
 * 实际是"同一个上下文重发了 58 次、其中 97.2% 走缓存"。这类数字必须配着命中率才能解读，
 * 那它就该待在浮层里，而不是占据概览位。
 *
 * 这个 computed 是**单次口径**，只喂浮层 `CacheHitTooltip` 的「本次请求」组
 * （浮层靠 `state === null` 判断要不要渲染那一整组）。
 * 概览栏上的读数走下面另一个 computed `barCacheUsage`（**会话累计**口径）。
 * 两者数据源不同，**不要合并**。
 */
const cacheUsage = computed(() => {
  const snap = agent.currentContextSnapshot;
  if (!snap) return null;
  const hit = snap.cacheHitTokens ?? null;
  if (hit === null) {
    return {
      label: t('input.cacheNotReported'),
      state: 'unknown' as const,
      percent: null,
      hitTokens: null,
      totalTokens: null,
    };
  }
  const total = hit + (snap.cacheMissTokens ?? 0);
  if (!total) return null;
  const percent = Math.round((hit / total) * 100);
  if (hit === 0) {
    return {
      label: t('input.cacheLabelWarmup'),
      state: 'warmup' as const,
      percent: 0,
      hitTokens: 0,
      totalTokens: total,
    };
  }
  return {
    label: `${t('input.cacheLabel')} ${percent}%`,
    state: 'hit' as const,
    percent,
    hitTokens: hit,
    totalTokens: total,
  };
});

/**
 * 概览栏上的缓存读数——**会话累计**口径（不是最近一次请求）。
 *
 * 为什么栏上用累计而不是单次：多 loop 的回合里，单次命中率只反映**最后一个 loop**，
 * 而那时前缀最暖 → **系统性偏乐观**（实测同一次会话里「本次 81%」vs「累计 88%」）。
 * 累计值更稳，且直接对应真实成本。
 *
 * 数据源是 `session.totalCacheHitTokens` / `totalCacheMissTokens`
 * （后端 `sessions` 表累加，见 `core/session/mod.rs:487`），
 * 与浮层「会话累计」组里的命中率是**同一个数**，两处不会打架。
 *
 * `hit + miss === 0` 判为「未报告」而不是 0%：后端对未上报端点会把两列都留在 0，
 * 所以两列全 0 恰好等价于"没数据"（与 `CacheHitTooltip` 的 `hasSessionCache` 同判据）。
 *
 * 百分比规则（`<1%`）必须与 `CacheHitTooltip` 的 `sessionHitPercentText` 保持一致：
 * 有命中却四舍五入成 `0%` 会和"一次都没命中"混淆。
 */
const barCacheUsage = computed(() => {
  // 还没跑过任何请求时不显示，避免开局就挂一条「未报告」噪音
  if (!agent.currentContextSnapshot) return null;

  const hit = session.totalCacheHitTokens || 0;
  const miss = session.totalCacheMissTokens || 0;
  if (hit + miss <= 0) {
    return { label: t('input.cacheNotReported'), state: 'unknown' as const };
  }
  if (hit === 0) {
    // 整个会话至今一次都没命中：预热期（或该端点根本不缓存），沿用原有蓝色状态
    return { label: t('input.cacheLabelWarmup'), state: 'warmup' as const };
  }
  const raw = (hit / (hit + miss)) * 100;
  const text = raw < 1 ? '<1' : `${Math.round(raw)}`;
  return { label: `${t('input.cacheLabelSession')} ${text}%`, state: 'hit' as const };
});

// 缓存读数浮层：hover 时展示明细（命中率进度条 + token 明细）
const showCacheTip = ref(false);

// 上下文进度环的浮层：只在悬停进度环时出现（与上面那个缓存浮层互不干扰）
const showContextTip = ref(false);

const openContextPanel = () => {
  // 打开右侧上下文监控窗口（App.vue 监听该状态并负责开窗）
  agent.showAgentPanel = true;
};

const formatToken = (n: number): string => {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1000).toFixed(1)}k`;
  return `${n}`;
};

// WorkMode 状态，与 usePreferences 双向同步 + 监听后端切换
const showWorkModeMenu = ref(false);
const showApprovalMenu = ref(false);
const currentWorkMode = ref<AgentWorkMode>(uiPrefs.agentWorkMode.value);
// 模式切换失败的提示（后端拒绝时显示）
const workModeWarning = ref("");

watch(() => uiPrefs.agentWorkMode.value, (val) => {
  currentWorkMode.value = val;
});

let unlistenDragDrop: (() => void) | null = null;
let unlistenConfig: UnlistenFn | null = null;
let unlistenWorkMode: UnlistenFn | null = null;

const appConfig = ref<any>(null);
const agentModel = computed(() => appConfig.value?.mainModel || '—');
const showProfileMenu = ref(false);
/** 预设菜单 DOM 引用：用于打开时把当前预设滚到可见处 */
const profileMenuRef = ref<HTMLElement | null>(null);
/**
 * 模型思考能力（唯一来源）。
 *
 * 由 `checkModelCapabilities` 异步填充，交给 `useThinkingMode` 做同步投影。
 * 注意：它**只影响 UI 显示**，不再决定"发什么参数"——本轮 thinking 由后端按
 * `sessionId` 自行裁决，因此这里即使短暂过期也污染不到真实请求。
 */
const modelThinkingCaps = ref<ThinkingCaps | null>(null);
const isThinkingForced = computed(() => modelThinkingCaps.value?.thinking_forced ?? false);
const canModelThink = computed(() => modelThinkingCaps.value?.thinking ?? true);
const canModelVision = ref(true);

/** 深度思考档位：会话级状态（切会话即切档位），不再是组件本地 ref */
const {
  isThinkingActive,
  saving: thinkingSaving,
  toggleThinking,
} = useThinkingMode(modelThinkingCaps);

/** 开关是否可点：模型不支持思考 / 强制思考时都不可点（点了也没用，但有文案解释） */
const thinkingToggleDisabled = computed(() => isThinkingToggleDisabled(modelThinkingCaps.value));

/**
 * 锁定态：模型强制开启思考，用户**改不了**（如 DeepSeek）。
 *
 * 注意 UI 上它读作"开启"而非"禁用"——思考确实是开着的，只是不可改。
 * 早期用 `.disabled { filter: grayscale(1) }` 把它整块灰掉，看起来像坏了。
 */
const thinkingLocked = computed(
  () => isThinkingForced.value && canModelThink.value && !thinkingSaving.value,
);

/**
 * 图片压缩参数直接读全局偏好（唯一来源是 `UiPreferences`，取值口径与后端
 * `get_image_compress_config` 完全一致）。
 *
 * 早期这里是"挂载时 invoke 一次、之后永不刷新"：用户在设置里改了档位，
 * 得等下次 `config-updated` 事件（只有存预设才发）才会重新拉，表现就是改完不生效。
 * 现在改成 computed，跟着偏好的响应式数据实时变（设置面板改档位后同窗口立即生效）。
 */
const imageCompressConfig = computed(() => ({
  maxWidth: uiPrefs.imageMaxWidth,
  maxHeight: uiPrefs.imageMaxHeight,
  quality: uiPrefs.imageQuality,
}));

const showInterruptedResumeHint = computed(() => {
  const view = session.currentSessionView;
  return !isRunning && (view.status === "INTERRUPTED" || Boolean(view.resumableRunId));
});

/**
 * 把压缩后的尺寸换算成"大约多少 token"。
 *
 * 口径：Anthropic 官方给出的图像 token 估算式 `(宽 × 高) / 750`。
 * 这是**展示用的估算值**，不参与任何请求构造 —— 真实用量以 API 返回为准。
 * 只给用户一个"这张图大概多贵"的数量级，因此不追求逐像素精确。
 */
const estimateImageTokens = (w: number | null, h: number | null): number | null => {
  if (!w || !h) return null;
  return Math.round((w * h) / 750);
};

const compressImage = async (
  fileData: Uint8Array,
  mimeType: string,
): Promise<{
  base64: string;
  mimeType: string;
  naturalWidth: number;
  naturalHeight: number;
  compressedWidth: number;
  compressedHeight: number;
}> => {
  const { maxWidth, maxHeight, quality } = imageCompressConfig.value;
  const blob = new Blob([new Uint8Array(fileData)], { type: mimeType });
  const bitmap = await createImageBitmap(blob);

  const naturalWidth = bitmap.width;
  const naturalHeight = bitmap.height;
  let w = naturalWidth;
  let h = naturalHeight;

  if (w > maxWidth || h > maxHeight) {
    const ratio = Math.min(maxWidth / w, maxHeight / h);
    w = Math.round(w * ratio);
    h = Math.round(h * ratio);
  }

  const canvas = new OffscreenCanvas(w, h);
  const ctx = canvas.getContext('2d')!;
  ctx.drawImage(bitmap, 0, 0, w, h);
  bitmap.close();

  const outputType = mimeType === 'image/png' ? 'image/png' : 'image/jpeg';
  const outputBlob = await canvas.convertToBlob({ type: outputType, quality });
  const reader = new FileReader();

  return new Promise((resolve) => {
    reader.onload = () => {
      const dataUrl = reader.result as string;
      resolve({
        base64: dataUrl,
        mimeType: outputType,
        naturalWidth,
        naturalHeight,
        compressedWidth: w,
        compressedHeight: h,
      });
    };
    reader.readAsDataURL(outputBlob);
  });
};

const checkModelCapabilities = async (modelId: string) => {
  if (!modelId) return;
  try {
    const caps = await invoke<any>('get_model_capabilities', { modelId });
    if (caps) {
      canModelVision.value = caps.vision ?? true;
      // 只更新能力；开关档位由 useThinkingMode 依据"会话档位 + 能力"统一投影，
      // 这里不再直接改写开关（改造前正是在这里漏掉了 isThinkingActive 的重置，
      // 导致切预设后沿用上一个预设的档位）。
      modelThinkingCaps.value = {
        thinking: caps.thinking ?? true,
        thinking_forced: caps.thinkingForced ?? false,
      };
    } else {
      // 注册表里没有该模型（自定义模型）：保守口径——支持思考、不强制
      canModelVision.value = true;
      modelThinkingCaps.value = { thinking: true, thinking_forced: false };
    }
  } catch (e) {
    console.error('Failed to check model capabilities:', e);
    canModelVision.value = true;
    modelThinkingCaps.value = { thinking: true, thinking_forced: false };
  }
};

const loadConfig = async () => {
  try {
    appConfig.value = await invoke('get_config');
    const activeProfile = appConfig.value.profiles.find((p: any) => p.id === appConfig.value.activeProfileId);
    if (activeProfile) {
      await checkModelCapabilities(activeProfile.config.mainModel);
    }
  } catch (e) {
    console.error('Failed to load config for input box:', e);
  }
};

/**
 * 激活预设变化时重新探测模型能力。
 *
 * 切会话（`Sidebar` 会把该会话的 `profileId` 写回激活预设）与切预设都会改变
 * `activeProfileId`；不重探就会一直拿着上一个预设的能力结论，开关的禁用态与
 * 提示文案都会滞后。
 */
watch(
  () => appConfig.value?.activeProfileId,
  (profileId, prevId) => {
    if (!profileId || profileId === prevId || !appConfig.value) return;
    const activeProfile = appConfig.value.profiles?.find((p: any) => p.id === profileId);
    if (activeProfile) {
      void checkModelCapabilities(activeProfile.config.mainModel);
    }
  },
);

const switchProfile = async (id: string) => {
  if (!appConfig.value) return;
  // 如果已有会话消息，切换模型会导致 prompt cache 失效，需确认
  const currentSessionId = await invoke<string | null>('get_active_session_id').catch(() => null);
  if (currentSessionId) {
    const msgs = await invoke<any[]>('get_session_messages', { sessionId: currentSessionId }).catch(() => []);
    if (msgs.length > 0) {
      pendingProfileId.value = id;
      showProfileCacheWarning.value = true;
      return;
    }
  }
  await doSwitchProfile(id);
};

const confirmSwitchProfile = async () => {
  showProfileCacheWarning.value = false;
  if (pendingProfileId.value) {
    await doSwitchProfile(pendingProfileId.value);
    pendingProfileId.value = null;
  }
};

const doSwitchProfile = async (id: string) => {
  if (!appConfig.value) return;
  const currentSessionId = await invoke<string | null>('get_active_session_id').catch(() => null);
  appConfig.value.activeProfileId = id;
  try {
    await invoke('save_config_cmd', { newConfig: appConfig.value });
    if (currentSessionId) {
      await invoke('update_session_profile', { id: currentSessionId, profileId: id });
    }
    showProfileMenu.value = false;
    const activeProfile = appConfig.value.profiles.find((p: any) => p.id === id);
    if (activeProfile) {
      await checkModelCapabilities(activeProfile.config.mainModel);
    }
    // 切模型后刷新历史，确保 data-user-message-index 同步
    try {
      if (session.activeSessionId) {
        const view = session.getSessionView(session.activeSessionId);
        if (view.status !== 'RUNNING') {
          try {
            const messages = await invoke<any[]>('get_session_messages', { sessionId: session.activeSessionId });
            session.replaceSessionMessages(session.activeSessionId, messages);
          } catch {
            const history = await invoke<string>('get_session_history', { sessionId: session.activeSessionId });
            session.replaceSessionHistory(session.activeSessionId, history);
          }
        }
      }
    } catch { /* ignore */ }
  } catch (e) {
    console.error('Failed to switch profile:', e);
  }
};

/** 当前权限档位（请求审批 / 帮我批准）：后端会话状态为准 */
const currentApprovalMode = ref<AgentApprovalMode>(uiPrefs.agentApprovalMode.value);

/** 只读保护（本会话禁止一切改动）：后端会话状态为准，不落盘 */
const agentReadOnly = ref(false);

/** 只读保护是独立闸门：不看权限档位、不看工作模式，切模式绕不过它 */
const toggleReadOnly = async () => {
  const next = !agentReadOnly.value;
  const prev = agentReadOnly.value;
  workModeWarning.value = "";
  try {
    if (session.activeSessionId) {
      await invoke('set_agent_read_only', { sessionId: session.activeSessionId, enabled: next });
    }
    agentReadOnly.value = next;
  } catch (e) {
    agentReadOnly.value = prev;
    workModeWarning.value = String(e);
    console.error('Failed to toggle read-only protection:', e);
  }
};

/**
 * 权限档位只改档位，**不碰工作模式**（两条轴分开：一个是"问得多严"，一个是"先出方案还是直接干"）
 */
const applyApprovalMode = async (mode: AgentApprovalMode) => {
  showApprovalMenu.value = false;
  if (mode === currentApprovalMode.value) {
    uiPrefs.setAgentApprovalMode(mode);
    return;
  }
  const prevApproval = currentApprovalMode.value;
  workModeWarning.value = "";
  try {
    if (session.activeSessionId) {
      await invoke('set_session_approval_mode', { sessionId: session.activeSessionId, mode });
    }
    uiPrefs.setAgentApprovalMode(mode);
    currentApprovalMode.value = mode;
  } catch (e) {
    currentApprovalMode.value = prevApproval;
    workModeWarning.value = String(e);
    console.error('Failed to switch approval mode:', e);
  }
};

/**
 * 工作模式只改模式（编辑 / 规划），**不碰权限档位**
 */
const applyWorkMode = async (mode: AgentUserMode) => {
  showWorkModeMenu.value = false;
  if (mode === currentWorkMode.value) return;
  const prev = currentWorkMode.value;
  workModeWarning.value = "";
  try {
    if (session.activeSessionId) {
      await invoke('set_session_work_mode', { sessionId: session.activeSessionId, mode });
    }
    currentWorkMode.value = mode;
    uiPrefs.setAgentWorkMode(mode);
  } catch (e) {
    currentWorkMode.value = prev;
    workModeWarning.value = String(e);
    console.error('Failed to switch work mode:', e);
  }
};

/** 会话状态才是事实来源：切换会话 / 刷新后校准档位与模式 */
const syncPermissionFromSession = async () => {
  const sid = session.activeSessionId;
  if (!sid) return;
  try {
    const mode = await invoke<AgentWorkMode>('get_session_work_mode', { sessionId: sid });
    currentWorkMode.value = mode;
    uiPrefs.setAgentWorkMode(mode);
    const settings = await invoke<{ approvalMode: AgentApprovalMode; readOnly?: boolean }>(
      'get_session_permission_settings',
      { sessionId: sid },
    );
    currentApprovalMode.value = settings.approvalMode;
    uiPrefs.setAgentApprovalMode(settings.approvalMode);
    agentReadOnly.value = settings.readOnly === true;
  } catch (e) {
    console.error('Failed to read session permission settings:', e);
  }
};

watch(() => session.activeSessionId, syncPermissionFromSession);

const hideWorkModeWarning = () => {
  workModeWarning.value = "";
};

const closeMenuOnOutsideClick = (e: MouseEvent) => {
  const target = e.target as HTMLElement;
  if (!target.closest('.profile-selector')) {
    showProfileMenu.value = false;
  }
  if (!target.closest('.work-mode-selector')) {
    showWorkModeMenu.value = false;
  }
  if (!target.closest('.approval-selector')) {
    showApprovalMenu.value = false;
  }
};

/**
 * 打开预设菜单时：① 按**实测可用空间**限制菜单高度 ② 把当前预设滚到可见处。
 *
 * 为什么不用固定 `vh`：输入区本身可能很高（用户粘贴了长文本、带附件预览），
 * 此时"按钮上方到窗口顶部"的可用空间远小于 55vh，固定值会让菜单顶出窗口被裁。
 * 这里用 `getBoundingClientRect()` 实测，再留出菜单与按钮之间的 12px 间隙和 12px 呼吸位。
 */
const MENU_GAP = 12;
const MENU_SAFE_MARGIN = 12;
const MENU_MAX_HEIGHT = 340;

const openProfileMenuLayout = async () => {
  await nextTick();
  const menu = profileMenuRef.value;
  if (!menu) return;

  const btn = menu.parentElement?.querySelector<HTMLElement>('.profile-btn');
  if (btn) {
    const spaceAbove = btn.getBoundingClientRect().top - MENU_GAP - MENU_SAFE_MARGIN;
    const height = Math.max(120, Math.min(MENU_MAX_HEIGHT, Math.floor(spaceAbove)));
    menu.style.maxHeight = `${height}px`;
  }

  // 当前项已在视野内时 `block: 'nearest'` 不会滚动，避免无谓跳动
  const activeItem = menu.querySelector<HTMLElement>('.profile-menu-item.active');
  activeItem?.scrollIntoView({ block: 'nearest' });
};

const hideVisionWarning = () => {
  showVisionWarning.value = false;
};

/**
 * 缩略图悬停提示：讲清"这张图被压到多大、大概吃多少 token"。
 *
 * 存在的理由：图片 token 混在总输入里根本看不出来（系统提示词+工具定义占大头），
 * 用户没法判断压缩档位到底有没有生效。这里把原图→压缩后的尺寸和估算 token 直接摆出来，
 * 切换档位时一眼可见。
 */
const mediaTooltip = (media: MediaFile): string => {
  const lines: string[] = [media.path.split(/[/\\]/).pop() || media.path];

  if (media.naturalWidth && media.naturalHeight) {
    let sizeLine = `${media.naturalWidth}×${media.naturalHeight}`;
    // 只有真的缩放过才显示箭头，避免"1920×1080 → 1920×1080"这种废话
    if (
      media.compressedWidth &&
      media.compressedHeight &&
      (media.compressedWidth !== media.naturalWidth || media.compressedHeight !== media.naturalHeight)
    ) {
      sizeLine += ` → ${media.compressedWidth}×${media.compressedHeight}`;
    } else {
      sizeLine += ` → ${t('input.imageNotCompressed')}`;
    }
    lines.push(sizeLine);
  }

  const tokens = estimateImageTokens(media.compressedWidth, media.compressedHeight);
  if (tokens !== null) {
    lines.push(t('input.imageTokenEstimate', { count: tokens.toLocaleString() }));
  }

  if (media.type === 'video') {
    lines.push(t('input.videoTokenUnknown'));
  }

  return lines.join('\n');
};

const processDroppedFiles = async (paths: string[]) => {
  let hasMediaFiles = false;
  
  for (const droppedPath of paths) {
    const lowerPath = droppedPath.toLowerCase();
    const isImage = lowerPath.endsWith('.jpg') || lowerPath.endsWith('.jpeg') || lowerPath.endsWith('.png') || lowerPath.endsWith('.gif') || lowerPath.endsWith('.webp');
    const isVideo = lowerPath.endsWith('.mp4') || lowerPath.endsWith('.webm') || lowerPath.endsWith('.mov');
    
    if (isImage || isVideo) {
      hasMediaFiles = true;
      
      let url = '';
      let base64 = '';
      // 尺寸在解码失败时保持 null → 缩略图提示降级显示，不编造数字
      let naturalWidth: number | null = null;
      let naturalHeight: number | null = null;
      let compressedWidth: number | null = null;
      let compressedHeight: number | null = null;

      if (isImage && canModelVision.value) {
        try {
          const fileData = await readFile(droppedPath);
          const ext = lowerPath.split('.').pop() || 'png';
          const mimeType = ext === 'jpg' || ext === 'jpeg' ? 'image/jpeg' : 
                           ext === 'gif' ? 'image/gif' : 
                           ext === 'webp' ? 'image/webp' : 'image/png';
          
          const compressed = await compressImage(new Uint8Array(fileData), mimeType);
          base64 = compressed.base64;
          naturalWidth = compressed.naturalWidth;
          naturalHeight = compressed.naturalHeight;
          compressedWidth = compressed.compressedWidth;
          compressedHeight = compressed.compressedHeight;
          url = URL.createObjectURL(new Blob([new Uint8Array(fileData)], { type: mimeType }));
        } catch (e) {
          console.error('Failed to read image file:', e);
        }
      }
      
      mediaFiles.value.push({
        path: droppedPath,
        type: isImage ? 'image' : 'video',
        url,
        base64,
        naturalWidth,
        naturalHeight,
        compressedWidth,
        compressedHeight,
      });
    } else {
      if (userInput.value) {
        userInput.value += ` ${droppedPath}`;
      } else {
        userInput.value = droppedPath;
      }
    }
  }
  
  if (hasMediaFiles && !canModelVision.value) {
    showVisionWarning.value = true;
  }
  
  nextTick(() => {
    inputRef.value?.focus();
    adjustHeight();
  });
};

onMounted(async () => {
  await loadConfig();
  // 档位/模式控件以会话状态为准（刷新后恢复显示）
  await syncPermissionFromSession();
  // 如果没有活跃会话，将模型初始化为全局默认
  if (!session.activeSessionId && appConfig.value?.globalProfileId) {
    appConfig.value.activeProfileId = appConfig.value.globalProfileId;
    await invoke('save_config_cmd', { newConfig: appConfig.value });
  }

  unlistenConfig = await listen('config-updated', async () => {
    loadConfig();
    try {
      if (session.activeSessionId) {
        const view = session.getSessionView(session.activeSessionId);
        if (view.status !== 'RUNNING') {
          try {
            const messages = await invoke<any[]>('get_session_messages', { sessionId: session.activeSessionId });
            session.replaceSessionMessages(session.activeSessionId, messages);
          } catch {
            const history = await invoke<string>('get_session_history', { sessionId: session.activeSessionId });
            session.replaceSessionHistory(session.activeSessionId, history);
          }
        }
      }
    } catch { /* ignore */ }
  });

  // 监听 Agent 自动切换 WorkMode 的事件
  unlistenWorkMode = await listen<{ from: string; to: string; reason: string }>('agent-work-mode-changed', (event) => {
    currentWorkMode.value = event.payload.to as AgentWorkMode;
  });

  // 权限档位变化（用户在设置面板里改了，或后端初始化）
  await listen<{ mode: AgentApprovalMode }>('approval-mode-changed', (event) => {
    currentApprovalMode.value = event.payload.mode;
  });
  // 监听监控窗口上下文压缩状态，压缩期间禁用输入
  unlistenCompacting = await listen<{ compacting: boolean }>('bg-compacting-changed', (event) => {
    isCompacting.value = event.payload.compacting;
  });
  // 挂载时主动查询后端，防止 F5 刷新丢失"压缩中"状态
  if (session.activeSessionId) {
    try {
      isCompacting.value = await invoke<boolean>('is_session_compacting', { sessionId: session.activeSessionId });
    } catch { /* ignore */ }
  }

  document.addEventListener('click', closeMenuOnOutsideClick);

  const appWindow = getCurrentWindow();
  unlistenDragDrop = await appWindow.onDragDropEvent((event) => {
    if (event.payload.type === 'enter') {
      isDragging.value = true;
      document.body.classList.add('dragging-active');
    } else if (event.payload.type === 'leave' || event.payload.type === 'drop') {
      isDragging.value = false;
      document.body.classList.remove('dragging-active');
    }
    
    if (event.payload.type === 'drop') {
      const paths = event.payload.paths;
      if (paths.length > 0) {
        processDroppedFiles(paths);
      }
    }
  });
});

/**
 * 把「输入区实际高度」写成根级 CSS 变量 `--input-area-height`。
 *
 * 为什么需要：消息列表的底部留白（`.response-area` 的 padding-bottom）和「滚动到底部」
 * 按钮的悬浮位置都必须贴着浮动输入框，而输入框高度是**内容/窗口驱动**的 —— 多行输入、
 * 附件条、权限卡片、计划面板、窄窗口下文字换行变多，都会改变它的高度。
 * 以前这两处各自硬编码了魔数（padding 200px、按钮 bottom 180px，分散在两个文件里），
 * 窗口或内容一变就失配：按钮压住输入框、或最后几条消息被输入框遮住。
 * 现在改为「一处测量、多处消费」。
 *
 * 观察的是外层 `.floating-terminal-container`（含它 32px 的 padding），
 * 这样量到的是「输入区整体占位高度」，直接用即可。
 */
const inputAreaRef = ref<HTMLElement | null>(null);
let inputAreaObserver: ResizeObserver | null = null;

const syncInputAreaHeight = () => {
  const host = inputAreaRef.value?.closest('.floating-terminal-container') as HTMLElement | null;
  if (!host) return;
  const height = Math.round(host.getBoundingClientRect().height);
  if (height > 0) {
    document.documentElement.style.setProperty('--input-area-height', `${height}px`);
  }
};

onMounted(() => {
  const host = inputAreaRef.value?.closest('.floating-terminal-container') as HTMLElement | null;
  syncInputAreaHeight(); // 首帧先量一次，避免先用兜底值闪一下
  if (!host || typeof ResizeObserver === 'undefined') return;
  inputAreaObserver = new ResizeObserver(syncInputAreaHeight);
  inputAreaObserver.observe(host);
});

onUnmounted(() => {
  if (unlistenDragDrop) unlistenDragDrop();
  if (unlistenConfig) unlistenConfig();
  if (unlistenWorkMode) unlistenWorkMode();
  if (unlistenCompacting) unlistenCompacting();
});

onBeforeUnmount(() => {
  document.removeEventListener('click', closeMenuOnOutsideClick);
  inputAreaObserver?.disconnect();
  inputAreaObserver = null;
  document.documentElement.style.removeProperty('--input-area-height');
});

const adjustHeight = () => {
  const el = inputRef.value;
  if (!el) return;
  el.style.height = 'auto';
  const newHeight = Math.min(el.scrollHeight, 200);
  el.style.height = `${newHeight}px`;
};

const handleInput = () => {
  adjustHeight();
};

const handleKeydown = (e: KeyboardEvent) => {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault();
    if (!isRunning.value && !isCompacting.value) {
      handleSubmit();
    }
  }
};

const handleSubmit = () => {
  const msg = userInput.value.trim();
  if (msg || mediaFiles.value.length > 0) {
    const imageBase64List = mediaFiles.value
      .filter(m => m.type === 'image' && m.base64)
      .map(m => m.base64);
    // 不再回传本地的思考开关值：本轮 thinking 由后端按 sessionId + 会话档位自行裁决，
    // 这样即使 UI 投影短暂过期（如能力查询未返回）也污染不到真实请求。
    chat.sendToJarvis(msg, null, imageBase64List);
    userInput.value = '';
    mediaFiles.value.forEach(m => {
      if (m.url) URL.revokeObjectURL(m.url);
    });
    mediaFiles.value = [];
    showVisionWarning.value = false;
    nextTick(() => {
      adjustHeight();
    });
  }
};

const isCancelling = ref(false);
const isCompacting = ref(false);
let unlistenCompacting: UnlistenFn | null = null;

const handleCancel = async () => {
  if (isCancelling.value) return;
  isCancelling.value = true;
  try {
    await chat.cancelJarvis();
  } finally {
    isCancelling.value = false;
  }
};

const removeMediaFile = (index: number) => {
  const [removed] = mediaFiles.value.splice(index, 1);
  if (removed?.url) {
    URL.revokeObjectURL(removed.url);
  }
  if (mediaFiles.value.length === 0) {
    showVisionWarning.value = false;
  }
};

watch(() => chat.rollbackRecalledMessage, (msg) => {
  if (msg) {
    userInput.value = msg;
    chat.rollbackRecalledMessage = "";
    nextTick(() => {
      inputRef.value?.focus();
      adjustHeight();
    });
  }
});

const handleRecallEdit = async () => {
  const text = await chat.recallAndEdit();
  if (text) {
    userInput.value = text;
    nextTick(() => {
      inputRef.value?.focus();
      adjustHeight();
    });
  }
};
</script>

<template>
  <div class="chat-input-container" ref="inputAreaRef">
    <div class="chat-input-wrapper">
      
      <div v-if="chat.showRecallEdit" class="recall-edit-bar">
        <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="1 4 1 10 7 10"></polyline>
          <path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"></path>
        </svg>
        <span>{{ t('input.recallHint') }}</span>
        <button class="recall-edit-btn" @click="handleRecallEdit">{{ t('input.recallEdit') }}</button>
        <button class="recall-dismiss-btn" @click="chat.dismissRecallEdit" :aria-label="t('common.close')">
          <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <div v-if="showInterruptedResumeHint" class="resume-run-bar">
        <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <path d="M3 12a9 9 0 0 1 15.5-6.2"></path>
          <polyline points="18 2 18 6 14 6"></polyline>
          <path d="M21 12a9 9 0 0 1-15.5 6.2"></path>
          <polyline points="6 22 6 18 10 18"></polyline>
        </svg>
        <span>{{ t('input.resumeHint') }}</span>
      </div>

      <div class="input-toolbar">
        <div class="profile-selector">
          <button class="profile-btn" @click="showProfileMenu = !showProfileMenu; showProfileMenu && openProfileMenuLayout()">
            <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2.5" fill="none" stroke-linecap="round" stroke-linejoin="round" class="profile-icon">
              <path d="m12 3-1.912 5.813a2 2 0 0 1-1.275 1.275L3 12l5.813 1.912a2 2 0 0 1 1.275 1.275L12 21l1.912-5.813a2 2 0 0 1 1.275-1.275L21 12l-5.813-1.912a2 2 0 0 1-1.275-1.275L12 3Z"></path>
            </svg>
            {{ appConfig?.profiles.find((p: any) => p.id === appConfig?.activeProfileId)?.name || t('input.selectModel') }}
            <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none"><polyline points="6 9 12 15 18 9"></polyline></svg>
          </button>
          
          <div v-if="showProfileMenu" class="profile-menu" ref="profileMenuRef">
            <div 
              v-for="profile in appConfig?.profiles" 
              :key="profile.id"
              class="profile-menu-item"
              :class="{ active: appConfig?.activeProfileId === profile.id }"
              @click="switchProfile(profile.id)"
            >
              <div class="profile-menu-name">{{ profile.name }}</div>
              <div class="profile-menu-model">{{ profile.config.mainModel }}</div>
            </div>
          </div>
        </div>
        
        <div class="toolbar-spacer"></div>

        <!-- 权限档位：只决定"问得多严"，与工作模式互不影响 -->
        <div class="work-mode-selector approval-selector">
          <button class="work-mode-btn" @click="showApprovalMenu = !showApprovalMenu">
            <span class="work-mode-dot" :class="currentApprovalMode"></span>
            <span class="work-mode-label">{{ t('settings.general.' + currentApprovalMode) }}</span>
            <svg viewBox="0 0 24 24" width="10" height="10" stroke="currentColor" stroke-width="2" fill="none"><polyline points="6 9 12 15 18 9"></polyline></svg>
          </button>

          <div v-if="showApprovalMenu" class="work-mode-menu">
            <div class="work-mode-menu-inner">
              <div
                v-for="mode in (['request_approval', 'auto_approve'] as AgentApprovalMode[])"
                :key="mode"
                class="work-mode-menu-item"
                :class="{ active: currentApprovalMode === mode }"
                @click="applyApprovalMode(mode)"
              >
                <span class="work-mode-dot" :class="mode"></span>
                <div class="work-mode-menu-text">
                  <div class="work-mode-menu-name">{{ t('settings.general.' + mode) }}</div>
                  <div class="work-mode-menu-desc">{{ t('settings.general.' + mode + 'DescShort') }}</div>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- 工作模式：编辑（直接干）/ 规划（先出方案），与权限档位互不影响 -->
        <div class="work-mode-selector">
          <button class="work-mode-btn" @click="showWorkModeMenu = !showWorkModeMenu">
            <span class="work-mode-dot" :class="currentWorkMode"></span>
            <span class="work-mode-label">{{ t('settings.general.' + currentWorkMode) }}</span>
            <svg viewBox="0 0 24 24" width="10" height="10" stroke="currentColor" stroke-width="2" fill="none"><polyline points="6 9 12 15 18 9"></polyline></svg>
          </button>

          <div v-if="showWorkModeMenu" class="work-mode-menu">
            <div class="work-mode-menu-inner">
              <div
                v-for="mode in (['edit', 'plan'] as AgentUserMode[])"
                :key="mode"
                class="work-mode-menu-item"
                :class="{ active: currentWorkMode === mode }"
                @click="applyWorkMode(mode)"
              >
                <span class="work-mode-dot" :class="mode"></span>
                <div class="work-mode-menu-text">
                  <div class="work-mode-menu-name">{{ t('settings.general.' + mode) }}</div>
                  <div class="work-mode-menu-desc">{{ t('settings.general.' + mode + 'DescShort') }}</div>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- 只读保护：独立闸门，开启后本会话禁止一切改动（写文件 / 非只读命令 / 派子代理 / 改工作目录） -->
        <button
          class="action-toggle-btn readonly-toggle"
          :class="{ active: agentReadOnly }"
          @click="toggleReadOnly"
          :title="agentReadOnly ? t('settings.general.readOnlyOnTitle') : t('settings.general.readOnlyOffTitle')"
        >
          <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round">
            <rect x="3" y="11" width="18" height="11" rx="2" />
            <path d="M7 11V7a5 5 0 0 1 10 0v4" />
          </svg>
          <span>{{ t('settings.general.readOnly') }}</span>
          <!-- 迷你滑块：状态信号全靠滑块位置，纯中性色 -->
          <span class="mini-switch" :class="{ on: agentReadOnly }" aria-hidden="true"></span>
        </button>

        <div class="toolbar-right">
          <button
            class="action-toggle-btn"
            :class="{
              active: isThinkingActive,
              disabled: thinkingToggleDisabled,
              locked: thinkingLocked
            }"
            :disabled="thinkingToggleDisabled || thinkingSaving"
            @click="toggleThinking"
            :title="!canModelThink ? t('input.thinkingUnsupportedTitle') : isThinkingForced ? t('input.thinkingForcedTitle') : (isThinkingActive ? t('input.thinkingOnTitle') : t('input.thinkingOffTitle'))"
          >
            <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round">
              <path d="M9.59 4.59A2 2 0 1 1 11 8H2m10.59 11.41A2 2 0 1 0 14 16H2m15.73-8.27a5 5 0 1 1-7.14 7.14" />
            </svg>
            <span>{{ !canModelThink ? t('input.thinkingUnsupported') : isThinkingForced ? t('input.thinkingForced') : t('input.thinking') }}</span>
            <!-- 迷你滑块：状态信号全靠滑块位置，纯中性色 -->
            <span class="mini-switch" :class="{ on: isThinkingActive }" aria-hidden="true"></span>
            <!-- 锁定角标：模型强制开启、用户改不了（读作"开启"而非"禁用"） -->
            <svg
              v-if="thinkingLocked"
              class="thinking-lock-badge"
              viewBox="0 0 24 24"
              width="9"
              height="9"
              fill="none"
              stroke="currentColor"
              stroke-width="2.6"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <rect x="5" y="11" width="14" height="10" rx="2" />
              <path d="M8 11V7a4 4 0 0 1 8 0v4" />
            </svg>
          </button>
        </div>
      </div>

      <div v-if="mediaFiles.length > 0" class="media-preview-container">
        <div v-for="(media, index) in mediaFiles" :key="index" class="media-preview-item">
          <template v-if="media.type === 'image' && media.url">
            <img :src="media.url" class="media-thumbnail" alt="preview" :title="mediaTooltip(media)" />
          </template>
          <template v-else>
            <div class="media-icon">
              <svg v-if="media.type === 'image'" viewBox="0 0 24 24" width="24" height="24" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
                <circle cx="8.5" cy="8.5" r="1.5"></circle>
                <polyline points="21 15 16 10 5 21"></polyline>
              </svg>
              <svg v-else viewBox="0 0 24 24" width="24" height="24" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
                <polygon points="23 7 16 12 23 17 23 7"></polygon>
                <rect x="1" y="5" width="15" height="14" rx="2" ry="2"></rect>
              </svg>
            </div>
          </template>
          <span class="media-name" :title="mediaTooltip(media)">{{ media.path.split(/[/\\]/).pop() }}</span>
          <button class="remove-media-btn" @click.stop="removeMediaFile(index)" :title="t('input.remove')" :aria-label="t('input.remove')">
            <svg viewBox="0 0 24 24" width="10" height="10" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <line x1="18" y1="6" x2="6" y2="18"></line>
              <line x1="6" y1="6" x2="18" y2="18"></line>
            </svg>
          </button>
        </div>
      </div>

      <div v-if="showVisionWarning" class="vision-warning">
        <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path>
          <line x1="12" y1="9" x2="12" y2="13"></line>
          <line x1="12" y1="17" x2="12.01" y2="17"></line>
        </svg>
        <span>{{ t('input.visionWarning') }}</span>
        <button class="warning-close-btn" @click="hideVisionWarning" :aria-label="t('common.close')">
          <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <div v-if="workModeWarning" class="vision-warning">
        <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
          <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"></path>
          <line x1="12" y1="9" x2="12" y2="13"></line>
          <line x1="12" y1="17" x2="12.01" y2="17"></line>
        </svg>
        <span>{{ workModeWarning }}</span>
        <button class="warning-close-btn" @click="hideWorkModeWarning" :aria-label="t('common.close')">
          <svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>

      <div class="input-row" @click="inputRef?.focus()">
        <textarea
          ref="inputRef"
          v-model="userInput"
          :placeholder="isCompacting ? '上下文压缩中，请稍候...' : t('input.placeholder')"
          class="editor-input"
          :class="{ 'input-disabled': isCompacting }"
          autofocus
          rows="1"
          :disabled="isCompacting"
          @input="handleInput"
          @keydown="handleKeydown"
        ></textarea>

        <button v-if="isCompacting" class="send-btn compacting-state" disabled :title="'上下文压缩中'">
          <svg class="spinner-icon" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="10" stroke-opacity="0.25"></circle>
            <path d="M12 2a10 10 0 0 1 10 10"></path>
          </svg>
        </button>
        <button v-else-if="!isRunning" class="send-btn" :class="{ active: userInput.trim() || mediaFiles.length > 0 }" @click="handleSubmit" :title="t('input.send')">
          <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
            <line x1="22" y1="2" x2="11" y2="13"></line>
            <polygon points="22 2 15 22 11 13 2 9 22 2"></polygon>
          </svg>
        </button>
        <button v-else class="send-btn active stop-state" @click="handleCancel" :title="t('input.stop')" :disabled="isCancelling">
          <svg v-if="!isCancelling" viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2" /></svg>
          <svg v-else class="spinner-icon" viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="10"></circle>
            <path d="M12 2a10 10 0 0 1 10 10"></path>
          </svg>
        </button>
      </div>

      <!-- 上下文与缓存读数 -->
      <div class="token-bar" v-if="contextTokens > 0 || sessionTokenTotal > 0">
        <!-- 上下文占用：概览栏只留一个进度环，读数收进悬停浮层（Codex / WorkBuddy 同款）。
             环的填充比例 = 已用 / 窗口；窗口未登记时只画灰轨道，浮层文字里注明「窗口未知」。

             刻意放在 `.token-bar-usage` **外面**：那一组整体 hover 会弹缓存明细浮层，
             环若嵌在里面，悬停时会同时冒出两个浮层、互相遮挡。 -->
        <span
          v-if="contextUsageLabel"
          class="token-bar-item token-bar-ring"
          @mouseenter="showContextTip = true"
          @mouseleave="showContextTip = false"
          @click="openContextPanel"
        >
          <svg class="context-ring" viewBox="0 0 24 24" width="13" height="13" aria-hidden="true">
            <circle class="ring-track" cx="12" cy="12" r="9" />
            <circle
              class="ring-fill"
              cx="12"
              cy="12"
              r="9"
              :stroke-dasharray="`${contextRingDash} ${CONTEXT_RING_CIRCUMFERENCE}`"
              transform="rotate(-90 12 12)"
            />
          </svg>
          <span v-if="showContextTip" class="context-tip" role="tooltip">
            {{ contextUsageLabel }}
          </span>
        </span>

        <!-- 缓存命中：hover 出明细浮层，点一下打开右侧上下文监控窗口 -->
        <span
          class="token-bar-usage"
          @mouseenter="showCacheTip = true"
          @mouseleave="showCacheTip = false"
          @click="openContextPanel"
        >
          <template v-if="barCacheUsage">
            <span v-if="contextTokens > 0" class="token-bar-sep">·</span>
            <span class="token-bar-item token-bar-cache" :class="`cache-${barCacheUsage.state}`">
              {{ barCacheUsage.label }}
            </span>
          </template>
          <CacheHitTooltip
            v-if="showCacheTip"
            :state="cacheUsage?.state ?? null"
            :percent="cacheUsage?.percent ?? null"
            :hit-tokens="cacheUsage?.hitTokens ?? null"
            :total-tokens="cacheUsage?.totalTokens ?? null"
            :session-input-tokens="session.totalInputTokens || 0"
            :session-output-tokens="session.totalOutputTokens || 0"
            :session-total-tokens="sessionTokenTotal"
            :session-cache-hit-tokens="session.totalCacheHitTokens"
            :session-cache-miss-tokens="session.totalCacheMissTokens"
          />
        </span>
        <span class="token-bar-spacer"></span>
        <span class="token-bar-item token-bar-model">{{ agentModel }}</span>
      </div>

    </div>
  </div>

  <ConfirmModal
    :open="showProfileCacheWarning"
    :title="t('settings.tabs.presets')"
    :message="t('settings.profiles.cacheWarning', { name: appConfig?.profiles.find((p: any) => p.id === pendingProfileId)?.name || '' })"
    confirm-kind="primary"
    @cancel="showProfileCacheWarning = false; pendingProfileId = null"
    @confirm="confirmSwitchProfile"
  />
</template>

<style scoped>
.chat-input-container {
  padding: 0;
  background-color: transparent;
  display: flex;
  flex-direction: column;
  position: relative;
  width: 100%;
  align-items: center; /* 居中核心 */
}

.chat-input-container::before {
  display: none;
}

.chat-input-wrapper {
  width: 100%;
  max-width: min(85%, 960px); /* 与消息区同一套宽度口径，上下对齐 */
  background: var(--surface-strong);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid var(--glass-border);
  border-radius: 24px; /* 增加圆角度，使其更圆润 */
  box-shadow: var(--shadow-lg);
  display: flex;
  flex-direction: column;
  transition: all var(--transition-normal);
  position: relative;
  z-index: 1;
}

.chat-input-wrapper:hover {
  border-color: var(--glass-border);
  box-shadow: var(--shadow-lg);
}

.chat-input-wrapper:focus-within {
  border-color: var(--glass-border);
  box-shadow: var(--shadow-lg);
}

.input-toolbar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 12px 16px 0;
  flex-wrap: wrap;
  gap: 8px;
}

.profile-selector {
  position: relative;
}

.profile-btn {
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  color: var(--text-main);
  padding: 6px 12px;
  font-size: 0.85rem;
  font-weight: 600;
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  transition: all var(--transition-fast);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}

.profile-btn:hover {
  background: var(--glass-bg);
  border-color: var(--glass-border);
  transform: translateY(-1px);
}

.profile-icon {
  flex-shrink: 0;
  color: var(--accent-blue);
  opacity: 0.8;
}

.profile-menu {
  position: absolute;
  bottom: calc(100% + 12px);
  left: 0;
  background: var(--surface-strong);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid color-mix(in srgb, var(--text-muted) 22%, transparent);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  min-width: 240px;
  /* 预设过多时不再无限向上撑高，改为在菜单内部滚动。
     取 min() 双保险：绝对高度上限 340px（约 4.5 项，暗示"下面还有"），
     同时不超过视口 55%，避免小窗口里菜单顶到窗口外被裁掉。 */
  max-height: min(340px, 55vh);
  overflow-y: auto;
  overflow-x: hidden;
  overscroll-behavior: contain; /* 滚到底不把滚动传给聊天区 */
  scrollbar-gutter: stable;     /* 有无滚动条时宽度不跳 */
  z-index: 100;
  animation: popIn var(--transition-fast);
  padding: 8px;
}

@keyframes popIn {
  from { opacity: 0; transform: translateY(8px) scale(0.96); }
  to { opacity: 1; transform: translateY(0) scale(1); }
}

.profile-menu-item {
  position: relative;
  padding: 10px 32px 10px 14px;
  cursor: pointer;
  border-bottom: 1px solid color-mix(in srgb, var(--text-muted) 12%, transparent);
  transition: background var(--transition-fast);
  border-radius: var(--radius-md);
  margin-bottom: 2px;
}

.profile-menu-item:last-child {
  border-bottom: none;
  margin-bottom: 0;
}

.profile-menu-item:hover {
  background: color-mix(in srgb, var(--text-muted) 8%, transparent);
}

.profile-menu-item.active {
  background: color-mix(in srgb, var(--text-muted) 16%, transparent);
  border: 1px solid var(--glass-border);
}

.profile-menu-name {
  font-size: 0.85rem;
  font-weight: 600;
  color: var(--text-main);
  margin-bottom: 4px;
}

.profile-menu-model {
  font-size: 0.75rem;
  color: var(--text-muted);
  font-family: var(--font-mono);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.toolbar-right {
  display: flex;
  align-items: center;
  gap: 8px;
}

/* 深度思考开关：中性色方案（N1）
   关闭 = 下沉面 + 细描边 + 中字重 + 弱文字
   开启 = 抬升面 + 常规边 + 粗描边 + 重字重 + 主色文字 + 微阴影
   主信号是"文字对比度跨度"（约 2.4 倍），面与边只作辅助——纯中性色下底色的
   可用对比度只有约 1.23:1，不足以独立承担状态区分。 */
.action-toggle-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: var(--thinking-off-surface);
  color: var(--thinking-off-fg);
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  padding: 6px 12px;
  font-size: 0.75rem;
  font-weight: 500;
  cursor: pointer;
  transition: all var(--transition-fast);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
  box-shadow: none;
  position: relative;
}

.action-toggle-btn svg {
  stroke-width: 1.7;
  transition: stroke-width var(--transition-fast);
}

/* 关闭 · hover：预示"可开启"，但不加粗描边，避免误读为已开启 */
.action-toggle-btn:hover {
  background: var(--glass-bg);
  border-color: var(--glass-border);
  color: var(--text-main);
}

/* 开启（选中）：三个信号同时拉满 */
.action-toggle-btn.active {
  background: var(--thinking-on-surface);
  border-color: var(--glass-border);
  color: var(--thinking-on-fg);
  font-weight: 700;
  box-shadow: var(--shadow-sm);
}

.action-toggle-btn.active svg {
  stroke-width: 2.4;
}

.action-toggle-btn.active:hover {
  background: var(--thinking-on-surface);
  border-color: var(--glass-border);
}

/* 鼠标按下：项目通用按压反馈 */
.action-toggle-btn:active {
  transform: scale(0.97);
}

/* 不可点（不支持思考 / 模型强制开启、用户改不了）：
   必须是"未选中"外观，否则用户会以为它选着。 */
.action-toggle-btn.disabled {
  cursor: not-allowed;
  opacity: 0.65;
  transform: none;
}

.action-toggle-btn.disabled:not(.active) {
  background: var(--thinking-off-surface);
  border-style: dashed;
  border-color: var(--glass-border);
  color: var(--text-muted);
  font-weight: 500;
}

.action-toggle-btn.disabled:not(.active) svg {
  stroke-width: 1.7;
}

/* 锁定态（模型强制开启）：读作"开启"，只靠锁角标与虚线边说明"你改不了" */
.action-toggle-btn.locked {
  cursor: not-allowed;
}

.action-toggle-btn.locked:active {
  transform: none;
}

.action-toggle-btn.locked .thinking-lock-badge {
  position: absolute;
  right: 3px;
  bottom: 2px;
  color: inherit;
  opacity: 0.85;
}

/* 迷你滑块开关：只承担"状态"信号，位置即状态（左=关 / 右=开）。
   刻意全中性色（与整个工具条的极简口径一致）：
   开 = 轨道填充 --text-main，滑块反白；
   关 = 下沉轨道 + 弱色滑块。不引入任何彩色。 */
.action-toggle-btn .mini-switch {
  flex: none;
  width: 22px;
  height: 12px;
  border-radius: 999px;
  border: 1px solid var(--glass-border);
  background: var(--thinking-off-surface);
  position: relative;
  transition: background var(--transition-fast), border-color var(--transition-fast);
}

.action-toggle-btn .mini-switch::after {
  content: "";
  position: absolute;
  top: 1px;
  left: 1px;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--text-muted);
  transition: transform var(--transition-fast), background var(--transition-fast);
}

.action-toggle-btn .mini-switch.on {
  background: var(--text-main);
  border-color: var(--text-main);
}

.action-toggle-btn .mini-switch.on::after {
  transform: translateX(10px);
  background: var(--bg-panel);
}

.toolbar-spacer {
  flex: 1;
}

.work-mode-selector {
  position: relative;
}

.work-mode-btn {
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  border-radius: var(--radius-md);
  color: var(--text-main);
  padding: 6px 12px;
  font-size: 0.8rem;
  font-weight: 600;
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  transition: background var(--transition-fast), border-color var(--transition-fast), transform var(--transition-fast), color var(--transition-fast), box-shadow var(--transition-fast);
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}

.work-mode-btn:hover {
  background: var(--glass-bg);
  border-color: var(--glass-border);
  transform: translateY(-1px);
}

.work-mode-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}

.work-mode-dot.chat { background: var(--text-muted); }
.work-mode-dot.edit { background: var(--text-muted); }
.work-mode-dot.plan { background: var(--text-muted); }
.work-mode-dot.readonly { background: var(--text-muted); }
.work-mode-dot.request_approval { background: var(--text-muted); }
.work-mode-dot.auto_approve { background: var(--text-muted); }

.work-mode-btn.readonly {
  border-color: var(--glass-border);
}

.work-mode-menu-hint {
  font-size: 0.68rem;
  color: var(--text-muted);
  padding: 4px 14px 2px;
}

.work-mode-label {
  white-space: nowrap;
}

.work-mode-menu {
  position: absolute;
  bottom: calc(100% + 12px);
  left: 50%;
  transform: translateX(-50%);
  z-index: 100;
  min-width: 200px;
}

.work-mode-menu-inner {
  background: var(--surface-strong);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid color-mix(in srgb, var(--text-muted) 22%, transparent);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  overflow: hidden;
  animation: popIn var(--transition-fast);
  padding: 8px;
}

.work-mode-menu-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 14px;
  cursor: pointer;
  transition: background var(--transition-fast);
  border-radius: var(--radius-md);
  margin-bottom: 2px;
}

.work-mode-menu-item:last-child {
  margin-bottom: 0;
}

.work-mode-menu-item:hover {
  background: color-mix(in srgb, var(--accent-blue) 10%, transparent);
}

.work-mode-menu-item.active {
  background: color-mix(in srgb, var(--accent-blue) 12%, transparent);
}

.work-mode-menu-text {
  display: flex;
  flex-direction: column;
}

.work-mode-menu-name {
  font-size: 0.85rem;
  font-weight: 600;
  color: var(--text-main);
}

.work-mode-menu-desc {
  font-size: 0.7rem;
  color: var(--text-muted);
  margin-top: 2px;
}

.work-mode-menu-sep {
  height: 1px;
  margin: 6px 8px;
  background: color-mix(in srgb, var(--text-muted) 22%, transparent);
}

.work-mode-menu-item.readonly-item .work-mode-menu-text {
  flex: 1;
}

.work-mode-switch {
  width: 30px;
  height: 17px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text-muted) 35%, transparent);
  display: inline-flex;
  align-items: center;
  padding: 2px;
  flex-shrink: 0;
  transition: background var(--transition-fast);
}

.work-mode-switch.on {
  background: var(--accent-blue);
}

.work-mode-switch-knob {
  width: 13px;
  height: 13px;
  border-radius: 50%;
  background: #fff;
  transition: transform var(--transition-fast);
}

.work-mode-switch.on .work-mode-switch-knob {
  transform: translateX(13px);
}

.input-row {
  display: flex;
  align-items: flex-end;
  padding: 14px 16px 16px;
  cursor: text;
}

.editor-input {
  flex: 1;
  background: transparent;
  border: none;
  border-radius: var(--radius-md);
  color: var(--text-main);
  font-family: var(--font-mono);
  font-size: 0.95rem;
  outline: none;
  resize: none;
  overflow-y: auto;
  line-height: 1.6;
  padding: 10px 12px;
  margin: 0;
  max-height: 200px;
  transition: all var(--transition-fast);
}

.editor-input:focus {
  background: transparent;
  border: none;
  box-shadow: none;
}

.editor-input::placeholder {
  color: var(--text-muted);
  opacity: 0.6;
}

.editor-input.input-disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.send-btn {
  background: var(--glass-bg);
  color: var(--text-muted);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
  width: 36px;
  height: 36px;
  display: flex;
  align-items: center;
  justify-content: center;
  margin-left: 12px;
  cursor: pointer;
  transition: all var(--transition-fast);
  flex-shrink: 0;
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}

.send-btn.active {
  background: var(--accent-blue);
  color: white;
  border-color: transparent;
}

.send-btn.active:hover {
  background: var(--accent-blue-hover);
}

.send-btn.active:active {
  background: var(--accent-blue-hover);
}

.send-btn.stop-state {
  background: var(--glass-bg-light);
  color: var(--text-soft);
  border-color: var(--border-color);
  box-shadow: none;
}

.send-btn.stop-state:hover {
  background: var(--border-color);
  color: var(--text-main);
}

.media-preview-container {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  padding: 12px 16px 0;
}

.media-preview-item {
  position: relative;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  border-radius: var(--radius-md);
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  max-width: 200px;
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}

.media-thumbnail {
  width: 40px;
  height: 40px;
  object-fit: cover;
  border-radius: var(--radius-sm);
  flex-shrink: 0;
}

.media-name {
  font-size: 0.75rem;
  color: var(--text-main);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.remove-media-btn {
  width: 16px;
  height: 16px;
  background: rgba(0, 0, 0, 0.15);
  color: var(--text-muted);
  border: none;
  border-radius: 50%;
  padding: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: all var(--transition-fast);
  flex-shrink: 0;
}

.remove-media-btn:hover {
  background: color-mix(in srgb, var(--accent-red) 90%, transparent);
  color: white;
}

.vision-warning {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: var(--glass-bg-light);
  border-top: 1px solid var(--border-color);
  color: var(--text-muted);
  font-size: 0.8rem;
}

.recall-edit-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: color-mix(in srgb, var(--accent-blue) 8%, transparent);
  border-bottom: 1px solid color-mix(in srgb, var(--accent-blue) 15%, transparent);
  color: var(--accent-blue);
  font-size: 0.8rem;
  animation: slideDown 0.2s ease-out;
}

@keyframes slideDown {
  from { opacity: 0; transform: translateY(-4px); }
  to { opacity: 1; transform: translateY(0); }
}

.recall-edit-btn {
  background: color-mix(in srgb, var(--accent-blue) 15%, transparent);
  color: var(--accent-blue);
  border: 1px solid color-mix(in srgb, var(--accent-blue) 30%, transparent);
  border-radius: var(--radius-md);
  padding: 4px 12px;
  font-size: 0.75rem;
  font-weight: 600;
  cursor: pointer;
  transition: all var(--transition-fast);
  white-space: nowrap;
}

.recall-edit-btn:hover {
  background: var(--accent-blue);
  color: white;
  border-color: transparent;
  transform: translateY(-1px);
}

.recall-dismiss-btn {
  background: none;
  border: none;
  color: var(--accent-blue);
  opacity: 0.6;
  cursor: pointer;
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  transition: all var(--transition-fast);
  display: flex;
  align-items: center;
  justify-content: center;
}

.recall-dismiss-btn:hover {
  opacity: 1;
  background: color-mix(in srgb, var(--accent-blue) 15%, transparent);
}

/* 提示条右侧关闭按钮：与 remove-media-btn 同构，跟随提示文字颜色 */
.warning-close-btn {
  background: none;
  border: none;
  padding: 2px;
  margin-left: auto;
  color: inherit;
  opacity: 0.55;
  cursor: pointer;
  border-radius: var(--radius-sm);
  transition: all var(--transition-fast);
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.warning-close-btn:hover {
  opacity: 1;
  background: color-mix(in srgb, var(--text-muted) 18%, transparent);
}

.resume-run-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: var(--glass-bg-light);
  border-bottom: 1px solid var(--border-color);
  color: var(--text-muted);
  font-size: 0.8rem;
  animation: slideDown 0.2s ease-out;
}

@keyframes spin {
  100% { transform: rotate(360deg); }
}

.spinner-icon {
  animation: spin 1s linear infinite;
  opacity: 0.7;
}

/* ── Token 统计栏 ── */
.token-bar {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  max-width: 85%;
  padding: 4px 16px 0;
  font-size: 0.65rem;
  color: var(--text-muted);
  user-select: none;
}

.token-bar-item {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  white-space: nowrap;
}

.token-bar-item svg {
  opacity: 0.5;
}

.token-bar-sep {
  opacity: 0.35;
}

.token-bar-total {
  font-weight: 650;
}

/* ── 上下文进度环（取代原来铺在概览栏上的一长串读数）── */

/* 环是浮层的定位上下文；`.token-bar-item svg { opacity: .5 }` 那条通用规则要盖掉，
   否则环会跟旁边的文字一样半透明，进度就看不出来了 */
.token-bar-ring {
  position: relative;
  cursor: pointer;
  padding: 0 3px;
  border-radius: 4px;
  transition: background-color 0.15s;
}

/* 与缓存读数同一套 hover 反馈：只给背景、不改字色 */
.token-bar-ring:hover {
  background: var(--glass-bg);
}

.token-bar-ring svg.context-ring {
  display: block;
  opacity: 1;
}

.ring-track {
  fill: none;
  stroke: var(--text-muted);
  stroke-opacity: 0.22;
  stroke-width: 2.5;
}

.ring-fill {
  fill: none;
  stroke: var(--text-muted);
  stroke-width: 2.5;
  stroke-linecap: round;
  transition: stroke-dasharray var(--transition-fast);
}

/* 单行读数浮层：宽度交给内容（读数本来就是一行），贴环正上方 */
.context-tip {
  position: absolute;
  bottom: calc(100% + 8px);
  left: 0;
  z-index: 120;
  padding: 5px 9px;
  white-space: nowrap;
  background: var(--surface-strong);
  backdrop-filter: blur(var(--glass-blur-heavy));
  -webkit-backdrop-filter: blur(var(--glass-blur-heavy));
  border: 1px solid color-mix(in srgb, var(--text-muted) 22%, transparent);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-lg);
  color: var(--text-muted);
  font-size: 0.65rem;
  user-select: none;
  /* 只淡入：位移动画会和定位打架（与 CacheHitTooltip 同一处理） */
  animation: contextTipIn var(--transition-fast);
}

/* 本组件 scoped，keyframes 必须自己定义：CacheHitTooltip 里的同名动画
   会被 scoped 重命名，跨组件引用拿不到。 */
@keyframes contextTipIn {
  from { opacity: 0; }
  to { opacity: 1; }
}

/* 透明桥接块，盖住浮层与环之间的 8px 间隙，指针往上挪不会中途触发 mouseleave */
.context-tip::after {
  content: "";
  position: absolute;
  left: 0;
  right: 0;
  top: 100%;
  height: 8px;
}

.token-bar-spacer {
  flex: 1;
}

/* 会话累计 + 缓存命中读数：整组可点击 → 打开上下文监控窗口；hover 出明细浮层 */
.token-bar-usage {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  position: relative; /* 明细浮层（CacheHitTooltip）的定位上下文 */
  cursor: pointer;
  border-radius: 4px;
  padding: 0 4px;
  transition: color 0.15s, background-color 0.15s;
}

/* hover 只给背景，不改字色。
   这里原本是 `color: var(--accent-blue)`——但蓝色在右侧上下文面板的语义里代表
   「预热」，拿它当命中态的 hover 色会让同一颜色在两处含义相反。 */
.token-bar-usage:hover {
  background: var(--glass-bg);
}

/* 状态色与右侧面板同口径：命中=绿、预热=蓝、未知=灰 */
.token-bar-cache.cache-hit {
  color: var(--accent-green);
}

.token-bar-cache.cache-warmup {
  color: var(--accent-blue);
  opacity: 0.8;
}

.token-bar-cache.cache-unknown {
  opacity: 0.5;
}

.token-bar-model {
  font-style: italic;
  font-size: 0.6rem;
  opacity: 0.6;
}
</style>
