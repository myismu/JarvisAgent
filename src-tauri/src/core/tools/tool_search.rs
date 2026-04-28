// ToolSearch — 渐进式工具披露
// 核心工具始终携带完整 schema，延迟工具仅先暴露名称+简述，
// LLM 通过 search_tools 按需获取完整参数定义后再调用。

use serde_json::json;

/// 获取核心工具（始终带完整 schema，永不延迟）
pub fn get_core_tool_definitions() -> Vec<serde_json::Value> {
    vec![
        json!({
            "name": "search_tools",
            "description": "搜索并获取延迟加载工具的完整参数定义。在使用任何名称已知但参数未知的工具前，必须先调用此工具获取其完整 JSON Schema。支持 'select:ToolName1,ToolName2' 精确选择，或关键词搜索（如 'file read write'）。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "查询字符串。使用 'select:ToolName1,ToolName2' 精确选择指定工具，或使用空格分隔的关键词搜索（如 'file read'、'git'、'task create'）"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "最大返回结果数，默认 5"
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "get_system_info",
            "description": "获取系统关键信息。",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "load_skill",
            "description": "按名称加载专业技能知识。在你需要处理特定领域（如查阅API、审查代码）的不熟悉知识时使用。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "要加载的技能名称"}
                },
                "required": ["name"]
            }
        }),
    ]
}

/// 获取延迟工具列表 (名称, 简述)，按意图筛选
pub fn get_deferred_tool_list(intent: &str) -> Vec<(String, String)> {
    if intent == "CHAT" || intent == "MEMORY_QUERY" || intent == "QUESTION" {
        return vec![];
    }

    // 文件与系统工具
    let mut tools = vec![
        ("set_workspace".to_string(), "设置或更改全局工作区目录".to_string()),
        ("list_directory".to_string(), "列出指定目录下的所有文件和文件夹".to_string()),
        ("git_command".to_string(), "执行低风险的 git 操作（status/diff/log 等）".to_string()),
        ("run_shell".to_string(), "执行 Windows PowerShell 命令".to_string()),
        ("read_file".to_string(), "读取文件内容，支持按行号精确读取".to_string()),
        ("read_file_skeleton".to_string(), "提取文件结构骨架（类、函数签名及行号）".to_string()),
        ("write_file".to_string(), "写入文件内容".to_string()),
        ("edit_file".to_string(), "基于搜索与替换修改文件中的特定文本".to_string()),
        ("search_repo".to_string(), "在指定目录下全局搜索包含关键词的文本".to_string()),
    ];

    // 编排工具
    let orchestrator_tools = vec![
        ("task_create".to_string(), "创建持久化任务条目到任务看板".to_string()),
        ("task_update".to_string(), "更新任务状态或依赖关系".to_string()),
        ("task_list".to_string(), "列出所有任务及其状态概要".to_string()),
        ("task_summary".to_string(), "生成任务全景报告（进度、瓶颈、下一步）".to_string()),
        ("task_get".to_string(), "获取单个任务的完整详细信息".to_string()),
        ("task".to_string(), "产生具有干净上下文的子代理执行具体操作".to_string()),
        ("background_run".to_string(), "在后台执行长时间运行的命令".to_string()),
        ("check_background".to_string(), "检查后台任务的执行状态和输出".to_string()),
        ("compact".to_string(), "手动触发对话上下文压缩".to_string()),
        ("dream".to_string(), "主动触发记忆整理（Dream Agent）".to_string()),
        ("propose_plan".to_string(), "提交复杂任务实施方案给用户审阅".to_string()),
    ];

    if intent == "SUBAGENT" {
        tools.extend(orchestrator_tools);
        tools.retain(|(name, _)| name != "task" && name != "dream" && name != "compact");
        tools
    } else {
        // PROJECT_ACTION
        tools.extend(orchestrator_tools);
        tools
    }
}

