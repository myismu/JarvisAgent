//! 系统提示词模块
//!
//! 定义各类代理的系统提示词，指导 LLM 的行为模式。
//! 提示词按三级分层（P0 / P1 / P2），各模块贡献规则，组装函数按级别排序渲染为 markdown。
//! 各规则内容存放在同级 `prompts/` 目录下的 .md 文件中，通过 `include_str!()` 编译期加载。

use std::borrow::Cow;
use std::path::PathBuf;

// ── 提示词文件注册表（唯一事实源）──
//
// 13 个提示词文件的元数据清单，服务于设置页的「提示词」可视化编辑：
// - 列表展示、分类分组、生效语义标注都以本表为准，**不扫描任何目录**；
// - `path` 同时是磁盘覆盖层文件名（data/prompts/<path>）与编译期内置文件名
//   （src/core/agent/prompts/<path>，经 `prompt!` 宏 include_str! 嵌入 exe）；
// - 分类是显式字段，不从文件夹路径派生——路径是源码组织概念，分类是 UI 语义，
//   今天对齐但解耦（挪文件夹不动 UI 分组）。
//
// 维护约定：加/删/改提示词文件 = 改本文件三处（md 本体、`prompt!` 引用、本表），
// `all_prompt_files_exist` 测试读本表做存在性断言，漏改编译期/测试期即暴露。

/// UI 分组（与设置页左侧列表一致）
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptCategory {
    /// 基础规则（全进 system）
    Base,
    /// 回复风格（按 audience 二选一进 system）
    Audience,
    /// 工作模式（进动态上下文，按会话模式二选一，下轮生效）
    Mode,
    /// 系统环境（按编译目标选一个进 system）
    Os,
    /// 子代理
    Subagent,
}

impl PromptCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            PromptCategory::Base => "base",
            PromptCategory::Audience => "audience",
            PromptCategory::Mode => "mode",
            PromptCategory::Os => "os",
            PromptCategory::Subagent => "subagent",
        }
    }
}

pub struct PromptFileMeta {
    /// 相对 prompts/ 的路径（如 "audience/user.md"）
    pub path: &'static str,
    /// UI 显示名
    pub display_name: &'static str,
    /// 一句话用途
    pub description: &'static str,
    pub category: PromptCategory,
    /// true = 进 system（会话内字节恒定，改动只对新会话生效）；
    /// false = 进动态上下文（每轮重发，改动下一轮即生效）。
    pub goes_into_system: bool,
}

