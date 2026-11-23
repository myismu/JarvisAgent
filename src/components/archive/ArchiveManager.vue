<!--
# ArchiveManager.vue — 归档管理页

列出已归档（软删除）的会话，支持按项目筛选、按归档时间/最后活跃排序、多选与
批量「取消归档」「彻底删除」。列表按项目分区（无项目的会话归入「独立会话」分区），
每行是 46px 紧凑行，不再使用卡片形态。

## Key Exports
- `ArchiveManager`: 归档管理页面主组件（`App.vue` 在 `archive` 视图下挂载）

## Dependencies
- Internal: `../../types`、`../../stores/appView`、`../../stores/session`、
  `../../composables/useToast`、`../../utils/tokenDisplay`、`../common/ConfirmModal`
- External: `vue-i18n`、`@tauri-apps/api/core`、`@tauri-apps/api/event`

## 后端契约（唯一来源，改签名必须同步这里）
- `list_deleted_sessions()` → `SessionMeta[]`（含 `deletedAt` / `projectName`）
- `restore_sessions(ids)` → `BatchOpResult`
- `purge_sessions(ids)` → `BatchOpResult`
- `BatchOpResult = { succeededIds, failed: [{ id, reason }], restoredProjectIds }`

## Constraints
- 彻底删除不可恢复，**必须**经 `ConfirmModal` 确认，单条与批量共用同一条命令
- 批量操作不允许整批回滚：失败项逐条汇总展示，不静默吞掉
- 全选分两级：工具条「全选」作用于当前筛选结果的全部可见行，分区头全选只作用于本组
   （见 `selectAllVisible` 与 `toggleGroupSelect`）
-->
<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { SessionMeta } from '../../types';
import { useAppViewStore } from '../../stores/appView';
import { useSessionStore } from '../../stores/session';
import { showToast } from '../../composables/useToast';
import { formatTokenCount } from '../../utils/tokenDisplay';
import ConfirmModal from '../common/ConfirmModal.vue';

const { t } = useI18n();
const appView = useAppViewStore();
const sessionStore = useSessionStore();

// ───────────────────────────── 后端返回类型 ─────────────────────────────

/** 批量操作中的单个失败项（镜像后端 `BatchOpFailure`） */
interface BatchOpFailure {
  id: string;
  /** 面向用户的短句，直接展示，前端不二次加工 */
  reason: string;
}

/**
 * 批量操作结果（镜像后端 `BatchOpResult`）。
 *
 * 三个字段各自独立：`succeededIds` 是真正成功的 id，`failed` 是失败的 id 与原因，
 * `restoredProjectIds` 是恢复会话时**连带复活**的归档项目（副作用，不能从前者推导）。
 */
interface BatchOpResult {
  succeededIds: string[];
  failed: BatchOpFailure[];
  restoredProjectIds: string[];
}

/** 本页只有两类批量动作，命令名与提示语各自成对 */
type BatchOpKind = 'restore' | 'purge';

/** 排序模式：归档时间倒序 / 最后活跃倒序 */
type SortMode = 'archived' | 'lastActive';

// ───────────────────────────── 状态 ─────────────────────────────

const sessions = ref<SessionMeta[]>([]);
const loading = ref(true);
const error = ref<string | null>(null);
/** 有批量命令在跑：期间禁用所有会改数据的按钮，避免连续点击打出多批请求 */
const busy = ref(false);
/** 已勾选的会话 id（跨筛选保留：切走再切回不会丢选择） */
const selectedIds = ref<Set<string>>(new Set());
/** 待确认彻底删除的 id；非空 = 确认框打开 */
const pendingPurgeIds = ref<string[]>([]);
/** 确认框里的确认按钮 loading，与 `busy` 分开是为了让弹窗按钮自己显示「处理中」 */
const purgeLoading = ref(false);

/** 组件卸载后不再写状态：`listen` / `invoke` 的回调可能在卸载后才回来 */
let disposed = false;
let unlistenUpdated: UnlistenFn | null = null;

// ───────────────────────────── 筛选与排序 ─────────────────────────────

/**
 * 筛选项哨兵值。
 *
 * 不能用空串或「全部项目」这类可读文本当值：项目名完全可能就叫「全部项目」，
 * 用固定前缀的哨兵可以保证与真实项目名永不冲突。
 */
const FILTER_ALL = '__all__';
const FILTER_STANDALONE = '__standalone__';

const projectFilter = ref<string>(FILTER_ALL);
const sortMode = ref<SortMode>('archived');

/**
 * 筛选下拉的选项：按 `projectName` 去重。
 *
 * 项目归档后已从项目列表消失，所以这里只能从会话数据本身反推项目名；
 * 没有项目的会话归到「独立会话」这一虚拟分组（`hasStandalone` 决定是否显示）。
 */
const projectOptions = computed(() => {
  const names = new Set<string>();
  let hasStandalone = false;
  for (const session of sessions.value) {
    if (session.projectName) names.add(session.projectName);
    else hasStandalone = true;
  }
  return {
    names: [...names].sort((a, b) => a.localeCompare(b)),
    hasStandalone,
  };
});

