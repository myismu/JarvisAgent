<script setup lang="ts">
import { computed } from "vue";
import type { Patch } from "../../types";

const props = defineProps<{
  patch: Patch;
}>();

interface DiffLine {
  type: "context" | "added" | "removed" | "empty";
  content: string;
  oldLineNum?: number;
  newLineNum?: number;
}

const diffLines = computed(() => {
  const patch = props.patch;

  if (patch.type === "create_file") {
    const lines = (patch.content || "").split("\n");
    return lines.map((line, i) => ({
      type: "added" as const,
      content: line,
      newLineNum: i + 1,
    }));
  }

  if (patch.type === "delete_file") {
    return [
      { type: "removed" as const, content: "(文件已删除)", oldLineNum: 1 },
    ];
  }

  if (patch.type === "update_file") {
    const oldLines = (patch.oldContent || "").split("\n");
    const newLines = (patch.newContent || "").split("\n");
    const result: DiffLine[] = [];

    const maxLen = Math.max(oldLines.length, newLines.length);
    let oldIdx = 0;
    let newIdx = 0;

    for (let i = 0; i < maxLen; i++) {
      const oldLine = oldIdx < oldLines.length ? oldLines[oldIdx] : undefined;
      const newLine = newIdx < newLines.length ? newLines[newIdx] : undefined;

      if (oldLine !== undefined && newLine !== undefined && oldLine === newLine) {
        result.push({
          type: "context",
          content: oldLine,
          oldLineNum: oldIdx + 1,
          newLineNum: newIdx + 1,
        });
        oldIdx++;
        newIdx++;
      } else {
        if (oldLine !== undefined) {
          result.push({
            type: "removed",
            content: oldLine,
            oldLineNum: oldIdx + 1,
          });
          oldIdx++;
        }
        if (newLine !== undefined) {
          result.push({
            type: "added",
            content: newLine,
            newLineNum: newIdx + 1,
          });
          newIdx++;
        }
      }
    }

    return result;
  }

  if (patch.type === "rename_file") {
    return [
      {
        type: "context" as const,
        content: `${patch.oldPath} → ${patch.newPath}`,
      },
    ];
  }

  return [];
});

const stats = computed(() => {
  const added = diffLines.value.filter((l) => l.type === "added").length;
  const removed = diffLines.value.filter((l) => l.type === "removed").length;
  return { added, removed };
});

const patchTitle = computed(() => {
  switch (props.patch.type) {
    case "create_file":
      return `📄 创建: ${props.patch.path}`;
    case "delete_file":
      return `🗑️ 删除: ${props.patch.path}`;
    case "update_file":
      return `✏️ 修改: ${props.patch.path}`;
    case "rename_file":
      return `📛 重命名: ${props.patch.oldPath} → ${props.patch.newPath}`;
    default:
      return props.patch.path;
  }
});
</script>

<template>
  <div class="diff-viewer">
    <div class="diff-header">
      <span class="diff-title">{{ patchTitle }}</span>
      <span class="diff-stats">
        <span v-if="stats.added" class="stat-added">+{{ stats.added }}</span>
        <span v-if="stats.removed" class="stat-removed">-{{ stats.removed }}</span>
      </span>
    </div>
    <div class="diff-body">
      <table class="diff-table">
        <tbody>
          <tr
            v-for="(line, idx) in diffLines"
            :key="idx"
            :class="['diff-line', line.type]"
          >
            <td class="line-num old-num">
              {{ line.type !== "added" ? line.oldLineNum ?? "" : "" }}
            </td>
            <td class="line-num new-num">
              {{ line.type !== "removed" ? line.newLineNum ?? "" : "" }}
            </td>
            <td class="line-prefix">
              {{ line.type === "added" ? "+" : line.type === "removed" ? "-" : " " }}
            </td>
            <td class="line-content">{{ line.content }}</td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>

<style scoped>
.diff-viewer {
  border: 1px solid var(--glass-border);
  border-radius: var(--radius-md);
  overflow: hidden;
  margin-bottom: 8px;
}

.diff-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 6px 12px;
  background: var(--glass-bg);
  border-bottom: 1px solid var(--glass-border-subtle);
}

.diff-title {
  font-size: 0.8rem;
  color: var(--text-main);
  font-weight: 500;
}

.diff-stats {
  display: flex;
  gap: 8px;
  font-size: 0.75rem;
  font-weight: 600;
}

.stat-added {
  color: var(--accent-green);
}

.stat-removed {
  color: var(--accent-red);
}

.diff-body {
  max-height: 300px;
  overflow-y: auto;
  font-family: var(--font-mono, "Consolas", "Monaco", monospace);
  font-size: 0.8rem;
  line-height: 1.5;
}

.diff-table {
  width: 100%;
  border-collapse: collapse;
}

.diff-line td {
  padding: 0 8px;
  white-space: pre;
  vertical-align: top;
}

.line-num {
  width: 40px;
  min-width: 40px;
  text-align: right;
  color: var(--text-muted);
  opacity: 0.5;
  user-select: none;
  padding: 0 4px;
  font-size: 0.7rem;
}

.line-prefix {
  width: 16px;
  min-width: 16px;
  text-align: center;
  user-select: none;
  font-weight: 600;
}

.line-content {
  color: var(--text-main);
}

.diff-line.context {
  background: transparent;
}

.diff-line.context .line-prefix {
  color: var(--text-muted);
  opacity: 0.3;
}

.diff-line.added {
  background: rgba(34, 197, 94, 0.08);
}

.diff-line.added .line-prefix {
  color: var(--accent-green);
}

.diff-line.added .line-content {
  color: var(--accent-green);
}

.diff-line.removed {
  background: rgba(239, 68, 68, 0.08);
}

.diff-line.removed .line-prefix {
  color: var(--accent-red);
}

.diff-line.removed .line-content {
  color: var(--accent-red);
}

.diff-line.empty td {
  background: var(--glass-bg);
  height: 20px;
}
</style>
