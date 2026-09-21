<!--
# ToolsPanel.vue — 设置面板的「工具」页

逐个启停工具。**关掉 = 模型完全看不到该工具** —— 它不进 tools 参数、延迟工具目录里
也搜不到，而不是"在、但调用被拒"那种软禁用。

## 生效时机：只对新会话生效
核心工具的 schema 会进请求体的 tools 参数，而它必须**整个会话内字节恒定**，否则每轮一变、
prompt cache 前缀就整体失效。所以改完开关只对新会话生效。

**刻意没做「应用到当前会话」按钮**（曾有过，2026-09-21 删）：它摆在页面上会暗示
"不点就不生效"，而实际上新会话本来就会生效 —— 制造的是误导，不是能力。真要验证效果，
开个新会话的成本远比"烧一次 prompt cache + 多一个生效时机的概念"低。
（对比：技能开关是立即生效的，因为它只改运行时数据、不碰 schema。这个差异是本质的，
不该用按钮去抹平。）

## Key Exports
- `ToolsPanel`: 工具开关列表（无 props / 无事件）

## Dependencies
- Internal: `../../composables/useToast`
- External: `@tauri-apps/api/core`（invoke）、`vue-i18n`

## Constraints
- 开关状态由后端 `app-config.json` 持有，本组件只做回显与转发，不自己存状态
-->

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useI18n } from 'vue-i18n';
import { showToast } from '../../composables/useToast';

/** 与后端 `command/tool.rs::ToolMeta` 对应 */
interface ToolMeta {
  name: string;
  description: string;
  category: string;
  /** true = 延迟工具（需 GetToolCatalog → DiscoverTools → ExecuteTool 三步才能用） */
  deferred: boolean;
  enabled: boolean;
  /** 完整 JSON Schema，与模型收到的那份是同一个对象 */
  schema?: JsonSchema;
}

/** Anthropic 工具 schema 的形状（只声明我们渲染用到的字段） */
interface JsonSchema {
  /** 完整描述。**提示词下沉过来的用法说明都在这里**（列表上那句是另一份简述） */
  description?: string;
  input_schema?: {
    properties?: Record<string, SchemaProp>;
    required?: string[];
  };
}

interface SchemaProp {
  type?: string;
  description?: string;
  enum?: string[];
  items?: { type?: string };
}

/** 拍平成可渲染的参数行 */
interface ParamRow {
  name: string;
  type: string;
  required: boolean;
  description: string;
  enumValues: string[];
}

const { t } = useI18n();

const tools = ref<ToolMeta[]>([]);
const loading = ref(true);
/** 展开了 schema 的工具名。用 Set 而非给每项加布尔字段：列表来自后端，不污染它 */
const expanded = ref<Set<string>>(new Set());
/** 搜索词：按名称 / 分类 / 描述过滤。40 个工具挨个滑太累，直接定位更快 */
const keyword = ref('');

const matches = (tool: ToolMeta, needle: string) =>
  tool.name.toLowerCase().includes(needle) ||
  tool.category.toLowerCase().includes(needle) ||
  tool.description.toLowerCase().includes(needle);

/**
 * 分两组展示，语义不同：
 * - 核心工具：schema 常驻请求体，模型随时看得到；
 * - 延迟工具：按需发现，模型不走到 DiscoverTools 那一步就看不见它。
 * 分组只是帮用户理解"关掉它的代价是什么"，开关本身的效力完全一样。
 */
const groups = computed(() => {
  const needle = keyword.value.trim().toLowerCase();
  const visible = needle ? tools.value.filter((tool) => matches(tool, needle)) : tools.value;
  return [
    {
      key: 'core',
      title: t('settings.tools.coreGroup'),
      hint: t('settings.tools.coreHint'),
      items: visible.filter((tool) => !tool.deferred),
    },
    {
      key: 'deferred',
      title: t('settings.tools.deferredGroup'),
      hint: t('settings.tools.deferredHint'),
      items: visible.filter((tool) => tool.deferred),
    },
  ];
});

/** 搜索后一个都没匹配上时给个明确交代，而不是一片空白 */
const noMatch = computed(
  () => !loading.value && keyword.value.trim() !== '' && groups.value.every((g) => g.items.length === 0),
);

const formatError = (err: unknown) => (err instanceof Error ? err.message : String(err));

/**
 * 把 JSON Schema 的 properties 拍平成参数行。
 *
 * 数组类型显示成 `string[]` 这种更好读的形式（原始 schema 里是
 * `{type:"array", items:{type:"string"}}` 两层嵌套）。
 */