/**
 * 当前可见行：先按项目筛选，再在内存里排序（本轮不做分页，量级可控）。
 *
 * 默认「归档时间倒序」——刚归档的排最上面，符合「最近操作优先」的直觉。
 * 「最后活跃」用 `updatedAt` 倒序，用来找出归档前仍在被使用、更可能想取回的会话。
 */
const visibleSessions = computed(() => {
  const filtered = sessions.value.filter((session) => {
    if (projectFilter.value === FILTER_ALL) return true;
    if (projectFilter.value === FILTER_STANDALONE) return !session.projectName;
    return session.projectName === projectFilter.value;
  });

  const sorted = [...filtered];
  sorted.sort((a, b) => {
    if (sortMode.value === 'lastActive') return b.updatedAt - a.updatedAt;
    // 归档时间缺失（旧库）时退回最后活跃时间，避免这类行被排到列表末尾看不见
    const aArchivedAt = a.deletedAt ?? a.updatedAt;
    const bArchivedAt = b.deletedAt ?? b.updatedAt;
    return bArchivedAt - aArchivedAt;
  });
  return sorted;
});

// ───────────────────────────── 运行中判定 ─────────────────────────────

/**
 * 该会话是否正在执行。
 *
 * 只采信现有 session store 的内存状态（`isSessionRunning`），不臆造后端字段：
 * 归档页里的会话通常不在内存视图里，此时一律视为「未执行」，不阻塞操作。
 */
function isRunning(session: SessionMeta): boolean {
  return sessionStore.isSessionRunning(session.id);
}

// ───────────────────────────── 分区 ─────────────────────────────

/**
 * 独立会话分区的固定 key。
 *
 * 与筛选哨兵同一套思路：真实项目完全可能就叫「独立会话」，用固定前缀的 key
 * 才能保证虚拟分区与项目分区永不撞车（分区折叠状态按 key 记录，撞了就会一起折叠）。
 */
const STANDALONE_GROUP_KEY = '__standalone__';

/**
 * 分区折叠状态：只存组件内存，不落用户偏好。
 *
 * 折叠是「这一眼想先看哪个项目」的临时视图状态，不是需要跨会话记住的配置；
 * 落盘反而会让用户下次进来发现列表全是收起的，还得逐个展开。
 */
const collapsedGroups = ref<Set<string>>(new Set());

const isCollapsed = (key: string): boolean => collapsedGroups.value.has(key);

/** 折叠/展开一个分区。整体替换 Set，保证依赖它的渲染被触发 */
function toggleGroupCollapsed(key: string) {
  const next = new Set(collapsedGroups.value);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  collapsedGroups.value = next;
}

/** 归档时间（旧库可能缺 `deletedAt`，退回最后活跃时间，与列表排序口径一致） */
function archivedStamp(session: SessionMeta): number {
  return session.deletedAt ?? session.updatedAt;
}

/**
 * 一个分区（= 一个项目，或独立会话）。
 *
 * `selectableIds` 只是「本分区内允许勾选的行」，全选/半选都按它算，
 * 因此分区头的复选框永远不会波及别的分区，也不会选中运行中的行。
 */
interface ArchiveGroup {
  key: string;
  name: string;
  sessions: SessionMeta[];
  selectableIds: string[];
  selectedCount: number;
  allSelected: boolean;
  indeterminate: boolean;
}

/**
 * 分区数据：先沿用 `visibleSessions` 的筛选与排序，再按 `projectName` 切段。
 *
 * - 组内顺序：直接继承筛排结果（切段不重排），所以「归档时间倒序 / 最后活跃倒序」
 *   在分区内依然成立；
 * - 组间顺序：按组内最近一次归档时间倒序——刚被动过的项目排最上；
 * - 独立会话分区固定最后：它不属于任何项目，混在项目之间会让「按项目浏览」失效。
 */
const groupedSessions = computed<ArchiveGroup[]>(() => {
  const buckets = new Map<string, { name: string; sessions: SessionMeta[]; latest: number }>();
  for (const session of visibleSessions.value) {
    const key = session.projectName ? `project:${session.projectName}` : STANDALONE_GROUP_KEY;
    let bucket = buckets.get(key);
    if (!bucket) {
      bucket = {
        name: session.projectName || t('archiveManager.standaloneSession'),
        sessions: [],
        latest: 0,
      };
      buckets.set(key, bucket);
    }
    bucket.sessions.push(session);
    bucket.latest = Math.max(bucket.latest, archivedStamp(session));
  }

  const projectGroups = [...buckets.entries()].filter(([key]) => key !== STANDALONE_GROUP_KEY);
  projectGroups.sort((a, b) => b[1].latest - a[1].latest);

  const ordered = [...projectGroups];
  const standalone = buckets.get(STANDALONE_GROUP_KEY);
  if (standalone) ordered.push([STANDALONE_GROUP_KEY, standalone]);

  return ordered.map(([key, bucket]) => {
    const selectableIds = bucket.sessions
      .filter((session) => !isRunning(session))
      .map((session) => session.id);
    const selectedCount = selectableIds.filter((id) => selectedIds.value.has(id)).length;
    return {
      key,
      name: bucket.name,
      sessions: bucket.sessions,
      selectableIds,
      selectedCount,
      allSelected: selectableIds.length > 0 && selectedCount === selectableIds.length,
      indeterminate: selectedCount > 0 && selectedCount < selectableIds.length,
    };
  });
});