/// 按名称获取一个延迟工具的完整 JSON Schema
pub fn get_deferred_tool_full_schema(name: &str) -> Option<serde_json::Value> {
    match name {
        "set_workspace" => Some(json!({
            "name": "set_workspace",
            "description": "设置或更改大模型当前运作的全局工作区（Working Directory）目录。跨大项目切换或是初始化指定项目目录时使用。由于会改变全局环境且会被系统持久化记住，必须使用绝对路径。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "工作区目录的绝对路径"}
                },
                "required": ["path"]
            }
        })),
        "list_directory" => Some(json!({
            "name": "list_directory",
            "description": "列出指定目录下的所有文件和文件夹。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "目录路径"}
                },
                "required": ["path"]
            }
        })),
        "git_command" => Some(json!({
            "name": "git_command",
            "description": "执行低风险的 git 操作（如 status, diff, log）。禁止执行修改历史或推送的操作。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "args": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "git 命令的参数列表，例如 [\"status\"] 或 [\"log\", \"-n\", \"5\"]"
                    }
                },
                "required": ["args"]
            }
        })),
        "run_shell" => Some(json!({
            "name": "run_shell",
            "description": "执行 Windows PowerShell 命令。（阻塞同步且高风险：所有命令都会在 PowerShell 环境下执行，支持管道。长周期运行或前端 dev server 服务端启动请使用 background_run 工具。请优先使用专用的只读工具）",
            "input_schema": {
                "type": "object",
                "properties": { "command": {"type": "string"} },
                "required": ["command"]
            }
        })),
        "read_file" => Some(json!({
            "name": "read_file",
            "description": "读取文件内容。支持语义化点读技术，可通过 start_line 和 end_line 获取特定代码块，避免 Context 过长。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "start_line": {"type": "integer", "description": "可选。起始行号（从 1 开始）"},
                    "end_line": {"type": "integer", "description": "可选。结束行号（包含）"}
                },
                "required": ["path"]
            }
        })),
        "read_file_skeleton" => Some(json!({
            "name": "read_file_skeleton",
            "description": "提取文件结构骨架（Skeleton）。快速扫描并返回文件的类、函数签名及其行号，结合 read_file 进行精确片段阅读。",
            "input_schema": {
                "type": "object",
                "properties": { "path": {"type": "string"} },
                "required": ["path"]
            }
        })),
        "write_file" => Some(json!({
            "name": "write_file",
            "description": "写入文件内容。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"]
            }
        })),
        "edit_file" => Some(json!({
            "name": "edit_file",
            "description": "基于搜索与替换修改文件中的特定文本片段。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_text": {"type": "string", "description": "要替换的确切旧文本内容"},
                    "new_text": {"type": "string", "description": "替换后的新文本内容"}
                },
                "required": ["path", "old_text", "new_text"]
            }
        })),
        "search_repo" => Some(json!({
            "name": "search_repo",
            "description": "在指定目录下全局搜索包含特定关键词的文本内容。自动忽略编译产物和静态资源。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "要搜索的确切关键词"},
                    "dir": {"type": "string", "description": "要搜索的目录路径，默认搜索整个项目根目录"}
                },
                "required": ["pattern"]
            }
        })),
        // 任务工具
        "task_create" => Some(json!({
            "name": "task_create",
            "description": "【仅记录】创建一个持久化任务条目到任务看板。注意：此工具仅创建一条记录，不会执行任何实际操作！要真正执行任务，请使用 task 工具委派子代理。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "subject": {"type": "string", "description": "任务简述"},
                    "description": {"type": "string", "description": "详细说明"}
                },
                "required": ["subject"]
            }
        })),
        "task_update" => Some(json!({
            "name": "task_update",
            "description": "更新任务状态或依赖关系。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "task_id": {"type": "integer"},
                    "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]},
                    "add_blocked_by": {"type": "array", "items": {"type": "integer"}},
                    "add_blocks": {"type": "array", "items": {"type": "integer"}}
                },
                "required": ["task_id"]
            }
        })),
        "task_list" => Some(json!({
            "name": "task_list",
            "description": "列出所有任务及其状态概要。",
            "input_schema": { "type": "object", "properties": {} }
        })),
        "task_summary" => Some(json!({
            "name": "task_summary",
            "description": "生成任务全景报告，包括进度、关键路径瓶颈、以及推荐的下一步待办任务。建议在开始新工作或完成重要任务后使用。",
            "input_schema": { "type": "object", "properties": {} }
        })),
        "task_get" => Some(json!({
            "name": "task_get",
            "description": "获取单个任务的完整详细信息。",
            "input_schema": {
                "type": "object",
                "properties": { "task_id": {"type": "integer"} },
                "required": ["task_id"]
            }
        })),
        "task" => Some(json!({
            "name": "task",
            "description": "【真正执行】产生一个具有干净上下文环境的子代理 (Subagent) 去实际执行探索或具体操作任务。这是唯一能让子代理实际干活的工具！主 Agent 必须使用此工具来委派文件读取、代码搜索和修改等具体工作，避免污染主对话上下文。与父进程共享文件系统但不共享对话历史。注意：每次只能委派一个子代理，不支持并行。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "要子代理完成的任务说明，越详细越好。包括你想要子代理返回什么数据。"},
                    "read_only": {"type": "boolean", "description": "是否以只读模式运行子代理。默认为 true。如果需要子代理修改文件、写代码或执行高风险命令，【必须】显式设置为 false，否则子代理将没有写入文件的权限！"}
                },
                "required": ["prompt"]
            }
        })),
        "background_run" => Some(json!({
            "name": "background_run",
            "description": "在后台执行长时间运行的命令（如启动前端 npm run dev、后端服务器等）。执行后立刻返回任务ID，不阻塞对话。长周期任务必须使用此工具！严禁使用 check_background 轮询等待！",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "要执行的具体命令（如 npm run dev）"},
                    "dir": {"type": "string", "description": "非常重要！必须提供命令执行的工作目录的绝对路径，绝对不要在 command 中手写 cd ！"}
                },
                "required": ["command", "dir"]
            }
        })),
        "check_background" => Some(json!({
            "name": "check_background",
            "description": "检查后台任务的执行状态和输出。仅当用户主动询问后台任务状态时才使用，严禁在自己的思考循环中连续轮询此工具！",
            "input_schema": {
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "后台任务 ID。如果留空则返回所有任务状态。"}
                }
            }
        })),
        "compact" => Some(json!({
            "name": "compact",
            "description": "手动触发对话上下文压缩。当对话上下文过长觉得需要清理或重置记忆时使用该工具。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "focus": { "type": "string", "description": "摘要时需要特别保留的重点方向" }
                }
            }
        })),
        "dream" => Some(json!({
            "name": "dream",
            "description": "主动触发记忆整理（Dream Agent）。将当前的零散碎片记忆提炼并合并进结构化用户画像中。",
            "input_schema": { "type": "object", "properties": {} }
        })),
        "propose_plan" => Some(json!({
            "name": "propose_plan",
            "description": "【方案审批工具】将实施方案提交给用户审阅。当面对复杂任务（涉及多步骤修改、架构变更等），必须使用此工具提交方案文档，等待用户确认后才能继续执行。方案内容使用 Markdown 格式。前端会以专门的预览面板展示方案，用户可以选择同意或拒绝。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "方案标题"},
                    "content": {"type": "string", "description": "方案正文（Markdown 格式），包含需求理解、变更范围、具体步骤、风险评估等"}
                },
                "required": ["title", "content"]
            }
        })),
        _ => None,
    }
}

