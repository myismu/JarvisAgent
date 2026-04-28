<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, computed } from "vue";
import { listen } from "@tauri-apps/api/event";
import {
  SnapshotTimelineService,
  collectAllIdsFromTree,
} from "../../services/snapshotService";
import type {
  SnapshotTreeView,
  SnapshotSummary,
  Snapshot,
  SnapshotNode,
} from "../../types";
import DiffViewer from "./DiffViewer.vue";
import { formatRelativeTime, getFileOpIcon } from "../../utils/timeline";

const props = defineProps<{
  sessionId: string | null;
  workspacePath?: string;
}>();

const tree = ref<SnapshotTreeView | null>(null);
const summaries = ref<Map<string, SnapshotSummary>>(new Map());
const selectedSnapshot = ref<Snapshot | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
const expandedNodes = ref<Set<string>>(new Set());
const rollbackConfirm = ref<{ snapshotId: string; message: string } | null>(
  null
);

let service: SnapshotTimelineService | null = null;

const currentSnapshotId = computed(
  () => tree.value?.currentSnapshotId || ""
);

const formatTime = formatRelativeTime;

const loadTree = async () => {
  if (!props.sessionId || !service) return;

  loading.value = true;
  error.value = null;

  try {
    tree.value = await service.loadTree();

    const allIds = collectAllIdsFromTree(tree.value);
    if (allIds.length > 0) {
      const summaryList = await service.loadSummaries(allIds);
      summaryList.forEach((s) => summaries.value.set(s.id, s));
    }
  } catch (err) {
    console.error("加载快照树失败:", err);
    error.value = String(err);
  } finally {
    loading.value = false;
  }
};

const loadSnapshotDetail = async (id: string) => {
  if (!service) return;

  try {
    const snapshot = await service.loadDetail(id);
    if (snapshot) {
      selectedSnapshot.value = snapshot;
    }
  } catch (err) {
    console.error("加载快照详情失败:", err);
  }
};

const toggleNode = (nodeId: string) => {
  if (expandedNodes.value.has(nodeId)) {
    expandedNodes.value.delete(nodeId);
  } else {
    expandedNodes.value.add(nodeId);
  }
};

const handleRollback = async (snapshotId: string) => {
  if (!service) return;

  let targetDir = props.workspacePath;
  if (!targetDir) {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const ws = await invoke<string | null>("get_workspace_dir", { sessionId: props.sessionId });
      if (ws) {
        targetDir = ws;
      }
    } catch {}
  }
  if (!targetDir) {
    error.value = "无法确定工作区路径，请先设置工作目录";
    rollbackConfirm.value = null;
    return;
  }

  try {
    await service.rollback(snapshotId, targetDir);
    rollbackConfirm.value = null;
    selectedSnapshot.value = null;
    await loadTree();
  } catch (err) {
    console.error("回滚失败:", err);
    error.value = `回滚失败: ${err}`;
  }
};

const switchBranch = async (branchName: string) => {
  if (!service) return;

  try {
    await service.switchBranch(branchName);
    await loadTree();
  } catch (err) {
    console.error("切换分支失败:", err);
    error.value = `切换分支失败: ${err}`;
  }
};

const createNewBranch = async (fromSnapshotId: string) => {
  if (!service) return;

  const branchName = prompt("请输入新分支名称:");
  if (!branchName) return;

  try {
    await service.createBranch(branchName, fromSnapshotId);
    await loadTree();
  } catch (err) {
    console.error("创建分支失败:", err);
    error.value = `创建分支失败: ${err}`;
  }
};

const renderTreeNode = (
  node: SnapshotNode,
  depth: number = 0
): { node: SnapshotNode; depth: number }[] => {
  const result: { node: SnapshotNode; depth: number }[] = [];
  if (node.id) {
    result.push({ node, depth });
  }
  if (expandedNodes.value.has(node.id) && node.children.length > 0) {
    for (const child of node.children) {
      result.push(...renderTreeNode(child, depth + 1));
    }
  }
  return result;
};

watch(
  () => props.sessionId,
  (newId) => {
    if (newId) {
      service = new SnapshotTimelineService(newId);
      loadTree();
    } else {
      tree.value = null;
      summaries.value.clear();
      selectedSnapshot.value = null;
      service = null;
    }
  },
  { immediate: true }
);

watch(
  () => tree.value?.currentSnapshotId,
  (newId) => {
    if (newId) {
      expandedNodes.value.add(newId);
    }
  }
);

let unlistenSnapshot: (() => void) | null = null;

onMounted(async () => {
  if (props.sessionId) {
    service = new SnapshotTimelineService(props.sessionId);
    await loadTree();
  }

  unlistenSnapshot = await listen("snapshot-created", () => {
    if (service) {
      service.clearCache();
      loadTree();
    }
  });
});