pub const PROMPT_FILES: &[PromptFileMeta] = &[
    PromptFileMeta {
        path: "base_p0.md",
        display_name: "基础规则",
        description: "违反即出事故的硬禁令：权限纪律、工具调用格式、禁止读二进制/依赖目录",
        category: PromptCategory::Base,
        goes_into_system: true,
    },
    PromptFileMeta {
        path: "base_p0_write.md",
        display_name: "编辑与审批纪律",
        description: "改动范围、遵循现有风格、复杂任务先出方案等写操作硬纪律",
        category: PromptCategory::Base,
        goes_into_system: true,
    },
    PromptFileMeta {
        path: "base_p1.md",
        display_name: "探索工具选择",
        description: "文件探索的工具优先级：FindFiles → SearchRepo → FindSymbol → ReadFile",
        category: PromptCategory::Base,
        goes_into_system: true,
    },
    PromptFileMeta {
        path: "base_p1_write.md",
        display_name: "写操作与命令执行",
        description: "EditFile/WriteFile/ApplyPatch 的选择原则、启动服务、任务编排指南",
        category: PromptCategory::Base,
        goes_into_system: true,
    },
    PromptFileMeta {
        path: "base_p2.md",
        display_name: "信息获取原则",
        description: "渐进式探索：不自动深入、不过度操作",
        category: PromptCategory::Base,
        goes_into_system: true,
    },
    PromptFileMeta {
        path: "audience/user.md",
        display_name: "回复风格 — 普通用户",
        description: "面向普通用户的措辞与详略（与开发者版二选一注入）",
        category: PromptCategory::Audience,
        goes_into_system: true,
    },
    PromptFileMeta {
        path: "audience/developer.md",
        display_name: "回复风格 — 开发者",
        description: "面向开发者的专业措辞（与普通用户版二选一注入）",
        category: PromptCategory::Audience,
        goes_into_system: true,
    },
    PromptFileMeta {
        path: "mode/edit.md",
        display_name: "编辑模式规则",
        description: "编辑模式的行为规范（进动态上下文，改动下一轮即生效）",
        category: PromptCategory::Mode,
        goes_into_system: false,
    },
    PromptFileMeta {
        path: "mode/plan.md",
        display_name: "规划模式规则",
        description: "规划模式的行为规范（进动态上下文，改动下一轮即生效）",
        category: PromptCategory::Mode,
        goes_into_system: false,
    },
    PromptFileMeta {
        path: "os/windows.md",
        display_name: "系统环境 — Windows",
        description: "PowerShell 5.1 语法约束（三个平台按编译目标选一个注入）",
        category: PromptCategory::Os,
        goes_into_system: true,
    },
    PromptFileMeta {
        path: "os/macos.md",
        display_name: "系统环境 — macOS",
        description: "macOS/zsh 环境说明（三个平台按编译目标选一个注入）",
        category: PromptCategory::Os,
        goes_into_system: true,
    },
    PromptFileMeta {
        path: "os/linux.md",
        display_name: "系统环境 — Linux",
        description: "Linux/bash 环境说明（三个平台按编译目标选一个注入）",
        category: PromptCategory::Os,
        goes_into_system: true,
    },
    PromptFileMeta {
        path: "subagent.md",
        display_name: "子代理核心规则",
        description: "子代理的工具调用格式、验证纪律、禁止事项",
        category: PromptCategory::Subagent,
        goes_into_system: true,
    },
];

/// path 是否在注册表中（防路径穿越的唯一闸门）。
pub fn is_registered_prompt(path: &str) -> bool {
    PROMPT_FILES.iter().any(|m| m.path == path)
}

// ── 包含路径宏 ──
// 注意：macro_rules! 是文本顺序作用域，必须先定义后使用（embedded_prompt 依赖它）。
// include_str! 只接受编译期字面量，因此内置内容经 embedded_prompt 的 match 静态查表
// 分发——新增文件时在此加一个臂，漏加会被 registry_embedded_not_empty 测试抓住。

macro_rules! prompt {
    ($path:literal) => {
        include_str!(concat!("prompts/", $path))
    };
}

/// 内置版查表：注册表 path → 编译期嵌入的出厂默认文本。
/// 未注册 path 返回空串（调用方都过白名单，不会走到）。
pub fn embedded_prompt(path: &str) -> &'static str {
    match path {
        "base_p0.md" => prompt!("base_p0.md"),
        "base_p0_write.md" => prompt!("base_p0_write.md"),
        "base_p1.md" => prompt!("base_p1.md"),
        "base_p1_write.md" => prompt!("base_p1_write.md"),
        "base_p2.md" => prompt!("base_p2.md"),
        "audience/user.md" => prompt!("audience/user.md"),
        "audience/developer.md" => prompt!("audience/developer.md"),
        "mode/edit.md" => prompt!("mode/edit.md"),
        "mode/plan.md" => prompt!("mode/plan.md"),
        "os/windows.md" => prompt!("os/windows.md"),
        "os/macos.md" => prompt!("os/macos.md"),
        "os/linux.md" => prompt!("os/linux.md"),
        "subagent.md" => prompt!("subagent.md"),
        _ => "",
    }
}

