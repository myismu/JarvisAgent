<!--
# FileOpIcon.vue — 文件操作类型图标

统一渲染快照 / 检查点里的文件操作图标（创建、修改、删除、重命名）。

## Usage
- `<FileOpIcon :op="patch.operation" />`（快照，后端返回 create/delete/update/rename）
- `<FileOpIcon :op="op.opType" />`（检查点，类型为 OpType）

## Constraints
- 纯 SVG 描边图标，跟随 `currentColor`，由父级容器控制颜色与尺寸
- 未知操作**不渲染**任何标记，避免用默认图标谎报操作类型
- 路径数据集中在 `utils/timeline.ts`，新增操作类型只改那一处
-->
<script setup lang="ts">
import { computed } from "vue";
import { getFileOpPaths } from "../../utils/timeline";

const props = defineProps<{
  /** 操作类型：create / delete / update(edit) / rename */
  op: string;
  /** 图标边长（px） */
  size?: number;
}>();

const paths = computed(() => getFileOpPaths(props.op));
const px = computed(() => `${props.size ?? 14}`);
</script>

<template>
  <svg
    v-if="paths.length"
    class="file-op-icon"
    :width="px"
    :height="px"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path v-for="d in paths" :key="d" :d="d" />
  </svg>
</template>

<style scoped>
.file-op-icon {
  flex: 0 0 auto;
  display: block;
}
</style>
