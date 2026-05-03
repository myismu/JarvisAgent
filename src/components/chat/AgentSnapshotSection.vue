<!--
# AgentSnapshotSection.vue — 监控区快照检查点紧凑视图

在 Agent 监控侧栏中展示当前会话的 snapshot/checkpoint 分支、当前节点和最近变更摘要。

## Key Exports
- `AgentSnapshotSection`: 快照检查点紧凑展示组件

## Dependencies
- Internal: `@/stores/session`, `@/services/snapshotService`
-->
<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { useSessionStore } from '../../stores/session';
import { SnapshotTimelineService } from '../../services/snapshotService';
import type { SnapshotNode, SnapshotSummary, SnapshotTreeView } from '../../types';

const session = useSessionStore();

const loading = ref(false);
const errorMessage = ref('');
const tree = ref<SnapshotTreeView | null>(null);
const selectedSnapshotId = ref<string | null>(null);
const selectedSummary = ref<SnapshotSummary | null>(null);
let service: SnapshotTimelineService | null = null;

const activeBranch = computed(() => {
  if (!tree.value) return null;
  return tree.value.branches.find((branch) => branch.name === tree.value?.currentBranch) ?? tree.value.branches[0] ?? null;
});

function flattenNodes(node: SnapshotNode | null | undefined, result: SnapshotNode[] = []) {
  if (!node) return result;
  result.push(node);
  for (const child of node.children || []) {
    flattenNodes(child, result);
  }
  return result;
}

const recentNodes = computed(() => {
  const nodes = flattenNodes(activeBranch.value?.root).sort((a, b) => b.timestamp - a.timestamp);
  return nodes.slice(0, 8);
});

function formatSnapshotTime(timestamp: number) {
  const date = new Date(timestamp);
  return `${date.getMonth() + 1}/${date.getDate()} ${date.getHours().toString().padStart(2, '0')}:${date.getMinutes().toString().padStart(2, '0')}`;
}

function snapshotTitle(node: SnapshotNode) {
  return node.message?.trim() || node.id;
}

async function loadSnapshotSection(force = false) {
  const sessionId = session.activeSessionId;
  if (!sessionId) {
    tree.value = null;
    selectedSnapshotId.value = null;
    selectedSummary.value = null;
    return;
  }
  try {
    loading.value = true;
    errorMessage.value = '';
    if (!service || force) {
      service = new SnapshotTimelineService(sessionId);
    }
    if (force) service.clearCache();
    tree.value = await service.loadTree();
    selectedSnapshotId.value = tree.value.currentSnapshotId || recentNodes.value[0]?.id || null;
    if (selectedSnapshotId.value) {
      await selectSnapshot(selectedSnapshotId.value);
    }
  } catch (err) {
    errorMessage.value = typeof err === 'string' ? err : err instanceof Error ? err.message : String(err);
  } finally {
    loading.value = false;
  }
}

async function selectSnapshot(id: string) {
  if (!service) return;
  selectedSnapshotId.value = id;
  try {
    const summaries = await service.loadSummaries([id]);
    selectedSummary.value = summaries[0] ?? null;
  } catch (err) {
    errorMessage.value = typeof err === 'string' ? err : err instanceof Error ? err.message : String(err);
  }
}

watch(
  () => session.activeSessionId,
  () => {
    service = null;
    loadSnapshotSection(true);
  },
  { immediate: true },
);
</script>

