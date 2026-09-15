// 时间线组件共享工具函数
export function formatRelativeTime(timestamp: number): string {
  const date = new Date(timestamp * 1000);
  const now = new Date();
  const diff = now.getTime() - date.getTime();

  if (diff < 60000) return "刚刚";
  if (diff < 3600000) return `${Math.floor(diff / 60000)} 分钟前`;
  if (diff < 86400000) return `${Math.floor(diff / 3600000)} 小时前`;

  return date.toLocaleDateString("zh-CN", {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/**
 * 文件操作 → SVG 图标路径（24x24 描边框，与全项目其它图标同规格）。
 *
 * 只保留路径数据、不含任何样式：颜色与尺寸由调用方容器决定。
 * 原本这里是 emoji（✏️📝📄🗑️📛📁），跨平台字体渲染不一致且与项目 SVG 图标体系混排，
 * 故统一改为可跟随 `currentColor` 的矢量路径。
 */
const FILE_OP_PATHS: Record<string, string[]> = {
  // 铅笔 — 编辑 / 修改
  edit: ["M17 3a2.85 2.85 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"],
  update: ["M17 3a2.85 2.85 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"],
  // 带文字行的文件 — 写入
  write: ["M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8Z", "M14 2v6h6", "M8 13h8", "M8 17h5"],
  // 空白文件 — 创建
  create: ["M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8Z", "M14 2v6h6"],
  // 垃圾桶 — 删除
  delete: ["M3 6h18", "M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2", "M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6", "M10 11v6", "M14 11v6"],
  // 标签 — 重命名
  rename: ["M20.59 13.41l-7.17 7.17a2 2 0 0 1-2.83 0L2 12V2h10l8.59 8.59a2 2 0 0 1 0 2.82Z", "M7 7h.01"],
  // 快照 Patch 的类型名（DiffViewer 用）——与上面同义，避免两处各写一份路径
  create_file: ["M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8Z", "M14 2v6h6"],
  delete_file: ["M3 6h18", "M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2", "M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6", "M10 11v6", "M14 11v6"],
  update_file: ["M17 3a2.85 2.85 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"],
  rename_file: ["M20.59 13.41l-7.17 7.17a2 2 0 0 1-2.83 0L2 12V2h10l8.59 8.59a2 2 0 0 1 0 2.82Z", "M7 7h.01"],
};

/** 取文件操作图标路径；未知操作返回空数组，由调用方决定不渲染 */
export function getFileOpPaths(opType: string): string[] {
  return FILE_OP_PATHS[opType] ?? [];
}

export function getFileOpLabel(opType: string): string {
  const labels: Record<string, string> = {
    edit: "编辑",
    update: "编辑",
    write: "写入",
    create: "创建",
    delete: "删除",
    rename: "重命名",
  };
  return labels[opType] || opType;
}
