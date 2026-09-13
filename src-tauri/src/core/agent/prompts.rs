//! 系统提示词模块
//!
//! 定义各类代理的系统提示词，指导 LLM 的行为模式。
//! 提示词按三级分层（P0 / P1 / P2），各模块贡献规则，组装函数按级别排序渲染为 markdown。
//! 各规则内容存放在同级 `prompts/` 目录下的 .md 文件中，通过 `include_str!()` 编译期加载。

use std::borrow::Cow;

// ── 数据结构 ──

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PromptLevel {
    P0Critical,
    P1Important,
    P2Reference,
}

struct PromptRule {
    level: PromptLevel,
    title: &'static str,
    body: Cow<'static, str>,
}

impl PromptRule {
    fn new(level: PromptLevel, title: &'static str, body: impl Into<Cow<'static, str>>) -> Self {
        PromptRule { level, title, body: body.into() }
    }
}

fn render_prompt(rules: &[PromptRule]) -> String {
    let mut sorted: Vec<&PromptRule> = rules.iter().collect();
    sorted.sort_by_key(|r| r.level);

    let mut out = String::new();
    let mut current_level: Option<PromptLevel> = None;

    for rule in &sorted {
        if current_level != Some(rule.level) {
            current_level = Some(rule.level);
            match rule.level {
                PromptLevel::P0Critical => out.push_str("\n## P0 · 最高优先级（违反将导致严重错误）\n\n"),
                PromptLevel::P1Important => out.push_str("\n## P1 · 核心规范\n\n"),
                PromptLevel::P2Reference => out.push_str("\n## P2 · 参考信息（按需查阅）\n\n"),
            }
        }
        out.push_str(&format!("### {}\n{}\n", rule.title, rule.body));
    }

    out
}

// ── 包含路径宏 ──

macro_rules! prompt {
    ($path:literal) => {
        include_str!(concat!("prompts/", $path))
    };
}

// ── 基础规则 ──

/// 基础规则（所有模式共用）。
///
/// 第二步起"只读保护（chat）"已取消，工作模式只有 edit / plan，
/// 因此写操作/命令执行/任务编排类规则在所有模式下都注入。
fn base_rules(work_mode: &str) -> Vec<PromptRule> {
    let mut rules = vec![
        PromptRule::new(PromptLevel::P0Critical, "基础规则", prompt!("base_p0.md")),
        PromptRule::new(PromptLevel::P1Important, "基础规则", prompt!("base_p1.md")),
        PromptRule::new(PromptLevel::P2Reference, "基础规则", prompt!("base_p2.md")),
    ];
    let _ = work_mode;
    rules.push(PromptRule::new(
        PromptLevel::P0Critical,
        "编辑与审批纪律",
        prompt!("base_p0_write.md"),
    ));
    rules.push(PromptRule::new(
        PromptLevel::P1Important,
        "写操作与命令执行",
        prompt!("base_p1_write.md"),
    ));
    rules
}

// ── Audience 规则 ──

fn audience_rules(audience: &str) -> Vec<PromptRule> {
    match audience {
        "user" => vec![PromptRule::new(PromptLevel::P1Important, "回复风格 — 普通用户模式",
            prompt!("audience/user.md"),
        )],
        _ => vec![PromptRule::new(PromptLevel::P1Important, "回复风格 — 开发者模式",
            prompt!("audience/developer.md"),
        )],
    }
}

// ── Mode 规则 ──

fn mode_rules(work_mode: &str) -> Vec<PromptRule> {
    match work_mode {
        "plan" => vec![
            PromptRule::new(PromptLevel::P0Critical, "当前模式：规划",
                prompt!("mode/plan.md"),
            ),
        ],
        _ => vec![
            PromptRule::new(PromptLevel::P0Critical, "当前模式：编辑",
                prompt!("mode/edit.md"),
            ),
        ],
    }
}

// ── OS 规则 ──

fn os_rules() -> Vec<PromptRule> {
    if cfg!(target_os = "windows") {
        vec![PromptRule::new(PromptLevel::P2Reference, "系统环境", prompt!("os/windows.md"))]
    } else if cfg!(target_os = "macos") {
        vec![PromptRule::new(PromptLevel::P2Reference, "系统环境", prompt!("os/macos.md"))]
    } else {
        vec![PromptRule::new(PromptLevel::P2Reference, "系统环境", prompt!("os/linux.md"))]
    }
}

// ── 沙箱规则（动态内容，format! 注入）──