onUnmounted(() => {
  if (unlistenSnapshot) unlistenSnapshot();
});
</script>

<template>
  <div class="snapshot-timeline">
    <div class="timeline-header">
      <span class="timeline-title">SNAPSHOTS</span>
      <button
        class="refresh-btn"
        @click="loadTree"
        :disabled="loading"
        title="刷新"
      >
        <svg
          :class="{ spin: loading }"
          viewBox="0 0 24 24"
          width="14"
          height="14"
          stroke="currentColor"
          stroke-width="2"
          fill="none"
        >
          <path
            d="M23 4v6h-6M1 20v-6h6M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"
          />
        </svg>
      </button>
    </div>

    <div v-if="tree && tree.branches.length > 1" class="branch-tabs">
      <button
        v-for="branch in tree.branches"
        :key="branch.name"
        :class="['branch-tab', { active: branch.isActive }]"
        @click="switchBranch(branch.name)"
      >
        {{ branch.name }}
        <span v-if="branch.agentId" class="agent-badge">{{ branch.agentId }}</span>
      </button>
    </div>

    <div v-if="error" class="timeline-error">{{ error }}</div>

    <div v-if="loading" class="timeline-loading">加载中...</div>

    <div v-else-if="!tree || tree.branches.length === 0" class="timeline-empty">
      暂无快照<br />
      <span class="timeline-hint">Agent 修改文件时将自动创建快照</span>
    </div>

    <template v-else>
      <div
        v-for="branch in tree.branches.filter((b) => b.isActive)"
        :key="branch.name"
        class="branch-content"
      >
        <ul class="snapshot-list">
          <template v-for="{ node, depth } in renderTreeNode(branch.root)">
            <li
              v-if="node.id"
              :key="node.id"
              class="snapshot-item"
              :class="{
                expanded: expandedNodes.has(node.id),
                current: node.id === currentSnapshotId,
                checkpoint: node.isCheckpoint,
              }"
              :style="{ paddingLeft: `${12 + depth * 16}px` }"
            >
              <div
                class="snapshot-header"
                @click="toggleNode(node.id)"
              >
                <div class="snapshot-icon">
                  <svg
                    v-if="node.children.length > 0"
                    :class="{ rotated: expandedNodes.has(node.id) }"
                    viewBox="0 0 24 24"
                    width="12"
                    height="12"
                    stroke="currentColor"
                    stroke-width="2"
                    fill="none"
                  >
                    <polyline points="9 18 15 12 9 6" />
                  </svg>
                  <span v-else class="dot">●</span>
                </div>
                <div class="snapshot-info">
                  <span class="snapshot-message">{{
                    node.message || "无消息"
                  }}</span>
                  <span class="snapshot-time">{{ formatTime(node.timestamp) }}</span>
                </div>
                <div class="snapshot-badges">
                  <span v-if="node.isCheckpoint" class="badge checkpoint-badge"
                    >CP</span
                  >
                  <span v-if="node.agentId" class="badge agent-badge">{{
                    node.agentId
                  }}</span>
                </div>
              </div>

              <div
                v-if="expandedNodes.has(node.id)"
                class="snapshot-details"
              >
                <div
                  v-if="summaries.has(node.id)"
                  class="patch-summary"
                >
                  <div
                    v-for="(patch, idx) in summaries.get(node.id)!.patchSummary"
                    :key="idx"
                    class="patch-item"
                  >
                    <span class="patch-icon">{{
                      getFileOpIcon(patch.operation)
                    }}</span>
                    <span class="patch-path">{{ patch.path }}</span>
                    <span class="patch-stats">
                      <span v-if="patch.linesAdded" class="added"
                        >+{{ patch.linesAdded }}</span
                      >
                      <span v-if="patch.linesRemoved" class="removed"
                        >-{{ patch.linesRemoved }}</span
                      >
                    </span>
                  </div>
                </div>

                <div class="snapshot-actions">
                  <button
                    class="action-btn detail-btn"
                    @click.stop="loadSnapshotDetail(node.id)"
                  >
                    查看详情
                  </button>
                  <button
                    class="action-btn rollback-btn"
                    @click.stop="
                      rollbackConfirm = {
                        snapshotId: node.id,
                        message: node.message || '此快照',
                      }
                    "
                  >
                    回滚
                  </button>
                  <button
                    class="action-btn branch-btn"
                    @click.stop="createNewBranch(node.id)"
                  >
                    创建分支
                  </button>
                </div>
              </div>
            </li>
          </template>
        </ul>
      </div>
    </template>

    <Teleport to="body">
      <div
        v-if="rollbackConfirm"
        class="rollback-modal-overlay"
        @click="rollbackConfirm = null"
      >
        <div class="rollback-modal" @click.stop>
          <h3>确认回滚</h3>
          <p>确定要回滚到快照「{{ rollbackConfirm.message }}」吗？</p>
          <p class="rollback-warning">
            ⚠️ 这将恢复该快照之前的文件状态，当前未保存的更改将丢失。
          </p>
          <div class="modal-actions">
            <button class="cancel-btn" @click="rollbackConfirm = null">
              取消
            </button>
            <button
              class="confirm-btn"
              @click="handleRollback(rollbackConfirm.snapshotId)"
            >
              确认回滚
            </button>
          </div>
        </div>
      </div>

      <div
        v-if="selectedSnapshot"
        class="rollback-modal-overlay"
        @click="selectedSnapshot = null"
      >
        <div class="detail-modal" @click.stop>
          <h3>快照详情</h3>
          <div class="detail-content">
            <div class="detail-row">
              <span class="detail-label">ID:</span>
              <span class="detail-value">{{ selectedSnapshot.id }}</span>
            </div>
            <div class="detail-row">
              <span class="detail-label">分支:</span>
              <span class="detail-value">{{ selectedSnapshot.branchName }}</span>
            </div>
            <div class="detail-row">
              <span class="detail-label">时间:</span>
              <span class="detail-value">{{
                formatTime(selectedSnapshot.createdAt)
              }}</span>
            </div>
            <div class="detail-row">
              <span class="detail-label">消息:</span>
              <span class="detail-value">{{
                selectedSnapshot.message || "无"
              }}</span>
            </div>
            <div class="detail-section">
              <span class="detail-label">Patches ({{ selectedSnapshot.patches.length }}):</span>
              <div class="patch-diff-list">
                <DiffViewer
                  v-for="(patch, idx) in selectedSnapshot.patches"
                  :key="idx"
                  :patch="patch"
                />
              </div>
            </div>
          </div>
          <button class="confirm-btn" @click="selectedSnapshot = null">
            关闭
          </button>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.snapshot-timeline {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.timeline-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 4px 12px 8px;
}

