/**
 * 用户消息文本的共用处理：主对话（ChatArea）与子 Agent 轨迹（SubAgentChatPane）
 * 共用同一份「是否为长消息」判定。
 *
 * 折叠能力最初只写在 ChatArea 里（内联阈值 + 自己的模板），子 Agent 轨迹另起了一套
 * user-bubble 渲染，于是同一种消息在两个界面表现不一致。判定收拢到这里后两个入口
 * 引用同一个函数；气泡样式仍各自 scoped —— 两处底色与对齐不同，强行共用一个带样式的
 * 组件反而要把背景色、对齐方式都做成 props 透传，得不偿失。
 */

/** 取出用户消息的纯文本：历史消息可能夹带 HTML 片段，气泡按纯文本展示。 */
export function plainUserText(raw: unknown): string {
  return String(raw ?? '').replace(/<[^>]*>/g, '');
}

/**
 * 判定用户消息是否需要折叠：超过 6 行或 500 字符。
 * 阈值取自主对话的既有行为，改这里会同时影响主对话与子 Agent 轨迹。
 */
export function isLongUserText(text: string): boolean {
  if (!text) return false;
  // 先把连续空行压成一段再数行：聊天里常见的两三连空行不该把普通消息判成长消息。
  const plain = plainUserText(text).replace(/\n{3,}/g, '\n\n');
  return plain.split('\n').length > 6 || plain.length > 500;
}
