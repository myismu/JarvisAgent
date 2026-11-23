/**
 * # tokenDisplay.ts — 归档页使用的 token 数量文本格式化
 *
 * 归档列表需要显示会话累计 token。存量组件（输入框读数环、上下文监控面板、
 * 子 Agent 面板、会话列表）各自持有一份格式函数，且口径并不一致
 * （`M` / `m`、是否带 `tok` 后缀）。本轮不强行统一，以免改动无关视图；
 * 新代码从这里取，存量迁移留作单独重构。
 *
 * ## 口径
 * - 小于 1000：原样输出整数（`842`）
 * - 1000 ~ 999_999：一位小数的 `k`（`3.4k`）
 * - 1000_000 及以上：一位小数的 `m`（`1.2m`）
 *
 * 不含 `tok` / `tokens` 后缀：单位文案属于展示层，由调用方用语言包拼
 * （例如 `t('archiveManager.tokenCount', { count: formatTokenCount(n) })`），
 * 硬编码在工具函数里会让英文/中文界面被迫共用同一句。
 *
 * ## 缺失值处理
 * `null` / `undefined` / 非有限数 / 负数一律当 0 —— 缺数据不该显示成
 * `NaNk` 或负数。0 在很多场景里另有含义（后端未上报），调用方自己决定隐藏或显示。
 */

/** 进位阈值：千 */
const THOUSAND = 1_000;
/** 进位阈值：百万 */
const MILLION = 1_000_000;

/**
 * 把 token 数格式化成紧凑的可读文本（不带单位）。
 *
 * @param value 后端给出的 token 数（会话累计值 / 单次用量均可）
 * @returns 例如 `0` / `842` / `3.4k` / `1.2m`
 */
export function formatTokenCount(value: number | null | undefined): string {
  const count = typeof value === 'number' && Number.isFinite(value) ? Math.max(0, value) : 0;
  if (count >= MILLION) return `${(count / MILLION).toFixed(1)}m`;
  if (count >= THOUSAND) return `${(count / THOUSAND).toFixed(1)}k`;
  return String(Math.round(count));
}
