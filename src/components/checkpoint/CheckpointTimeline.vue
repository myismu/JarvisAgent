<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, computed } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { Checkpoint, Branch, CheckpointTree } from '../../types';
import { formatRelativeTime, getFileOpIcon, getFileOpLabel } from '../../utils/timeline';

const props = defineProps<{
  sessionId: string | null;
}>();

const checkpoints = ref<Checkpoint[]>([]);
const branches = ref<Branch[]>([]);
const activeBranch = ref<Branch | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
const expandedCheckpoint = ref<string | null>(null);
const rollbackConfirm = ref<{ checkpointId: string; message: string } | null>(null);
const rollbackResult = ref<string[] | null>(null);

const sortedCheckpoints = computed(() => {
  return [...checkpoints.value].sort((a, b) => b.createdAt - a.createdAt);
});

const formatTime = formatRelativeTime;

const loadCheckpointTree = async () => {
  if (!props.sessionId) return;
  
  loading.value = true;
  error.value = null;
  
  try {
    const tree = await invoke<CheckpointTree>('get_checkpoint_tree', { 
      sessionId: props.sessionId 
    });
    checkpoints.value = tree.checkpoints;
    
    const branchList = await invoke<Branch[]>('list_branches', { 
      sessionId: props.sessionId 
    });
    branches.value = branchList;
    
    const active = await invoke<Branch>('get_active_branch', { 
      sessionId: props.sessionId 
    });
    activeBranch.value = active;
  } catch (err) {
    console.error('加载检查点失败:', err);
    error.value = String(err);
  } finally {
    loading.value = false;
  }
};

const handleRollback = async (checkpointId: string) => {
  if (!props.sessionId) return;
  
  try {
    const result = await invoke<string[]>('rollback_to_checkpoint', {
      sessionId: props.sessionId,
      checkpointId
    });
    rollbackResult.value = result;
    rollbackConfirm.value = null;
    expandedCheckpoint.value = null;
    
    await loadCheckpointTree();
  } catch (err) {
    console.error('回滚失败:', err);
    error.value = `回滚失败: ${err}`;
  }
};

const switchBranch = async (branchName: string) => {
  if (!props.sessionId) return;
  
  try {
    await invoke('switch_branch', {
      sessionId: props.sessionId,
      branchName
    });
    await loadCheckpointTree();
  } catch (err) {
    console.error('切换分支失败:', err);
    error.value = `切换分支失败: ${err}`;
  }
};

const createNewBranch = async (fromCheckpointId: string) => {
  if (!props.sessionId) return;
  
  const branchName = prompt('请输入新分支名称:');
  if (!branchName) return;
  
  try {
    await invoke('create_branch', {
      sessionId: props.sessionId,
      branchName,
      fromCheckpointId,
      description: `从检查点 ${fromCheckpointId} 创建`
    });
    await loadCheckpointTree();
  } catch (err) {
    console.error('创建分支失败:', err);
    error.value = `创建分支失败: ${err}`;
  }
};

let unlistenCheckpoint: (() => void) | null = null;

watch(() => props.sessionId, (newId) => {
  if (newId) {
    loadCheckpointTree();
  } else {
    checkpoints.value = [];
    branches.value = [];
    activeBranch.value = null;
  }
});

onMounted(async () => {
  if (props.sessionId) {
    await loadCheckpointTree();
  }
  
  unlistenCheckpoint = await listen('checkpoint-created', () => {
    if (props.sessionId) {
      loadCheckpointTree();
    }
  });
});

onUnmounted(() => {
  if (unlistenCheckpoint) unlistenCheckpoint();
});
</script>