/**
 * 工具条「全选」的作用域：当前筛选结果里所有可勾选的行（跨分区）。
 *
 * 刻意不用「全部归档数据」：被筛掉的项目用户看不见，一个能选中看不见数据的批量入口，
 * 配上「彻底删除」就是灾难。运行中的行同样排除。
 */
const visibleSelectableIds = computed(() =>
  visibleSessions.value.filter((session) => !isRunning(session)).map((session) => session.id),
);

/** 当前可见行是否已全部勾选（用于工具条全选按钮的禁用态） */
const allVisibleSelected = computed(
  () =>
    visibleSelectableIds.value.length > 0 &&
    visibleSelectableIds.value.every((id) => selectedIds.value.has(id)),
);

/** 全选当前可见行：只追加，不清掉跨筛选保留的已有选择 */
function selectAllVisible() {
  const next = new Set(selectedIds.value);
  for (const id of visibleSelectableIds.value) next.add(id);
  selectedIds.value = next;
}

/** 所有可见分区是否都已折叠（决定工具条按钮文案：全部折叠 / 全部展开） */
const allCollapsed = computed(
  () =>
    groupedSessions.value.length > 0 &&
    groupedSessions.value.every((group) => collapsedGroups.value.has(group.key)),
);

/** 折叠或展开全部分区：纯视图操作，不影响已选集合 */
function toggleAllGroups() {
  collapsedGroups.value = allCollapsed.value
    ? new Set()
    : new Set(groupedSessions.value.map((group) => group.key));
}

// ───────────────────────────── 选择状态 ─────────────────────────────

const selectedCount = computed(() => selectedIds.value.size);
const hasSelection = computed(() => selectedIds.value.size > 0);

/** 批量按钮是否可用：都没选就没得批量，跑着命令时也不给点 */
const batchActionsDisabled = computed(() => !hasSelection.value || busy.value);

const isSelected = (id: string): boolean => selectedIds.value.has(id);

/** 单行勾选：整体替换 Set，保证依赖 `selectedIds` 的计算属性被触发 */
function toggleOne(id: string, checked: boolean) {
  const next = new Set(selectedIds.value);
  if (checked) next.add(id);
  else next.delete(id);
  selectedIds.value = next;
}

/**
 * 分区头全选：作用域严格限制在「该分区内当前可见且可勾选」的行。
 *
 * 原来的全局表头全选已删除。彻底删除不可恢复，一个能选中「自己没看见的行」的
 * 全局入口风险大于便利；分区头全选至少让作用范围有明确的视觉边界（就是这一节）。
 * 运行中的行不在 `selectableIds` 里，所以也不会被顺手选中。
 */
function toggleGroupSelect(group: ArchiveGroup, checked: boolean) {
  const next = new Set(selectedIds.value);
  for (const id of group.selectableIds) {
    if (checked) next.add(id);
    else next.delete(id);
  }
  selectedIds.value = next;
}

function clearSelection() {
  selectedIds.value = new Set();
}

// ───────────────────────────── 展示格式化 ─────────────────────────────

/**
 * 归档时间 / 最后活跃的展示格式：`M/D HH:mm`，与侧栏会话行同一口径。
 *
 * 刻意不用 `toLocaleString`：归档页要横向对齐一排时间，逐行按语言格式化成
 * 「2026年9月20日 05:30:06」会把行撑爆；也不做相对时间（「3 分钟前」在列表不刷新
 * 时会停在一个过期的说法上）。
 */
function formatStamp(timestamp?: number | null): string {
  if (typeof timestamp !== 'number' || !Number.isFinite(timestamp) || timestamp <= 0) return '--';
  const date = new Date(timestamp);
  const month = date.getMonth() + 1;
  const day = date.getDate();
  const hours = String(date.getHours()).padStart(2, '0');
  const minutes = String(date.getMinutes()).padStart(2, '0');
  return `${month}/${day} ${hours}:${minutes}`;
}

/** 会话累计 token（输入 + 输出）。0 表示后端从未上报过用量，此时整段不渲染 */
function sessionTotalTokens(session: SessionMeta): number {
  return (session.totalInputTokens || 0) + (session.totalOutputTokens || 0);
}

/**
 * 行元信息：消息数 · tokens · 归档时间 · 最后活跃，用「·」拼成一行。
 *
 * token 从未上报过时整段省略（不留「0 tokens」这种噪音），剩余项自动聚拢，
 * 由模板一行省略号收尾——紧凑行只给一次扫读机会，不换行。
 */