.timeline-title {
  font-size: 0.75rem;
  font-weight: 600;
  color: var(--text-muted);
  letter-spacing: 0.05em;
  text-transform: uppercase;
}

.refresh-btn {
  background: transparent;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: var(--radius-md);
  display: flex;
  align-items: center;
  justify-content: center;
  transition: all var(--transition-fast);
}

.refresh-btn:hover:not(:disabled) {
  color: var(--accent-blue);
  background: var(--glass-bg-light);
}

.refresh-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.branch-tabs {
  display: flex;
  gap: 4px;
  padding: 0 12px 8px;
  flex-wrap: wrap;
}

.branch-tab {
  background: var(--glass-bg-light);
  border: 1px solid var(--glass-border-subtle);
  color: var(--text-muted);
  padding: 4px 10px;
  border-radius: var(--radius-md);
  font-size: 0.75rem;
  cursor: pointer;
  display: flex;
  align-items: center;
  gap: 4px;
  transition: all var(--transition-fast);
}

.branch-tab:hover {
  border-color: var(--glass-border);
  color: var(--text-main);
}

.branch-tab.active {
  background: var(--accent-blue);
  border-color: var(--accent-blue);
  color: white;
}

.agent-badge {
  font-size: 0.65rem;
  background: rgba(255, 255, 255, 0.2);
  padding: 1px 4px;
  border-radius: 4px;
}

.timeline-error {
  padding: 8px 12px;
  color: var(--accent-red);
  font-size: 0.8rem;
  background: rgba(239, 68, 68, 0.1);
  border-radius: var(--radius-md);
  margin: 0 12px;
}

.timeline-loading,
.timeline-empty {
  padding: 16px;
  text-align: center;
  color: var(--text-muted);
  font-size: 0.85rem;
}

.timeline-hint {
  font-size: 0.75rem;
  opacity: 0.6;
  margin-top: 4px;
  display: inline-block;
}

.snapshot-list {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
}

.snapshot-item {
  border-left: 2px solid transparent;
  transition: all var(--transition-fast);
}

.snapshot-item.current {
  border-left-color: var(--accent-blue);
  background: rgba(59, 130, 246, 0.05);
}

.snapshot-item.checkpoint {
  border-left-color: var(--accent-green);
}

.snapshot-item.expanded {
  background: var(--glass-bg);
}

.snapshot-header {
  display: flex;
  align-items: center;
  padding: 8px 12px;
  cursor: pointer;
  gap: 8px;
}

