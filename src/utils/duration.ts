/**
 * # duration.ts — 时长文本格式化的**唯一来源**
 *
 * 前端有多处要显示「这轮跑了多久」（聊天气泡下方的实时秒数、Agent 面板的汇总、
 * 历史轮次的定论时长）。各处自己拼字符串必然出现「1分3秒」「1 分 3 秒」
 * 「63 秒」三种写法，所以统一到这一个函数。
 *
 * ## 职责边界
 * - **只负责时长**（一段已经确定的毫秒数 → 文案）；
 * - **不负责「多久以前」**——「3 分钟前」属于时间戳相对化，口径与格式化不同，
 *   不要塞进来。
 *
 * ## 显示口径
 * - 小于 1 分钟：`N 秒`
 * - 大于等于 1 分钟：`M 分 S 秒`；秒为 0 时只输出 `M 分`
 *
 * 秒数一律**向下取整**：不足 1 秒不算 1 秒，避免刚发起就显示「1 秒」。
 *
 * ## 文案语言
 * 当前按中文口径输出。若后续要支持英文界面，应在此接入 i18n
 * （`t('duration.minutes'...)`），不要在调用方各自拼接。
 */

/** 1 秒对应的毫秒数 */
const MS_PER_SECOND = 1_000;
/** 1 分钟对应的毫秒数 */
const MS_PER_MINUTE = 60_000;

/**
 * 把毫秒时长格式化成可读文案。
 *
 * @param ms 后端给出的时长（毫秒）。`null` / `undefined` / 非有限数 / 负数
 *           一律按 0 处理 —— 缺失数据不该显示成 `NaN 秒` 或负数。
 */
export function formatDurationMs(ms: number | null | undefined): string {
  const value = typeof ms === 'number' && Number.isFinite(ms) ? Math.max(0, ms) : 0;
  const totalSeconds = Math.floor(value / MS_PER_SECOND);

  if (value < MS_PER_MINUTE) {
    return `${totalSeconds} 秒`;
  }

  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return seconds === 0 ? `${minutes} 分` : `${minutes} 分 ${seconds} 秒`;
}