function metaParts(session: SessionMeta): string {
  const parts: string[] = [t('archiveManager.messageCount', { count: session.messageCount })];
  const tokens = sessionTotalTokens(session);
  if (tokens > 0) {
    parts.push(t('archiveManager.tokenCount', { count: formatTokenCount(tokens) }));
  }
  parts.push(`${t('archiveManager.archivedAt')} ${formatStamp(session.deletedAt)}`);
  parts.push(`${t('archiveManager.lastActive')} ${formatStamp(session.updatedAt)}`);
  return parts.join(' · ');
}

function formatError(err: unknown): string {
  if (typeof err === 'string') return err;
  if (err instanceof Error) return err.message;
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
}

// ───────────────────────────── 数据加载 ─────────────────────────────

/**
 * 拉取归档会话列表。
 *
 * 刷新后必须把选中集合里**已不存在的 id 剔除**：另一个窗口恢复/彻底删除某条会话时
 * 后端会广播 `session-updated`，本页跟着重拉；若不清洗，本地会留着幽灵 id，
 * 用户点批量恢复就会得到一串「会话不存在」的失败。
 */
async function loadSessions() {
  try {
    const list = await invoke<SessionMeta[]>('list_deleted_sessions');
    if (disposed) return;
    sessions.value = list;
    const existingIds = new Set(list.map((session) => session.id));
    selectedIds.value = new Set([...selectedIds.value].filter((id) => existingIds.has(id)));
    error.value = null;
  } catch (err) {
    if (disposed) return;
    error.value = formatError(err);
    console.error('加载归档会话失败:', err);
  } finally {
    if (!disposed) loading.value = false;
  }
}

onMounted(async () => {
  // 先挂监听再拉数据：反序的话「挂载瞬间被别的窗口归档/恢复」这一拍会漏掉
  unlistenUpdated = await listen('session-updated', () => {
    // 归档、取消归档、彻底删除都会广播这个事件 —— 统一以服务端为准重拉一次
    void loadSessions();
  });
  if (disposed) {
    unlistenUpdated?.();
    unlistenUpdated = null;
    return;
  }
  await loadSessions();
});

onBeforeUnmount(() => {
  disposed = true;
  unlistenUpdated?.();
  unlistenUpdated = null;
});

// ───────────────────────────── 批量操作 ─────────────────────────────

/** 标题快照：批量操作后列表会被刷新，失败提示要用人看得懂的标题而不是 uuid */
function captureTitles(): Map<string, string> {
  const titles = new Map<string, string>();
  for (const session of sessions.value) titles.set(session.id, session.title);
  return titles;
}

/**
 * 汇报一次批量操作的结果。
 *
 * 三条信息可以同时出现（部分成功 + 复活了项目 + 部分失败），所以分开弹：
 * - 成功数：用户最关心「我这批点了到底动了几条」；
 * - `restoredProjectIds`：恢复会话的**副作用**——其所属项目已归档时会一并复活，
 *   但恢复出来的其实是一条独立会话，必须明确告知，否则用户会去项目列表里找；
 * - `failed`：后端不允许整批回滚，逐条列出「标题: 原因」，绝不静默吞掉。
 */
function reportBatchResult(kind: BatchOpKind, result: BatchOpResult, titles: Map<string, string>) {
  const succeeded = result.succeededIds.length;
  if (succeeded > 0) {
    showToast(
      t(kind === 'restore' ? 'archiveManager.restoreResult' : 'archiveManager.purgeResult', {
        count: succeeded,
      }),
    );
  }

  if (kind === 'restore' && result.restoredProjectIds.length > 0) {
    showToast(t('archiveManager.restoredProjectHint'));
  }

  if (result.failed.length > 0) {
    const detail = result.failed
      .map((failure) => `${titles.get(failure.id) ?? failure.id}: ${failure.reason}`)
      .join('；');
    showToast(
      t(kind === 'restore' ? 'archiveManager.restoreFailed' : 'archiveManager.purgeFailed', {
        count: result.failed.length,
        detail,
      }),
      'error',
    );
  }
}

/**
 * 执行批量命令并统一收尾（清空已选 → 重拉列表 → 汇报结果）。
 *
 * 恢复与彻底删除共用这条通道，行内单条只是 `ids` 长度为 1 的特例 ——
 * 这样「单条」与「批量」永远是同一套错误处理与提示，不会走两条各有漏洞的路径。
 *
 * @returns 命令是否整体调用成功（用于决定确认框要不要关掉）
 */
async function runBatch(kind: BatchOpKind, ids: string[]): Promise<boolean> {
  if (ids.length === 0 || busy.value) return false;

  const titles = captureTitles();
  busy.value = true;
  try {
    const result = await invoke<BatchOpResult>(
      kind === 'restore' ? 'restore_sessions' : 'purge_sessions',
      { ids },
    );
    clearSelection();
    await loadSessions();
    reportBatchResult(kind, result, titles);
    return true;
  } catch (err) {
    console.error(kind === 'restore' ? '批量取消归档失败:' : '批量彻底删除失败:', err);
    showToast(formatError(err), 'error');
    return false;
  } finally {
    busy.value = false;
  }
}

/** 行内「取消归档」：ids 长度为 1 的批量 */
function restoreOne(session: SessionMeta) {
  void runBatch('restore', [session.id]);
}

