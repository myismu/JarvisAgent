//! # capabilities.rs — 会话能力清单（单一事实来源）
//!
//! 问题背景：工具定义列表是恒定的（为 prompt cache 命中），写操作类工具都是"延迟工具"，
//! 模型必须通过 GetToolCatalog → DiscoverTools → ExecuteTool 才能碰到它们。
//! 只靠运行时拦截（`should_block_write_tool`）意味着：模型要先把发现流程走完，
//! 才会在执行那一步吃到拦截 —— token 和时间已经烧掉了。
//!
//! 设计：把"当前模式能做什么"提升为一等公民，并且**由工具注册表反推**，
//! 而不是另外维护一套规则（两套规则迟早会漂移）。同一个清单同时喂给：
//! 1. 能力冲突快速判定：受限模式下要求了被禁能力 → 直接给确定性回复
//! 2. 第一轮动态上下文：模型一开始就知道自己的能力边界，无需试探
//! 3. 工具目录 / 搜索输出：一次调用就给出确定结论
//! 4. 运行时校验（`ToolRegistry::is_available`）：兜底拒绝
//!
//! 约束：`Capabilities::for_work_mode()` 的每个布尔值都必须通过 `is_available` 探测得出，
//! 任何"手写的能力表"都会与目录/拦截产生第二套真相。

use super::registry::ToolRegistry;

/// 会话能力清单。字段名即能力名，探测用的代表工具写在 `for_work_mode()` 里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// 读取文件/代码
    pub read: bool,
    /// 修改文件（WriteFile / EditFile / ApplyPatch / DeleteFile / RenameFile）
    pub write: bool,
    /// 执行命令（RunCommand / StartBackgroundCommand）
    pub run_commands: bool,
    /// 任务编排（CreateTask / UpdateTask / DeleteTask / UpdateTodos）
    pub orchestrate: bool,
    /// 派子代理（RunSubagent / RunSubagentsSequentially）
    ///
    /// 刻意与 `orchestrate` 分成两个字段：规划模式要禁的是"把活派出去让别人写"
    /// （子代理内层固定 `edit` 模式，写工具对它全量可见），而不是"列任务清单"。
    /// 合成一个字段的后果是规划模式连 `CreateTask` 一起报不可用，把规划能力也砍掉了。
    pub delegate: bool,
    /// 提交方案审批（ProposePlan）
    pub plan: bool,
    /// 切换工作模式（SwitchWorkMode）
    pub switch_mode: bool,
}

impl Capabilities {
    /// 按工作模式计算能力清单。
    ///
    /// 每个能力都通过 `ToolRegistry::is_available(代表工具, PROJECT_ACTION, 模式)` 探测：
    /// 能力清单是工具目录的**投影**，不可能与目录/拦截不一致。
    pub fn for_work_mode(work_mode: &str) -> Self {
        let allowed = |tool: &str| {
            ToolRegistry::global()
                .get(tool)
                .map(|def| ToolRegistry::is_available(def, "PROJECT_ACTION", work_mode))
                .unwrap_or(false)
        };
        Self {
            read: allowed("ReadFile"),
            write: allowed("WriteFile"),
            run_commands: allowed("RunCommand"),
            orchestrate: allowed("CreateTask"),
            delegate: allowed("RunSubagent"),
            plan: allowed("ProposePlan"),
            switch_mode: allowed("SwitchWorkMode"),
        }
    }

    /// 单行摘要，用于工具目录/搜索结果的"能力边界"结论
    pub fn summary_line(&self) -> String {
        let mark = |ok: bool| if ok { "可用" } else { "不可用" };
        format!(
            "读取文件 {} / 修改文件 {} / 执行命令 {} / 任务编排 {} / 派子代理 {} / 方案规划 {} / 模式切换 {}",
            mark(self.read),
            mark(self.write),
            mark(self.run_commands),
            mark(self.orchestrate),
            mark(self.delegate),
            mark(self.plan),
            mark(self.switch_mode),
        )
    }

    /// 注入第一轮上下文的完整能力声明。
    ///
    /// 这段文本的作用是"消灭探测"：模型在第一轮就知道哪些能力不存在，
    /// 不需要（也不允许）用 GetToolCatalog / DiscoverTools 去验证。
    pub fn context_block(&self) -> String {
        let mark = |ok: bool| if ok { "可用" } else { "不可用（本会话不存在对应工具）" };
        let mut out = String::from("<capabilities>\n【本会话能力边界 · 系统强制】\n");
        out.push_str(&format!("- 读取文件/代码：{}\n", mark(self.read)));
        out.push_str(&format!("- 修改文件（写入/编辑/删除/重命名）：{}\n", mark(self.write)));
        out.push_str(&format!("- 执行命令（含启动服务）：{}\n", mark(self.run_commands)));
        out.push_str(&format!("- 任务编排（建任务清单 / 待办）：{}\n", mark(self.orchestrate)));
        out.push_str(&format!("- 派子代理（RunSubagent / RunSubagentsSequentially）：{}\n", mark(self.delegate)));
        out.push_str(&format!("- 提交方案审批（ProposePlan）：{}\n", mark(self.plan)));
        out.push_str(&format!("- 切换工作模式（SwitchWorkMode）：{}\n", mark(self.switch_mode)));
        out.push_str(
            "标记为「不可用」的能力在本会话没有任何工具可实现：不要调用 GetToolCatalog / \
             DiscoverTools 去搜索它们，也不要尝试用其他工具绕行。用户要求这类操作时，\
             直接用一两句话说明当前限制与解除方式，然后停下。\n</capabilities>",
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::Capabilities;

    #[test]
    fn edit_mode_has_full_capabilities() {
        let caps = Capabilities::for_work_mode("edit");
        assert!(caps.read && caps.write && caps.run_commands);
        assert!(caps.orchestrate && caps.delegate && caps.plan && caps.switch_mode);
    }

    #[test]
    fn plan_mode_explores_but_does_not_write() {
        let caps = Capabilities::for_work_mode("plan");
        assert!(caps.read);
        assert!(!caps.write, "规划模式不能直接改文件");
        assert!(!caps.run_commands, "规划模式不能执行写命令");
        assert!(caps.plan, "规划模式必须能提交方案");
        // 规划模式必须能列任务清单，但不能把活派出去让别人写
        assert!(caps.orchestrate, "规划模式要能建任务清单");
        assert!(
            !caps.delegate,
            "规划模式禁止派子代理：子代理内层固定 edit 模式，等于绕过写保护"
        );
    }

    #[test]
    fn context_block_states_unavailable_capabilities_explicitly() {
        // 规划模式：写操作不可用，提示词必须明确写出来
        let block = Capabilities::for_work_mode("plan").context_block();
        assert!(block.contains("不可用（本会话不存在对应工具）"));
        assert!(block.contains("不要调用 GetToolCatalog"));
        // 子代理能力必须在提示词里显式声明不可用，否则模型会去试探
        assert!(block.contains("派子代理（RunSubagent / RunSubagentsSequentially）：不可用"));
    }
}