<template>
  <div class="checkpoint-timeline">
    <div class="timeline-header">
      <span class="timeline-title">CHECKPOINTS</span>
      <button 
        class="refresh-btn" 
        @click="loadCheckpointTree" 
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
          <path d="M23 4v6h-6M1 20v-6h6M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"/>
        </svg>
      </button>
    </div>
    
    <div v-if="branches.length > 1" class="branch-tabs">
      <button
        v-for="branch in branches"
        :key="branch.name"
        :class="['branch-tab', { active: branch.isActive }]"
        @click="switchBranch(branch.name)"
      >
        {{ branch.name }}
        <span v-if="branch.headCheckpointId" class="branch-count">●</span>
      </button>
    </div>
    
    <div v-if="error" class="timeline-error">
      {{ error }}
    </div>
    
    <div v-if="loading" class="timeline-loading">
      加载中...
    </div>
    
    <div v-else-if="checkpoints.length === 0" class="timeline-empty">
      暂无检查点
    </div>
    
    <ul v-else class="checkpoint-list">
      <li 
        v-for="cp in sortedCheckpoints" 
        :key="cp.id" 
        class="checkpoint-item"
        :class="{ expanded: expandedCheckpoint === cp.id }"
      >
        <div class="checkpoint-header" @click="expandedCheckpoint = expandedCheckpoint === cp.id ? null : cp.id">
          <div class="checkpoint-icon">
            <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none">
              <circle cx="12" cy="12" r="10"/>
              <polyline points="12 6 12 12 16 14"/>
            </svg>
          </div>
          <div class="checkpoint-info">
            <span class="checkpoint-message">{{ cp.triggerMessage }}</span>
            <span class="checkpoint-time">{{ formatTime(cp.createdAt) }}</span>
          </div>
          <span class="checkpoint-ops-count">{{ cp.operations.length }} 操作</span>
        </div>
        
        <div v-if="expandedCheckpoint === cp.id" class="checkpoint-details">
          <ul class="operation-list">
            <li v-for="(op, idx) in cp.operations" :key="idx" class="operation-item">
              <span class="op-icon">{{ getFileOpIcon(op.opType) }}</span>
              <span class="op-type">{{ getFileOpLabel(op.opType) }}</span>
              <span class="op-path" :title="op.path">{{ op.path.split(/[\\/]/).pop() }}</span>
              <span v-if="op.diffSummary" class="op-diff">{{ op.diffSummary }}</span>
            </li>
          </ul>
          
          <div class="checkpoint-actions">
            <button class="action-btn rollback-btn" @click.stop="rollbackConfirm = { checkpointId: cp.id, message: cp.triggerMessage }">
              回滚到此点
            </button>
            <button class="action-btn branch-btn" @click.stop="createNewBranch(cp.id)">
              创建分支
            </button>
          </div>
        </div>
      </li>
    </ul>
    
    <Teleport to="body">
      <div v-if="rollbackConfirm" class="rollback-modal-overlay" @click="rollbackConfirm = null">
        <div class="rollback-modal" @click.stop>
          <h3>确认回滚</h3>
          <p>确定要回滚到检查点「{{ rollbackConfirm.message }}」吗？</p>
          <p class="rollback-warning">⚠️ 这将恢复该检查点之前的文件状态，当前未保存的更改将丢失。</p>
          <div class="modal-actions">
            <button class="cancel-btn" @click="rollbackConfirm = null">取消</button>
            <button class="confirm-btn" @click="handleRollback(rollbackConfirm.checkpointId)">确认回滚</button>
          </div>
        </div>
      </div>
      
      <div v-if="rollbackResult" class="rollback-modal-overlay" @click="rollbackResult = null">
        <div class="rollback-modal success" @click.stop>
          <h3>回滚成功</h3>
          <ul class="rollback-result-list">
            <li v-for="(file, idx) in rollbackResult" :key="idx">{{ file }}</li>
          </ul>
          <button class="confirm-btn" @click="rollbackResult = null">确定</button>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.checkpoint-timeline {
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

.branch-count {
  font-size: 0.6rem;
}

.timeline-error {
  padding: 8px 12px;
  color: var(--accent-red);
  font-size: 0.8rem;
  background: rgba(239, 68, 68, 0.1);
  border-radius: var(--radius-md);
  margin: 0 12px;
}

.timeline-loading, .timeline-empty {
  padding: 16px;
  text-align: center;
  color: var(--text-muted);
  font-size: 0.85rem;
}

.checkpoint-list {
  list-style: none;
  padding: 0;
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.checkpoint-item {
  background: transparent;
  border: 1px solid transparent;
  border-radius: var(--radius-md);
  transition: all var(--transition-fast);
}

.checkpoint-item.expanded {
  background: var(--glass-bg);
  border-color: var(--glass-border);
}

.checkpoint-header {
  display: flex;
  align-items: center;
  padding: 8px 12px;
  cursor: pointer;
  gap: 8px;
}

.checkpoint-header:hover {
  background: var(--glass-bg-light);
}

.checkpoint-icon {
  color: var(--text-muted);
  flex-shrink: 0;
}

.checkpoint-info {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.checkpoint-message {
  font-size: 0.85rem;
  color: var(--text-main);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.checkpoint-time {
  font-size: 0.7rem;
  color: var(--text-muted);
}

.checkpoint-ops-count {
  font-size: 0.7rem;
  color: var(--accent-blue);
  background: rgba(59, 130, 246, 0.1);
  padding: 2px 6px;
  border-radius: 10px;
  flex-shrink: 0;
}

.checkpoint-details {
  padding: 0 12px 12px;
  border-top: 1px solid var(--glass-border-subtle);
  margin-top: 8px;
  padding-top: 8px;
}

.operation-list {
  list-style: none;
  padding: 0;
  margin: 0 0 12px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.operation-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  background: var(--glass-bg-light);
  border-radius: var(--radius-sm);
  font-size: 0.8rem;
}

.op-icon {
  font-size: 0.9rem;
}

.op-type {
  color: var(--text-muted);
  font-size: 0.75rem;
}

.op-path {
  flex: 1;
  color: var(--text-main);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.op-diff {
  color: var(--text-muted);
  font-size: 0.7rem;
  max-width: 100px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.checkpoint-actions {
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

.rollback-modal {
  background: var(--glass-bg);
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-lg);
  padding: 24px;
  max-width: 400px;
  width: 90%;
  box-shadow: var(--shadow-lg);
}

.rollback-modal h3 {
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

.rollback-modal.success h3 {
  color: var(--accent-green);
}

.rollback-result-list {
  list-style: none;
  padding: 0;
  margin: 0 0 16px;
  max-height: 200px;
  overflow-y: auto;
}

.rollback-result-list li {
  padding: 6px 0;
  color: var(--text-muted);
  font-size: 0.85rem;
  border-bottom: 1px solid var(--glass-border-subtle);
}

.rollback-result-list li:last-child {
  border-bottom: none;
}

.modal-actions {
  display: flex;
  gap: 12px;
  margin-top: 16px;
}

.cancel-btn, .confirm-btn {
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
  100% { transform: rotate(360deg); }
}

.spin {
  animation: spin 1s linear infinite;
}
</style>