/// 磁盘覆盖层目录：data/prompts/（与 app-config.json 同级，运行时数据不进 git）
pub fn prompt_disk_dir() -> PathBuf {
    crate::infra::config::data_paths::data_root().join("prompts")
}

fn prompt_disk_path(path: &str) -> PathBuf {
    prompt_disk_dir().join(path)
}

/// 解析提示词内容：**磁盘优先、内置兜底**。
///
/// - `data/prompts/<path>` 存在且非空 → 用磁盘版（用户自定义）；
/// - 否则 → 用编译期内置版（`prompt!` 宏 include_str! 嵌入的出厂默认）。
///
/// 数据目录未初始化（单测 / 启动极早期）时直接回落内置，不 panic。
/// 读盘成本：每次组装 system 时读 ≤13 个共 28KB 小文件，毫秒级，
/// 且组装只在每轮 turn 发生一次，不值得做缓存。
pub fn resolve_prompt(path: &'static str) -> Cow<'static, str> {
    match crate::infra::config::data_paths::try_data_root() {
        Some(root) => resolve_prompt_from(&root.join("prompts"), path),
        None => Cow::Borrowed(embedded_prompt(path)),
    }
}

/// 可注入磁盘目录的解析纯函数（生产走 `resolve_prompt`，测试用临时目录）。
pub fn resolve_prompt_from(disk_dir: &std::path::Path, path: &'static str) -> Cow<'static, str> {
    let embedded = embedded_prompt(path);
    match std::fs::read_to_string(disk_dir.join(path)) {
        Ok(content) if !content.trim().is_empty() => Cow::Owned(content),
        _ => Cow::Borrowed(embedded),
    }
}

/// 磁盘上是否存在该文件的用户自定义版。
pub fn has_disk_override(path: &str) -> bool {
    is_registered_prompt(path)
        && std::fs::read_to_string(prompt_disk_path(path))
            .map(|c| !c.trim().is_empty())
            .unwrap_or(false)
}

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
        PromptRule {
            level,
            title,
            body: body.into(),
        }
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
                PromptLevel::P0Critical => {
                    out.push_str("\n## P0 · 最高优先级（违反将导致严重错误）\n\n")
                }
                PromptLevel::P1Important => out.push_str("\n## P1 · 核心规范\n\n"),
                PromptLevel::P2Reference => out.push_str("\n## P2 · 参考信息（按需查阅）\n\n"),
            }
        }
        out.push_str(&format!("### {}\n{}\n", rule.title, rule.body));
    }

    out
}

// ── 基础规则 ──

/// 基础规则（所有模式共用）。
///
/// 第二步起"只读保护（chat）"已取消，工作模式只有 edit / plan，
/// 因此写操作/命令执行/任务编排类规则在所有模式下都注入。
fn base_rules(work_mode: &str) -> Vec<PromptRule> {
    let mut rules = vec![
        PromptRule::new(
            PromptLevel::P0Critical,
            "基础规则",
            resolve_prompt("base_p0.md"),
        ),
        PromptRule::new(
            PromptLevel::P1Important,
            "基础规则",
            resolve_prompt("base_p1.md"),
        ),
        PromptRule::new(
            PromptLevel::P2Reference,
            "基础规则",
            resolve_prompt("base_p2.md"),
        ),
    ];
    let _ = work_mode;
    rules.push(PromptRule::new(
        PromptLevel::P0Critical,
        "编辑与审批纪律",
        resolve_prompt("base_p0_write.md"),
    ));
    rules.push(PromptRule::new(
        PromptLevel::P1Important,
        "写操作与命令执行",
        resolve_prompt("base_p1_write.md"),
    ));
    rules
}

// ── Audience 规则 ──

fn audience_rules(audience: &str) -> Vec<PromptRule> {
    match audience {
        "user" => vec![PromptRule::new(
            PromptLevel::P1Important,
            "回复风格 — 普通用户模式",
            resolve_prompt("audience/user.md"),
        )],
        _ => vec![PromptRule::new(
            PromptLevel::P1Important,
            "回复风格 — 开发者模式",
            resolve_prompt("audience/developer.md"),
        )],
    }
}

