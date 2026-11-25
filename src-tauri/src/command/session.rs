//! # session.rs — 会话生命周期管理 Tauri 命令
//!
//! 提供会话的创建、切换、删除、重命名、元数据查询等核心命令，
//! 以及 Agent 步骤、方案文档、Agent Run、子 Agent 等扩展查询命令。
//! 还包含自动命名、撤回最后消息等内部辅助函数。
//!
//! ## 关键导出
//! - `create_session()`: 创建新会话，可指定工作目录沙箱
//! - `set_session_work_mode()`: 用户手动切换会话工作模式（chat/edit/plan）
//! - `switch_session()`: 切换活跃会话
//! - `delete_session()` / `rename_session()`: 会话管理
//! - `recall_last_message()`: 撤回最后一条用户消息
//! - `auto_name_session()`: 使用 LLM 自动生成会话名称（内部函数）
//! - `list_agent_runs()` / `get_subagent_runs()`: Agent 运行记录查询
//! - `list_subagent_trace_runs()`: 按会话查询持久化子 Agent 列表
//! - `get_session_context_snapshot()`: 查询最近一次上下文 token 快照

use crate::core::orchestration::{agent_run_repository, agent_runs};
use crate::core::session;
use crate::infra::llm::api_client;
use crate::infra::state::state::*;
use crate::infra::types::models::*;
use tauri::{Emitter, Manager};

#[tauri::command]
pub async fn get_active_session_id() -> Result<Option<String>, String> {
    Ok(session::get_last_active_session_id())
}

#[tauri::command]
pub async fn clear_active_session_id() -> Result<(), String> {
    crate::core::session::repository::clear_last_active_session_id()
}

/// 用户手动切换当前会话的工作模式（edit / plan）。
///
/// 模式是会话级属性：写会话状态 + **落库**（`sessions.work_mode`，随会话恢复），
/// 再广播 `agent-work-mode-changed`（前端已在监听该事件）。
/// 设置里的"默认工作模式"只决定新会话的初始值，不再被这里的切换牵动。
///
/// 第二步起"只读保护（chat）"已取消：安全由权限档位承担（见 set_session_approval_mode）。
/// 这里只接受 edit / plan。
#[tauri::command]
pub async fn set_session_work_mode(
    session_id: String,
    mode: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if !["edit", "plan"].contains(&mode.as_str()) {
        return Err(format!(
            "不支持的工作模式「{}」。支持的模式：edit（编辑）、plan（规划）。",
            mode
        ));
    }

    let ctx = session_manager.get_or_create(&session_id).await;
    let current = ctx.agent_work_mode.lock().await.clone();
    if current == mode {
        return Ok(());
    }
    *ctx.agent_work_mode.lock().await = mode.clone();
    if let Err(e) = crate::core::session::update_session_work_mode(&session_id, &mode) {
        eprintln!("[JARVIS] 工作模式落库失败（会话 {}）：{}", session_id, e);
    }

    let _ = app.emit(
        "agent-work-mode-changed",
        crate::infra::types::events::AgentWorkModeChangedEvent {
            session_id: session_id.clone(),
            from: current,
            to: mode,
            reason: "用户手动切换".to_string(),
        },
    );

    Ok(())
}

/// 读取当前会话的工作模式（chat = 只读保护 / edit / plan）。
///
/// 前端用它校准模式控件：会话状态才是唯一事实来源，UI 偏好只决定新会话的初始模式。
#[tauri::command]
pub async fn get_session_work_mode(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<String, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let mode = ctx.agent_work_mode.lock().await.clone();
    Ok(mode)
}

#[tauri::command]
pub async fn list_sessions() -> Result<Vec<session::SessionMeta>, String> {
    Ok(session::list_sessions())
}

/// 已归档的会话列表（界面「归档」区入口）。
///
/// 与 `list_sessions` 成对：归档的会话不在这里、也不在那里同时出现。
#[tauri::command]
pub async fn list_deleted_sessions() -> Result<Vec<session::SessionMeta>, String> {
    session::list_deleted_sessions()
}

/// 取消归档，把一个会话放回活跃列表。
///
/// 它此前保留的消息、快照数据、回收站本体立即重新可用（归档从不删这些）。
#[tauri::command]
pub async fn restore_session(id: String) -> Result<(), String> {
    session::restore_session(&id)
}

/// **彻底删除**一个会话（不可恢复）：真删行 + 清快照数据 + 清回收站目录。
///
/// 入口在侧边栏「归档」区里 —— 用户对已归档的会话点「彻底删除」才会走到这里。
/// 与归档 [`delete_session`]（只打标记、可取消）的区别见 `core::session` 的说明。
///
/// 这是 `hard_delete_session` 目前**唯一**的用户入口：另一个调用点（自动清理空会话）
/// 已随归档改造移除 —— 归档不再按"有没有内容"分流。
/// 单条入口之外还有批量入口 [`purge_sessions`]（归档区多选彻底删除），两者落到同一个
/// `repository::purge_sessions` 上，不再各写一份 SQL。
#[tauri::command]
pub async fn purge_session(id: String) -> Result<(), String> {
    session::hard_delete_session(&id)
}

#[tauri::command]
pub async fn create_session(
    session_manager: tauri::State<'_, SessionManager>,
    project_id: Option<String>,
) -> Result<session::SessionMeta, String> {
    println!(
        "[DEBUG] create_session called with project_id: {:?}",
        project_id
    );

    // 先验后建：项目目录被删/改名时直接拒绝，不创建会话记录（避免孤儿空会话）。
    // 存量项目记录可能带 \\?\ 前缀，展示前剥离。
    let working_directory = project_id
        .as_ref()
        .and_then(|pid| session::repository::get_project_path(pid).ok().flatten())
        .map(|ws| session::strip_extended_path_prefix(&ws).to_string());
    if let Some(ws) = &working_directory {
        if !std::path::Path::new(ws).exists() {
            return Err(format!(
                "项目目录已不存在: {}（可能已被删除或移动），无法创建沙箱会话。请重新打开项目后再试。",
                ws
            ));
        }
    }

    let meta = session::create_session(project_id);

    // 初始化上下文（存量记录带 \\?\ 前缀，绑定前剥离，下游报错文案与显示才干净）
    let ctx = session_manager.get_or_create(&meta.id).await;
    *ctx.workspace.lock().await = meta
        .working_directory
        .clone()
        .map(|ws| std::path::PathBuf::from(session::strip_extended_path_prefix(&ws)));

    // 挂载项目后把该项目的授权账本注入内存（与 state.rs 会话恢复路径同口径）
    if let Some(root) = ctx.workspace.lock().await.clone() {
        crate::core::tools::framework::allowance_store::load_into(&ctx, &root).await;
    }

    // 新会话的工作模式与权限档位跟随用户偏好
    let prefs = crate::command::app_config::get_ui_preferences()
        .await
        .unwrap_or_default();
    let initial_mode = if prefs.agent_work_mode == "plan" {
        "plan"
    } else {
        "edit"
    };
    *ctx.agent_work_mode.lock().await = initial_mode.to_string();
    // 新会话的权限档位跟随偏好（请求审批 / 帮我批准）
    *ctx.approval_mode.lock().await = if prefs.agent_approval_mode == "auto_approve" {
        "auto_approve".to_string()
    } else {
        "request_approval".to_string()
    };

    Ok(meta)
}

