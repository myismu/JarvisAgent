//! Lightweight subagent registry.
//!
//! This module keeps the agent-type contract separate from the tool registry:
//! agent definitions decide which tools a subagent may see, while ToolRegistry
//! remains the source of truth for tool schemas and read-only metadata.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use super::registry::ToolRegistry;

pub const IMPLEMENTATION_AGENT_ROLE: &str = "implementation";

/// 省略 `subagent_type` 时的缺省角色。
///
/// 角色收敛后只剩两个，缺省即执行角色 —— 但它**默认只读**（`read_only_default = true`），
/// 要写必须由 caller 显式传 `read_only: false`。所以"省略一切"拿到的仍是只读子代理，
/// 与收敛前的 `general` 行为一致。
pub const DEFAULT_AGENT_ROLE: &str = IMPLEMENTATION_AGENT_ROLE;

const GENERAL_TOOLS: &[&str] = &[
    "LoadSkill",
    "ListDirectory",
    "FindFiles",
    "SearchText",
    "SearchRepo",
    "ReadFile",
    "ReadFileSkeleton",
    "FindSymbol",
    "ReadSymbol",
    "FindReferences",
    "CodeSearch",
    "WriteFile",
    "EditFile",
    "EditNotebook",
    // 删除 / 改名 / 打补丁此前不在名额里，但 `subagent.md` 的 P0 硬规则**点名要求**用它们
    // （"写/改/删/改名用 WriteFile / EditFile / ApplyPatch / DeleteFile / RenameFile"）。
    // 后果（2026-09-21 实测）：子代理被告知"删除用 DeleteFile"却拿不到该工具，只能退回
    // RunCommand 去删，被 shell 安全检查直接拒绝；失败回传主 Agent 后再交代一遍仍是同样结果，
    // 最后只能主 Agent 自己动手删。
    //
    // 安全性已核实：三者都记录补丁进快照（file_tools/{patch,delete,rename}.rs →
    // record_patch_to_snapshot），删除另有 `.jarvis_trash` 软删除兜底；
    // 权限侧 DeleteFile 归 Delete 类，**两档都要人工弹卡**；只读子代理会被
    // resolve_tools 的 read_only 过滤挡掉。
    "ApplyPatch",
    "DeleteFile",
    "RenameFile",
    "RunCommand",
    // StartBackgroundCommand 不给子代理：后台服务由主 Agent 统一管理（registry.rs 意图过滤同口径），
    // CheckBackgroundCommand 保留——只读探测后台输出，不违反"统一管理启动"
    "CheckBackgroundCommand",
];

const VERIFICATION_TOOLS: &[&str] = &[
    "LoadSkill",
    "ListDirectory",
    "FindFiles",
    "SearchText",
    "SearchRepo",
    "ReadFile",
    "ReadFileSkeleton",
    "FindSymbol",
    "ReadSymbol",
    "FindReferences",
    "CodeSearch",
    "RunCommand",
    "CheckBackgroundCommand",
];

#[derive(Debug, Clone)]
pub struct AgentDefinition {
    pub agent_role: &'static str,
    pub when_to_use: &'static str,
    pub system_prompt: &'static str,
    pub tools: &'static [&'static str],
    /// 白名单之外**额外**要排除的工具。
    ///
    /// 语义：`tools` 是白名单，本字段是"从白名单里再挖掉几个"。所以填进来的名字若本来
    /// 就不在 `tools` 里，这条配置**是无效的**（交集为空、拦不住任何东西）。2026-09-21
    /// 清理过一条这样的：verification 曾 deny 四个写工具，而它的白名单里本就没有它们。
    pub disallowed_tools: &'static [&'static str],
    /// 按角色覆盖模型。目前五个角色**全是 `None`**（统一走 `cfg.main_model`）。
    ///
    /// 保留字段是因为 `run_subagent` 的解析链已经接好
    /// （`model_override` > `agent.model` > 主模型，见 `subagent.rs`），
    /// 将来要给某个角色固定模型时只需在这里填值，不必再动调用链。
    pub model: Option<&'static str>,
    /// 省略 `read_only` 参数时的缺省值。
    ///
    /// ⚠️ 两条调用路径的取值方式不同，**这是有意的、不是 bug**（2026-09-21 核实过）：
    /// - 模型直调 `RunSubagent`：省略 → 用本字段（见 `core/tools/mod.rs` 的 dispatch 分支）；
    /// - 调度器派发任务：**恒传 `false`**（`scheduler.rs` 三处）—— 调度器就是那个 "caller"，
    ///   任务图里的活本来就要落地，所以它显式放权。这正是 general 的 `when_to_use` 里
    ///   "unless the caller explicitly allows writes" 所指的情况。
    pub read_only_default: bool,
    pub max_turns: Option<usize>,
}

