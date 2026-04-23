// --- 工具系统入口模块 ---
// 模块注册、工具定义、路由分发

pub mod permission;
pub mod file_tools;
pub mod shell_tools;
pub mod system_tools;
pub mod task_tools;
pub mod agent_tools;

use serde_json::json;
use std::path::Path;

use crate::core::models::Skill;
use crate::get_agent_home;

// Re-export 供外部使用的公开接口
pub use permission::{is_path_safe, ensure_path_permission, request_permission};
pub use file_tools::{generate_repo_map, search_in_dir};
pub use agent_tools::run_subagent;

// 加载所有技能（从 Agent 家目录下的 skills 文件夹加载）
pub fn load_all_skills() -> Vec<Skill> {
    let mut skills = Vec::new();
    let home = get_agent_home();
    let mut skills_dir = home.join(crate::core::constants::DIR_SKILLS);
    if !skills_dir.exists() {
        skills_dir = home.join("..").join("skills");
    }

    fn scan_skills(dir: &Path, skills: &mut Vec<Skill>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    scan_skills(&path, skills);
                } else if path.file_name().unwrap_or_default() == "SKILL.md" {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Some(skill) = parse_skill(&content, &path) {
                            skills.push(skill);
                        }
                    }
                }
            }
        }
    }
    scan_skills(&skills_dir, &mut skills);
    skills
}

// 解析技能文件
pub fn parse_skill(text: &str, path: &Path) -> Option<Skill> {
    if text.starts_with("---\n") || text.starts_with("---\r\n") {
        let parts: Vec<&str> = text.splitn(3, "---").collect();
        if parts.len() >= 3 {
            let frontmatter = parts[1];
            let body = parts[2].trim().to_string();

            let mut name = path
                .parent()
                .and_then(|p| p.file_name())
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let mut description = String::from("No description");

            for line in frontmatter.lines() {
                let parts: Vec<&str> = line.splitn(2, ':').collect();
                if parts.len() == 2 {
                    let k = parts[0].trim();
                    let v = parts[1].trim();
                    if k == "name" {
                        name = v.to_string();
                    } else if k == "description" {
                        description = v.to_string();
                    }
                }
            }
            return Some(Skill {
                name,
                description,
                body,
            });
        }
    }
    None
}

// 获取工具定义（按意图筛选）
pub fn get_tools_definition(intent: &str) -> Vec<serde_json::Value> {
    let mut tools = vec![
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
    ];

    if intent == "GENERAL_CHAT" {
        return tools;
    }
    //记忆工具
    if intent == "MEMORY_QUERY" {
        tools.extend(vec![
            json!({
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
            }),
            json!({
                "name": "compact",
                "description": "手动触发对话上下文压缩。当对话上下文过长觉得需要清理或重置记忆时使用该工具。",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "focus": { "type": "string", "description": "摘要时需要特别保留的重点方向" }
                    }
                }
            }),
            json!({
                "name": "dream",
                "description": "主动触发记忆整理（Dream Agent）。将当前的零散碎片记忆提炼并合并进结构化用户画像中。",
                "input_schema": { "type": "object", "properties": {} }
            })
        ]);
        return tools;
    }
    // 文件工具
    let file_and_system_tools = vec![
        json!({
            "name": "set_workspace",
            "description": "设置或更改大模型当前运作的全局工作区（Working Directory）目录。跨大项目切换或是初始化指定项目目录时使用。由于会改变全局环境且会被系统持久化记住，必须使用绝对路径。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "工作区目录的绝对路径"}
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "list_directory",
            "description": "列出指定目录下的所有文件和文件夹。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "目录路径"}
                },
                "required": ["path"]
            }
        }),
        json!({
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
        }),
        json!({
            "name": "run_shell",
            "description": "执行 Windows PowerShell 命令。（阻塞同步且高风险：所有命令都会在 PowerShell 环境下执行，支持管道。长周期运行或前端 dev server 服务端启动请使用 background_run 工具。请优先使用专用的只读工具）",
            "input_schema": {
                "type": "object",
                "properties": { "command": {"type": "string"} },
                "required": ["command"]
            }
        }),
        json!({
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
        }),
        json!({
            "name": "read_file_skeleton",
            "description": "提取文件结构骨架（Skeleton）。快速扫描并返回文件的类、函数签名及其行号，结合 read_file 进行精确片段阅读。",
            "input_schema": {
                "type": "object",
                "properties": { "path": {"type": "string"} },
                "required": ["path"]
            }
        }),
        json!({
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
        }),
        json!({
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
        }),
        json!({
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
        }),
    ];
    // 任务工具
    let orchestrator_tools = vec![
        json!({
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
        }),
        json!({
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
        }),
        json!({
            "name": "task_list",
            "description": "列出所有任务及其状态概要。",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "task_summary",
            "description": "生成任务全景报告，包括进度、关键路径瓶颈、以及推荐的下一步待办任务。建议在开始新工作或完成重要任务后使用。",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "task_get",
            "description": "获取单个任务的完整详细信息。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "task_id": {"type": "integer"}
                },
                "required": ["task_id"]
            }
        }),
        json!({
            "name": "task",
            "description": "【真正执行】产生一个具有干净上下文环境的子代理 (Subagent) 去实际执行探索或具体操作任务。这是唯一能让子代理实际干活的工具！主 Agent 必须使用此工具来委派文件读取、代码搜索和修改等具体工作，避免污染主对话上下文。与父进程共享文件系统但不共享对话历史。注意：每次只能委派一个子代理，不支持并行。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "要子代理完成的任务说明，越详细越好。包括你想要子代理返回什么数据。"},
                    "read_only": {"type": "boolean", "description": "是否以只读模式运行子代理。在只读模式下，子代理无法使用修改文件、系统状态或执行高风险命令的工具。默认为 true。"}
                },
                "required": ["prompt"]
            }
        }),
        json!({
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
        }),
        json!({
            "name": "check_background",
            "description": "检查后台任务的执行状态和输出。仅当用户主动询问后台任务状态时才使用，严禁在自己的思考循环中连续轮询此工具！",
            "input_schema": {
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "后台任务 ID。如果留空则返回所有任务状态。"}
                }
            }
        }),
        json!({
            "name": "compact",
            "description": "手动触发对话上下文压缩。当对话上下文过长觉得需要清理或重置记忆时使用该工具。",
            "input_schema": {
                "type": "object",
                "properties": {
                    "focus": { "type": "string", "description": "摘要时需要特别保留的重点方向" }
                }
            }
        }),
        json!({
            "name": "dream",
            "description": "主动触发记忆整理（Dream Agent）。将当前的零散碎片记忆提炼并合并进结构化用户画像中。",
            "input_schema": { "type": "object", "properties": {} }
        }),
        json!({
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
        }),
    ];
    // 根据意图选择工具
    if intent == "SUBAGENT" {
        tools.extend(file_and_system_tools);
        tools.extend(orchestrator_tools);
        tools.retain(|t| t["name"] != "task");
        tools.retain(|t| t["name"] != "dream");
        tools.retain(|t| t["name"] != "compact");
    } else {
        // Default to PROJECT_ACTION (Main Agent)
        tools.extend(orchestrator_tools);
    }

    tools
}

