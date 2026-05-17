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

fn base_rules() -> Vec<PromptRule> {
    vec![
        PromptRule::new(PromptLevel::P0Critical, "基础规则", prompt!("base_p0.md")),
        PromptRule::new(PromptLevel::P1Important, "基础规则", prompt!("base_p1.md")),
        PromptRule::new(PromptLevel::P2Reference, "基础规则", prompt!("base_p2.md")),
    ]
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
        "chat" => vec![
            PromptRule::new(PromptLevel::P1Important, "当前模式：聊天",
                prompt!("mode/chat.md"),
            ),
        ],
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

pub fn get_system_prompt(audience: &str, work_mode: &str) -> String {
    let mut rules: Vec<PromptRule> = Vec::new();
    rules.extend(base_rules());
    rules.extend(audience_rules(audience));
    rules.extend(mode_rules(work_mode));
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

pub const MEMORY_AGENT_SYSTEM: &str = "你是记忆维护系统。分析对话，决定是否更新用户全局记忆。

你应该记录（跨项目通用）：
- 用户身份（名字、角色、职业阶段）
- 通用偏好（语言、编辑器、代码风格、交互方式）
- 工作习惯（喜欢先看方案再执行、偏好简洁回复等）
- 技术栈偏好（常用语言、框架倾向）
- 长期有效的环境信息（操作系统、常用工具路径）

绝对不要记录（这些属于当前会话/项目，会话结束时自然丢弃）：
- 当前项目的技术栈和端口号
- 当前任务的进度和状态
- 某个具体文件的路径或内容
- 当前 bug 的修复过程
- 任何只在本次会话中有意义的临时信息

规则:
1. 只记录跨会话仍然有用的信息
2. 项目相关的一律跳过（存 session_memory 即可）
3. 不确定时宁可少记，不要多记
4. 有新信息时更新记忆
2. 生成更新后的完整 Markdown 内容
3. 无新信息则不操作
4. 通过 update_memory 工具提交更新
";

pub const INTENT_CLASSIFIER_PROMPT_LIGHT: &str = r#"
Classify user intent into one category as JSON:
{"category": "CODE_READ|CODE_WRITE|CODE_REVIEW|TASK_EXECUTE|TASK_PLAN|TASK_CONTINUE|QUESTION|MEMORY_QUERY|SETTINGS|CHAT|DANGEROUS", "reasoning": "short why"}

Routing rules:
- DANGEROUS takes priority for destructive local actions such as deleting many files, rm -rf, formatting disks, dropping databases.
- CODE_READ/CODE_WRITE/CODE_REVIEW/TASK_EXECUTE/TASK_PLAN are for local files, code, software projects, commands, apps, websites, repos, databases, or other computer operations.
- Everyday requests such as writing an email, polishing copy, translating, summarizing pasted text, brainstorming, roleplay, or casual conversation are CHAT unless they clearly require local file/project tools.
- Knowledge questions, comparisons, explanations, and "how do I..." questions are QUESTION.
- MEMORY_QUERY is only for asking about prior conversation, saved memory, or something said earlier.
- SETTINGS is only for changing this app's configuration, model, theme, or preferences.
- Short replies like "继续"/"好的"/"yes" are TASK_CONTINUE if a task is active in context, otherwise CHAT.
- If the input is understandable but not tool-related, prefer CHAT or QUESTION over UNCLEAR.
Return JSON only.
"#;

#[cfg(test)]
mod debug_tests {
    use super::*;

    #[test]
    fn save_developer_edit_prompt() {
        let prompt = get_system_prompt("developer", "edit");
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
        get_system_prompt("developer", "edit")
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
    fn edit_rules_not_in_chat_prompt() {
        let chat = get_system_prompt("developer", "chat");
        assert!(!chat.contains("委派规范"));
        assert!(!chat.contains("复杂任务流程"));
        assert!(!chat.contains("先判断复杂度"));
    }

    #[test]
    fn chat_is_read_only() {
        let chat = get_system_prompt("user", "chat");
        assert!(chat.contains("只读"));
        assert!(chat.contains("不能修改"));
    }

    // ── 规则不为空 ──

    #[test]
    fn base_rules_not_empty() {
        let rules = base_rules();
        assert_eq!(rules.len(), 3, "base should have P0, P1, P2 rules");
        for rule in &rules {
            assert!(!rule.body.trim().is_empty(), "Rule '{}' has empty body", rule.title);
        }
    }

    // ── 文件存在性 ──

    #[test]
    fn all_prompt_files_exist() {
        let files: &[&str] = &[
            "prompts/base_p0.md",
            "prompts/base_p1.md",
            "prompts/base_p2.md",
            "prompts/audience/user.md",
            "prompts/audience/developer.md",
            "prompts/mode/edit.md",
            "prompts/mode/plan.md",
            "prompts/mode/chat.md",
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
