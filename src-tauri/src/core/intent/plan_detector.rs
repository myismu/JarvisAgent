use regex::Regex;
use std::sync::LazyLock;

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
    ]
});

pub fn detect_plan_in_text(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }

    let keyword_hit = PLAN_KEYWORDS.iter().any(|kw| text.contains(kw));
    if keyword_hit {
        let pattern_hits: usize = PLAN_PATTERNS
            .iter()
            .map(|p| p.find_iter(text).count())
            .sum();
        return pattern_hits >= 1;
    }

    let pattern_hits: usize = PLAN_PATTERNS
        .iter()
        .map(|p| p.find_iter(text).count())
        .sum();

    pattern_hits >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

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