/// 工具调用路由：根据工具名分发到对应模块
pub async fn handle_tool_call(
    app: &tauri::AppHandle,
    name: &str,
    input: &serde_json::Value,
) -> (String, u64, u64) {
    if name == "task" {
        let prompt = input["prompt"].as_str().unwrap_or("");
        let read_only = input["read_only"].as_bool().unwrap_or(true);
        let fut = run_subagent(app.clone(), prompt.to_string(), read_only);
        Box::pin(fut).await
    } else {
        (handle_tool_call_inner(app, name, input).await, 0, 0)
    }
}

/// 内部工具调用分发（非子代理工具）
pub async fn handle_tool_call_inner(
    app: &tauri::AppHandle,
    name: &str,
    input: &serde_json::Value,
) -> String {
    match name {
        // 系统工具
        "set_workspace" => system_tools::set_workspace(app, input).await,
        "get_system_info" => system_tools::get_system_info(app, input).await,

        // 文件工具
        "list_directory" => file_tools::list_directory(app, input).await,
        "search_repo" => file_tools::search_repo(app, input).await,
        "read_file" => file_tools::read_file(app, input).await,
        "read_file_skeleton" => file_tools::read_file_skeleton(app, input).await,
        "write_file" => file_tools::write_file(app, input).await,
        "edit_file" => file_tools::edit_file(app, input).await,

        // Shell 工具
        "git_command" => shell_tools::git_command(app, input).await,
        "run_shell" => shell_tools::run_shell(app, input).await,
        "background_run" => shell_tools::background_run(app, input).await,
        "check_background" => shell_tools::check_background(app, input).await,

        // 任务工具
        "task_create" => task_tools::task_create(app, input).await,
        "task_update" => task_tools::task_update(app, input).await,
        "task_list" => task_tools::task_list(app, input).await,
        "task_get" => task_tools::task_get(app, input).await,
        "task_summary" => task_tools::task_summary(app, input).await,

        // Agent 工具
        "load_skill" => agent_tools::load_skill(app, input).await,
        "compact" => agent_tools::compact(app, input).await,
        "dream" => agent_tools::dream(app, input).await,

        // 方案审批工具
        "propose_plan" => agent_tools::propose_plan(app, input).await,

        _ => format!("未知工具: {}", name),
    }
}
