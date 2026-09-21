<!--
# ToolsPanel.vue — 设置面板的「工具」页

逐个启停工具。**关掉 = 模型完全看不到该工具** —— 它不进 tools 参数、延迟工具目录里
也搜不到，而不是"在、但调用被拒"那种软禁用。

## 生效时机（与技能开关不同，必须让用户知道）
- 改开关 → 只对**新会话**生效。原因：核心工具的 schema 会进请求体的 tools 参数，
  而它必须会话内字节恒定，否则每轮一变、prompt cache 前缀整体失效。
- 想立刻用上 → 点「应用到当前会话」。代价是掉一次 prompt cache，所以只由用户显式触发。
- 技能开关则是**立即生效**的（它只改运行时数据、不碰 schema）—— 两者并排放在设置里，
  不写清楚用户一定会困惑。

## Key Exports
- `ToolsPanel`: 工具开关列表（无 props / 无事件）

## Dependencies
- Internal: `../../composables/useToast`、`../../stores/session`
- External: `@tauri-apps/api/core`（invoke）、`vue-i18n`

## Constraints
- 开关状态由后端 `app-config.json` 持有，本组件只做回显与转发，不自己存状态
-->

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useI18n } from 'vue-i18n';
import { showToast } from '../../composables/useToast';
import { useSessionStore } from '../../stores/session';

/** 与后端 `command/tool.rs::ToolMeta` 对应 */
interface ToolMeta {
  name: string;
  description: string;
  category: string;
  /** true = 延迟工具（需 GetToolCatalog → DiscoverTools → ExecuteTool 三步才能用） */
  deferred: boolean;
  enabled: boolean;
}

const { t } = useI18n();
const session = useSessionStore();

const tools = ref<ToolMeta[]>([]);
const loading = ref(true);
const applying = ref(false);

/**
 * 分两组展示，语义不同：
 * - 核心工具：schema 常驻请求体，模型随时看得到；
 * - 延迟工具：按需发现，模型不走到 DiscoverTools 那一步就看不见它。
 * 分组只是帮用户理解"关掉它的代价是什么"，开关本身的效力完全一样。
 */
const groups = computed(() => [
  {
    key: 'core',
    title: t('settings.tools.coreGroup'),
    hint: t('settings.tools.coreHint'),
    items: tools.value.filter((tool) => !tool.deferred),
  },
  {
    key: 'deferred',
    title: t('settings.tools.deferredGroup'),
    hint: t('settings.tools.deferredHint'),
    items: tools.value.filter((tool) => tool.deferred),
  },
]);

const formatError = (err: unknown) => (err instanceof Error ? err.message : String(err));

const load = async () => {
  try {
    tools.value = await invoke<ToolMeta[]>('list_tools');
  } catch (err) {
    console.error('加载工具列表失败:', err);
    showToast(t('settings.tools.loadError', { error: formatError(err) }), 'error');
  } finally {
    loading.value = false;
  }
};

const toggle = async (tool: ToolMeta) => {
  const next = !tool.enabled;
  try {
    await invoke('set_tool_active', { toolName: tool.name, enabled: next });
    // 后端写成功后只回显本地状态，不重新拉列表（省一次往返，且避免顺序错乱）
    tool.enabled = next;
    showToast(t('settings.tools.savedHint'));
  } catch (err) {
    console.error('切换工具开关失败:', err);
    showToast(t('settings.tools.saveError', { error: formatError(err) }), 'error');
  }
};

/**
 * 让当前会话立刻用上最新开关。
 *
 * 后端实现是丢掉会话快照、下次构建请求时重读配置 —— `tools` 参数会变，
 * **掉一次 prompt cache**。所以只在这里、由用户点击触发。
 */
const applyNow = async () => {
  if (applying.value) return;
  const sessionId = session.activeSessionId;
  if (!sessionId) {
    showToast(t('settings.tools.noSession'), 'error');
    return;
  }
  applying.value = true;
  try {
    await invoke('apply_tool_filter_now', { sessionId });
    showToast(t('settings.tools.applied'));
  } catch (err) {
    console.error('应用工具开关失败:', err);
    showToast(t('settings.tools.applyError', { error: formatError(err) }), 'error');
  } finally {
    applying.value = false;
  }
};

onMounted(load);
</script>

