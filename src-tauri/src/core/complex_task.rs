//! # complex_task.rs — 复杂任务/方案内容的正则检测器
//!
//! 方案审批流程的两道确定性检测，与 LLM 的语义判断互补：
//!
//! - 前置（用户输入）：`is_complex_task()` 识别"需要先出方案审批"的复杂任务
//!   表达，命中则 pipeline 首轮强制切换 Plan 模式。设计定位是廉价的前置快路径
//!   （零延迟、确定性），不是完整的意图分类器；漏报由 mode/edit.md 的模型
//!   动态判断兜底，误判可按 mode/plan.md 的降级条件自行切回 Edit。
//! - 后置（LLM 输出）：`detect_plan_in_text()` 检测模型是否绕过 ProposePlan
//!   工具、把方案写进了回复正文，命中则 pipeline 重定向到方案审批流程。
//!
//! ## Key Exports
//! - `is_complex_task()`: 判断用户输入是否为需要方案审批的复杂任务（布尔）
//! - `detect_plan_in_text()`: 判断 LLM 纯文本输出是否包含方案/计划内容（布尔）
//!
//! ## Constraints
//! - 前置只做典型表达的前置抢跑，不追求覆盖面
//! - 后置要求"关键词 + 结构特征"同时命中，避免把普通列举误判为方案

use regex::Regex;
use std::sync::LazyLock;

// ============================================================================
// 前置检测：用户输入是否为复杂任务（pipeline 首轮强制方案审批的依据）
// ============================================================================

// ----------------------------------------------------------------------------
// 复杂项目/方案审批关键词：匹配需要先规划再执行的项目级任务
// ----------------------------------------------------------------------------
static COMPLEX_TASK_KEYWORDS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let patterns = [
        r"(?i)(先|先不要|不要直接).*(方案|计划|审批|审阅)",
        r"(?i)(提交|生成|制定|提出).*(方案|计划).*(审批|审阅|确认)",
        r"(?i)(完整|最小可用|MVP).*(项目|系统|应用|app|project|system)",
        r"(?i)(创建|新建|开发|实现|搭建).*(项目|系统|应用|前端|后端|API)",
        r"(?i)(plan|proposal|approve|review).*(before|first|then)",
        r"(?i)(create|build|implement|develop).*(project|system|app|frontend|backend|api)",
    ];
    patterns.iter().filter_map(|p| Regex::new(p).ok()).collect()
});

/// 判断输入是否为需要方案审批的复杂任务。
///
/// 返回 true 时，pipeline 会把本回合强制切到 Plan 模式并要求 ProposePlan；
/// 其余一切输入（原 Chat/Question/Action/Unclear 的区分）对系统行为无影响，
/// 统一返回 false 交给主 LLM。
pub fn is_complex_task(input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return false;
    }
    COMPLEX_TASK_KEYWORDS.iter().any(|p| p.is_match(trimmed))
}

// ============================================================================
// 后置检测：LLM 纯文本输出是否绕过 ProposePlan 写了方案（响应后置拦截的依据）
// ============================================================================

/// 方案内容的**结构标签词**：只有真正描述方案骨架的词才算，用于确认
/// "这段文本的意图是提交一份方案"。
///
/// ⚠️ 刻意剔除了一批"元讨论词"——它们描述的是**方案流程本身**，而非方案内容：
/// `实施方案` / `执行方案` / `技术方案` / `整体方案` / `开发计划` / `实施步骤` /
/// `项目规划` / `架构设计` / `我来帮你搭建|开发|实现`。
/// 典型误判案例（2026-09-19 事故）：模型回复"我提交一份**实施方案**供您审批 →
/// 您批准后我切回编辑模式"，正文是在**说明流程**，却被旧关键词表命中
/// `实施方案` + 编号列表结构，连续多轮被判为"输出计划"而陷入拦截死循环。
static PLAN_KEYWORDS: LazyLock<Vec<&str>> = LazyLock::new(|| {
    vec![
        // 任务拆分类：方案的骨架，必然带依赖/编号等硬特征
        "任务分解",
        "任务拆解",
        "子任务分配",
        "依赖关系",
        "步骤拆解",
    ]
});