pub struct AgentRegistry {
    agents: HashMap<&'static str, AgentDefinition>,
    insertion_order: Vec<&'static str>,
}

static AGENT_REGISTRY: OnceLock<AgentRegistry> = OnceLock::new();

impl AgentRegistry {
    pub fn global() -> &'static AgentRegistry {
        AGENT_REGISTRY.get_or_init(|| {
            let mut registry = AgentRegistry {
                agents: HashMap::new(),
                insertion_order: Vec::new(),
            };

            // 角色从 5 个收敛到 2 个（2026-09-21）。被删的三个与保留的两个是什么关系：
            //
            // - `general` / `explore` / `review` 的工具集**逐字节相同**（都是只读那 12 个）：
            //   GENERAL_TOOLS 的只读子集恰好就是 READ_ONLY_RESEARCH_TOOLS，
            //   所以「general 缺省只读」时三者实得工具完全一致，差别只剩提示词里一句话
            //   和 max_turns。实测分布也印证：180 次运行里 review 0 次、explore 0.6%、
            //   general 1.1%（而占 92.8% 的 implementation 基本是调度器兜底、不是模型选的）。
            //   review 更是自 2026-05-20 起就被反思审查机制架空 ——
            //   `execute_review()` 自己构造 messages，压根不走 AgentRegistry。
            //
            // - 它们的场景现在由 **implementation + read_only: true** 表达：
            //   implementation 的缺省就是只读，所以"省略一切"拿到的仍是只读子代理，
            //   工具集与原来的 general/explore/review 逐字节相同。
            //
            // - `verification` **保留**：它那个"能跑测试命令、但改不了文件"的档位
            //   无法用 read_only 布尔表达 —— true 会连 RunCommand 一起滤掉，
            //   false 又放开了写文件，而验证的独立性正依赖这个中间档。
            registry.register(AgentDefinition {
                agent_role: "verification",
                when_to_use: "Verify behavior after changes by inspecting code and running targeted checks or tests.",
                system_prompt: "You are a verification subagent. Run targeted checks when useful, inspect failures, and report pass/fail evidence. Do not edit files.",
                tools: VERIFICATION_TOOLS,
                // 这里原先 deny 了 WriteFile / EditFile / EditNotebook / StartBackgroundCommand，
                // 但一个都不在 VERIFICATION_TOOLS 里 —— 白名单本就不含它们，deny 与白名单
                // 的交集恒为空，是条"看着在拦、实际拦不住任何东西"的配置（2026-09-21 清理）。
                // verification 拿不到写工具，靠的是白名单，不是这一行。
                disallowed_tools: &[],
                model: None,
                read_only_default: false,
                max_turns: Some(20),
            });

            registry.register(AgentDefinition {
                agent_role: IMPLEMENTATION_AGENT_ROLE,
                // 吸收了原 `general` 的语义：缺省只读，要写必须由 caller 显式放权。
                // 这样"省略 subagent_type + 省略 read_only"拿到的仍是只读子代理 ——
                // 安全默认没有因为删掉 general 而丢失（这一点在 auto_approve 档位下尤其要紧，
                // 那种配置下权限弹卡不拦，只读默认是最后一道软防护）。
                when_to_use: "General delegated work and concrete implementation. Defaults to read-only unless the caller explicitly allows writes.",
                system_prompt: "You are an implementation subagent. Complete the delegated task directly, make the requested changes, keep scope tight, and verify the result when practical.",
                tools: GENERAL_TOOLS,
                disallowed_tools: &[],
                model: None,
                read_only_default: true,
                max_turns: Some(50),
            });

            registry
        })
    }

    fn register(&mut self, agent: AgentDefinition) {
        if !self.agents.contains_key(agent.agent_role) {
            self.insertion_order.push(agent.agent_role);
        }
        self.agents.insert(agent.agent_role, agent);
    }

    pub fn get(&self, agent_role: &str) -> Option<&AgentDefinition> {
        self.agents.get(agent_role)
    }

    pub fn default_agent(&self) -> &AgentDefinition {
        self.get(DEFAULT_AGENT_ROLE)
            .expect("default subagent definition must exist")
    }

    pub fn available_types(&self) -> Vec<&'static str> {
        self.insertion_order.clone()
    }

    pub fn prompt_listing(&self) -> String {
        self.insertion_order
            .iter()
            .filter_map(|agent_role| self.agents.get(agent_role))
            .map(|agent| format!("- {}: {}", agent.agent_role, agent.when_to_use))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn resolve_tools(
        &self,
        agent: &AgentDefinition,
        read_only: bool,
    ) -> Vec<serde_json::Value> {
        let tool_registry = ToolRegistry::global();
        let deny: HashSet<&str> = agent.disallowed_tools.iter().copied().collect();
        let mut seen = HashSet::new();
        let mut schemas = Vec::new();

        for tool_name in agent.tools {
            let tool_name = *tool_name;
            if !seen.insert(tool_name) || deny.contains(tool_name) {
                continue;
            }
            let Some(tool) = tool_registry.get(tool_name) else {
                continue;
            };
            if !tool.is_enabled {
                continue;
            }
            if read_only && !tool.is_read_only {
                continue;
            }
            schemas.push(tool.schema.clone());
        }

        schemas
    }
}