<template>
  <div class="tools-panel">
    <p class="tools-intro">{{ t('settings.tools.intro') }}</p>

    <div class="apply-row">
      <button type="button" class="apply-btn" :disabled="applying || loading" @click="applyNow">
        {{ applying ? t('settings.tools.applying') : t('settings.tools.applyNow') }}
      </button>
      <span class="apply-hint">{{ t('settings.tools.applyHint') }}</span>
    </div>

    <div v-if="loading" class="tools-empty">{{ t('settings.tools.loading') }}</div>

    <template v-else>
      <section v-for="group in groups" :key="group.key" class="tools-group">
        <div v-if="group.items.length" class="group-head">
          <h4 class="group-title">{{ group.title }}</h4>
          <span class="group-hint">{{ group.hint }}</span>
        </div>
        <div
          v-for="tool in group.items"
          :key="tool.name"
          class="tool-row"
          :class="{ off: !tool.enabled }"
        >
          <div class="tool-main">
            <div class="tool-head">
              <span class="tool-name">{{ tool.name }}</span>
              <span class="tool-category">{{ tool.category }}</span>
            </div>
            <div class="tool-desc">{{ tool.description }}</div>
          </div>
          <button
            type="button"
            class="tool-switch"
            :class="{ on: tool.enabled }"
            :aria-pressed="tool.enabled"
            :aria-label="tool.name"
            @click="toggle(tool)"
          >
            <span class="knob" />
          </button>
        </div>
      </section>
    </template>
  </div>
</template>

<style scoped>
.tools-panel {
  display: flex;
  flex-direction: column;
  gap: 18px;
  padding: 4px 0 24px;
}

.tools-intro {
  margin: 0;
  padding: 10px 12px;
  border-radius: var(--radius-md);
  background: var(--glass-bg-light);
  color: var(--text-soft);
  font-size: 0.8rem;
  line-height: 1.6;
}

/* ── 应用到当前会话 ── */
.apply-row {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.apply-btn {
  padding: 6px 14px;
  border-radius: var(--radius-md);
  border: 1px solid var(--glass-border);
  background: var(--glass-bg);
  color: var(--text-main);
  font-size: 0.8rem;
  cursor: pointer;
  transition: var(--transition-fast);
}
.apply-btn:hover:not(:disabled) {
  background: var(--glass-bg-light);
  border-color: var(--accent-blue);
  color: var(--accent-blue);
}
.apply-btn:disabled {
  opacity: 0.5;
  cursor: default;
}
.apply-hint {
  color: var(--text-muted);
  font-size: 0.75rem;
}

.tools-empty {
  color: var(--text-muted);
  font-size: 0.8rem;
  padding: 8px 0;
}

/* ── 分组 ── */
.tools-group {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.group-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin-bottom: 4px;
}
.group-title {
  margin: 0;
  font-size: 0.82rem;
  font-weight: 650;
  color: var(--text-main);
}
.group-hint {
  color: var(--text-muted);
  font-size: 0.72rem;
}

/* ── 单个工具 ── */
.tool-row {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  padding: 8px 10px;
  border-radius: var(--radius-md);
  transition: background var(--transition-fast);
}
.tool-row:hover {
  background: var(--glass-bg-light);
}
/* 关掉的行整体压暗：一眼能看出"这些是不生效的" */
.tool-row.off .tool-name,
.tool-row.off .tool-desc {
  opacity: 0.55;
}

.tool-main {
  flex: 1;
  min-width: 0;
}
.tool-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
}
.tool-name {
  font-family: var(--font-mono);
  font-size: 0.8rem;
  font-weight: 600;
  color: var(--text-main);
}
.tool-category {
  flex-shrink: 0;
  color: var(--text-muted);
  font-size: 0.7rem;
}
.tool-desc {
  margin-top: 2px;
  color: var(--text-muted);
  font-size: 0.75rem;
  line-height: 1.5;
  word-break: break-word;
}

/* ── 开关 ── */
.tool-switch {
  flex-shrink: 0;
  margin-top: 2px;
  width: 34px;
  height: 18px;
  padding: 0;
  border: none;
  border-radius: 9px;
  background: var(--toggle-track-inactive);
  cursor: pointer;
  position: relative;
  transition: background var(--transition-fast);
}
.tool-switch.on {
  background: var(--accent-green);
}
.knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: #fff;
  transition: transform var(--transition-fast);
}
.tool-switch.on .knob {
  transform: translateX(16px);
}
</style>