fn sandbox_rules(workspace: &str) -> Vec<PromptRule> {
    vec![PromptRule::new(PromptLevel::P1Important, "会话沙箱",
        format!(
            "当前工作目录已锁定为沙箱：'{}'\n- 沙箱内你可以自由操作：读写文件、创建目录、执行命令，没有限制\n- 当前工作目录就是沙箱根目录，所有相对路径都从这个目录出发\n- 只需注意：不要访问沙箱外的路径（系统会自动拦截），沙箱内的操作完全自由\n- 绝对禁止使用 cd / Set-Location / chdir 切换目录（会被拦截）\n- 如果在子目录执行命令，用 dir 参数：\n  - StartBackgroundCommand(command=\"npm install\", dir=\"{}/subdir\")\n  - RunCommand 会在沙箱根目录执行，可用 --prefix 指定子目录",
            workspace, workspace
        ),
    )]
}

// ── 组装入口 ──

pub fn get_system_prompt(
    audience: &str,
    work_mode: &str,
    workspace: Option<&std::path::Path>,
) -> String {
    let mut rules: Vec<PromptRule> = Vec::new();
    rules.extend(base_rules(work_mode));
    rules.extend(audience_rules(audience));
    rules.extend(mode_rules(work_mode));
    // 工作目录语义（沙箱规则 / 无沙箱说明）注入到 system：
    // system 每轮必发且是缓存前缀，放这里不会像以前那样在每条用户消息里重复一遍。
    if let Some(ws) = workspace {
        rules.extend(sandbox_rules(&ws.to_string_lossy()));
    } else {
        rules.push(PromptRule::new(
            PromptLevel::P2Reference,
            "工作目录",
            "当前会话未绑定工作区（无沙箱限制）：文件操作以进程当前目录为基准，不受沙箱约束。若用户要求针对某个项目工作，先用 SetWorkspace 绑定工作目录。",
        ));
    }
    rules.extend(os_rules());
    render_prompt(&rules)
}

// ── 子代理系统提示词 ──

pub fn get_subagent_system_prompt(cwd: &str, workspace: Option<&str>) -> String {
    let mut rules: Vec<PromptRule> = Vec::new();

    rules.push(PromptRule::new(PromptLevel::P0Critical, "子代理核心规则",
        prompt!("subagent.md"),
    ));

    rules.push(PromptRule::new(PromptLevel::P2Reference, "工作目录",
        format!(
            "工作目录: {}{}\n操作系统: {}",
            cwd,
            match workspace {
                Some(ws) => format!("\n沙箱: 文件操作限制在 '{}' 内", ws),
                None => String::new(),
            },
            if cfg!(target_os = "windows") { "Windows" } else if cfg!(target_os = "macos") { "macOS" } else { "Linux" },
        ),
    ));

    if let Some(ws) = workspace {
        rules.extend(sandbox_rules(ws));
    }
    rules.extend(os_rules());

    render_prompt(&rules)
}

// ── 独立提示词（不参与组装）──

pub const MEMORY_CURATOR_SYSTEM: &str = "你是「全局记忆」的整理者。全局记忆是一份跨会话、跨项目复用的用户档案，会被附加到之后每一轮对话的上下文里，因此必须短小、稳定、零过期信息。

你的职责只有整理：合并同类项、删除过期与不合格条目、压缩冗余表述。不要新增当前记忆里没有依据的事实。

## 保留标准
一条信息必须同时满足三条才可保留：
1. 跨会话：下个月开新会话依然成立
2. 跨项目：换个仓库、换个任务依然成立
3. 影响协作：它会改变你之后怎么配合用户（怎么说话、怎么动手、怎么取舍）
三条缺一，就删掉。

例外：用户明确要求「记住」的档案信息（年龄、籍贯、学历、所在地、经历、求职或学习方向等）不受第 3 条约束，必须保留——用户主动交代的档案本身就是该记的东西。

## 应保留的内容
- 身份：称呼、角色、职业阶段、学历、籍贯、所在地
- 交互偏好：语言、语气、详略程度、是否要先给方案再动手
- 工程偏好：常用语言与框架倾向、代码风格、依赖与工具取舍
- 审美与设计倾向
- 稳定环境事实：操作系统、默认 shell、路径书写习惯（仅当它直接决定你怎么干活）

## 必须删除
- 任何「当前/最近/正在进行」的任务状态：当前项目、当前任务、当前 bug、当前分支、当前进度（用户明确要求长期记住的求职/学习方向不在此列，应放进「身份」或「环境」）
- 会随时间失效的配置：正在使用的模型名、API 提供商、端口、版本号、订阅或额度状态
- 具体文件路径、目录结构、代码片段、命令输出
- 某次对话里临时提出的要求
- 与本机或宿主应用有关的琐事，除非它直接改变你的行为
- 重复或同义的条目（合并成一条，不要并列保留）