/// 关键词搜索延迟工具，返回匹配的工具名列表
pub fn search_deferred_tools(
    query: &str,
    deferred_list: &[(String, String)],
    max_results: usize,
) -> Vec<String> {
    let query_lower = query.to_lowercase().trim().to_string();

    // select: 精确选择
    if let Some(select_query) = query_lower.strip_prefix("select:") {
        let requested: Vec<&str> = select_query
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        let mut found = Vec::new();
        for name in requested {
            // 不区分大小写匹配
            if let Some((exact_name, _)) = deferred_list
                .iter()
                .find(|(n, _)| n.to_lowercase() == name.to_lowercase())
            {
                if !found.contains(exact_name) {
                    found.push(exact_name.clone());
                }
            }
        }
        return found;
    }

    // 关键词搜索
    let terms: Vec<&str> = query_lower.split_whitespace().collect();
    let mut scored: Vec<(String, usize)> = deferred_list
        .iter()
        .map(|(name, desc)| {
            let name_lower = name.to_lowercase();
            let desc_lower = desc.to_lowercase();
            let mut score = 0usize;
            for term in &terms {
                if name_lower == *term {
                    score += 12;
                } else if name_lower.contains(term) {
                    score += 5;
                }
                if desc_lower.contains(term) {
                    score += 2;
                }
            }
            (name.clone(), score)
        })
        .filter(|(_, s)| *s > 0)
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.truncate(max_results);
    scored.into_iter().map(|(name, _)| name).collect()
}