const paramsOf = (tool: ToolMeta): ParamRow[] => {
  const inputSchema = tool.schema?.input_schema;
  const properties = inputSchema?.properties ?? {};
  const required = inputSchema?.required ?? [];
  return Object.entries(properties).map(([name, prop]) => {
    const type = prop.type ?? 'any';
    return {
      name,
      type: type === 'array' && prop.items?.type ? `${prop.items.type}[]` : type,
      required: required.includes(name),
      description: prop.description ?? '',
      enumValues: prop.enum ?? [],
    };
  });
};

/**
 * schema 里的**完整描述**。
 *
 * 这是提示词下沉重构的落点：原先散在提示词里的用法说明（"删目录是整棵一起删"、
 * "绝对禁止读取二进制文件"、"本工具不能用 cd 要传 dir"…）都搬到了这里 ——
 * 也就是模型实际收到的那份。列表上那句是 `ToolDef.description`，只是简述，
 * 两者通常不同。相同就不必重复显示。
 */
const fullDescription = (tool: ToolMeta): string => {
  const text = (tool.schema?.description ?? '').trim();
  return text === tool.description.trim() ? '' : text;
};

const toggleSchema = (name: string) => {
  const next = new Set(expanded.value);
  if (next.has(name)) {
    next.delete(name);
  } else {
    next.add(name);
  }
  // 整体替换而非原地改：ref 包 Set 时原地 mutate 不触发更新
  expanded.value = next;
};

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

onMounted(load);
</script>

<template>
  <div class="tools-panel">
    <p class="tools-intro">{{ t('settings.tools.intro') }}</p>

    <input
      v-model="keyword"
      type="search"
      class="tools-search"
      :placeholder="t('settings.tools.searchPlaceholder')"
    />

    <div v-if="loading" class="tools-empty">{{ t('settings.tools.loading') }}</div>
    <div v-else-if="noMatch" class="tools-empty">
      {{ t('settings.tools.noMatch', { keyword: keyword.trim() }) }}
    </div>

    <template v-else>
      <section v-for="group in groups" :key="group.key" class="tools-group">
        <div v-if="group.items.length" class="group-head">
          <h4 class="group-title">{{ group.title }}</h4>
          <span class="group-hint">{{ group.hint }}</span>
          <span class="group-count">{{ group.items.length }}</span>
        </div>
        <!-- 每个工具压成一行：名称 / 分类 / 描述（单行省略，hover 看全） / 参数 / 开关。
             40 个工具挨个占三行的话要滑很久，所以描述不折行，需要看全就 hover。 -->
        <div
          v-for="tool in group.items"
          :key="tool.name"
          class="tool-row"
          :class="{ off: !tool.enabled, expanded: expanded.has(tool.name) }"
        >
          <div class="tool-line">
            <span class="tool-name">{{ tool.name }}</span>
            <span class="tool-category">{{ tool.category }}</span>
            <span class="tool-desc" :title="tool.description">{{ tool.description }}</span>
            <button type="button" class="schema-toggle" @click="toggleSchema(tool.name)">
              {{ expanded.has(tool.name) ? t('settings.tools.hideSchema') : t('settings.tools.showSchema') }}
            </button>
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

          <!-- 完整 schema（点开才渲染）：这就是模型实际收到的那份，不是另写的文档 -->
          <div v-if="expanded.has(tool.name)" class="tool-schema">
            <!-- 完整描述：提示词下沉过来的用法说明都在这儿 -->
            <div v-if="fullDescription(tool)" class="schema-desc">
              {{ fullDescription(tool) }}
            </div>
            <div
              v-if="paramsOf(tool).length === 0 && !fullDescription(tool)"
              class="schema-empty"
            >
              {{ t('settings.tools.noParams') }}
            </div>
            <div v-for="param in paramsOf(tool)" :key="param.name" class="param-row">
              <div class="param-line">
                <span class="param-name">{{ param.name }}</span>
                <span class="param-type">{{ param.type }}</span>
                <span v-if="param.required" class="param-required">
                  {{ t('settings.tools.required') }}
                </span>
                <span v-if="param.description" class="param-desc">{{ param.description }}</span>
              </div>
              <div v-if="param.enumValues.length" class="param-enum">
                {{ param.enumValues.join(' / ') }}
              </div>
            </div>
          </div>
        </div>
      </section>
    </template>
  </div>
</template>

<style scoped>
.tools-panel {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 4px 0 24px;
}