## 固定结构
按下面五节输出，不要自创小节；某节为空就整节省略：
# Global Memory
## 身份
## 交互偏好
## 工程偏好
## 审美偏好
## 环境

## 长度预算
- 每节最多 5 条，每条一行、不超过 40 字，写结论不写解释
- 全文控制在 1000 字以内
- 越影响协作的条目越靠前；超出预算时按 身份 > 交互偏好 > 工程偏好 > 审美偏好 > 环境 的优先级合并或删除

## 输出
只输出整理后的完整 Markdown 文件内容。不要解释、不要加代码块围栏、不要保留「(暂无记录)」这类占位文本。
";


#[cfg(test)]
mod debug_tests {
    use super::*;

    #[test]
    fn save_developer_edit_prompt() {
        let prompt = get_system_prompt("developer", "edit", None);
        let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("doc");
        std::fs::write(out_dir.join("assembled_prompt_developer_edit.txt"), prompt)
            .expect("failed to write");
    }
}

// ── 结构性断言测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    fn developer_edit_prompt() -> String {
        get_system_prompt("developer", "edit", None)
    }

    // ── 级别标签不重复 ──

    #[test]
    fn each_level_header_appears_once() {
        let prompt = developer_edit_prompt();
        assert_eq!(prompt.matches("## P0 · 最高优先级").count(), 1);
        assert_eq!(prompt.matches("## P1 · 核心规范").count(), 1);
        assert_eq!(prompt.matches("## P2 · 参考信息").count(), 1);
    }

    #[test]
    fn p0_before_p1_before_p2_in_prompt() {
        let prompt = developer_edit_prompt();
        let p0 = prompt.find("## P0 · 最高优先级").expect("P0 header");
        let p1 = prompt.find("## P1 · 核心规范").expect("P1 header");
        let p2 = prompt.find("## P2 · 参考信息").expect("P2 header");
        assert!(p0 < p1, "P0 must appear before P1");
        assert!(p1 < p2, "P1 must appear before P2");
    }

    // ── 模式不交叉污染 ──

    #[test]
    fn edit_prompt_keeps_write_and_orchestration_rules() {
        let edit = developer_edit_prompt();
        for needle in [
            "编辑纪律",
            "压缩文件处理",
            "工具选择指南 — 写操作与命令执行",
            "SwitchWorkMode",
            "UpdateTodos",
        ] {
            assert!(
                edit.contains(needle),
                "编辑模式的系统提示词应包含「{}」",
                needle
            );
        }
    }

    // ── 规则不为空 ──

    #[test]
    fn base_rules_not_empty() {
        for (mode, expected) in [("edit", 5), ("plan", 5)] {
            let rules = base_rules(mode);
            assert_eq!(rules.len(), expected, "base({}) rule count", mode);
            for rule in &rules {
                assert!(!rule.body.trim().is_empty(), "Rule '{}' has empty body", rule.title);
            }
        }
    }

    // ── 静态纪律与沙箱语义的落点 ──

    #[test]
    fn discipline_rules_live_in_system_prompt() {
        // 这两条以前重复塞在每条用户消息的 ctx 里，现在只出现一次、且在最权威的位置
        let prompt = developer_edit_prompt();
        assert!(prompt.contains("探索与审批纪律"));
        assert!(prompt.contains("ProposePlan 提交方案"));
    }

    #[test]
    fn sandbox_rules_only_when_workspace_bound() {
        let with_ws =
            get_system_prompt("developer", "edit", Some(std::path::Path::new("E:\\proj")));
        assert!(with_ws.contains("当前工作目录已锁定为沙箱"));
        assert!(with_ws.contains("E:\\proj"));
        assert!(with_ws.contains("禁止使用 cd"));

        let without = get_system_prompt("developer", "edit", None);
        assert!(!without.contains("当前工作目录已锁定为沙箱"));
        assert!(without.contains("未绑定工作区"));
    }

    // ── 文件存在性 ──

    #[test]
    fn all_prompt_files_exist() {
        let files: &[&str] = &[
            "prompts/base_p0.md",
            "prompts/base_p0_write.md",
            "prompts/base_p1.md",
            "prompts/base_p1_write.md",
            "prompts/base_p2.md",
            "prompts/audience/user.md",
            "prompts/audience/developer.md",
            "prompts/mode/edit.md",
            "prompts/mode/plan.md",
            "prompts/os/windows.md",
            "prompts/os/macos.md",
            "prompts/os/linux.md",
            "prompts/subagent.md",
        ];
        for path in files {
            let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/core/agent")
                .join(path);
            assert!(full.exists(), "Prompt file missing: {}", path);
        }
    }
}
