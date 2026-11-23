pub mod agent_registry;
pub mod allowance_store;
pub mod capabilities;
pub mod permission;
pub mod permission_audit_logger;
pub mod policy;
pub mod policy_guard;
pub mod registry;
pub mod tool_call_logger;
pub mod tool_search;

/// 工具调用的结构化返回结果。
///
/// 替代朴素的字符串关键词匹配来判断工具调用成败。
/// `dispatch_tool_call` 返回此类型，日志层直接读取 `is_error` 字段，
/// 不再扫描返回文本中是否包含 "error" 等字样（工具 schema 描述中
/// 天然会包含这些词，导致误判）。
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    /// 返回给 LLM 的文本内容
    pub output: String,
    /// 是否为错误结果（由各工具处理器显式标记）
    pub is_error: bool,
    /// 是否为安全/策略拦截（禁止执行，非工具自身失败）
    pub is_blocked: bool,
    /// 是否要求 Agent Loop 立即结束本轮循环。
    /// 用于 ProposePlan 等需要等待用户操作的工具：
    /// 提交方案后立即结束 turn，用户审批后开启新 turn。
    pub break_loop: bool,
    /// 本次调用消耗的 input token（默认 0，见 [`Self::with_usage`]）
    pub input_tokens: u64,
    /// 本次调用消耗的 output token（默认 0，见 [`Self::with_usage`]）
    pub output_tokens: u64,
}

impl ToolCallResult {
    /// 构造一个成功结果
    pub fn ok(output: String) -> Self {
        Self {
            output,
            is_error: false,
            is_blocked: false,
            break_loop: false,
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    /// 构造一个错误结果（工具执行失败）
    pub fn error(output: String) -> Self {
        Self {
            output,
            is_error: true,
            is_blocked: false,
            break_loop: false,
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    /// 构造一个安全拦截结果（策略拒绝，非工具失败）
    pub fn blocked(output: String) -> Self {
        Self {
            output,
            is_error: true,
            is_blocked: true,
            break_loop: false,
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    /// 构造一个成功结果，并要求 Agent Loop 立即结束本轮
    pub fn ok_break(output: String) -> Self {
        Self {
            output,
            is_error: false,
            is_blocked: false,
            break_loop: true,
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    /// 提取纯文本（丢弃错误标记），用于返回给 LLM
    pub fn into_output(self) -> String {
        self.output
    }

    /// 附加本次调用的 token 用量（链式调用）。
    ///
    /// 目前只有 RunSubagent 会产生非零值：子代理跑的是独立 Agent Loop，它把
    /// 自己的用量带回主线程只为了**展示**（监控面板、`agent_runs` 回合汇总）。
    /// ⚠️ 它不是账本：子代理每一轮已经自己把明细与会话累计写进了库
    /// （`subagent.rs::record_subagent_turn_usage`），上层若把这些值当增量
    /// 再入库一次，同一笔用量就会被计两遍。
    /// 按需工具统一经 ExecuteTool → dispatch 执行，若不在这条链路上透传，
    /// 展示层（以及回归对比用的 `req_sub_*` 诊断值）就拿不到这两个数。
    pub fn with_usage(mut self, input_tokens: u64, output_tokens: u64) -> Self {
        self.input_tokens = input_tokens;
        self.output_tokens = output_tokens;
        self
    }
}