#[tauri::command]
pub async fn open_project(path: String) -> Result<session::ProjectMeta, String> {
    let normalized = std::path::Path::new(&path);
    if !normalized.exists() || !normalized.is_dir() {
        return Err(format!("目录不存在或不是文件夹: {}", path));
    }
    let abs_canonical = normalized
        .canonicalize()
        .map_err(|e| format!("解析路径失败: {}", e))?
        .to_string_lossy()
        .to_string();
    // 剥离 \\?\ 前缀后再入库：展示美观，且与用户输入的普通路径一致
    let abs = session::strip_extended_path_prefix(&abs_canonical).to_string();

    // 已存在则直接返回（兼容存量记录：老数据以 canonicalize 原样含 \\?\ 存储）。
    //
    // 命中的记录可能是**已归档**的项目：归档项目必须在这里「复活」，绝不能改成 INSERT。
    // 两条硬理由：
    // 1. `projects.path` 上有 UNIQUE 约束，同一个 path 再插一条会直接失败；
    // 2. 即使能插进去，新记录的 id 也是新的 —— 历史会话、快照、回收站全都挂在旧 id 上，
    //    用户看到的会是「刚打开项目，以前的会话全没了」，等同于数据丢失。
    if let Some(project) = find_project_by_path(&abs, &abs_canonical)? {
        return revive_project_if_archived(project);
    }

    let name = std::path::Path::new(&abs)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| abs.clone());

    crate::core::session::repository::create_project(&name, &abs)
}

#[tauri::command]
pub async fn list_projects() -> Result<Vec<session::ProjectMeta>, String> {
    crate::core::session::repository::list_projects()
}

/// 按「普通路径优先、含 `\\?\` 前缀的存量路径兜底」的顺序查项目。
///
/// 顺序不能反：同一个目录在库里可能同时存在两种写法（老数据是 canonicalize 原样），
/// 先查用户刚输入的那一种，命中的才是他真正点开的记录。
fn find_project_by_path(
    abs: &str,
    abs_canonical: &str,
) -> Result<Option<session::ProjectMeta>, String> {
    use crate::core::session::repository;
    if let Some(project) = repository::get_project_by_path(abs)? {
        return Ok(Some(project));
    }
    if abs != abs_canonical {
        return repository::get_project_by_path(abs_canonical);
    }
    Ok(None)
}

/// 打开项目时，如果该项目处于**归档**状态，先取消归档再返回。
///
/// 为什么判断放在命令层、用 `is_project_archived` 单独查一次：
/// `ProjectMeta` 的列清单（`PROJECT_META_COLUMNS`）是列表 / 单条查询 / 护栏测试
/// 三方共用的契约，只有「打开项目」这一条路径需要知道归档态，不值得为它加字段。
fn revive_project_if_archived(
    project: session::ProjectMeta,
) -> Result<session::ProjectMeta, String> {
    use crate::core::session::repository;
    if !repository::is_project_archived(&project.id)? {
        return Ok(project);
    }
    repository::restore_project(&project.id)?;
    // 复活后重读一次：返回给前端的 `session_count` 必须是归档之后的真实值
    // （归档期间名下的会话可能已被恢复或彻底删除过）。
    repository::get_project(&project.id)?.ok_or_else(|| "项目不存在".to_string())
}

/// 归档项目（软删）：项目本身 + 名下所有未归档会话，在同一事务里打上同一时间戳。
///
/// 这是原 `delete_project`（硬删项目行 + 硬删名下会话 + 清回滚产物）的替代：项目从
/// 侧边栏消失，但会话记录、消息、快照、回收站全部原地保留，可在「归档」区逐条恢复
/// （恢复会话时同事务连带复活归档项目）。
///
/// 返回值是被归档的会话数，供前端提示「该项目下 N 个会话一并归档」。
#[tauri::command]
pub async fn archive_project(
    id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<u64, String> {
    // 必须在归档**之前**记下「当前活动会话」与它所属的项目：所有读路径都带
    // `AND deleted_at IS NULL`，归档一落地 `get_session_meta` 就查不到这条会话，
    // 也就无从判断它到底属不属于这次被归档的项目。
    let last_active_id = session::get_last_active_session_id();
    let last_active_project = last_active_id
        .as_deref()
        .and_then(|sid| session::get_session_meta(sid).ok())
        .and_then(|meta| meta.project_id);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let archived_sessions = session::archive_project(&id, now)?;

    // 项目列表与侧边栏会话列表同时变化（名下会话整体从活跃列表消失）
    let _ = app.emit("session-updated", ());

    // 当前活动会话正好属于被归档的项目 → 必须切走并释放内存态。
    // 否则后端仍把它当作"当前会话"：后续发消息会继续往一个已归档会话里写，
    // 而所有读路径都查不到它（界面停在一个刷不出历史的会话上）。
    if let (Some(sid), Some(pid)) = (last_active_id, last_active_project) {
        if pid == id {
            session_manager.remove(&sid).await;
            switch_away(&sid, &app).await?;
        }
    }

    Ok(archived_sessions)
}

/// 批量取消归档会话（归档区多选「恢复」的入口）。
///
/// 单条 [`restore_session`] 仍保留（侧边栏单条恢复用它）；批量版把 N 次 IPC 合成一次。
/// 逐条复活 + 连带复活所属归档项目都由 `repository::restore_sessions` 在一个事务里完成；
/// 单条失败只进 `failed`、不回滚整批 —— 归档区里另一窗口或另一台机器刚恢复/刚删掉的
/// 失效 id，不应该让用户勾选的其余项全部白做。
#[tauri::command]
pub async fn restore_sessions(
    ids: Vec<String>,
    app: tauri::AppHandle,
) -> Result<session::BatchOpResult, String> {
    let result = session::restore_sessions(&ids)?;
    // 活跃列表 / 归档列表 / 项目列表都可能变化（会话复活会连带复活归档项目）
    let _ = app.emit("session-updated", ());
    Ok(result)
}

/// 批量**彻底删除**归档会话（不可恢复）。
///
/// 回滚侧产物（快照表 / 回收站目录）的清理由 `repository::purge_sessions` 在事务提交后负责：
/// 数据库行与文件系统没法放进同一个原子操作里，理由见该函数的文档注释。
/// 活跃（未归档）会话会被拒绝、进 `failed` —— 彻底删除不可撤销，不能因为前端传错 id 就清掉在用会话。
#[tauri::command]
pub async fn purge_sessions(
    ids: Vec<String>,
    app: tauri::AppHandle,
) -> Result<session::BatchOpResult, String> {
    let result = session::purge_sessions(&ids)?;
    let _ = app.emit("session-updated", ());
    Ok(result)
}

/// 已归档会话总数（归档页角标）。变化由 `session-updated` 事件驱动重读，不必轮询。
#[tauri::command]
pub async fn count_deleted_sessions() -> Result<usize, String> {
    session::count_deleted_sessions()
}

/// 重命名项目。
///
/// 入口在侧边栏项目标题行 —— 用户点标题旁的改名按钮进入就地编辑，提交后走这里。
/// 只改展示名：`path`、`updated_at` 都不动，理由见
/// `repository::rename_project` 的文档注释。
#[tauri::command]
pub async fn rename_project(id: String, name: String) -> Result<session::ProjectMeta, String> {
    crate::core::session::repository::rename_project(&id, &name)
}

/// 切换活跃会话。
///
/// `app: tauri::AppHandle` 由 Tauri 自动注入（前端 `invoke('switch_session', { id })`
/// 的参数表不变），仅用于在切换后对齐「激活预设」。
#[tauri::command]
pub async fn switch_session(
    id: String,
    session_manager: tauri::State<'_, SessionManager>,
    snapshot_registry: tauri::State<'_, SnapshotRegistry>,
    app: tauri::AppHandle,
) -> Result<session::SessionMeta, String> {
    // 切换会话时释放旧会话的快照管理器缓存
    {
        let registry = snapshot_registry.0.read().await;
        registry.remove(&id).await;
    }

    // 前端通知切换到了该 session，预加载到内存
    let _ = session_manager.get_or_create(&id).await;
    let meta = session::get_session_meta(&id)?;
    println!(
        "[DEBUG] switch_session: id={}, working_directory={:?}",
        id, meta.working_directory
    );

    // 会话激活态与激活预设由后端统一写入：这里按会话的 `profile_id` 对齐
    // `AppConfig.active_profile_id`（pipeline 每次读的是后者，不对齐就会继续用
    // 上一个会话/全局默认的预设）。前端只消费返回的 meta，不再双写配置文件，
    // 也不再自己发 `config-updated`。
    // 对齐函数内部有「目标值相同即早退」判断，不会产生无谓写盘与事件广播。
    if let Some(state) = app.try_state::<crate::infra::config::config::ConfigState>() {
        align_active_profile_to_session(&app, &state, meta.profile_id.as_deref());
    }

    Ok(meta)
}

/// 把"当前会话"的模型预设同步为 `AppConfig.active_profile_id` 并落库。
///
/// **为什么必须有这个函数**：模型预设是会话级状态（`sessions.profile_id`），
/// 但 pipeline 每次读的是 `AppConfig.active_profile_id`。因此**任何"让某会话成为
/// 当前会话"的后端路径都必须对齐一次**，否则后续请求会继续用上一个会话/全局默认的预设。
///
/// 已覆盖的路径：启动恢复会话、点击切会话（本文件 `switch_session`，前端不再双写）、
/// 归档当前会话后切到新对话（本文件 `switch_away`，重置为全局默认预设）。
fn align_active_profile_to_session(
    app: &tauri::AppHandle,
    config_state: &tauri::State<'_, crate::infra::config::config::ConfigState>,
    profile_id: Option<&str>,
) {
    let Ok(mut current) = config_state.0.try_lock() else {
        // 拿不到配置锁就跳过：宁可这次不对齐，也不能阻塞删除流程
        return;
    };
    let target = profile_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| current.global_profile_id.clone());
    if current.active_profile_id == target {
        return;
    }
    current.active_profile_id = target;
    let snapshot = current.clone();
    drop(current);

    if let Err(e) = crate::infra::config::config::save_config(&snapshot) {
        println!("[配置] 对齐会话预设落库失败: {}", e);
        return;
    }
    println!(
        "[配置] 已对齐激活预设: {} (main_model={})",
        snapshot.active_profile_id,
        snapshot.active_config().main_model
    );
    // 通知前端刷新（输入栏据此重读模型名与能力）
    let _ = app.emit("config-updated", ());
}