/** 批量「取消归档」 */
function restoreSelected() {
  void runBatch('restore', [...selectedIds.value]);
}

/**
 * 打开彻底删除确认框。
 *
 * 彻底删除会清掉消息、快照与回收站副本，**不可恢复**，因此行内按钮与批量按钮
 * 都不直接调命令，一律先落到这个待确认集合上，由 `ConfirmModal` 收口。
 */
function requestPurge(ids: string[]) {
  if (ids.length === 0 || busy.value) return;
  pendingPurgeIds.value = [...ids];
}

function requestPurgeSelected() {
  requestPurge([...selectedIds.value]);
}

function cancelPurge() {
  // 命令执行中不允许用遮罩/取消键关掉弹窗：请求已经发出去了，关窗只会让用户以为没删
  if (purgeLoading.value) return;
  pendingPurgeIds.value = [];
}

const purgeOpen = computed(() => pendingPurgeIds.value.length > 0);

/** 确认框里要展示的数量。用 `pendingPurgeIds` 而不是 `selectedIds`：单条删除时后者可能是 0 */
const purgeCount = computed(() => pendingPurgeIds.value.length);

/** 确认后真正执行；失败时保留弹窗，让用户能重试或取消，而不是静默关掉了事 */
async function confirmPurge() {
  const ids = pendingPurgeIds.value;
  if (ids.length === 0 || purgeLoading.value) return;
  purgeLoading.value = true;
  try {
    const ok = await runBatch('purge', ids);
    if (ok) pendingPurgeIds.value = [];
  } finally {
    purgeLoading.value = false;
  }
}
</script>