.tools-intro {
  margin: 0;
  padding: 9px 12px;
  border-radius: var(--radius-md);
  background: var(--glass-bg-light);
  color: var(--text-soft);
  font-size: 0.78rem;
  line-height: 1.55;
}

.tools-search {
  width: 100%;
  padding: 6px 10px;
  border-radius: var(--radius-md);
  border: 1px solid var(--glass-border);
  background: var(--glass-bg);
  color: var(--text-main);
  font-size: 0.8rem;
  outline: none;
  transition: border-color var(--transition-fast);
}
.tools-search:focus {
  border-color: var(--accent-blue);
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
  gap: 1px; /* 行贴紧：40 个工具要能一眼扫完 */
}
.group-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin: 10px 0 4px;
  position: sticky; /* 滚到中段也知道自己在哪个分组 */
  top: -24px; /* 抵消 .settings-body 的 padding-top，贴住内容区顶部 */
  padding: 4px 0;
  background: var(--bg-dark);
  z-index: 1;
}
.group-title {
  margin: 0;
  font-size: 0.8rem;
  font-weight: 650;
  color: var(--text-main);
}
.group-hint {
  color: var(--text-muted);
  font-size: 0.7rem;
}
.group-count {
  margin-left: auto;
  color: var(--text-muted);
  font-size: 0.7rem;
}

/* ── 单个工具：一行 ── */
.tool-row {
  display: flex;
  flex-direction: column;
  border-radius: var(--radius-md);
  transition: background var(--transition-fast);
}
.tool-line {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 10px;
  border-radius: var(--radius-md);
  transition: background var(--transition-fast);
}
.tool-row:hover .tool-line,
.tool-row.expanded .tool-line {
  background: var(--glass-bg-light);
}
/* 关掉的行整体压暗：一眼能看出"这些是不生效的" */
.tool-row.off .tool-name,
.tool-row.off .tool-desc {
  opacity: 0.5;
}

.tool-name {
  flex-shrink: 0;
  font-family: var(--font-mono);
  font-size: 0.78rem;
  font-weight: 600;
  color: var(--text-main);
}
.tool-category {
  flex-shrink: 0;
  width: 4.5em; /* 定宽：名称长度不一时分类列也能对齐 */
  color: var(--text-muted);
  font-size: 0.7rem;
}
/* 描述单行省略：需要看全就 hover（title 属性）。
   折行会让 40 个工具变成 80+ 行，正是"滑下去很累"的来源。 */
.tool-desc {
  flex: 1;
  min-width: 0;
  color: var(--text-muted);
  font-size: 0.74rem;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.schema-toggle {
  flex-shrink: 0;
  padding: 0;
  border: none;
  background: none;
  color: var(--text-muted);
  font-size: 0.7rem;
  cursor: pointer;
  transition: color var(--transition-fast);
}
.schema-toggle:hover {
  color: var(--accent-blue);
}

/* ── 参数表（展开后跨全宽，缩进对齐名称列） ── */
.tool-schema {
  margin: 2px 10px 6px;
  padding: 8px 10px;
  border-radius: var(--radius-md);
  border: 1px solid var(--glass-border-subtle);
  display: flex;
  flex-direction: column;
  gap: 6px;
}
/* 完整描述：与参数列表用一条淡分隔线隔开（两者是不同的东西 ——
   上面是"这工具是干嘛的、有什么禁忌"，下面是"每个参数怎么填"） */
.schema-desc {
  color: var(--text-soft);
  font-size: 0.74rem;
  line-height: 1.6;
  white-space: pre-wrap; /* 描述里用 \n\n 分段，保留它 */
  word-break: break-word;
  padding-bottom: 8px;
  border-bottom: 1px solid var(--glass-border-subtle);
}
.schema-empty {
  color: var(--text-muted);
  font-size: 0.72rem;
}
.param-row {
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.param-line {
  display: flex;
  align-items: baseline;
  gap: 6px;
  flex-wrap: wrap;
}
.param-name {
  font-family: var(--font-mono);
  font-size: 0.74rem;
  font-weight: 600;
  color: var(--text-main);
}
.param-type {
  font-family: var(--font-mono);
  font-size: 0.7rem;
  color: var(--accent-blue);
}
.param-required {
  font-size: 0.68rem;
  color: var(--text-warning);
}
.param-desc {
  flex: 1;
  min-width: 0;
  color: var(--text-muted);
  font-size: 0.72rem;
  line-height: 1.45;
  word-break: break-word;
}
.param-enum {
  font-family: var(--font-mono);
  font-size: 0.7rem;
  color: var(--text-soft);
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