/// 方案的**硬特征**：ProposePlan 内容规范（prompt/mode/plan.md §内容规范）里
/// 定义的核心必填项在人话文本里的可检测形态——依赖记号、任务编号、目录树。
///
/// 这是新口径的核心：要求文本里**出现方案规范特有的结构性标记**，
/// 而不是随便一个 `1. xxx` 编号列表。任何普通说明文都能写编号列表，
/// 但不会去画任务依赖图或目录树。
static PLAN_HARD_SIGNALS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let patterns = [
        // 任务依赖记号：blocked_by / depends_on / [#N] / ← / 并行标注
        r"(?i)blocked_by",
        r"(?i)depends_on",
        r"\[#\d+\]",
        // 任务编号：#1 初始化项目 / #2 创建路由
        r"#\d+\s*[\u4e00-\u9fa5A-Za-z]",
        // 目录树：├── / └── / │
        r"[├└]──",
        // "← blocked_by" / "← [#1]" 这类依赖箭头（ASCII 箭头 → 也算）
        r"←\s*\[?#?\d",
    ];
    patterns.iter().filter_map(|p| Regex::new(p).ok()).collect()
});

/// 判断 LLM 的纯文本输出是否**真的在提交一份方案**（而非说明流程/普通列举）。
///
/// 判定条件（三选二式收紧后的口径）：
/// 1. 命中**方案结构标签**（任务拆解/依赖关系等，已剔除元讨论词）；
/// 2. 且文本中存在至少一处**方案硬特征**（依赖记号/任务编号/目录树）。
///
/// 两者缺一不可：
/// - 只有标签词 → 可能只是在讨论"任务该怎么拆"，还没给出方案实体；
/// - 只有硬特征 → 可能是普通代码/目录说明，与方案审批无关。
///
/// 与 `task_breakdown` 规范的关系：`task_breakdown` 要求每个任务项带
/// `subject`（格式 `#序号 任务描述`）与 `depends_on`。真方案的 Markdown 正文
/// 会把这些结构化信息以「`#1 xxx` + `← blocked_by: [#2]`」的形式呈现，
/// 上述硬特征正是对这些必填项的检测。
pub fn detect_plan_in_text(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }

    let keyword_hit = PLAN_KEYWORDS.iter().any(|kw| text.contains(kw));
    if !keyword_hit {
        return false;
    }

    PLAN_HARD_SIGNALS.iter().any(|p| p.is_match(text))
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ── 前置：is_complex_task ──

    #[test]
    fn test_complex_task_expressions() {
        assert!(is_complex_task(
            "请先提交一份可审批的实施方案，然后创建一个完整最小可用项目"
        ));
        assert!(is_complex_task(
            "在桌面创建一个包含前端和后端的任务管理系统"
        ));
        assert!(is_complex_task("先给个方案再动手"));
    }

    #[test]
    fn test_typical_inputs_are_not_complex() {
        // 提问 / 闲聊 / 短输入 / 普通操作：不做区分，一律交给主 LLM
        assert!(!is_complex_task("Rust的ownership是什么"));
        assert!(!is_complex_task("你好"));
        assert!(!is_complex_task("1"));
        assert!(!is_complex_task("666"));
        assert!(!is_complex_task("帮我创建一个文件"));
        assert!(!is_complex_task(""));
    }

    #[test]
    fn test_noun_combo_not_complex() {
        // "前端 + 接口"这类名词组合不再触发方案审批（原第 5 条正则已删）
        assert!(!is_complex_task("前端的接口怎么调用？"));
        assert!(!is_complex_task("介绍一下这个后端的数据库设计"));
    }

    // ── 后置：detect_plan_in_text ──

    #[test]
    fn test_detect_plan_with_task_graph() {
        // 规范形态的方案：结构标签词 + 任务编号 + 依赖记号
        assert!(detect_plan_in_text(
            "## 任务拆解与依赖关系\n#1 初始化后端项目\n#2 初始化前端项目\n#3 数据库 schema ← blocked_by: [#1]\n#4 /api/users 路由 ← blocked_by: [#3]"
        ));
    }

    #[test]
    fn test_detect_plan_with_directory_tree() {
        // 目录树 + 结构标签词：规范必填的「目录结构」项
        assert!(detect_plan_in_text(
            "任务分解如下，目录结构：\nsrc/\n├── main.rs\n└── api/\n    ├── users.rs\n    └── tasks.rs"
        ));
    }

    #[test]
    fn test_detect_plan_with_depends_on() {
        assert!(detect_plan_in_text(
            "依赖关系：任务 1 无依赖；任务 2 depends_on 任务 1。子任务分配完毕。"
        ));
    }

    #[test]
    fn test_detect_plan_step_breakdown() {
        assert!(detect_plan_in_text(
            "步骤拆解：\n#1 复现问题\n#2 定位根因 ← [#1]\n#3 修复实现 ← [#2]\n#4 回归测试 ← [#3]"
        ));
    }

    // ── 回归防护：2026-09-19 拦截死循环事故 ──

    #[test]
    fn test_no_false_positive_flow_explanation_with_keyword() {
        // 事故原文：正文在**说明方案流程**，命中旧关键词「实施方案」+ 编号列表，
        // 连续 4 轮被判为"输出计划"而陷入拦截死循环。新口径必须放行。
        assert!(!detect_plan_in_text(
            "可以，先生，但当前会话处于**规划模式**，本回合系统没有给我写文件的工具，所以我暂时**不能直接动手改代码**。\n\n实际操作路径有两种：\n\n1. **简单改动**（只涉及 1~2 个文件、位置明确）：我直接把工作模式切回编辑模式，然后定点修改即可。\n2. **复杂改动**（3 个以上文件、跨层调整或架构决策）：我先探索代码库，通过方案审批面板提交实施方案，您审批通过后我再切回编辑模式执行。\n\n您想改哪部分？"
        ));
    }

    #[test]
    fn test_no_false_positive_flow_explanation_variant() {
        // 事故第 2/3 轮变体：同样是"说明解除方式"的编号列表
        assert!(!detect_plan_in_text(
            "可以，但有个前提限制要向您说明：当前会话处于**规划模式**，系统没有给我开放文件写入工具。\n\n能做的和解除方式如下：\n\n1. **现在可做**：读取、探索代码库，定位要改的位置。\n2. **要走改动落地**：我提交一份实施方案（ProposePlan）供您审批 → 您批准后我切换到编辑模式。\n3. **如果是小改动**：我也可以直接切回编辑模式动手。\n\n您想改哪部分？"
        ));
    }

    #[test]
    fn test_no_false_positive_keyword_without_hard_signal() {
        // 有结构标签词但无任何硬特征：仍在讨论阶段，不算提交方案
        assert!(!detect_plan_in_text(
            "这个需求的任务拆解我建议先讨论一下，你觉得怎么分比较合理？"
        ));
    }

    #[test]
    fn test_no_false_positive_hard_signal_without_keyword() {
        // 有硬特征（目录树）但无结构标签词：可能是普通目录说明
        assert!(!detect_plan_in_text(
            "项目目录长这样：\nsrc/\n├── main.rs\n└── lib.rs"
        ));
    }

    #[test]
    fn test_no_false_positive_simple() {
        assert!(!detect_plan_in_text("好的，我已经修改了这个文件。"));
    }

    #[test]
    fn test_no_false_positive_question() {
        assert!(!detect_plan_in_text("你想要怎么实现这个功能？"));
    }

    #[test]
    fn test_no_false_positive_single_step() {
        assert!(!detect_plan_in_text("1. 运行 npm install 安装依赖"));
    }
}