<template>
  <div class="archive-manager">
    <!-- 头部三段式：左返回 / 中标题左对齐 / 右归档总数。高度与标题栏同级（44px） -->
    <header class="archive-header">
      <div class="header-left">
        <button type="button" class="back-btn" @click="appView.showChat()">
          <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none" aria-hidden="true">
            <polyline points="15 18 9 12 15 6"></polyline>
          </svg>
          {{ t('archiveManager.backToChat') }}
        </button>
      </div>
      <div class="header-titles">
        <h2 class="archive-title">{{ t('archiveManager.title') }}</h2>
        <p class="archive-subtitle">{{ t('archiveManager.subtitle') }}</p>
      </div>
      <div class="header-right">
        <!-- 复用侧栏「归档」文案拼出「N · 归档」：本页不新增语言键，计数取全量已归档，不随筛选变化 -->
        <span v-if="!loading && !error">{{ sessions.length }} · {{ t('sidebar.recentlyDeleted') }}</span>
      </div>
    </header>

    <!-- 工具条 40px：筛选 / 排序常驻左侧，批量动作贴右。选中数为 0 时批量按钮禁用 -->
    <div class="archive-toolbar">
      <div class="select-wrap">
        <select
          v-model="projectFilter"
          class="toolbar-select"
          :aria-label="t('archiveManager.filterByProject')"
          :title="t('archiveManager.filterByProject')"
        >
          <option :value="FILTER_ALL">{{ t('archiveManager.allProjects') }}</option>
          <option v-if="projectOptions.hasStandalone" :value="FILTER_STANDALONE">
            {{ t('archiveManager.standaloneSession') }}
          </option>
          <option v-for="name in projectOptions.names" :key="name" :value="name">{{ name }}</option>
        </select>
        <!-- 自绘下三角：原生箭头在暗色下不跟随设计系统，这里用 currentColor 的 SVG 覆盖 -->
        <svg
          class="select-caret"
          viewBox="0 0 24 24"
          width="10"
          height="10"
          stroke="currentColor"
          stroke-width="2"
          fill="none"
          aria-hidden="true"
        >
          <polyline points="6 9 12 15 18 9"></polyline>
        </svg>
      </div>

      <div class="select-wrap">
        <select
          v-model="sortMode"
          class="toolbar-select"
          :aria-label="t('archiveManager.sortBy')"
          :title="t('archiveManager.sortBy')"
        >
          <option value="archived">{{ t('archiveManager.archivedAt') }}</option>
          <option value="lastActive">{{ t('archiveManager.lastActive') }}</option>
        </select>
        <svg
          class="select-caret"
          viewBox="0 0 24 24"
          width="10"
          height="10"
          stroke="currentColor"
          stroke-width="2"
          fill="none"
          aria-hidden="true"
        >
          <polyline points="6 9 12 15 18 9"></polyline>
        </svg>
      </div>

      <!-- 全部折叠 / 展开：单个按钮按当前状态切换文案，避免两个常驻按钮争位置 -->
      <button
        type="button"
        class="toolbar-text-btn groups-toggle"
        :disabled="groupedSessions.length === 0"
        @click="toggleAllGroups"
      >
        {{ allCollapsed ? t('archiveManager.expandAllGroups') : t('archiveManager.collapseAllGroups') }}
      </button>

      <div class="toolbar-actions">
        <!-- 全选 / 取消选择成对出现：前者作用域是当前筛选可见行，后者清空全部选择 -->
        <button
          type="button"
          class="toolbar-text-btn"
          :disabled="visibleSelectableIds.length === 0 || allVisibleSelected"
          @click="selectAllVisible"
        >
          {{ t('archiveManager.selectAll') }}
        </button>
        <button
          type="button"
          class="toolbar-text-btn"
          :disabled="!hasSelection"
          @click="clearSelection"
        >
          {{ t('archiveManager.clearSelection') }}
        </button>
        <span class="selected-count">{{ t('archiveManager.selectedCount', { count: selectedCount }) }}</span>
        <button type="button" class="batch-btn" :disabled="batchActionsDisabled" @click="restoreSelected">
          {{ t('archiveManager.batchRestore') }}
        </button>
        <button type="button" class="batch-btn danger" :disabled="batchActionsDisabled" @click="requestPurgeSelected">
          {{ t('archiveManager.batchPurge') }}
        </button>
      </div>
    </div>

    <!-- 内容区是唯一滚动容器：分区头 sticky 依赖它作为滚动上下文 -->
    <div class="archive-manager-content">
      <div v-if="loading" class="state-block">
        <div class="loading-spinner"></div>
        <span>{{ t('archiveManager.loading') }}</span>
      </div>

      <div v-else-if="error" class="state-block">
        <svg viewBox="0 0 24 24" width="48" height="48" stroke="currentColor" stroke-width="1.5" fill="none" opacity="0.3" aria-hidden="true">
          <circle cx="12" cy="12" r="9"></circle>
          <path d="M12 8v5"></path>
          <path d="M12 16.5v.01"></path>
        </svg>
        <p>{{ t('archiveManager.loadError', { error }) }}</p>
      </div>

      <div v-else-if="groupedSessions.length === 0" class="state-block">
        <svg viewBox="0 0 24 24" width="48" height="48" stroke="currentColor" stroke-width="1.5" fill="none" opacity="0.3" aria-hidden="true">
          <path d="M3 6h18"></path>
          <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path>
          <path d="M10 11v6"></path>
          <path d="M14 11v6"></path>
        </svg>
        <p>{{ t('archiveManager.empty') }}</p>
      </div>

      <template v-else>
        <section v-for="group in groupedSessions" :key="group.key" class="archive-group">
          <!-- 分区头：折叠开关（chevron + 名称 + 计数）+ 本分区全选，粘在内容区顶部 -->
          <div class="group-head">
            <button
              type="button"
              class="group-toggle"
              :aria-expanded="!isCollapsed(group.key)"
              @click="toggleGroupCollapsed(group.key)"
            >
              <svg
                class="group-chevron"
                :class="{ collapsed: isCollapsed(group.key) }"
                viewBox="0 0 24 24"
                width="10"
                height="10"
                stroke="currentColor"
                stroke-width="2.4"
                fill="none"
                aria-hidden="true"
              >
                <polyline points="6 9 12 15 18 9"></polyline>
              </svg>
              <span class="group-name">{{ group.name }}</span>
              <span class="group-count">{{ t('archiveManager.groupSessionCount', { count: group.sessions.length }) }}</span>
            </button>

            <label class="group-select">
              <input
                type="checkbox"
                class="row-checkbox"
                :checked="group.allSelected"
                :indeterminate="group.indeterminate"
                :disabled="group.selectableIds.length === 0"
                :aria-label="t('archiveManager.groupSelectAll')"
                :title="t('archiveManager.groupSelectAll')"
                @change="toggleGroupSelect(group, ($event.target as HTMLInputElement).checked)"
              />
            </label>
          </div>

          <ul v-if="!isCollapsed(group.key)" class="session-rows">
            <li
              v-for="session in group.sessions"
              :key="session.id"
              class="session-row"
              :class="{ selected: isSelected(session.id), running: isRunning(session) }"
            >
              <input
                type="checkbox"
                class="row-checkbox"
                :checked="isSelected(session.id)"
                :disabled="isRunning(session)"
                :title="isRunning(session) ? t('archiveManager.runningCannotRestore') : ''"
                :aria-label="session.title"
                @change="toggleOne(session.id, ($event.target as HTMLInputElement).checked)"
              />

              <!-- 项目归属已由分区表达，行内只留标题 + 一行元信息 -->
              <div class="row-main">
                <div class="row-title">{{ session.title }}</div>
                <div class="row-meta" :title="metaParts(session)">{{ metaParts(session) }}</div>
              </div>

              <div class="row-actions">
                <button
                  type="button"
                  class="row-action restore"
                  :disabled="busy || isRunning(session)"
                  :title="isRunning(session) ? t('archiveManager.runningCannotRestore') : t('archiveManager.restore')"
                  @click="restoreOne(session)"
                >
                  {{ t('archiveManager.restore') }}
                </button>
                <!-- 彻底删除是不可恢复动作：默认隐藏，行 hover / 键盘聚焦时才出现，避免误点 -->
                <button
                  type="button"
                  class="row-action purge"
                  :disabled="busy || isRunning(session)"
                  :title="isRunning(session) ? t('archiveManager.runningCannotRestore') : t('archiveManager.purge')"
                  @click="requestPurge([session.id])"
                >
                  {{ t('archiveManager.purge') }}
                </button>
              </div>
            </li>
          </ul>
        </section>
      </template>
    </div>

    <!-- 彻底删除的唯一入口：单条与批量共用，不可绕过 -->
    <ConfirmModal
      :open="purgeOpen"
      :title="t('archiveManager.batchPurge')"
      :message="t('archiveManager.purgeConfirm', { count: purgeCount })"
      :confirm-text="t('archiveManager.purge')"
      confirm-kind="danger"
      :loading="purgeLoading"
      @confirm="confirmPurge"
      @cancel="cancelPurge"
    />
  </div>