/// 归档一个会话，并处理它是不是"当前会话"。
///
/// **这是单条会话归档路径的收口**（用户点归档 / 回滚后会话变空）。项目归档（
/// [`archive_project`]）已经把名下会话在同一个事务里整体打上标记，不再走本函数，
/// 但"切走"那一段完全复用 [`switch_away`]。
/// 函数名里的 `delete`、"删除"时代的命名一律保留不动 —— 改名要同时动 tauri 命令名、
/// 前端 invoke 与 DB 列，成本远大于收益；语义以本注释为准。
pub async fn switch_away_and_delete_session(
    deleted_session_id: &str,
    app: &tauri::AppHandle,
) -> Result<(), String> {
    // 归档：一律只打 `deleted_at` 标记，**不再按有没有内容分流**。
    //
    // 原先空会话走硬删除（没有内容可挽留，留着只会让列表堆噪音）—— 那是"删除"语义下
    // 的合理取舍。改成归档后这条分流必须去掉：归档的语义是"东西还在，只是收起来"，
    // 空会话归档后也得在归档区里找得到。否则用户点了归档，记录却凭空消失，
    // 而且是静默的（不报错、归档区里也翻不到），等同于数据丢失。
    //
    // 查询失败（会话已不存在 / 已在归档列表）→ 什么都不做，只完成"切走"。
    if session::get_session_meta(deleted_session_id).is_ok() {
        session::delete_session(deleted_session_id)?;
    }
    if let Some(manager) = app.try_state::<SessionManager>() {
        manager.remove(deleted_session_id).await;
    }

    // 归档到此结束；剩下只处理"它是不是当前会话"：
    // 是 → 回新对话（清空指针）；否 → 当前会话保持不动。交给 switch_away。
    switch_away(deleted_session_id, app).await
}

/// 归档会话后的"当前会话"处理。
///
/// 口径只看被处理的是不是**当前会话**：
/// - 是当前会话 → 清空会话指针，前端回到新对话欢迎页；
/// - 不是当前会话 → 当前会话保持不变，只广播事件让前端清理被删会话的缓存。
///
/// **不挑"第一个别的会话"当 fallback**：归档当前会话时把用户甩进列表里另一个
/// 会话（往往是"上一个对话"）不符合归档的语义——用户归档的是一个会话，不是
/// 要求切换；归档非当前会话时擅自改 `active_session_id` 也会造成同样的跳动。
/// 新对话的模型预设沿用侧栏「新建对话」的口径（重置到全局默认）。
pub async fn switch_away(deleted_session_id: &str, app: &tauri::AppHandle) -> Result<(), String> {
    let last_active = session::get_last_active_session_id();
    let was_active = last_active.as_deref() == Some(deleted_session_id);
    let next_active_id = if was_active { None } else { last_active };

    if was_active {
        // 新对话沿用全局默认预设，与 `prepareNewSession`（侧栏新建对话）同口径；
        // 归档的不是当前会话时保持正在用的预设不动。
        if let Some(state) = app.try_state::<crate::infra::config::config::ConfigState>() {
            align_active_profile_to_session(app, &state, None);
        }
        // 指针不留在已归档会话上，否则下次启动会拿一个已删除的 id 去 switch_session
        let _ = crate::core::session::repository::clear_last_active_session_id();
    }

    let _ = app.emit(
        "active-session-changed",
        SessionCleanupResult {
            deleted_session_id: Some(deleted_session_id.to_string()),
            active_session_id: next_active_id,
        },
    );
    let _ = app.emit("session-updated", ());

    Ok(())
}