.snapshot-header:hover {
  background: var(--glass-bg-light);
}

.snapshot-icon {
  color: var(--text-muted);
  flex-shrink: 0;
  width: 16px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.snapshot-icon .rotated {
  transform: rotate(90deg);
}

.snapshot-icon .dot {
  font-size: 6px;
}

.snapshot-info {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.snapshot-message {
  font-size: 0.85rem;
  color: var(--text-main);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.snapshot-time {
  font-size: 0.7rem;
  color: var(--text-muted);
}

.snapshot-badges {
  display: flex;
  gap: 4px;
  flex-shrink: 0;
}

.badge {
  font-size: 0.65rem;
  padding: 1px 4px;
  border-radius: 4px;
}

.checkpoint-badge {
  background: rgba(34, 197, 94, 0.2);
  color: var(--accent-green);
}

.snapshot-details {
  padding: 0 12px 12px;
  border-top: 1px solid var(--glass-border-subtle);
  margin-top: 8px;
  padding-top: 8px;
}

.patch-summary {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-bottom: 12px;
}

.patch-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 8px;
  background: var(--glass-bg-light);
  border-radius: var(--radius-sm);
  font-size: 0.8rem;
}

.patch-icon {
  font-size: 0.9rem;
}

.patch-path {
  flex: 1;
  color: var(--text-main);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.patch-stats {
  display: flex;
  gap: 4px;
  font-size: 0.7rem;
}

.patch-stats .added {
  color: var(--accent-green);
}

.patch-stats .removed {
  color: var(--accent-red);
}

.snapshot-actions {
  display: flex;
  gap: 8px;
}

.action-btn {
  flex: 1;
  padding: 6px 12px;
  border-radius: var(--radius-md);
  font-size: 0.8rem;
  cursor: pointer;
  transition: all var(--transition-fast);
  border: 1px solid transparent;
}

.detail-btn {
  background: var(--glass-bg-light);
  color: var(--text-muted);
  border-color: var(--glass-border-subtle);
}

.detail-btn:hover {
  background: var(--glass-bg);
  color: var(--text-main);
}

.rollback-btn {
  background: rgba(239, 68, 68, 0.1);
  color: var(--accent-red);
  border-color: rgba(239, 68, 68, 0.2);
}

.rollback-btn:hover {
  background: rgba(239, 68, 68, 0.2);
}

.branch-btn {
  background: rgba(59, 130, 246, 0.1);
  color: var(--accent-blue);
  border-color: rgba(59, 130, 246, 0.2);
}

.branch-btn:hover {
  background: rgba(59, 130, 246, 0.2);
}

.rollback-modal-overlay {
  position: fixed;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background: rgba(0, 0, 0, 0.6);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 9999;
  backdrop-filter: blur(4px);
}

.rollback-modal,
.detail-modal {
  background: var(--glass-bg);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
  padding: 24px;
  max-width: 400px;
  width: 90%;
  box-shadow: var(--shadow-lg);
}

.rollback-modal h3,
.detail-modal h3 {
  margin: 0 0 12px;
  font-size: 1.1rem;
  color: var(--text-main);
}

.rollback-modal p {
  margin: 0 0 8px;
  color: var(--text-muted);
  font-size: 0.9rem;
}

.rollback-warning {
  color: var(--accent-yellow) !important;
  font-size: 0.85rem !important;
}

.detail-content {
  margin-bottom: 16px;
}

.detail-row {
  display: flex;
  gap: 8px;
  padding: 4px 0;
  font-size: 0.85rem;
}

.detail-label {
  color: var(--text-muted);
  min-width: 60px;
}

.detail-value {
  color: var(--text-main);
}

.detail-section {
  margin-top: 12px;
}

.patch-diff-list {
  margin: 8px 0 0;
  max-height: 400px;
  overflow-y: auto;
}

.modal-actions {
  display: flex;
  gap: 12px;
  margin-top: 16px;
}

.cancel-btn,
.confirm-btn {
  flex: 1;
  padding: 10px 16px;
  border-radius: var(--radius-md);
  font-size: 0.9rem;
  cursor: pointer;
  transition: all var(--transition-fast);
  border: 1px solid transparent;
}

.cancel-btn {
  background: var(--glass-bg-light);
  color: var(--text-muted);
  border-color: var(--glass-border-subtle);
}

.cancel-btn:hover {
  background: var(--glass-bg);
  color: var(--text-main);
}

.confirm-btn {
  background: var(--accent-blue);
  color: white;
}

.confirm-btn:hover {
  filter: brightness(1.1);
}

@keyframes spin {
  100% {
    transform: rotate(360deg);
  }
}

.spin {
  animation: spin 1s linear infinite;
}
</style>