</template>

<style scoped>
.archive-manager {
  /* 内容区与工具条占面板宽度的 78%（两侧各留 11%），比例恒定：窗口拉宽时它们同比例变宽，
     不会像写死上限那样一旦超过阈值就停住不动。
     头部不引用本值——标题栏贴左固定内边距，不跟内容缩进。 */
  --archive-content-gutter: 11%;
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg-panel);
  overflow: hidden;
}

/* ── 头部：44px 三段式，无阴影无渐变 ── */
.archive-header {
  display: flex;
  align-items: center;
  gap: 16px;
  height: var(--header-height);
  /* 标题栏贴左固定 20px，与内容区的百分比缩进彻底脱钩：
     窗口拉宽时返回按钮与标题留在原位，只有下方列表按比例收放。 */
  padding: 0 20px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border-color);
  flex-shrink: 0;
}

.header-left {
  display: flex;
  align-items: center;
  flex-shrink: 0;
}

/* 标题与副标题左对齐，紧贴返回按钮右侧 */
.header-titles {
  display: flex;
  flex-direction: column;
  gap: 1px;
  flex: 1;
  min-width: 0;
  text-align: left;
}

.archive-title {
  margin: 0;
  font-size: 0.86rem;
  font-weight: 600;
  color: var(--text-main);
  line-height: 1.25;
}

.archive-subtitle {
  margin: 0;
  font-size: 0.7rem;
  color: var(--text-faint);
  line-height: 1.25;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.header-right {
  margin-left: auto;
  flex-shrink: 0;
  font-size: 0.72rem;
  color: var(--text-faint);
  font-variant-numeric: tabular-nums;
}

.back-btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 26px;
  padding: 0 10px;
  background: transparent;
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  color: var(--text-soft);
  font-size: 0.76rem;
  cursor: pointer;
  transition: background var(--transition-fast), color var(--transition-fast),
    border-color var(--transition-fast);
  white-space: nowrap;
}

.back-btn:hover {
  background: var(--surface-1);
  color: var(--text-main);
  border-color: var(--border-strong);
}

/* ── 工具条：40px，仅一排控件，不换行 ── */
.archive-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 40px;
  /* 工具条是列表的表头，跟随内容区同一边距，保证筛选控件与下方行左边缘对齐 */
  padding: 0 var(--archive-content-gutter);
  background: var(--bg-panel);
  border-bottom: 1px solid var(--glass-border-subtle);
  flex-shrink: 0;
}

.select-wrap {
  position: relative;
  display: inline-flex;
  align-items: center;
}

/* 原生箭头在暗色下不跟随设计系统：关掉外观，下三角由后面的 SVG 画 */
.toolbar-select {
  appearance: none;
  -webkit-appearance: none;
  height: 26px;
  max-width: 200px;
  padding: 0 24px 0 8px;
  background: transparent;
  color: var(--text-soft);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  font-size: 0.76rem;
  cursor: pointer;
  outline: none;
  transition: border-color var(--transition-fast);
}

.toolbar-select:hover {
  border-color: var(--border-strong);
}

.toolbar-select:focus {
  border-color: var(--accent-blue);
}

.select-caret {
  position: absolute;
  right: 8px;
  top: 50%;
  transform: translateY(-50%);
  color: var(--text-faint);
  pointer-events: none;
}

.toolbar-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-left: auto;
}

/* 工具条文字按钮：无边框、低存在感，用于全选/取消选择与全部折叠这类辅助动作 */
.toolbar-text-btn {
  flex-shrink: 0;
  padding: 0 4px;
  background: transparent;
  border: none;
  color: var(--text-soft);
  font-size: 0.74rem;
  cursor: pointer;
  white-space: nowrap;
}

.toolbar-text-btn:hover:not(:disabled) {
  color: var(--text-main);
}

.toolbar-text-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

/* 折叠全部与筛选相邻，用一点空隙与右侧的选择/批量动作分开 */
.toolbar-text-btn.groups-toggle {
  margin-left: 4px;
}

.selected-count {
  font-size: 0.74rem;
  color: var(--text-faint);
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}

.batch-btn {
  display: inline-flex;
  align-items: center;
  height: 26px;
  padding: 0 10px;
  background: transparent;
  color: var(--text-soft);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  font-size: 0.76rem;
  cursor: pointer;
  transition: background var(--transition-fast), color var(--transition-fast),
    border-color var(--transition-fast);
  white-space: nowrap;
}