/// 生成延迟工具名称列表上下文（注入到 system prompt 区域的用户消息中）
pub fn get_deferred_tools_context(intent: &str) -> String {
    let deferred = get_deferred_tool_list(intent);
    if deferred.is_empty() {
        return String::new();
    }

    let names: Vec<String> = deferred
        .iter()
        .map(|(name, desc)| format!("- **{}**: {}", name, desc))
        .collect();

    format!(
        "\n\n【延迟加载工具】（使用 search_tools 获取完整参数定义后才能调用）:\n{}\n",
        names.join("\n")
    )
}

/// search_tools 工具的处理函数
pub async fn handle_search_tools(
    input: &serde_json::Value,
    intent: &str,
) -> String {
    let query = input["query"].as_str().unwrap_or("");
    let max_results = input["max_results"]
        .as_u64()
        .unwrap_or(5)
        .clamp(1, 20) as usize;

    let deferred = get_deferred_tool_list(intent);

    if deferred.is_empty() {
        return "当前意图下没有可用的延迟加载工具。".to_string();
    }

    let matches = search_deferred_tools(query, &deferred, max_results);

    if matches.is_empty() {
        let all_names: Vec<String> = deferred.iter().map(|(n, _)| n.clone()).collect();
        return format!(
            "未找到匹配 '{}' 的工具。\n\n当前可用的延迟加载工具: {}\n\n请使用 'select:工具名' 精确选择，或使用关键词重新搜索。",
            query,
            all_names.join(", ")
        );
    }

    let mut result = format!("匹配到 {} 个工具，完整参数定义如下：\n", matches.len());

    for name in &matches {
        if let Some(schema) = get_deferred_tool_full_schema(name) {
            let json_str = serde_json::to_string(&schema).unwrap_or_default();
            result.push_str(&format!("\n<function>{}</function>", json_str));
        }
    }

    result.push_str("\n\n现在可以直接调用以上工具。");

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_select_exact() {
        let deferred = get_deferred_tool_list("PROJECT_ACTION");
        let result = search_deferred_tools("select:read_file,write_file", &deferred, 5);
        assert_eq!(result, vec!["read_file", "write_file"]);
    }

    #[test]
    fn test_search_select_case_insensitive() {
        let deferred = get_deferred_tool_list("PROJECT_ACTION");
        let result = search_deferred_tools("select:Read_File", &deferred, 5);
        assert_eq!(result, vec!["read_file"]);
    }

    #[test]
    fn test_search_keyword() {
        let deferred = get_deferred_tool_list("PROJECT_ACTION");
        let result = search_deferred_tools("git command", &deferred, 5);
        // git_command should score highest
        assert!(result.contains(&"git_command".to_string()));
    }

    #[test]
    fn test_search_no_match() {
        let deferred = get_deferred_tool_list("PROJECT_ACTION");
        let result = search_deferred_tools("nonexistent_xyz_tool", &deferred, 5);
        assert!(result.is_empty());
    }

    #[test]
    fn test_deferred_list_subagent_excludes_task() {
        let deferred = get_deferred_tool_list("SUBAGENT");
        let names: Vec<&str> = deferred.iter().map(|(n, _)| n.as_str()).collect();
        assert!(!names.contains(&"task"));
        assert!(!names.contains(&"dream"));
        assert!(!names.contains(&"compact"));
    }

    #[test]
    fn test_deferred_list_chat_empty() {
        let deferred = get_deferred_tool_list("CHAT");
        assert!(deferred.is_empty());
    }

    #[test]
    fn test_get_full_schema_returns_valid_json() {
        let schema = get_deferred_tool_full_schema("read_file");
        assert!(schema.is_some());
        let s = schema.unwrap();
        assert_eq!(s["name"], "read_file");
        assert!(s["input_schema"]["properties"]["path"].is_object());
    }

    #[test]
    fn test_core_tools_include_search_tools() {
        let core = get_core_tool_definitions();
        let names: Vec<&str> = core.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"search_tools"));
        assert!(names.contains(&"get_system_info"));
        assert!(names.contains(&"load_skill"));
    }
}
