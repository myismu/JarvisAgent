/**
 * # governanceEvents.ts — 权限确认与方案审批事件域
 *
 * 注册并处理权限请求队列（`permission-request` / `permission-resolved`）与方案审批流
 * （`plan-proposal-stream` / `plan-proposal` / `plan-document-updated`）。
 *
 * ## Key Exports
 * - `registerGovernanceEvents()`: 注册权限与方案审批域全部事件监听
 * - `hideSubmittedPlanFromChat()`: 将正文缓冲区中的方案原文替换为"已提交审批面板"提示
 *
 * ## Dependencies
 * - Internal: `./types`, `@/stores/session`, `@/types`
 */
import type { SessionViewState } from "../../stores/session";
import type { PlanProposal } from "../../types";
import type { AgentEventsContext, EventRegisterFn } from "./types";

function replacePlanTextWithNotice(source: string, planContent: string, notice: string): string {
  const content = planContent.trim();
  if (!content || !source.includes(content)) {
    return source;
  }
  return source.replace(content, notice);
}

/**
 * 把正文里的方案原文替换成一句"已提交到审批面板"的提示。
 *
 * `notice` 由调用方传入（已通过 i18n 翻译），避免英文界面下冒出硬编码中文。
 */
export function hideSubmittedPlanFromChat(
  view: SessionViewState,
  proposal: PlanProposal,
  notice: string,
): void {
  const content = proposal.content || "";
  view.contentBuffer = replacePlanTextWithNotice(view.contentBuffer, content, notice);
  view.tempBuffer = replacePlanTextWithNotice(view.tempBuffer, content, notice);

  for (const block of view.currentTurn.textBlocks) {
    if (block.kind !== "assistant") continue;
    block.content = replacePlanTextWithNotice(block.content, content, notice);
  }
  view.currentTurn.revision += 1;
}

export async function registerGovernanceEvents(
  on: EventRegisterFn,
  ctx: AgentEventsContext,
): Promise<void> {
  const { t, session, perm, syncActiveSessionView } = ctx;

  // permission
  await on("permission-request", (event) => {
    // 入队而不是覆盖：并行子代理可能同时发起多个权限请求
    perm.enqueuePermission(event.payload);
  });

  await on("permission-resolved", (event) => {
    const sid = event.payload.sessionId ?? session.activeSessionId;
    // 按 id 精确移除：一张卡的决议不能把排在它后面的请求一起抹掉
    perm.removePermission(sid, event.payload.id);
  });

  // 运行期用户咨询（AskUser 工具）
  await on("user-question-request", (event) => {
    perm.enqueueUserQuestion(event.payload);
  });

  await on("user-question-resolved", (event) => {
    const sid = event.payload.sessionId ?? session.activeSessionId;
    perm.removeUserQuestion(sid, event.payload.id);
  });

  // plan proposal stream (chunked)
  await on("plan-proposal-stream", (event) => {
    const sid = event.payload.sessionId ?? session.activeSessionId;
    if (sid) {
      perm.updatePlanProposalStreamingContent(sid, event.payload.content);
      session.markSessionReady(sid);
      syncActiveSessionView(sid, false);
    }
  });

  // plan proposal
  await on("plan-proposal", (event) => {
    const sid = event.payload.sessionId ?? session.activeSessionId;
    if (sid) {
      const view = session.getSessionView(sid);
      hideSubmittedPlanFromChat(
        view,
        event.payload,
        t("plan.submittedNotice", { title: event.payload.title }),
      );
      perm.finalizePlanProposal(sid, event.payload);
      perm.upsertPlanDocument(
        {
          id: event.payload.id,
          sessionId: sid,
          title: event.payload.title,
          content: event.payload.content,
          status: "pending",
          path: null,
          // 时间戳口径：毫秒（v16 起全项目 DB 统一毫秒，与后端 PlanDocument 一致）
          createdAt: Date.now(),
          updatedAt: Date.now(),
          decidedAt: null,
        },
        sid,
      );
      session.markSessionReady(sid);
      syncActiveSessionView(sid, true);
    }
  });

  // plan document updated
  await on("plan-document-updated", (event) => {
    perm.upsertPlanDocument(event.payload);
  });
}