.batch-btn:hover:not(:disabled) {
  background: var(--surface-1);
  color: var(--text-main);
  border-color: var(--border-strong);
}

/* 彻底删除：默认中性，只在 hover 时露出红色语义，避免整条工具条被红色压住 */
.batch-btn.danger {
  color: var(--text-soft);
  border-color: var(--border-color);
}

.batch-btn.danger:hover:not(:disabled) {
  background: transparent;
  color: var(--accent-red);
  border-color: var(--border-danger);
}

.batch-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

/* ── 内容区：唯一滚动容器，内容与头部同宽居中 ── */
.archive-manager-content {
  flex: 1;
  overflow-y: auto;
  padding: 0 var(--archive-content-gutter) 24px;
}

.state-block {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  padding: 48px;
  color: var(--text-faint);
  font-size: 0.8rem;
  text-align: center;
}

.state-block p {
  margin: 0;
  max-width: 520px;
  font-size: 0.8rem;
  line-height: 1.7;
  color: var(--text-faint);
}

.loading-spinner {
  width: 24px;
  height: 24px;
  border: 2px solid var(--border-color);
  border-top-color: var(--accent-blue);
  border-radius: 50%;
  animation: spin 1s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

/* ── 分区头：sticky，32px，底色必须实心否则行会透出来 ── */
.group-head {
  position: sticky;
  top: 0;
  z-index: 1;
  display: flex;
  align-items: center;
  gap: 8px;
  height: 32px;
  padding: 0 16px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--glass-border-subtle);
}

.group-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  padding: 0;
  background: transparent;
  border: none;
  color: var(--text-soft);
  font-size: 0.78rem;
  font-weight: 600;
  cursor: pointer;
  transition: color var(--transition-fast);
}

.group-toggle:hover {
  color: var(--text-main);
}

.group-chevron {
  flex-shrink: 0;
  transition: transform var(--transition-fast);
}

/* 折叠时 chevron 从「下」转到「右」，与侧栏项目行的展开语义一致 */
.group-chevron.collapsed {
  transform: rotate(-90deg);
}

.group-name {
  max-width: 420px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.group-count {
  flex-shrink: 0;
  font-size: 0.72rem;
  font-weight: 400;
  color: var(--text-faint);
  font-variant-numeric: tabular-nums;
}

.group-select {
  display: inline-flex;
  align-items: center;
  margin-left: auto;
  cursor: pointer;
}

/* ── 数据行：单行 46px 网格 ── */
.session-rows {
  list-style: none;
  margin: 0;
  padding: 0;
}

.session-row {
  display: grid;
  grid-template-columns: 18px minmax(0, 1fr) auto;
  gap: 12px;
  align-items: center;
  height: 46px;
  padding: 0 12px 0 16px;
  border-bottom: 1px solid var(--glass-border-subtle);
  transition: background var(--transition-fast), box-shadow var(--transition-fast);
}

.session-row:hover {
  background: var(--surface-1);
}

/* 选中态只靠表面层级（比 hover 更深一档），不再加蓝色装饰条：
   本项目里蓝色只表示链接与焦点，装饰性竖条与这条约定冲突；
   复选框本身已是最明确的选中信号。 */
.session-row.selected {
  background: var(--surface-2);
}

/* 运行中的行整体压暗：勾选与两个动作按钮同时被禁用，只保留可读性 */
.session-row.running {
  opacity: 0.55;
}

.row-checkbox {
  width: 14px;
  height: 14px;
  margin: 0;
  accent-color: var(--accent-blue);
  cursor: pointer;
}

.row-checkbox:disabled {
  cursor: not-allowed;
}

.row-main {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.row-title {
  color: var(--text-main);
  font-size: 0.82rem;
  line-height: 1.3;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.row-meta {
  color: var(--text-faint);
  font-size: 0.72rem;
  line-height: 1.3;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}

.row-actions {
  display: inline-flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}

.row-action {
  padding: 0;
  background: transparent;
  border: none;
  font-size: 0.74rem;
  cursor: pointer;
  white-space: nowrap;
  transition: color var(--transition-fast), opacity var(--transition-fast);
}

.row-action.restore {
  color: var(--text-soft);
}

.row-action.restore:hover:not(:disabled) {
  color: var(--accent-blue);
}

.row-action.restore:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

/* 彻底删除默认隐藏：不可恢复的动作不该一直摆在行尾等误点 */
.row-action.purge {
  color: var(--text-faint);
  opacity: 0;
}

.session-row:hover .row-action.purge,
.session-row:focus-within .row-action.purge {
  opacity: 1;
}

.session-row:hover .row-action.purge:disabled,
.session-row:focus-within .row-action.purge:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

.row-action.purge:hover:not(:disabled) {
  color: var(--accent-red);
}

@media (max-width: 768px) {
  .archive-header,
  .archive-toolbar {
    padding: 0 12px;
  }

  .archive-subtitle {
    display: none;
  }

  .group-name {
    max-width: 180px;
  }

  .toolbar-select {
    max-width: 140px;
  }
}
</style>