// ── Mode 规则（供“上下文快照”使用，不再进 system）──

pub fn get_mode_prompt(work_mode: &str) -> Cow<'static, str> {
    // 返回 Cow 以支持磁盘覆盖（data/prompts/mode/*.md）：
    // mode 文件进动态上下文（每轮重发、不进缓存前缀），用户改动**下一轮即生效**。
    match work_mode {
        "plan" => resolve_prompt("mode/plan.md"),
        _ => resolve_prompt("mode/edit.md"),
    }
}

// ── OS 规则 ──

fn os_rules() -> Vec<PromptRule> {
    if cfg!(target_os = "windows") {
        vec![PromptRule::new(
            PromptLevel::P2Reference,
            "系统环境",
            resolve_prompt("os/windows.md"),
        )]
    } else if cfg!(target_os = "macos") {
        vec![PromptRule::new(
            PromptLevel::P2Reference,
            "系统环境",
            resolve_prompt("os/macos.md"),
        )]
    } else {
        vec![PromptRule::new(
            PromptLevel::P2Reference,
            "系统环境",
            resolve_prompt("os/linux.md"),
        )]
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
    // 模式规则不进入 system：system 必须字节恒定才能最大化前缀缓存命中。
    // 模式 / 记忆 / 项目结构等易变状态统一放进每回合的“上下文快照”。
    rules.push(PromptRule::new(
        PromptLevel::P0Critical,
        "上下文快照读取规则",
        "历史中的上下文快照（<context_snapshot>）是当时的现场状态。处理当前请求时，始终以 seq 最大（最新）的快照为准；旧快照仅作历史参考。当前回合的工作模式、能力、项目结构与用户画像都看最新快照。",
    ));
    // 工作目录语义（沙箱规则 / 无沙箱说明）注入到 system：
    // system 每轮必发且是缓存前缀，放这里不会像以前那样在每条用户消息里重复一遍。
    if let Some(ws) = workspace {
        rules.extend(sandbox_rules(&ws.to_string_lossy()));
    } else {
        rules.push(PromptRule::new(
            PromptLevel::P2Reference,
            "工作目录",
            "当前会话为非沙箱会话（未绑定项目目录）：没有目录边界，操作其他项目请直接使用绝对路径。\
             工作区没有\"切换\"工具；若用户要求针对某个项目持续工作，请让用户在界面打开该项目（会生成绑定该项目的沙箱会话）。",
        ));
    }
    rules.extend(os_rules());
    render_prompt(&rules)
}

// ── 子代理系统提示词 ──

pub fn get_subagent_system_prompt(cwd: &str, workspace: Option<&str>) -> String {
    let mut rules: Vec<PromptRule> = Vec::new();

    rules.push(PromptRule::new(
        PromptLevel::P0Critical,
        "子代理核心规则",
        resolve_prompt("subagent.md"),
    ));

    rules.push(PromptRule::new(
        PromptLevel::P2Reference,
        "工作目录",
        format!(
            "工作目录: {}{}\n操作系统: {}",
            cwd,
            match workspace {
                Some(ws) => format!("\n沙箱: 文件操作限制在 '{}' 内", ws),
                None => String::new(),
            },
            if cfg!(target_os = "windows") {
                "Windows"
            } else if cfg!(target_os = "macos") {
                "macOS"
            } else {
                "Linux"
            },
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
        // 「压缩文件处理」不在断言清单里：2026-09-21 该节作为 ReadFile 单工具的
        // 用法边界，从 base_p0.md 下沉进了 ReadFile 的 schema description
        // （见 file_tools/registry.rs 的 ToolDef 注释），提示词里不再有这段。
        for needle in [
            "编辑纪律",
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
                assert!(
                    !rule.body.trim().is_empty(),
                    "Rule '{}' has empty body",
                    rule.title
                );
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
        assert!(without.contains("非沙箱会话"));
        // SetWorkspace 已退役，提示词不得再教模型调用它
        assert!(!without.contains("SetWorkspace"));
    }

    // ── 文件存在性 ──

    #[test]
    fn all_prompt_files_exist() {
        // 清单唯一事实源是 PROMPT_FILES 注册表（与 UI 列表同源），此处遍历它断言
        // 每个内置 md 都真实存在于源码目录——加文件漏注册/注册了没文件都在此暴露
        assert!(!PROMPT_FILES.is_empty(), "注册表不能为空");
        for meta in PROMPT_FILES {
            let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/core/agent/prompts")
                .join(meta.path);
            assert!(full.exists(), "Prompt file missing: {}", meta.path);
        }
    }

    #[test]
    fn registry_paths_pass_whitelist() {
        // 注册表里的 path 必须全部通过白名单校验（save_prompt 的防穿越闸门同源）
        for meta in PROMPT_FILES {
            assert!(
                is_registered_prompt(meta.path),
                "注册表 path 未通过白名单: {}",
                meta.path
            );
        }
        assert!(!is_registered_prompt("../../Cargo.toml"));
        assert!(!is_registered_prompt("base_p0.md/../../x.md"));
    }

    #[test]
    fn registry_embedded_not_empty() {
        // 注册表每一项都必须有对应的内置内容（embedded_prompt 的 match 臂漏加在此暴露）
        for meta in PROMPT_FILES {
            assert!(
                !embedded_prompt(meta.path).trim().is_empty(),
                "内置内容为空：注册表项 {} 缺少 embedded_prompt match 臂",
                meta.path
            );
        }
    }

    // ── 磁盘覆盖解析（resolve_prompt_from：可注入目录的纯函数）──

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "jarvis_prompts_test_{}_{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create tmp dir");
        dir
    }

    #[test]
    fn disk_override_wins_over_embedded() {
        let dir = tmp_dir("override");
        let p = dir.join("base_p2.md");
        std::fs::write(&p, "用户自定义的 P2 内容").expect("write override");
        let resolved = resolve_prompt_from(&dir, "base_p2.md");
        assert!(matches!(resolved, Cow::Owned(_)), "磁盘存在时应返回 Owned");
        assert_eq!(&*resolved, "用户自定义的 P2 内容");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_disk_file_falls_back_to_embedded() {
        let dir = tmp_dir("missing");
        let resolved = resolve_prompt_from(&dir, "base_p2.md");
        assert!(
            matches!(resolved, Cow::Borrowed(_)),
            "无磁盘文件应返回内置 Borrowed"
        );
        // 内置版就是源码 md 的原文
        assert_eq!(&*resolved, include_str!("prompts/base_p2.md"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn blank_disk_file_falls_back_to_embedded() {
        // 空文件/纯空白 = 无效覆盖，回落内置（防止用户清空后模型收到空规则）
        let dir = tmp_dir("blank");
        std::fs::write(dir.join("base_p2.md"), "   \n  \n").expect("write blank");
        let resolved = resolve_prompt_from(&dir, "base_p2.md");
        assert!(
            matches!(resolved, Cow::Borrowed(_)),
            "空白磁盘文件应回落内置"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn nested_disk_override_works() {
        // 子目录文件（audience/os/mode）的覆盖路径拼接
        let dir = tmp_dir("nested");
        std::fs::create_dir_all(dir.join("audience")).expect("mkdir");
        std::fs::write(dir.join("audience/developer.md"), "- 自定义开发者风格").expect("write");
        let resolved = resolve_prompt_from(&dir, "audience/developer.md");
        assert_eq!(&*resolved, "- 自定义开发者风格");
        std::fs::remove_dir_all(&dir).ok();
    }
}
