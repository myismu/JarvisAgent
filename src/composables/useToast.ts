/**
 * # useToast.ts — 全局轻提示（气泡消息）
 *
 * 一处 `showToast()` 处处可用：把提示推进模块级队列，由 `<ToastHost />` 统一渲染成
 * 悬浮气泡，几秒后自动消失。用于"操作完成 / 失败"这类**不需要用户操作**的反馈。
 *
 * 为什么不用页内提示条：它会占据布局、把列表挤变形（侧边栏的「会话已重命名」就吃过
 * 这个亏）。气泡浮在内容之上，出现即走，不改变任何布局。
 *
 * ## Key Exports
 * - `ToastKind`: 提示语气（`info` / `error`）
 * - `ToastItem`: 队列条目（id / message / kind / duration）
 * - `toasts`: 只读队列，供 `ToastHost` 渲染
 * - `showToast()`: 推一条提示（同文案刷新而不堆叠）
 * - `dismissToast()` / `clearToasts()`: 手动关掉某条 / 清空
 *
 * ## Constraints
 * - 模块级单例：任何组件 import 后调用都作用在同一个队列上，不需要 provide/inject
 * - 只读消费方（`ToastHost`）只能读，增删一律走导出的函数
 */

import { readonly, ref } from 'vue';

export type ToastKind = 'info' | 'error';

export interface ToastItem {
  id: number;
  message: string;
  kind: ToastKind;
  /** 毫秒；到时自动移除 */
  duration: number;
}

/** 默认停留时长 */
const DEFAULT_DURATION = 3500;
/** 错误留久一点：用户要读完才知道该不该重试 */
const ERROR_DURATION = 5000;
/** 同时最多显示几条（多了会糊住界面，超出的挤掉最旧的） */
const MAX_VISIBLE = 3;

const items = ref<ToastItem[]>([]);
const timers = new Map<number, ReturnType<typeof setTimeout>>();
let nextId = 1;

/** 只读队列：渲染方只读，增删都走下面的函数 */
export const toasts = readonly(items);

export function dismissToast(id: number) {
  const timer = timers.get(id);
  if (timer) {
    clearTimeout(timer);
    timers.delete(id);
  }
  items.value = items.value.filter((item) => item.id !== id);
}

export function clearToasts() {
  for (const id of [...timers.keys()]) dismissToast(id);
  items.value = [];
}

/**
 * 推一条提示。
 *
 * 同文案去重：队列里已有同内容同语气的条目时，把它**挪到最新并重新计时**，
 * 而不是再堆一条（连点两次「恢复」、反复重命名都会触发这种场景）。
 */
export function showToast(message: string, kind: ToastKind = 'info', duration?: number) {
  const text = message.trim();
  if (!text) return;

  const existing = items.value.find((item) => item.message === text && item.kind === kind);
  if (existing) dismissToast(existing.id);

  const id = nextId++;
  const stay = duration ?? (kind === 'error' ? ERROR_DURATION : DEFAULT_DURATION);
  items.value = [...items.value, { id, message: text, kind, duration: stay }];

  const overflow = items.value.length - MAX_VISIBLE;
  if (overflow > 0) {
    for (const stale of items.value.slice(0, overflow)) dismissToast(stale.id);
  }

  timers.set(
    id,
    setTimeout(() => dismissToast(id), stay),
  );
}
