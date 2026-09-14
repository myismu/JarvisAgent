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

/// 方案内容的结构特征：分步骤、编号列表、阶段划分等
static PLAN_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let patterns = [
        r"第[一二三四五六七八九十\d]+[步章节部分]",
        r"Step\s*\d+",
        r"\d+\.\s*.{4,}",
        r"[一二三四五六七八九十]、\s*.{4,}",
        r"首先[，,].*然后[，,]",
        r"首先[，,].*最后[，,]",
        r"第一步.*第二步",
        r"Phase\s*\d+",
        r"阶段[一二三四五六七八九十\d]",
    ];
    patterns.iter().filter_map(|p| Regex::new(p).ok()).collect()
});

/// 方案内容的关键词：必须先命中其一，再叠加结构特征，降低误判
static PLAN_KEYWORDS: LazyLock<Vec<&str>> = LazyLock::new(|| {
    vec![
        "实施方案",
        "任务分解",
        "开发计划",
        "实施步骤",
        "执行方案",
        "架构设计",
        "技术方案",
        "项目规划",
        "我来帮你搭建",
        "我来帮你开发",
        "我来帮你实现",
        "整体方案",
        "分步实施",
        // 复杂任务拆解的特征标题
        "任务拆解",
        "依赖关系",
        "子任务分配",
        "执行计划",
        "步骤拆解",
    ]
});

/// 判断 LLM 的纯文本输出是否包含方案/计划内容。
///
/// 判定条件：命中任一方案关键词，且文本中存在至少一处结构特征
/// （分步骤/编号列表/阶段划分）。两者缺一不可——只有关键词可能是
/// 普通讨论，只有结构特征可能是普通列举。
pub fn detect_plan_in_text(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }

    let keyword_hit = PLAN_KEYWORDS.iter().any(|kw| text.contains(kw));
    if !keyword_hit {
        return false;
    }

    let pattern_hits: usize = PLAN_PATTERNS
        .iter()
        .map(|p| p.find_iter(text).count())
        .sum();

    pattern_hits >= 1
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
        assert!(is_complex_task("在桌面创建一个包含前端和后端的任务管理系统"));
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
    fn test_detect_plan_steps() {
        assert!(detect_plan_in_text("好的，我来帮你搭建这个项目。\n第一步，初始化后端\n第二步，创建数据库\n第三步，实现API"));
    }

    #[test]
    fn test_detect_plan_numbered() {
        assert!(detect_plan_in_text("实施方案如下：\n1. 创建项目结构\n2. 实现后端API\n3. 搭建前端页面"));
    }

    #[test]
    fn test_detect_plan_chinese_number() {
        assert!(detect_plan_in_text("开发计划：\n一、后端开发\n二、前端开发\n三、集成测试"));
    }

    #[test]
    fn test_detect_plan_keyword_with_step() {
        assert!(detect_plan_in_text("我来帮你开发这个系统。第一步是创建项目。"));
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