pub fn normalize_agent_role(value: Option<&str>) -> &str {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_AGENT_ROLE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_agent_exists() {
        let registry = AgentRegistry::global();
        assert_eq!(registry.default_agent().agent_role, DEFAULT_AGENT_ROLE);
        assert!(registry.available_types().contains(&"implementation"));
    }

    /// 角色收敛后的不变量：只剩两个。
    ///
    /// 加角色之前先想清楚它是不是"用 read_only 布尔就能表达"——
    /// 被删掉的 general / explore / review 就是因为工具集逐字节相同（都是只读那 12 个），
    /// 三个名字在描述同一件事。
    #[test]
    fn only_two_roles_remain_after_convergence() {
        let registry = AgentRegistry::global();
        let mut types = registry.available_types();
        types.sort_unstable();
        assert_eq!(types, vec!["implementation", "verification"]);
    }

    /// **安全默认**：省略一切时（不传 `subagent_type`、不传 `read_only`）拿到的是只读子代理。
    ///
    /// 这条是删掉 general 之后刻意保住的语义 —— implementation 的 `read_only_default`
    /// 从 false 改成了 true。在 auto_approve 档位下权限弹卡不拦，只读默认是最后一道软防护。
    #[test]
    fn omitted_read_only_yields_read_only_subagent() {
        let registry = AgentRegistry::global();
        let agent = registry.default_agent();
        assert_eq!(agent.agent_role, IMPLEMENTATION_AGENT_ROLE);
        assert!(
            agent.read_only_default,
            "缺省角色必须默认只读：模型忘传参数时不该拿到能改文件的子代理"
        );

        let tools = registry.resolve_tools(agent, agent.read_only_default);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        // 工具集应与收敛前的 general / explore / review 逐字节相同
        assert!(names.contains(&"ReadFile"));
        assert!(names.contains(&"SearchText"));
        assert!(!names.contains(&"EditFile"));
        assert!(!names.contains(&"RunCommand"));
    }

    #[test]
    fn implementation_agent_can_include_mutating_tools() {
        let registry = AgentRegistry::global();
        let agent = registry.get("implementation").unwrap();
        let tools = registry.resolve_tools(agent, false);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();

        assert!(names.contains(&"EditFile"));
        assert!(names.contains(&"EditNotebook"));
        assert!(names.contains(&"RunCommand"));
        // 后台服务锁：子代理（含 implementation）不许启动后台服务，主 Agent 统一管理
        assert!(!names.contains(&"StartBackgroundCommand"));
    }

    #[test]
    fn read_only_filter_uses_tool_metadata() {
        let registry = AgentRegistry::global();
        let agent = registry.get("implementation").unwrap();
        let tools = registry.resolve_tools(agent, true);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();

        assert!(names.contains(&"ReadFile"));
        assert!(names.contains(&"SearchText"));
        assert!(!names.contains(&"EditFile"));
        assert!(!names.contains(&"EditNotebook"));
        assert!(!names.contains(&"StartBackgroundCommand"));
    }

    #[test]
    fn task_schema_exposes_typed_agent_fields() {
        let tool_registry = ToolRegistry::global();
        let task = tool_registry.get("RunSubagent").unwrap();
        let properties = &task.schema["input_schema"]["properties"];

        assert!(properties["description"].is_object());
        assert!(properties["subagent_type"].is_object());
        assert!(properties["model"].is_object());
        assert!(properties["read_only"].is_object());
    }

    /// 子代理提示词里**故意点名禁止**的工具（写法是"无权/不可用"，不是"要用"）。
    ///
    /// 除这些之外，提示词里出现的任何已注册工具名都必须真的授权给它。
    const PROMPT_NAMED_BUT_DENIED: &[&str] = &["StartBackgroundCommand"];

    /// 提示词与工具名单的一致性护栏（2026-09-21 事故的正面锁）。
    ///
    /// 事故：`subagent.md` 的 P0 硬规则点名"写/改/删/改名用 WriteFile / EditFile /
    /// ApplyPatch / DeleteFile / RenameFile"，但这三个里的 `ApplyPatch` / `DeleteFile` /
    /// `RenameFile` **不在** `GENERAL_TOOLS` 里 —— 子代理被告知"删除用 DeleteFile"却拿不到
    /// 这个工具，只能退回 RunCommand 去删、被 shell 安全检查直接拒绝；失败回传主 Agent 后
    /// 再交代一遍仍是同样结果，最后只能主 Agent 自己删。
    ///
    /// 这条测试把"提示词点名了工具、名单却没给"拦在 `cargo test` 阶段：
    /// 以后往提示词里加工具名，要么同时授权，要么显式进 `PROMPT_NAMED_BUT_DENIED`。
    /// 顺带覆盖另一种错：提示词里写了个**根本没注册**的工具名（改名/退役后忘了同步）——
    /// 那种名字不会出现在 `all_tool_names()` 里，因此不会被本测试看到，需要在评审时留意。
    #[test]
    fn every_tool_named_in_subagent_prompt_is_granted_or_explicitly_denied() {
        const DOC: &str = include_str!("../../agent/prompts/subagent.md");
        let granted = AgentRegistry::global().default_agent().tools;

        for name in ToolRegistry::global().all_tool_names() {
            if !DOC.contains(name) {
                continue;
            }
            assert!(
                granted.contains(&name) || PROMPT_NAMED_BUT_DENIED.contains(&name),
                "subagent.md 点名了 {name}，但它既不在授权名单（GENERAL_TOOLS）里，\
                 也没进 PROMPT_NAMED_BUT_DENIED —— 提示词与工具名单不一致"
            );
        }

        // 反向锁：标为"故意禁止"的必须真的没授权，防止这张名单腐烂成摆设
        for name in PROMPT_NAMED_BUT_DENIED {
            assert!(
                !granted.contains(name),
                "{name} 被标为「故意禁止」，却出现在授权名单里"
            );
        }
    }

    /// 反向锁：不该能改文件的角色，白名单里就不能有写工具。
    ///
    /// `verification` 是唯一需要这条锁的角色 —— 它 `read_only_default = false`（要跑命令），
    /// 所以"拿不到写工具"**完全靠白名单**，白名单一旦被顺手放宽就没别的拦阻了。
    #[test]
    fn verification_allowlist_stays_free_of_mutating_tools() {
        let agent = AgentRegistry::global().get("verification").unwrap();
        for tool in [
            "WriteFile",
            "EditFile",
            "EditNotebook",
            "ApplyPatch",
            "DeleteFile",
            "RenameFile",
            "StartBackgroundCommand",
        ] {
            assert!(
                !agent.tools.contains(&tool),
                "verification 不该拿到 {tool}：它 read_only_default=false，白名单是唯一的拦阻"
            );
        }
    }

    // ────────────────────────────────────────────────────────────────
    //  第二道护栏：提示词里的**工具名**是否都存在
    // ────────────────────────────────────────────────────────────────

    /// 提示词里合法出现的**非工具** PascalCase 词。
    ///
    /// ⚠️ 往这里加东西之前先自问：这个词是不是本该是个工具名？如果是，那它就该被注册，
    /// 而不是塞进白名单把测试糊过去 —— 白名单是给"平台名 / 语言名 / 概念词 / 示例代码里
    /// 的类型名"这类词准备的，不是给漏注册的工具兜底的。
    const NON_TOOL_PASCAL_WORDS: &[&str] = &[
        "PowerShell",   // 平台名：os/windows.md 通篇在讲 PowerShell 5.1 语法
        "TypeScript",   // 语言名：mode/edit.md 讲编译检查
        "SubAgent",     // 概念词，即中文的"子 Agent"（mode/edit.md）
        "TaskPriority", // 示例代码里的类型名：mode/plan.md 的委派 prompt 示例
    ];

    /// 全部提示词文件。路径只用于报错时定位。
    const PROMPT_SOURCES: &[(&str, &str)] = &[
        ("base_p0.md", include_str!("../../agent/prompts/base_p0.md")),
        (
            "base_p0_write.md",
            include_str!("../../agent/prompts/base_p0_write.md"),
        ),
        ("base_p1.md", include_str!("../../agent/prompts/base_p1.md")),
        (
            "base_p1_write.md",
            include_str!("../../agent/prompts/base_p1_write.md"),
        ),
        ("base_p2.md", include_str!("../../agent/prompts/base_p2.md")),
        (
            "audience/user.md",
            include_str!("../../agent/prompts/audience/user.md"),
        ),
        (
            "audience/developer.md",
            include_str!("../../agent/prompts/audience/developer.md"),
        ),
        ("mode/edit.md", include_str!("../../agent/prompts/mode/edit.md")),
        ("mode/plan.md", include_str!("../../agent/prompts/mode/plan.md")),
        ("os/windows.md", include_str!("../../agent/prompts/os/windows.md")),
        ("os/macos.md", include_str!("../../agent/prompts/os/macos.md")),
        ("os/linux.md", include_str!("../../agent/prompts/os/linux.md")),
        ("subagent.md", include_str!("../../agent/prompts/subagent.md")),
    ];

    /// 扫出"长得像工具名"的词：PascalCase（至少两段），且**前面不是连字符或单词字符**。
    ///
    /// 后半个条件是为了跳过 `Get-ChildItem` / `Invoke-RestMethod` 这类 PowerShell cmdlet ——
    /// 它们的后半段也符合 PascalCase 形态，但前面挂着连字符，不该被当成工具名。
    fn tool_like_words(text: &str) -> Vec<String> {
        let re = regex::Regex::new(r"(?:^|[^A-Za-z0-9_-])([A-Z][a-z]+(?:[A-Z][a-z]+)+)")
            .expect("正则写死在源码里，不该编译失败");
        re.captures_iter(text)
            .map(|cap| cap[1].to_string())
            .collect()
    }

    /// 提示词里出现的每个"像工具名"的词，都必须真的注册过。
    ///
    /// 补的是上一道护栏（`every_tool_named_in_subagent_prompt_is_granted_or_explicitly_denied`）
    /// 写在注释里的盲区：**提示词写了个根本没注册的工具名**（工具改名/退役后忘了同步）。
    /// 那种名字不在 `all_tool_names()` 里，只能反过来扫文本才看得见。
    ///
    /// 为什么值得单开一道：2026-09-21 那次盘点撞出的 5 条「文档与实现不符」里，
    /// "用数据库工具查询"就属这一类 —— 模型会照着一个**调不通的名字**去试，
    /// 而且因为提示词里就这么写着，它试错后也无从纠正。
    ///
    /// ⚠️ 它能保证的只有**名字存在**。参数级的错误描述（"RunCommand 有 dir 参数"而实现
    /// 没读、"PDF 用 pages 参数"而该参数不存在）机器判不了，仍需人工评审 ——
    /// 这两条本次都已单独修掉，见 `common.rs` 与 `shell_tools` 的改动。
    #[test]
    fn every_tool_like_word_in_prompts_is_registered() {
        let registered = ToolRegistry::global().all_tool_names();
        let mut offenders: Vec<String> = Vec::new();

        for (file, doc) in PROMPT_SOURCES {
            for word in tool_like_words(doc) {
                if registered.contains(&word.as_str())
                    || NON_TOOL_PASCAL_WORDS.contains(&word.as_str())
                {
                    continue;
                }
                offenders.push(format!("{file}: {word}"));
            }
        }

        assert!(
            offenders.is_empty(),
            "以下词长得像工具名，却不在注册表里：\n  {}\n\n\
             两种可能：\n\
             ① 工具已改名或退役，提示词忘了同步 —— 模型会照着一个调不通的名字去做；\n\
             ② 它本就不是工具（平台名 / 语言名 / 概念词 / 示例代码里的类型名），\n\
                那就加进 NON_TOOL_PASCAL_WORDS，并在注释里写清它是什么。",
            offenders.join("\n  ")
        );
    }
}
