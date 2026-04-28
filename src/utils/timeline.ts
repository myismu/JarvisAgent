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

export function getFileOpIcon(opType: string): string {
  const icons: Record<string, string> = {
    edit: "✏️",
    update: "✏️",
    write: "📝",
    create: "📄",
    delete: "🗑️",
    rename: "📛",
  };
  return icons[opType] || "📁";
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