<template>
  <div class="snapshot-compact">
    <div class="snapshot-head">
      <div>
        <strong>{{ activeBranch?.name || '无分支' }}</strong>
        <span v-if="tree?.currentSnapshotId">当前 {{ tree.currentSnapshotId.slice(0, 8) }}</span>
      </div>
      <button type="button" :disabled="loading" @click="loadSnapshotSection(true)">
        {{ loading ? '刷新中' : '刷新' }}
      </button>
    </div>

    <div v-if="errorMessage" class="snapshot-error">{{ errorMessage }}</div>
    <div v-else-if="!session.activeSessionId" class="snapshot-empty">暂无活跃会话</div>
    <div v-else-if="recentNodes.length === 0 && !loading" class="snapshot-empty">暂无快照检查点</div>

    <div v-if="recentNodes.length > 0" class="snapshot-node-list">
      <button
        v-for="node in recentNodes"
        :key="node.id"
        type="button"
        class="snapshot-node"
        :class="{ active: node.id === selectedSnapshotId, checkpoint: node.isCheckpoint }"
        @click="selectSnapshot(node.id)"
      >
        <span class="snapshot-node-dot"></span>
        <span class="snapshot-node-main">
          <strong>{{ snapshotTitle(node) }}</strong>
          <small>{{ formatSnapshotTime(node.timestamp) }} · {{ node.id.slice(0, 8) }}</small>
        </span>
        <span v-if="node.isCheckpoint" class="snapshot-badge">CP</span>
      </button>
    </div>

    <div v-if="selectedSummary" class="snapshot-summary">
      <div class="snapshot-summary-head">
        <span>变更摘要</span>
        <span>{{ selectedSummary.patchCount }} 项</span>
      </div>
      <div v-if="selectedSummary.patchSummary.length > 0" class="snapshot-patches">
        <div v-for="patch in selectedSummary.patchSummary.slice(0, 6)" :key="`${patch.operation}-${patch.path}`" class="snapshot-patch">
          <span>{{ patch.operation }}</span>
          <strong>{{ patch.path }}</strong>
          <small>+{{ patch.linesAdded }} -{{ patch.linesRemoved }}</small>
        </div>
      </div>
      <div v-else class="snapshot-empty">该节点没有文件变更摘要</div>
    </div>
  </div>
</template>

<style scoped>
.snapshot-compact {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.snapshot-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 7px;
  border: 1px solid var(--border-color);
  border-radius: 7px;
  background: var(--glass-bg);
}

.snapshot-head div {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.snapshot-head strong {
  color: var(--text-main);
  font-size: 0.68rem;
}

.snapshot-head span,
.snapshot-empty,
.snapshot-error {
  color: var(--text-muted);
  font-size: 0.62rem;
}

.snapshot-error {
  color: var(--accent-red);
}

.snapshot-head button {
  height: 22px;
  padding: 0 7px;
  color: var(--text-muted);
  border: 1px solid var(--border-color);
  border-radius: 5px;
  background: transparent;
  font-size: 0.6rem;
  cursor: pointer;
}

.snapshot-head button:hover:not(:disabled) {
  color: var(--accent-blue);
  border-color: rgba(59, 130, 246, 0.45);
  background: rgba(59, 130, 246, 0.08);
}

.snapshot-node-list {
  display: flex;
  flex-direction: column;
  gap: 5px;
}

.snapshot-node {
  display: flex;
  align-items: center;
  gap: 7px;
  width: 100%;
  padding: 7px;
  border: 1px solid var(--border-color);
  border-radius: 7px;
  background: var(--glass-bg);
  text-align: left;
  cursor: pointer;
}

.snapshot-node:hover,
.snapshot-node.active {
  border-color: rgba(139, 92, 246, 0.45);
  background: var(--glass-bg-light);
}

.snapshot-node.checkpoint .snapshot-node-dot {
  background: var(--accent-yellow);
}

.snapshot-node-dot {
  width: 7px;
  height: 7px;
  border-radius: 999px;
  background: var(--accent-blue);
  flex-shrink: 0;
}

.snapshot-node-main {
  min-width: 0;
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.snapshot-node-main strong {
  color: var(--text-main);
  font-size: 0.64rem;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.snapshot-node-main small,
.snapshot-badge {
  color: var(--text-muted);
  font-size: 0.58rem;
}

.snapshot-badge {
  padding: 1px 4px;
  border: 1px solid color-mix(in srgb, var(--accent-yellow) 45%, transparent);
  border-radius: 4px;
  color: var(--accent-yellow);
}

.snapshot-summary {
  padding: 7px;
  border: 1px solid var(--border-color);
  border-radius: 7px;
  background: var(--glass-bg);
}

.snapshot-summary-head {
  display: flex;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 6px;
  color: var(--text-muted);
  font-size: 0.6rem;
  font-weight: 700;
}

.snapshot-patches {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.snapshot-patch {
  display: grid;
  grid-template-columns: 44px minmax(0, 1fr) 46px;
  gap: 5px;
  align-items: baseline;
  font-size: 0.6rem;
}

.snapshot-patch span,
.snapshot-patch small {
  color: var(--text-muted);
  white-space: nowrap;
}

.snapshot-patch strong {
  min-width: 0;
  color: var(--text-main);
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