/// 使用 utility 模型自动生成会话名称。
///
/// 提取前几条用户消息 + 助手首条文本，过滤掉系统注入的上下文消息。
/// 对 LLM 返回结果做长度校验，防止 LLM 复述原文。
///
/// 命名请求的 token 会以 `UsageSource::SessionName` 落一行用量明细（`run_id` 留空，
/// 理由见函数内注释）；记账失败只打日志，不影响取名与 rename。
pub async fn auto_name_session(
    app: tauri::AppHandle,
    session_id: String,
    memory: SessionMemory,
) -> Result<(), String> {
    if memory.messages.is_empty() {
        return Ok(());
    }

    // 提取用于命名的摘要文本：取用户消息 + 最后一条助手消息（反映实际产出），跳过系统注入
    // 不再提前 break —— 遍历全部消息以获取最后一条助手回复，首条助手回复往往是「我来做X」
    // 这种流程性表述，而最后一条更可能包含实际成果。
    let mut user_texts: Vec<String> = Vec::new();
    let mut assistant_text: Option<String> = None;
    for msg in &memory.messages {
        match msg {
            Message::User { content } => {
                // 动态上下文是独立的 Context 块，extract_plain_text 只取 Text 块，
                // 所以这里拿到的就是用户原文，无需再做字符串手术
                let text = extract_plain_text(content);
                if !text.trim().is_empty() {
                    user_texts.push(text);
                }
            }
            Message::Assistant { content } => {
                // 始终覆盖，取最后一条助手消息（更可能描述实际产出而非流程承诺）
                if let Content::Multiple(blocks) = content {
                    for b in blocks {
                        if let crate::infra::types::models::ContentBlock::Text { text } = b {
                            let t = text.trim().to_string();
                            if !t.is_empty() {
                                assistant_text = Some(t);
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    if user_texts.is_empty() {
        return Ok(());
    }

    // 拼成简洁摘要：用户说的话 + 助手简要
    let mut context = String::new();
    for (i, t) in user_texts.iter().enumerate() {
        let short: String = t.chars().take(300).collect();
        context.push_str(&format!("用户{}: {}\n", i + 1, short));
    }
    if let Some(ref at) = assistant_text {
        let short: String = at.chars().take(120).collect();
        context.push_str(&format!("助手: {}\n", short));
    }

    let summary_prompt = format!(
        "为以上对话生成一个描述性标题，让用户以后能一眼回忆起对话的核心主题和关键成果。\n\
         \n\
         规则：\n\
         - 标题应概括「核心主题 + 关键产出」，而非描述「当前处于哪个流程阶段」\n\
         - 避免使用「审批」「讨论中」「进行中」「审查」「确认」「评估」等流程节点词\n\
         - 长度 3~20 字，不含引号、标点和解释\n\
         - 只输出标题本身，不要输出任何其他内容\n\
         \n\
         正确示例（描述内容与成果）：\n\
         - 任务与知识库管理系统架构设计\n\
         - 支付模块空指针异常修复\n\
         - 用户认证 JWT 方案设计\n\
         - 搜索结果分页改用游标方案\n\
         \n\
         错误示例（描述流程阶段）：\n\
         - 实施方案审批 → 看不出具体项目\n\
         - 代码审查 → 不知道审查什么\n\
         - 需求讨论 → 不知道讨论什么\n\
         \n\
         对话内容：\n{}",
        context
    );

    let cfg = crate::infra::config::config::load_config();
    let agent_cfg = cfg.active_config();

    // 辅助调用统一走带超时的客户端
    let client = api_client::build_utility_client();
    let completion = api_client::call_llm_simple(
        &client,
        &agent_cfg.api_key,
        &agent_cfg.base_url,
        &agent_cfg.utility_model,
        agent_cfg.api_format_enum(),
        "你是一个会话标题生成器。为对话生成描述性标题，概括核心主题与关键成果，避免流程节点词。只输出标题文本。",
        &summary_prompt,
        30,
    )
    .await;

    // 记账口径：命名请求是一次真实的 utility 模型调用，token 确实花掉了，
    // 所以必须落一行 `session_name` 明细（否则按来源拆账时「系统开销」永远偏小，
    // 而且偏得没有痕迹）。失败（拿不到 `completion`）时连 usage 都没有，自然不入账。
    //
    // 厂商没上报 `usage`（`None`）或报了 0/0 时，`record_llm_usage` 返回 `Ok(false)`，
    // 这是既定口径（不写零行），不是错误。
    //
    // 不设 `run_id`：自动命名发生在一轮 agent run 的**收尾之后**（首条助手回复产出
    // 后才触发一次），它不属于任何一次 loop；硬塞外层 run_id 会让「本轮累计」被这条
    // 边缘请求污染，`run_id = NULL` 才是「不属于任何一轮」的准确表达。
    if let Ok(completion) = completion.as_ref() {
        let record = crate::core::session::RequestUsageRecord::new(
            session_id.clone(),
            crate::core::session::UsageSource::SessionName,
        )
        // 标题生成固定走 utility 模型（与上面请求入参同一个配置字段）
        .model_id(agent_cfg.utility_model.clone());
        // `Ok(false)` 还可能是「会话不存在或已归档」，同样是正常的跳过口径
        if let Err(err) =
            crate::core::session::usage::record_llm_usage(&app, record, completion.usage.as_ref())
        {
            // 账本问题不得影响取名与随后的 rename：只记日志，继续往下走
            eprintln!("[JARVIS] 写入会话命名用量明细失败: {}", err);
        }
    }

    // 记账之后才取正文：厂商哪怕回了空正文，这次 token 也已经是事实。
    // 调用失败时 `completion` 是 `Err`，仍回退空串 —— 行为与改造前完全一致。
    let title = completion
        .map(|completion| completion.text)
        .unwrap_or_default();

    let title = title
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('「')
        .trim_matches('」')
        .to_string();

    // 校验：标题必须在 2~30 字之间，不是原文复述
    let char_count = title.chars().count();
    if (2..=30).contains(&char_count) {
        // 额外检查：标题不能是某条原始消息的原文复述
        let is_verbatim = user_texts
            .iter()
            .any(|t| t.contains(&title) && t.len() < title.len() * 3)
            || assistant_text
                .as_ref()
                .map(|t| t.contains(&title) && t.len() < title.len() * 3)
                .unwrap_or(false);
        if !is_verbatim {
            let _ = session::rename_session(&session_id, &title, true);
            let _ = app.emit("session-renamed", ());
            return Ok(());
        }
    }

    Ok(())
}

/// 从 Content 中提取纯文本
fn extract_plain_text(content: &Content) -> String {
    match content {
        Content::Single(s) => s.clone(),
        Content::Multiple(blocks) => blocks
            .iter()
            .filter_map(|b| {
                if let crate::infra::types::models::ContentBlock::Text { text } = b {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// 撤回最后一条用户消息，返回撤回的文本内容
#[tauri::command]
/// 撤回最后一条用户消息。委托给 recall_message 统一处理。
pub async fn recall_last_message(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    // 以消息表为准找"最后一条真实用户输入"，再按 message_id 交给统一撤回入口。
    // 不再把内存下标传下去：它会被压缩/撤回改变，且与 DB seq 不是同一坐标系。
    let visible = session::list_visible_session_messages(&session_id)?;
    let target = visible
        .iter()
        .rev()
        .find(|message| is_real_user_message(message))
        .ok_or_else(|| "没有可撤回的用户消息".to_string())?;
    let message_id = target.message_id.clone();
    recall_message(
        session_id,
        Some(message_id),
        None,
        None,
        session_manager,
        app,
    )
    .await
}

/// 这条存储消息是否是"真实用户输入"（不是工具结果 / 后台通知 / 压缩摘要）。
fn is_real_user_message(stored: &crate::core::session::repository::StoredSessionMessage) -> bool {
    use crate::infra::types::models::{Content, ContentBlock, Message, MessageSource};

    if stored.role != "user" || !stored.source.rendered_in_ui() {
        return false;
    }
    if stored.source == MessageSource::Compact {
        return false;
    }
    match &stored.content {
        Message::User { content } => match content {
            Content::Single(text) => !text.trim().is_empty(),
            Content::Multiple(blocks) => blocks.iter().any(|block| match block {
                ContentBlock::Text { text } => !text.trim().is_empty(),
                ContentBlock::Image { .. } => true,
                _ => false,
            }),
        },
        _ => false,
    }
}

/// 统一撤回入口。recall_last_message / rollback_to_checkpoint_with_recall 均委托至此。
///
/// ## 坐标口径（v29 起）
/// - `message_id`：唯一首选锚点；
/// - `user_message_index`：仅在缺少 message_id 时作为**可见消息表下标**兜底
///   （不是内存 `messages` 下标，前端也不应再传聚合气泡下标）。
///
/// 截断与关联清理都以 `session_messages.seq` 为准：它全局递增、跨压缩稳定。
#[tauri::command]
pub async fn recall_message(
    session_id: String,
    message_id: Option<String>,
    user_message_index: Option<usize>,
    prune_metadata_cutoff: Option<u64>,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    // 撤回期间持有执行槽：拒绝运行中的会话，并阻止新执行在截断历史时启动。
    let _execution_guard = ctx.execution_gate.lock().await;
    if ctx.has_active_execution() {
        return Err("当前任务尚未结束，请等待任务停止后再撤回。".to_string());
    }
    recall_message_locked(
        &session_id,
        message_id,
        user_message_index,
        prune_metadata_cutoff,
        &ctx,
        &app,
    )
    .await
}

/// 已持有执行槽时使用的撤回实现。
///
/// 文件回滚与会话截断必须处于同一个执行槽内：否则删完文件后 `recall_message`
/// 才因为"任务仍在运行"失败，会留下工作区已变、历史没变的半截状态。
pub(crate) async fn recall_message_locked(
    session_id: &str,
    message_id: Option<String>,
    user_message_index: Option<usize>,
    prune_metadata_cutoff: Option<u64>,
    ctx: &crate::infra::state::state::SessionContext,
    app: &tauri::AppHandle,
) -> Result<String, String> {
    let visible = session::list_visible_session_messages(session_id)?;
    let position = if let Some(message_id) = message_id.filter(|id| !id.trim().is_empty()) {
        visible
            .iter()
            .position(|message| message.message_id == message_id)
            .ok_or_else(|| "撤回消息不存在".to_string())?
    } else if let Some(index) = user_message_index {
        if index >= visible.len() {
            return Err("撤回消息不存在".to_string());
        }
        index
    } else {
        return Err("撤回消息不存在".to_string());
    };

    let stored_target = visible[position].clone();
    if !is_real_user_message(&stored_target) {
        return Err("撤回目标不是用户消息".to_string());
    }
    let recalled_text = match &stored_target.content {
        Message::User { content } => message_text_content(content),
        _ => return Err("撤回目标不是用户消息".to_string()),
    };
    // 被撤回区间内的所有 message_id：每条都可能触发过 agent_run，逐个清理，
    // 防止崩溃恢复机制把已撤回的内容重新补回（复活）。
    let removed_ids: Vec<String> = visible[position..]
        .iter()
        .map(|message| message.message_id.clone())
        .filter(|id| !id.trim().is_empty())
        .collect();

    let is_empty;
    {
        let mut session = ctx.memory.lock().await;
        // 在内存里按 message_id 定位同一条消息。压缩摘要等非可见消息仍会留在内存中，
        // 所以不能直接用"可见表下标"截断内存。
        let memory_position = session
            .message_ids
            .iter()
            .position(|id| id == &stored_target.message_id)
            .ok_or_else(|| "会话内存与消息表不一致，请刷新会话后重试".to_string())?;
        session.messages.truncate(memory_position);
        session.message_ids.truncate(memory_position);
        for message_id in &removed_ids {
            crate::core::orchestration::agent_runs::cleanup_by_message_id(message_id);
        }

        if let Some(cutoff) = prune_metadata_cutoff {
            if cutoff == 0 {
                session.plan_documents.clear();
            } else {
                session
                    .plan_documents
                    .retain(|document| document.created_at <= cutoff);
            }
        }
        is_empty = session.messages.is_empty();
    }

    // 顺序不能反：关联清理的子查询要靠 session_messages 里的 seq 找到 message_id。
    crate::infra::db::delete_checkpoint_user_message_links_from_seq(session_id, stored_target.seq)?;
    session::delete_session_messages_from_seq(session_id, stored_target.seq)?;

    if is_empty {
        switch_away_and_delete_session(session_id, app).await?;
    } else {
        {
            let memory = ctx.memory.lock().await.clone();
            session::save_session(session_id, &memory, None);
        }
        // 保存后从 DB 重新加载，确保内存与持久化数据完全一致
        match session::load_session(session_id) {
            Ok(reloaded) => {
                let mut session = ctx.memory.lock().await;
                *session = reloaded;
            }
            Err(e) => {
                eprintln!("[Recall] 撤回后重新加载会话内存失败: {}", e);
            }
        }
        let _ = app.emit("session-updated", ());
        // 撤回会截断消息表：可见条数与最新 seq 同时变化，必须广播
        crate::core::session::notify_messages_changed(app, session_id, "recall");
    }

    Ok(recalled_text)
}

fn message_text_content(content: &Content) -> String {
    match content {
        Content::Single(s) => s.clone(),
        Content::Multiple(blocks) => blocks
            .iter()
            .filter_map(|b| {
                if let ContentBlock::Text { text } = b {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

#[tauri::command]
pub async fn delete_session(
    id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // 复用统一的删除+回落逻辑：删掉指定会话、选 fallback、对齐激活预设、
    // 广播 active-session-changed（前端据此把 activeSessionId 切到 fallback）。
    // 早期这里只做"删行 + 清内存"，既不回落也不切预设，删掉当前会话后
    // 前端仍指向已删除的会话、后端仍用被删会话的预设。
    session_manager.remove(&id).await;
    switch_away_and_delete_session(&id, &app).await
}

#[tauri::command]
pub async fn rename_session(id: String, title: String) -> Result<session::SessionMeta, String> {
    session::rename_session(&id, &title, false)
}

#[tauri::command]
pub async fn update_session_profile(id: String, profile_id: String) -> Result<(), String> {
    session::update_session_profile(&id, &profile_id)
}

#[tauri::command]
pub async fn get_session_meta(id: String) -> Result<session::SessionMeta, String> {
    session::get_session_meta(&id)
}

// ── 深度思考档位（会话级，布尔） ──

/// 取出**当前激活预设**的主模型，用于查模型能力。
///
/// 与旧版 `resolve_profile_thinking_default` 的区别：**不再解析任何默认档位**。
/// 设置默认值的解析只发生在"会话首次发消息"那一刻（`pipeline::start_run`），
/// 这里只为构造 UI 快照而查能力（能力夹紧仍然需要它）。
async fn current_main_model(
    config_state: &tauri::State<'_, crate::infra::config::config::ConfigState>,
) -> String {
    let cfg = config_state.0.lock().await.clone();
    cfg.active_config().main_model
}

/// 组装前端的思考档位快照（`resolvedEnabled` 由后端裁决层算出，前端不做二次判断）。
/// 会话深度思考档位的权威快照（用于查询、修改回包与 `session-thinking-mode-changed` 事件）
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionThinkingSnapshot {
    pub session_id: String,
    pub thinking_enabled: bool,
    pub resolved_enabled: bool,
    pub reason: String,
    #[ts(optional = nullable)]
    pub notice_i18n_key: Option<String>,
}

///
/// `thinkingEnabled` 与 `resolvedEnabled` 的区别：
/// - `thinkingEnabled`：会话库里存的值（用户意图 / 建会话时固化的值）；
/// - `resolvedEnabled`：**实际生效**的值（已过模型能力夹紧，如 DeepSeek 上恒为 true）。
///
/// 前端渲染开关高亮用后者，但要保留前者以便提示"意愿被模型否决"。
fn build_thinking_snapshot(
    session_id: &str,
    session_mode: session::thinking::ThinkingMode,
    caps: Option<&crate::infra::llm::registry::ModelCapabilities>,
) -> SessionThinkingSnapshot {
    let decision = session::thinking::decide(None, session_mode, caps);
    SessionThinkingSnapshot {
        session_id: session_id.to_string(),
        thinking_enabled: session_mode.0,
        resolved_enabled: decision.enabled,
        reason: format!("{:?}", decision.reason),
        notice_i18n_key: decision.notice_i18n_key.map(str::to_string),
    }
}

/// 计算「**尚无会话时**」深度思考开关该显示成什么（只读，不落库）。
///
/// ## 为什么需要它
///
/// 新建会话在**发送首条消息之前并不存在**（`create_session` 时才建行），
/// 但输入框上的开关此时就要有个确定状态。按「跟随全局」的定义，
/// 这个值等于「设置默认档位 + 当前主模型是否强制思考」的解析结果——
/// 这套解析**只能在后端做**（前端不读设置、不查能力），所以单独开一个命令。
///
/// 返回值只是为了**首屏渲染**：真正入库的值仍在首条消息时由
/// `pipeline::ensure_session_thinking_initialized` 重新解析固化，
/// 所以这里即使短暂过期也污染不到真实请求。
///
/// 与 `get_session_thinking` 的回包同构（`sessionId` 为空串），前端可共用解析逻辑。
#[tauri::command]
pub async fn get_pending_thinking_enabled(
    config_state: tauri::State<'_, crate::infra::config::config::ConfigState>,
) -> Result<SessionThinkingSnapshot, String> {
    let model_id = current_main_model(&config_state).await;
    let caps = crate::infra::llm::registry::query_capabilities(&model_id);
    let thinking_forced = caps.as_ref().map(|c| c.thinking_forced).unwrap_or(false);

    let default_raw = crate::command::app_config::read_file()
        .ui_preferences
        .thinking_default;
    let resolved = session::thinking::ThinkingDefault::parse(&default_raw).resolve(thinking_forced);

    Ok(build_thinking_snapshot(
        "",
        session::thinking::ThinkingMode(resolved),
        caps.as_ref(),
    ))
}

/// 读取某会话的思考档位快照。
///
/// 优先从内存 `SessionContext` 取（`switch_session` 已同步），
/// 内存无记录时回落 DB —— 保证冷启动/监控窗口也能拿到权威值。
#[tauri::command]
pub async fn get_session_thinking(
    id: String,
    session_manager: tauri::State<'_, SessionManager>,
    config_state: tauri::State<'_, crate::infra::config::config::ConfigState>,
) -> Result<SessionThinkingSnapshot, String> {
    let ctx = session_manager.get_or_create(&id).await;
    let session_mode = session::thinking::ThinkingMode(*ctx.thinking_mode.lock().await);

    let model_id = current_main_model(&config_state).await;
    let caps = crate::infra::llm::registry::query_capabilities(&model_id);

    Ok(build_thinking_snapshot(&id, session_mode, caps.as_ref()))
}

/// 设置某会话的深度思考档位（**布尔**）。
///
/// 与工作模式/权限档位完全同构的"单一写入口"：
/// 写 DB → 写 `SessionContext` → 广播事件 → **返回权威快照**。
/// 前端以返回值为准（服务端 last-write-wins），不做乐观本地状态。
///
/// 调用时机：界面拨动后**发送消息**时由前端 flush（与另外两个字段共用一个函数），
/// 因此"拨了不发消息"的表态不会被记住。
#[tauri::command]
pub async fn set_session_thinking_enabled(
    id: String,
    enabled: bool,
    session_manager: tauri::State<'_, SessionManager>,
    config_state: tauri::State<'_, crate::infra::config::config::ConfigState>,
    app: tauri::AppHandle,
) -> Result<SessionThinkingSnapshot, String> {
    // 1) 落库（布尔，永不写 NULL）
    session::update_session_thinking_mode(&id, enabled)?;

    // 2) 同步内存态，避免本轮决策读到旧值
    let ctx = session_manager.get_or_create(&id).await;
    *ctx.thinking_mode.lock().await = enabled;

    // 3) 组装权威快照
    let model_id = current_main_model(&config_state).await;
    let caps = crate::infra::llm::registry::query_capabilities(&model_id);
    let snapshot =
        build_thinking_snapshot(&id, session::thinking::ThinkingMode(enabled), caps.as_ref());

    // 4) 广播（带 payload，跨窗口各自过滤 sessionId）
    let _ = app.emit("session-thinking-mode-changed", snapshot.clone());

    Ok(snapshot)
}

#[tauri::command]
pub async fn get_session_context_snapshot(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<Option<crate::infra::types::models::SessionContextSnapshot>, String> {
    let mut snapshot = match session::get_context_snapshot(&session_id)? {
        Some(s) => s,
        None => return Ok(None),
    };
    // 从当前 session memory 重建 "Session Messages" 段，
    // 确保包含完整的最后一轮 assistant 回复
    let ctx = session_manager.get_or_create(&session_id).await;
    let memory = ctx.memory.lock().await;
    if !memory.messages.is_empty() {
        // 可读文本统一由 `context_format::format_messages_readable` 产出，
        // 与 `pipeline::build_context_estimate` 的面板内容保持同一套排版，
        // 否则压缩/刷新后的浮层文本会与实时视图对不上。
        // 注意输入是**内存态** messages（含最后一轮 assistant 回复），不是落库的 history_snapshot。
        let messages_text =
            crate::core::agent::context_format::format_messages_readable(&memory.messages);
        let new_chars = messages_text.chars().count();
        let token_count =
            crate::infra::llm::token_count::count_text(&snapshot.model, &messages_text);
        let messages_json = serde_json::to_string_pretty(&memory.messages).unwrap_or_default();
        // 更新 messages 段
        let old_msg_chars: usize = snapshot
            .sections
            .iter()
            .find(|s| s.key == "messages")
            .map(|s| s.chars)
            .unwrap_or(0);
        snapshot.total_chars = snapshot
            .total_chars
            .saturating_sub(old_msg_chars)
            .saturating_add(new_chars);
        snapshot.sections.retain(|s| s.key != "messages");
        snapshot
            .sections
            .push(crate::infra::types::models::ContextSectionSnapshot {
                key: "messages".to_string(),
                label: "Session Messages".to_string(),
                chars: new_chars,
                estimated_tokens: token_count.tokens,
                token_count_method: token_count.method.as_str().to_string(),
                item_count: memory.messages.len(),
                content: messages_text,
                truncated: false,
                raw_content: Some(messages_json),
            });
        snapshot.estimated_tokens = snapshot.sections.iter().map(|s| s.estimated_tokens).sum();
        snapshot.message_count = memory.messages.len();
    }
    Ok(Some(snapshot))
}

/// 读取会话的用量账本（按来源分组的请求明细汇总，v28）。
///
/// 与 `SessionMeta::total_*` 的分工：后者是四个累计总数（概览栏用），
/// 这里能回答“这些 token 是谁烧的、有几成请求厂商没报缓存”。
/// 无明细的历史会话返回 `hasDetail = false`（数字全 0，不可当真实用量展示），
/// 此时前端应回退到 `total_*`。
#[tauri::command]
pub async fn get_session_usage_breakdown(
    session_id: String,
) -> Result<session::UsageBreakdown, String> {
    session::get_session_usage_breakdown(&session_id)
}

/// 读取会话的用量视图（扁平四值 + 主对话 / 子 Agent / 系统开销三层投影）。
///
/// 与 `get_session_usage_breakdown` 的分工：
/// - 那个返回**账本原形**（`sources` 平铺），是审计口径；
/// - 这个返回与 `session-usage-updated` 事件载荷**同形**的展示投影，
///   供启动 / 切会话时首屏加载。
///
/// 为什么必须有这个命令：分组明细只在运行期通过事件推送，没有回放通道。
/// 启动后打开历史会话时如果不回读，前端 `sessionUsageBreakdown` 一直是空占位，
/// 浮层只能退回 `hasDetail = false` 的兜底四指标，直到用户再发一轮才出现三层。
/// 折叠仍复用 `SessionUsageView::from_breakdown`，不在前端重做第二套聚合。
#[tauri::command]
pub async fn get_session_usage(session_id: String) -> Result<session::SessionUsageView, String> {
    let breakdown = session::get_session_usage_breakdown(&session_id)?;
    Ok(session::SessionUsageView::from_breakdown(&breakdown))
}

#[tauri::command]
pub async fn get_workspace_dir(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<Option<String>, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let ws = ctx.workspace.lock().await.clone();
    if let Some(path) = ws {
        Ok(Some(path.to_string_lossy().to_string()))
    } else {
        // 非沙箱会话返回 None，避免前端或 Agent 误以为存在沙箱限制
        Ok(None)
    }
}

#[tauri::command]
pub async fn list_plan_documents(
    session_id: String,
) -> Result<Vec<crate::infra::types::models::PlanDocument>, String> {
    session::list_plan_documents(&session_id)
}

#[tauri::command]
pub async fn list_agent_runs(
    session_id: Option<String>,
    app: tauri::AppHandle,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<Vec<crate::core::orchestration::agent_runs::AgentRun>, String> {
    let contexts: Vec<_> = {
        let sessions = session_manager.0.read().await;
        sessions
            .iter()
            .filter(|(sid, _)| {
                session_id
                    .as_deref()
                    .is_none_or(|target| target == sid.as_str())
            })
            .map(|(_, ctx)| ctx.clone())
            .collect()
    };

    let mut active_run_ids = std::collections::HashSet::new();
    let mut active_session_ids = std::collections::HashSet::new();
    for ctx in contexts {
        let is_active = ctx
            .current_cancel_token()
            .as_ref()
            .map(|token| !token.is_cancelled())
            .unwrap_or(false);
        if is_active {
            active_session_ids.insert(ctx.id.clone());
            if let Some(run_id) = ctx.active_run_id.lock().await.clone() {
                active_run_ids.insert(run_id);
            }
        }
    }

    for run_id in &active_run_ids {
        let _ = crate::core::orchestration::agent_runs::mark_active_run(&app, run_id);
    }
    crate::core::orchestration::agent_runs::mark_stale_runs_interrupted(
        &app,
        session_id.as_deref(),
        &active_run_ids,
        &active_session_ids,
    );

    Ok(crate::core::orchestration::agent_runs::list_runs(
        session_id.as_deref(),
    ))
}

#[tauri::command]
pub async fn list_agent_run_events(
    session_id: Option<String>,
    run_id: Option<String>,
) -> Result<Vec<crate::core::orchestration::agent_runs::AgentRunLoopEvent>, String> {
    Ok(crate::core::orchestration::agent_runs::list_loop_events(
        session_id.as_deref(),
        run_id.as_deref(),
    ))
}

#[tauri::command]
pub async fn prepare_resume_agent_run(
    run_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<crate::core::orchestration::agent_runs::ResumeAgentRunPlan, String> {
    // v15：`prepare_resume` 不再返回 checkpoint（已无 checkpoint 体系），
    // 恢复上下文由门卫 `ensure_session_recovered` 从 events 重放。
    let (_run, plan) = agent_runs::prepare_resume(&run_id)?;
    let ctx = session_manager.get_or_create(&plan.session_id).await;
    let active_run_id = ctx.active_run_id.lock().await.clone();
    let recovered = {
        let mut memory = ctx.memory.lock().await;
        session::reset_message_ids(&mut memory);
        ensure_session_recovered(&plan.session_id, &mut memory, active_run_id.as_deref())
    };
    if recovered {
        let _ = app.emit("session-updated", ());
    }
    Ok(plan)
}

/// 应用退出（ExitRequested）收尾：把内存态会话整仓落库，并把所有仍为
/// Running 的 run 标记为 Interrupted。
///
/// 此前"关闭窗口 = 进程死亡无痕"：事件驱动检查点的落库时机里没有
/// "进程退出"这一项，活跃 run 永久停留 Running，检查点之后的内存进度
/// 全部丢失且不留任何痕迹（崩溃恢复也因状态门槛不满足而无法收口）。
/// 此处在退出路径做最后的同步落库 —— blocking 系列锁在 ExitRequested
/// 已有先例（后台进程树清理），代价是关窗多一次整仓写（大会话数百毫秒量级）。
pub fn finalize_active_runs_on_exit(handle: &tauri::AppHandle) {
    use tauri::Manager;

    if let Some(sm) = handle.try_state::<SessionManager>() {
        // 先收集再落库：锁只覆盖 clone 的瞬间即释放，不与 pipeline 的短锁长持竞争
        let snapshots: Vec<(String, SessionMemory)> = {
            let map = sm.0.blocking_read();
            map.iter()
                .map(|(sid, ctx)| {
                    let memory = ctx.memory.blocking_lock();
                    (sid.clone(), memory.clone())
                })
                .collect()
        };
        for (sid, memory) in &snapshots {
            let _ = session::save_session(sid, memory, None);
        }
    }
    crate::core::orchestration::agent_runs::mark_running_interrupted_on_exit();
}

/// 中断恢复**唯一闸门**（门卫）。
///
/// 全系统所有"把中断/崩溃遗留的消息补回会话"的路径（历史命令、全量/分页消息命令、
/// 恢复命令、发消息的前端前置与后端 pre_loop）都必须经过本函数，禁止各自实现恢复链
/// ——此前 6 个入口各自手抄"恢复 + save + 盖章"且无互斥，10b68285 的 13ms
/// 双写坏数据即两个入口并发的产物。
///
/// 流程（锁 → 干活 → 先落库后盖章）：
///
/// 1. **活跃闸门**：本会话存在进程内活跃 run（`ctx.active_run_id`）→ 直接返回。
///    这是比 STALE 时间戳强得多的"run 还活着"判定——活跃 run 跑得再久也不会被误判遗留。
/// 2. **遗留发现**：`find_interrupted_run`（interrupted / running+STALE / recovering+超时）。
/// 3. **原子抢占（锁）**：`try_claim_run_for_recovery` 以单条条件 UPDATE 置为
///    `recovering`。SQLite 单写者保证并发只有一个赢家，抢不到的直接退出。
/// 4. **重放增量**：从 agent_run_events 重建 + diff_tail，只补缺的部分（纯计算）。
/// 5. **先落库后盖章**：append 后 `save_session`，成功才 `mark_run_recovered`；
///    盖章失败必须打日志——run 停留 recovering，由超时接管机制兜底重试，
///    绝不允许静默吞掉（否则要么永久卡 recovering，要么重复恢复）。
///
/// 返回是否补回了消息（调用方据此刷新界面状态）。
pub(crate) fn ensure_session_recovered(
    session_id: &str,
    memory: &mut SessionMemory,
    active_run_id: Option<&str>,
) -> bool {
    session::normalize_message_ids(memory);

    // 闸门 1：会话内有 run 正在跑 → 不做任何恢复
    if active_run_id.is_some() {
        return false;
    }

    // 闸门 2：发现遗留 run（无可恢复属正常路径，绝大多数加载走这里，不打日志）
    let Some(run) = crate::core::orchestration::agent_runs::find_interrupted_run(session_id) else {
        return false;
    };

    // 闸门 3：原子抢占。stale_cutoff 同时承担两个职责：
    // running 遗留的 STALE 判定 + recovering 半途而废的接管判定
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let stale_cutoff =
        now_ms.saturating_sub(crate::core::orchestration::agent_runs::RUN_STALE_MS as i64);
    match agent_run_repository::try_claim_run_for_recovery(&run.run_id, stale_cutoff) {
        Ok(true) => {}
        Ok(false) => return false, // 被其它入口/实例抢走：它们正在恢复，直接退出
        Err(e) => {
            eprintln!("[JARVIS][恢复] 抢占 run {} 失败：{}", run.run_id, e);
            return false;
        }
    }

    // 干活：从 events 重放 + diff 增量（纯计算，状态已由抢占闸门保证独占）
    let current_messages = memory.messages.clone();
    match agent_runs::recover_outcome_for_run(&run, &current_messages) {
        agent_runs::RecoveryOutcome::None => {
            // 无内容可补且无悬尾：内容已完整落库。也必须把 run 标掉结束恢复态，
            // 否则每次加载都重复抢占空转（无害但浪费且日志噪音）。
            if let Err(e) = agent_runs::mark_run_recovered(&run.run_id) {
                eprintln!(
                    "[JARVIS][恢复] 收口：标记 run {} 为已恢复失败：{}（停留 recovering，待超时接管）",
                    run.run_id, e
                );
            }
            false
        }
        agent_runs::RecoveryOutcome::Content { messages } => {
            println!(
                "[JARVIS][恢复] 抢占 run {} 成功，从 events 重放出 {} 条消息（session {}）",
                run.run_id,
                messages.len(),
                session_id
            );
            // v15：重放序列本身已是**按 loop 顺序铺开**的完整结构
            //（assistant 响应 → user 工具结果 → 下一轮 …），
            // 半截内容本就住在最后一个 loop 的 resp_blocks 里，由重放一并带出。
            for message in messages {
                // 重放出来的是**主对话**的原始消息（响应 + 工具结果），来源即 `Chat`
                session::append_message(memory, message, MessageSource::Chat);
            }
            finish_recovery(session_id, memory, &run.run_id)
        }
        agent_runs::RecoveryOutcome::NeedsClosure => {
            // 崩溃 run 无内容可补但会话尾部悬尾（最后一条是 user 且无人回应）：
            // 补一条 assistant 中断占位，维持消息级 user/assistant 严格交替。
            //
            // `MessageSource::Interrupted` 会进界面渲染；阶段二起小字由**结构化 kind**
            // 驱动（`command/history.rs::interrupt_notice_for`），正文不再放标记
            // 文本——标记在发送给模型前按 kind 拼回。
            println!(
                "[JARVIS][恢复] run {} 无内容可补，补中断占位收口（session {}）",
                run.run_id, session_id
            );
            session::append_message_with_kind(
                memory,
                Message::Assistant {
                    content: Content::Single(String::new()),
                },
                MessageSource::Interrupted,
                Some(crate::infra::types::models::InterruptKind::AppClosed.as_str()),
            );
            finish_recovery(session_id, memory, &run.run_id)
        }
    }
}

/// 恢复收尾（第二刀）：**先落库，后盖章；盖章失败必须喊出来**。
///
/// `save_session` 不返回 Result（历史签名，19 个调用点，保持口径不变）：
/// 其内部的 DB 写失败同样会作用在下方盖章的 update 上——真失败时 run
/// 停留 `recovering`，由超时接管机制在下一次恢复时重试，不会丢恢复机会。
fn finish_recovery(session_id: &str, memory: &SessionMemory, run_id: &str) -> bool {
    session::save_session(session_id, memory, None);
    if let Err(e) = agent_runs::mark_run_recovered(run_id) {
        eprintln!(
            "[JARVIS][恢复] 严重：session {} 落库完成，但标记 run {} 为已恢复失败：{}（run 停留 recovering，待超时接管重试）",
            session_id, run_id, e
        );
    }
    true
}

#[tauri::command]
pub async fn recover_interrupted_session_messages(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let active_run_id = ctx.active_run_id.lock().await.clone();
    let recovered = {
        let mut memory = ctx.memory.lock().await;
        ensure_session_recovered(&session_id, &mut memory, active_run_id.as_deref())
    };
    if recovered {
        let _ = app.emit("session-updated", ());
        // 恢复补回的半截内容已由闸门内统一落库，这里广播一次可见集合变更
        crate::core::session::notify_messages_changed(&app, &session_id, "recovery");
    }
    Ok(recovered)
}

#[tauri::command]
pub async fn get_background_tasks(
    session_id: Option<String>,
    bg_state: tauri::State<'_, crate::infra::background::BackgroundState>,
) -> Result<Vec<crate::infra::background::BackgroundTask>, String> {
    let bg = bg_state.0.lock().await;
    let tasks: Vec<_> = if let Some(sid) = session_id {
        bg.tasks
            .values()
            .filter(|t| t.session_id.as_deref() == Some(&sid))
            .cloned()
            .collect()
    } else {
        bg.tasks.values().cloned().collect()
    };
    Ok(tasks)
}

#[tauri::command]
pub async fn dismiss_background_task(
    task_id: String,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    Ok(crate::infra::background::BackgroundManager::dismiss_task(&app, &task_id).await)
}

#[tauri::command]
pub async fn kill_background_task(task_id: String, app: tauri::AppHandle) -> Result<bool, String> {
    Ok(crate::infra::background::BackgroundManager::kill_task(&app, &task_id).await)
}

#[tauri::command]
pub async fn clear_session_background_tasks(
    session_id: String,
    app: tauri::AppHandle,
) -> Result<usize, String> {
    Ok(crate::infra::background::BackgroundManager::clear_session_tasks(&app, &session_id).await)
}

#[tauri::command]
pub async fn get_subagent_runs(
    session_id: Option<String>,
    monitor_state: tauri::State<'_, crate::core::orchestration::subagents::SubAgentMonitorState>,
) -> Result<Vec<crate::core::orchestration::subagents::SubAgentRun>, String> {
    let mut monitor = monitor_state.0.lock().await;
    Ok(monitor.list(session_id.as_deref()))
}

#[tauri::command]
pub async fn list_subagents(
    session_id: Option<String>,
    monitor_state: tauri::State<'_, crate::core::orchestration::subagents::SubAgentMonitorState>,
) -> Result<Vec<crate::core::orchestration::subagents::SubAgentRun>, String> {
    let mut monitor = monitor_state.0.lock().await;
    Ok(monitor.list(session_id.as_deref()))
}

#[tauri::command]
pub async fn list_subagent_trace_runs(
    session_id: String,
) -> Result<Vec<crate::core::orchestration::subagents::SubAgentTraceRunSummary>, String> {
    crate::core::orchestration::subagents::SubAgentMonitor::list_trace_runs(&session_id)
}

#[tauri::command]
pub async fn list_subagent_trace_events(
    run_id: String,
) -> Result<Vec<crate::core::orchestration::subagents::SubAgentTraceEvent>, String> {
    crate::core::orchestration::subagents::SubAgentMonitor::list_trace_events(&run_id)
}

#[tauri::command]
pub async fn cancel_subagent_run(
    run_id: String,
    app: tauri::AppHandle,
) -> Result<crate::core::orchestration::subagents::SubAgentRun, String> {
    crate::core::orchestration::subagents::SubAgentMonitor::cancel_run(&app, &run_id).await
}

#[tauri::command]
pub async fn get_session_todos(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
) -> Result<Vec<crate::infra::types::models::TodoGroup>, String> {
    let ctx = session_manager.get_or_create(&session_id).await;
    let todos = ctx.todos.lock().await;
    Ok(todos.clone())
}

/// 压缩命令的生命周期守卫：无论正常返回、报错还是异步任务被丢弃，都清除会话状态并广播结束。
struct CompactingSessionGuard<'a> {
    state: &'a crate::infra::background::CompactingState,
    session_id: String,
    app: tauri::AppHandle,
}

impl Drop for CompactingSessionGuard<'_> {
    fn drop(&mut self) {
        let _ = self.state.set_compacting(&self.session_id, false);
        let _ = self.app.emit(
            "bg-compacting-changed",
            crate::infra::types::events::BgCompactingChangedEvent {
                session_id: self.session_id.clone(),
                compacting: false,
            },
        );
    }
}

#[tauri::command]
pub async fn compact_conversation(
    session_id: String,
    session_manager: tauri::State<'_, SessionManager>,
    compacting: tauri::State<'_, crate::infra::background::CompactingState>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    if !compacting.set_compacting(&session_id, true) {
        return Err("当前会话已有上下文压缩正在执行，请等待完成。".to_string());
    }
    let _ = app.emit(
        "bg-compacting-changed",
        crate::infra::types::events::BgCompactingChangedEvent {
            session_id: session_id.clone(),
            compacting: true,
        },
    );
    let guard = CompactingSessionGuard {
        state: compacting.inner(),
        session_id: session_id.clone(),
        app: app.clone(),
    };
    let result = compact_inner(&session_id, session_manager, &app).await;
    drop(guard);
    result
}

async fn compact_inner(
    session_id: &str,
    session_manager: tauri::State<'_, SessionManager>,
    app: &tauri::AppHandle,
) -> Result<String, String> {
    let ctx = session_manager.get_or_create(session_id).await;
    let mut memory = ctx.memory.lock().await;
    let keep = crate::infra::types::constants::COMPACT_MIN_MESSAGES;
    if memory.messages.len() <= keep {
        return Ok(format!(
            "消息不足（仅有 {} 条，下限 {} 条），无需压缩。",
            memory.messages.len(),
            keep
        ));
    }
    // 辅助调用统一走带超时的客户端
    let client = api_client::build_utility_client();
    let cfg = {
        let config = crate::infra::config::config::load_config();
        config.active_config().clone()
    };
    crate::core::session::memory::compact_messages(
        &mut memory,
        &client,
        &cfg.api_key,
        &cfg.base_url,
        &cfg.utility_model,
        crate::infra::llm::api_format::ApiFormat::OpenAI,
    )
    .await
    .map_err(|e| format!("压缩失败: {}", e))?;

    // 从 DB 中删除刚才被压缩剔除的消息行。
    // 名单与 `compact_messages` 的剔除判据同源（`!in_llm_context()`，只剩 `Placeholder`），
    // 不允许各写一份 —— 见 `core/session/memory.rs` 同一处的说明。
    if let Err(e) = crate::core::session::repository::delete_session_messages_by_source(
        session_id,
        &[MessageSource::Placeholder],
    ) {
        println!("[compact] 清理内部填充消息失败: {}", e);
    }

    let ids: Vec<String> = (0..memory.messages.len())
        .map(|i| format!("compact:{}:{}", i, uuid::Uuid::new_v4().simple()))
        .collect();
    memory.message_ids = ids;
    let cloned = memory.clone();
    drop(memory);
    drop(ctx);

    let _ = crate::core::session::save_session(session_id, &cloned, None);
    // 压缩会隐藏被摘要的旧消息：可见条数变化，广播一次让其它视图标脏
    crate::core::session::notify_messages_changed(app, session_id, "compaction");
    Ok("上下文已压缩。".to_string())
}

#[tauri::command]
pub fn is_session_compacting(
    session_id: String,
    compacting: tauri::State<'_, crate::infra::background::CompactingState>,
) -> bool {
    compacting.is_compacting(&session_id)
}
