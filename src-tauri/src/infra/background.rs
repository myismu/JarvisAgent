//! 后台任务管理模块
//!
//! 提供异步进程执行能力，支持：
//! - 非阻塞启动 shell 命令（避免阻塞主对话）
//! - 自动检测服务端口和任务类型
//! - stdout/stderr 实时捕获与缓冲
//! - 任务状态追踪与通知队列

use crate::infra::shell_command::NoWindow;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

fn kill_process_tree(pid: u32) {
    if pid == 0 {
        return;
    }
    if cfg!(target_os = "windows") {
        let _ = std::process::Command::new("taskkill")
            .no_window()
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// 一次性查询 Windows 进程父子关系快照表 `(parent_pid, child_pid)`。
///
/// 此前对进程树的每一层、每个节点各启动一次 PowerShell，单次终止可能拉起 5~10 个进程耗时数秒；
/// 改为单次 PowerShell 采样全量 `(ParentProcessId, ProcessId)` 并在 Rust 内存内完成 BFS。
fn snapshot_windows_process_edges() -> Vec<(u32, u32)> {
    if !cfg!(target_os = "windows") {
        return vec![];
    }
    let Ok(out) = std::process::Command::new("powershell")
        .no_window()
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ParentProcessId) $($_.ProcessId)\" }",
        ])
        .output()
    else {
        return vec![];
    };

    parse_process_edges(&String::from_utf8_lossy(&out.stdout))
}

fn parse_process_edges(stdout: &str) -> Vec<(u32, u32)> {
    let mut edges = Vec::new();
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let (Some(p), Some(c)) = (parts.next(), parts.next()) else {
            continue;
        };
        if let (Ok(parent), Ok(child)) = (p.parse::<u32>(), c.parse::<u32>()) {
            if child > 0 {
                edges.push((parent, child));
            }
        }
    }
    edges
}

/// 纯内存 BFS：基于 `(parent_pid, child_pid)` 边表收集 `roots` 的全部子孙 PID（不含 `roots` 自身，防环且限深）。
fn collect_descendants_from_edges(roots: &[u32], edges: &[(u32, u32)]) -> Vec<u32> {
    let mut children_map: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(parent, child) in edges {
        children_map.entry(parent).or_default().push(child);
    }

    let mut visited: std::collections::HashSet<u32> =
        roots.iter().copied().filter(|&p| p > 0).collect();
    let mut queue: Vec<u32> = visited.iter().copied().collect();
    let mut descendants = Vec::new();

    for _ in 0..16 {
        if queue.is_empty() {
            break;
        }
        let current = std::mem::take(&mut queue);
        for pid in current {
            if let Some(children) = children_map.get(&pid) {
                for &child in children {
                    if child > 0 && visited.insert(child) {
                        descendants.push(child);
                        queue.push(child);
                    }
                }
            }
        }
    }
    descendants
}

/// 收集指定根 PID 列表的完整进程树（去重，后代在前、根在后，便于自底向上终止）。
fn collect_process_tree_targets(seed_pids: &[u32]) -> Vec<u32> {
    let mut roots = Vec::new();
    for &pid in seed_pids {
        if pid > 0 && !roots.contains(&pid) {
            roots.push(pid);
        }
    }
    if roots.is_empty() {
        return vec![];
    }
    let edges = snapshot_windows_process_edges();
    let descendants = collect_descendants_from_edges(&roots, &edges);

    // 自底向上：最深后代先杀，最后杀根进程，防止父进程退出后子进程变为孤儿
    let mut targets = Vec::with_capacity(descendants.len() + roots.len());
    targets.extend(descendants.into_iter().rev());
    targets.extend(roots);
    targets
}

/// 阻塞式终止整批根 PID 及其所有子孙进程（供 `spawn_blocking` 与退出同步路径共用）。
fn kill_pids_and_descendants(seed_pids: &[u32]) -> Vec<u32> {
    let targets = collect_process_tree_targets(seed_pids);
    for &pid in &targets {
        kill_process_tree(pid);
    }
    targets
}

/// 查找单个根 PID 的全部后代 PID（单次 WMI 快照采样）。
fn find_descendant_pids(parent_pid: u32) -> Vec<u32> {
    if !cfg!(target_os = "windows") || parent_pid == 0 {
        return vec![];
    }
    let edges = snapshot_windows_process_edges();
    collect_descendants_from_edges(&[parent_pid], &edges)
}

/// 提取某任务记录的全部种子 PID（合并 `task.pid` 与 `task.pids`，覆盖启动前 1.5s 窗口）。
fn extract_task_seed_pids(task: &BackgroundTask) -> Vec<u32> {
    let mut seeds = Vec::new();
    if let Some(root) = task.pid {
        if root > 0 {
            seeds.push(root);
        }
    }
    for &pid in &task.pids {
        if pid > 0 && !seeds.contains(&pid) {
            seeds.push(pid);
        }
    }
    seeds
}

/// 后台任务信息
#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
pub struct BackgroundTask {
    pub id: String,
    #[ts(optional = nullable)]
    pub session_id: Option<String>,
    pub command: String,
    pub status: String,
    #[ts(optional = nullable)]
    pub result: Option<String>,
    #[ts(optional = nullable)]
    pub port: Option<u16>,
    #[ts(optional = nullable)]
    pub task_type: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub pids: Vec<u32>,
}

/// 任务完成通知（Tauri 事件推送 + 轮询兼容）
#[derive(Clone, Debug, Serialize, ts_rs::TS)]
pub struct Notification {
    pub task_id: String,
    #[ts(optional = nullable)]
    pub session_id: Option<String>,
    pub status: String,
    pub command: String,
    pub result: String,
    #[ts(optional = nullable)]
    pub port: Option<u16>,
    #[ts(optional = nullable)]
    pub task_type: Option<String>,
}

/// 受异步锁保护的后台子进程句柄别名（供管理器存储与无锁查杀摘取共用）。
type ChildProcessHandle = Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>;

/// 后台任务管理器
///
/// 维护所有运行中任务的状态和子进程句柄，用于状态查询与安全终止。
/// 任务完成/失败的通知走 `bg-task-done` / `background-failed` Tauri 事件推给前端，
/// 不再经会话上下文注入（前端小字提醒用户，由用户决定是否让 Agent 排查）。
pub struct BackgroundManager {
    pub tasks: HashMap<String, BackgroundTask>,
    pub child_processes: HashMap<String, ChildProcessHandle>,
}

impl Default for BackgroundManager {
    fn default() -> Self {
        Self::new()
    }
}

impl BackgroundManager {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            child_processes: HashMap::new(),
        }
    }

    /// 摘取全部运行中任务的终止目标并标记为 `killed`（仅修改内存态，绝不在锁内做同步进程查杀）。
    fn detach_all_running_for_kill(&mut self) -> (usize, Vec<u32>, Vec<ChildProcessHandle>) {
        let mut count = 0usize;
        let mut seed_pids = Vec::new();
        let mut children = Vec::new();

        let running_ids: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_, t)| t.status == "running")
            .map(|(id, _)| id.clone())
            .collect();

        for id in running_ids {
            count += 1;
            if let Some(task) = self.tasks.get_mut(&id) {
                for pid in extract_task_seed_pids(task) {
                    if !seed_pids.contains(&pid) {
                        seed_pids.push(pid);
                    }
                }
                task.status = "killed".to_string();
            }
            if let Some(child_arc) = self.child_processes.remove(&id) {
                children.push(child_arc);
            }
        }

        (count, seed_pids, children)
    }

    /// 从命令字符串中检测服务端口和任务类型
    ///
    /// 支持显式端口参数（`--port`/`-p`）和框架默认端口推断
    fn detect_port_and_type(command: &str, dir: Option<&str>) -> (Option<u16>, Option<String>) {
        let lower = command.to_lowercase();
        let dir_lower = dir.map(|d| d.to_lowercase()).unwrap_or_default();

        let task_type: Option<String> = {
            // 优先根据目录名判断
            if dir_lower.contains("frontend")
                || dir_lower.contains("client")
                || dir_lower.contains("web")
                || dir_lower.ends_with("/fe")
            {
                Some("frontend".to_string())
            } else if dir_lower.contains("backend")
                || dir_lower.contains("server")
                || dir_lower.contains("api")
                || dir_lower.ends_with("/be")
            {
                Some("backend".to_string())
            } else if lower.contains("vite")
                || lower.contains("vue-cli-service serve")
                || lower.contains("next dev")
                || lower.contains("nuxt dev")
            {
                Some("frontend".to_string())
            } else if lower.contains("python")
                || lower.contains("flask")
                || lower.contains("uvicorn")
                || lower.contains("cargo run")
            {
                Some("backend".to_string())
            } else {
                // npm run dev/node 命令无法仅从命令字符串判断类型，留空
                None
            }
        };

        let port = if lower.contains("--port") || lower.contains("-p ") {
            let parts: Vec<&str> = command.split_whitespace().collect();
            for i in 0..parts.len() {
                if parts[i] == "--port" && i + 1 < parts.len() {
                    if let Ok(p) = parts[i + 1].parse::<u16>() {
                        return (Some(p), task_type);
                    }
                }
                if parts[i].starts_with("-p") {
                    let port_str = if parts[i] == "-p" && i + 1 < parts.len() {
                        parts[i + 1]
                    } else {
                        &parts[i][2..]
                    };
                    if let Ok(p) = port_str.parse::<u16>() {
                        return (Some(p), task_type);
                    }
                }
            }
            None
        } else if lower.contains("vite") {
            Some(5173)
        } else if lower.contains("next dev") || lower.contains("nuxt dev") {
            Some(3000)
        } else if lower.contains("npm run dev") || lower.contains("npm start") {
            // 根据目录推断端口：backend 通常是 3000/8000，frontend 通常是 5173
            if dir_lower.contains("backend")
                || dir_lower.contains("server")
                || dir_lower.contains("api")
            {
                Some(3000)
            } else {
                Some(5173)
            }
        } else if lower.contains("flask run") {
            Some(5000)
        } else if lower.contains("uvicorn") {
            Some(8000)
        } else if lower.contains("cargo run") && lower.contains("tauri") {
            Some(1420)
        } else {
            None
        };

        (port, task_type)
    }

    /// 启动后台任务
    ///
    /// 通过 PowerShell 执行命令，异步捕获输出，任务完成后推送通知
    pub async fn run(
        app: tauri::AppHandle,
        command: String,
        dir: Option<String>,
        session_id: Option<String>,
    ) -> String {
        let task_id = uuid::Uuid::new_v4().to_string()[..8].to_string();

        let mut short_cmd = command.clone();
        if short_cmd.len() > 80 {
            short_cmd.truncate(80);
            short_cmd.push_str("...");
        }

        let (detected_port, task_type) = Self::detect_port_and_type(&command, dir.as_deref());

        let state = match app.try_state::<BackgroundState>() {
            Some(s) => s,
            // 无全局状态（理论不发生）：诚实返回失败，而不是假"started"
            None => {
                return format!(
                    "Background task {} failed: background state unavailable",
                    task_id
                );
            }
        };
        let state_clone = state.0.clone();
        {
            let mut bg = state_clone.lock().await;
            bg.tasks.insert(
                task_id.clone(),
                BackgroundTask {
                    id: task_id.clone(),
                    session_id: session_id.clone(),
                    command: command.clone(),
                    status: "running".to_string(),
                    result: None,
                    port: detected_port,
                    task_type: task_type.clone(),
                    pid: None,
                    pids: vec![],
                },
            );
        }

        let app_handle = app.clone();
        let task_id_async = task_id.clone();
        let cmd_async = command.clone();
        let port_async = detected_port;
        let type_async = task_type.clone();
        let session_id_clone = session_id.clone();

        // —— 进程 spawn 挪到 run() 本体（原在 tokio::spawn 闭包内）——
        // 这样返回给 tool_result 的文案能反映"启动后瞬间的真实状态"，
        // 而不是无条件的 started（秒挂的命令会被当场拦截，见下）。
        let target_dir = dir.clone().unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });

        // 与前台 run_shell_async 共用同一方言判定：后台任务不能因为走了另一条
        // 代码路径就换回 PowerShell（否则同一条命令前台能跑、后台语法报错）。
        let (shell, shell_args): (String, Vec<String>) =
            match crate::infra::shell_dialect::current() {
                crate::infra::shell_dialect::ShellDialect::PowerShell => {
                    // PS 5.1 不支持 `&&`，公共层把链式命令展开成"逐段执行 + 前段失败即停"。
                    let ps_cmd = crate::infra::shell_command::build_windows_ps_command(&cmd_async);
                    (
                        "powershell".to_string(),
                        vec!["-NoProfile".to_string(), "-Command".to_string(), ps_cmd],
                    )
                }
                crate::infra::shell_dialect::ShellDialect::Bash { program, .. } => (
                    program.to_string_lossy().to_string(),
                    vec!["-c".to_string(), cmd_async.clone()],
                ),
            };

        let mut cmd = tokio::process::Command::new(&shell);
        cmd.no_window().current_dir(&target_dir).args(&shell_args);

        let mut child = match cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
            Ok(c) => c,
            Err(e) => {
                // spawn 失败：标记 error；错误经返回值直接进 tool_result
                //（模型与用户当轮可见，无需再走通知队列）
                let msg = format!("Failed to spawn: {}", e);
                if let Some(st) = app_handle.try_state::<BackgroundState>() {
                    let mut bg = st.0.lock().await;
                    if let Some(task) = bg.tasks.get_mut(&task_id_async) {
                        task.status = "error".to_string();
                        task.result = Some(msg.clone());
                    }
                }
                return format!("Background task {} failed to start: {}", task_id, msg);
            }
        };

        // —— 快速失败探测 ——
        // 秒挂的命令（如 shell 语法错误）在数百毫秒内退出。轮询 try_wait，
        // 命中则把错误输出直接带回 tool_result——模型当轮就能看到失败，
        // 不必等下一轮 <background-results> 注入后才自我纠正。
        let mut quick_exit: Option<String> = None;
        for _ in 0..8 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            match child.try_wait() {
                Ok(Some(status)) => {
                    // 进程已退出：管道 EOF 已到，残留输出可立即读尽
                    let mut out = String::new();
                    {
                        use tokio::io::AsyncReadExt;
                        if let Some(mut so) = child.stdout.take() {
                            let _ = AsyncReadExt::read_to_string(&mut so, &mut out).await;
                        }
                        if let Some(mut se) = child.stderr.take() {
                            let _ = AsyncReadExt::read_to_string(&mut se, &mut out).await;
                        }
                    }
                    let text = if out.trim().is_empty() {
                        format!("(no output, exit code {:?})", status.code())
                    } else {
                        out
                    };
                    quick_exit = Some(format!("[error] exit code: {:?}\n{}", status.code(), text));
                    break;
                }
                Ok(None) => {}   // 仍在运行：继续轮询
                Err(_) => break, // 探测异常：交给异步路径兜底
            }
        }
        if let Some(err_text) = quick_exit {
            // 与闭包内 error 收尾同口径：task 标 error + emit + 入通知队列
            let notif = Notification {
                task_id: task_id_async.clone(),
                session_id: session_id_clone.clone(),
                status: "error".to_string(),
                command: cmd_async.clone(),
                result: err_text.clone(),
                port: port_async,
                task_type: type_async.clone(),
            };
            let _ = app_handle.emit("bg-task-done", &notif);
            if let Some(st) = app_handle.try_state::<BackgroundState>() {
                let mut bg = st.0.lock().await;
                if let Some(task) = bg.tasks.get_mut(&task_id_async) {
                    task.status = "error".to_string();
                    task.result = Some(err_text.clone());
                }
            }
            return format!(
                "Background task {} failed immediately:\n{}",
                task_id, err_text
            );
        }

        // —— 正常路径：句柄提取与注册（原闭包内逻辑挪出）——
        let child_pid = child.id();
        // 先提取 stdout/stderr pipe，避免延迟期间管道缓冲区满导致进程阻塞
        let child_stdout = child.stdout.take();
        let child_stderr = child.stderr.take();
        let child_arc = Arc::new(tokio::sync::Mutex::new(Some(child)));
        {
            let mut bg = state_clone.lock().await;
            bg.child_processes
                .insert(task_id.clone(), child_arc.clone());
            if let (Some(pid), Some(task)) = (child_pid, bg.tasks.get_mut(&task_id)) {
                // 先存 PowerShell PID 作为 root，稍后延迟捕获完整进程树
                task.pid = Some(pid);
            }
        }

        // 立即启动 stdout/stderr 读取，避免管道阻塞
        let output_buffer = Arc::new(tokio::sync::Mutex::new(String::new()));
        let max_output = crate::infra::types::constants::MAX_BACKGROUND_OUTPUT_LEN;

        if let Some(stdout) = child_stdout {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            let task_id_for_stdout = task_id.clone();
            let buf = output_buffer.clone();
            tokio::spawn(async move {
                while let Ok(Some(line)) = lines.next_line().await {
                    println!("[bg:{}] {}", task_id_for_stdout, line);
                    let mut b = buf.lock().await;
                    if b.len() < max_output {
                        b.push_str(&line);
                        b.push('\n');
                    }
                }
            });
        }

        if let Some(stderr) = child_stderr {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            let task_id_for_stderr = task_id.clone();
            let buf = output_buffer.clone();
            tokio::spawn(async move {
                while let Ok(Some(line)) = lines.next_line().await {
                    println!("[bg:{} ERR] {}", task_id_for_stderr, line);
                    let mut b = buf.lock().await;
                    if b.len() < max_output {
                        b.push_str(&line);
                        b.push('\n');
                    }
                }
            });
        }

        tokio::spawn(async move {
            // 延迟等待子进程树完全展开（npm/node 等需要时间启动），再递归捕获全部 PID
            // 此时 stdout/stderr 已在后台读取，不会阻塞进程
            tokio::time::sleep(Duration::from_millis(1500)).await;
            if let Some(root_pid) = child_pid {
                let descendants =
                    tokio::task::spawn_blocking(move || find_descendant_pids(root_pid))
                        .await
                        .unwrap_or_default();
                let mut all_pids = vec![root_pid];
                all_pids.extend(descendants);
                let mut bg = state_clone.lock().await;
                if let Some(task) = bg.tasks.get_mut(&task_id_async) {
                    task.pids = all_pids;
                }
            }

            // 取出 child 进行 wait
            let mut child_owned = {
                let mut guard = child_arc.lock().await;
                match guard.take() {
                    Some(c) => c,
                    None => {
                        // child 已被 kill_task 取走，直接退出
                        return;
                    }
                }
            };

            let status = match child_owned.wait().await {
                Ok(s) => {
                    if s.success() {
                        "completed"
                    } else {
                        "error"
                    }
                }
                Err(_) => "error",
            };

            // 等待一小段时间让 stdout/stderr 任务完成写入
            tokio::time::sleep(Duration::from_millis(50)).await;

            let final_output = {
                let buf = output_buffer.lock().await;
                if buf.is_empty() {
                    "(process finished)".to_string()
                } else {
                    buf.clone()
                }
            };

            let notif = Notification {
                task_id: task_id_async.clone(),
                session_id: session_id_clone.clone(),
                status: status.to_string(),
                command: cmd_async.clone(),
                result: final_output,
                port: port_async,
                task_type: type_async,
            };

            // Tauri 事件推送（实时通知前端，替代轮询）
            let _ = app_handle.emit("bg-task-done", &notif);

            let mut was_killed = false;
            if let Some(st) = app_handle.try_state::<BackgroundState>() {
                let mut bg = st.0.lock().await;
                if let Some(task) = bg.tasks.get_mut(&task_id_async) {
                    // 先检查是否已被用户主动 kill，再决定是否覆盖状态（此前先覆盖再判断导致永远为 false）
                    was_killed = task.status == "killed";
                    if !was_killed {
                        task.status = status.to_string();
                    }
                    task.result = Some(notif.result.clone());
                } else {
                    // 记录已被 `kill_task` 移除也属于用户主动终止，不触发失败提醒
                    was_killed = true;
                }
            }

            // 失败小字提醒：任务以 error 结束（且不是用户主动 kill）时，
            // 发结构化事件让前端在聊天流里显示 notice 小字。用户看到后
            // 自行决定是否让 Agent 排查。
            // 秒挂场景不走这里：错误已经在启动时的 tool_result 里带回。
            if status == "error" && !was_killed {
                // 错误详情接入提醒（截断到合理长度，完整输出仍在任务面板）
                let mut summary = notif.result.trim().to_string();
                if summary.chars().count() > 400 {
                    summary = summary.chars().take(400).collect::<String>() + "…";
                }
                // 字段全部从 notif 取：notif 构造时已 move 掉原变量，这里不能再借用
                let _ = app_handle.emit(
                    "background-failed",
                    crate::infra::types::events::BackgroundFailedEvent {
                        session_id: notif.session_id,
                        task_id: notif.task_id,
                        command: notif.command,
                        result: summary,
                        port: notif.port,
                        task_type: notif.task_type,
                    },
                );
            }
        });

        let type_info = task_type
            .as_ref()
            .map(|t| format!(" [{}]", t))
            .unwrap_or_default();
        let port_info = detected_port
            .map(|p| format!(" :{}", p))
            .unwrap_or_default();
        // P0-2：返回"已提交"而非"已启动"——spawn 成功不代表命令会跑成功。
        // 秒挂的命令已在上面被快速失败探测拦截；此处能返回即表示进程仍在运行，
        // 但执行结果（如 npm install 是否成功）尚未确认。失败时前端会以小字
        // 提醒用户（background-failed 事件），由用户决定是否让 Agent 排查；
        // 模型上下文里不再注入后台任务结果。
        format!(
            "Background task {} submitted{}{}: {} (已提交，尚未确认执行结果；若任务失败，界面会提醒用户，可发消息让 Agent 排查)",
            task_id, type_info, port_info, short_cmd
        )
    }

    /// 移除单个后台任务（无论状态）
    fn remove_task(&mut self, task_id: &str) {
        self.tasks.remove(task_id);
        self.child_processes.remove(task_id);
        println!("[BACKGROUND] Dismissed task {}", task_id);
    }

    /// 清理指定会话的所有非 running 任务
    fn remove_session_tasks(&mut self, session_id: &str) {
        let ids: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_, t)| t.session_id.as_deref() == Some(session_id) && t.status != "running")
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            self.tasks.remove(id);
            self.child_processes.remove(id);
        }
        println!(
            "[BACKGROUND] Cleared {} tasks for session {}",
            ids.len(),
            session_id
        );
    }

    /// 清理已完成且子进程句柄已释放的后台任务，防止内存无限增长
    fn cleanup_expired(&mut self) {
        // 清理那些已经不在 child_processes 中的非 running 任务
        // child_processes 在任务完成时会被移除 (task 完成逻辑中 child_processes.remove)
        self.tasks
            .retain(|id, task| task.status == "running" || self.child_processes.contains_key(id));
    }

    /// 查询任务状态
    ///
    /// 提供 task_id 时返回单个任务详情，否则返回所有任务摘要
    pub async fn check(app: &tauri::AppHandle, task_id: Option<String>) -> String {
        if let Some(state) = app.try_state::<BackgroundState>() {
            let mut bg = state.0.lock().await;
            bg.cleanup_expired();
            if let Some(tid) = task_id {
                if let Some(t) = bg.tasks.get(&tid) {
                    let mut short_cmd = t.command.clone();
                    if short_cmd.len() > 60 {
                        short_cmd.truncate(60);
                        short_cmd.push_str("...");
                    }
                    format!(
                        "[{}] {}\n{}",
                        t.status,
                        short_cmd,
                        t.result.as_deref().unwrap_or("(running)")
                    )
                } else {
                    format!("Error: Unknown task {}", tid)
                }
            } else {
                let mut lines = Vec::new();
                for (tid, t) in &bg.tasks {
                    let mut short_cmd = t.command.clone();
                    if short_cmd.len() > 60 {
                        short_cmd.truncate(60);
                        short_cmd.push_str("...");
                    }
                    lines.push(format!("{}: [{}] {}", tid, t.status, short_cmd));
                }
                if lines.is_empty() {
                    "No background tasks.".to_string()
                } else {
                    lines.join("\n")
                }
            }
        } else {
            "Error: Background state not initialized.".to_string()
        }
    }

    /// 终止所有运行中的后台任务（撤回文件前调用，释放文件锁）。
    ///
    /// 先持短锁摘取目标 PID 与子进程句柄后立即释放锁，再在 `spawn_blocking` 中批量查杀，
    /// 避免在持有 `BackgroundState` 锁或 Tokio worker 上同步阻塞。
    pub async fn kill_all_background(app: &tauri::AppHandle) {
        let Some(state) = app.try_state::<BackgroundState>() else {
            return;
        };
        let (count, seed_pids, children) = {
            let mut bg = state.0.lock().await;
            bg.detach_all_running_for_kill()
        };

        for child_arc in children {
            if let Ok(mut guard) = child_arc.try_lock() {
                if let Some(ref mut child) = *guard {
                    let _ = child.start_kill();
                }
            }
        }

        if !seed_pids.is_empty() {
            let _ =
                tokio::task::spawn_blocking(move || kill_pids_and_descendants(&seed_pids)).await;
        }
        println!("[BACKGROUND] Killed {} running tasks", count);
    }

    /// 移除单个后台任务（仅清理记录，不杀进程）
    pub async fn dismiss_task(app: &tauri::AppHandle, task_id: &str) -> bool {
        if let Some(state) = app.try_state::<BackgroundState>() {
            let mut bg = state.0.lock().await;
            bg.remove_task(task_id);
            true
        } else {
            false
        }
    }

    /// 同步杀所有运行中任务的进程树（用于退出时清理，不依赖 tokio runtime）
    pub fn kill_all_process_tree(&mut self) {
        let mut all_seeds = Vec::new();
        let running_ids: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_, task)| task.status == "running")
            .map(|(id, _)| id.clone())
            .collect();

        for task_id in &running_ids {
            if let Some(task) = self.tasks.get(task_id) {
                for pid in extract_task_seed_pids(task) {
                    if !all_seeds.contains(&pid) {
                        all_seeds.push(pid);
                    }
                }
            }
            if let Some(child_arc) = self.child_processes.remove(task_id) {
                if let Ok(mut guard) = child_arc.try_lock() {
                    if let Some(ref mut child) = *guard {
                        let _ = child.start_kill();
                    }
                }
            }
        }
        if !all_seeds.is_empty() {
            let _ = kill_pids_and_descendants(&all_seeds);
        }
        self.tasks.clear();
        self.child_processes.clear();
    }

    /// 终止并移除单个后台任务（短锁摘取目标 + 锁外 `spawn_blocking` 终止整棵进程树）
    pub async fn kill_task(app: &tauri::AppHandle, task_id: &str) -> bool {
        let Some(state) = app.try_state::<BackgroundState>() else {
            return false;
        };

        let (seed_pids, child_arc) = {
            let mut bg = state.0.lock().await;
            let seeds = bg
                .tasks
                .get(task_id)
                .map(extract_task_seed_pids)
                .unwrap_or_default();
            let child = bg.child_processes.remove(task_id);
            bg.remove_task(task_id);
            (seeds, child)
        };

        if let Some(child_arc) = child_arc {
            if let Ok(mut guard) = child_arc.try_lock() {
                if let Some(ref mut child) = *guard {
                    let _ = child.start_kill();
                }
            }
        }

        let all_targets = if seed_pids.is_empty() {
            vec![]
        } else {
            tokio::task::spawn_blocking(move || kill_pids_and_descendants(&seed_pids))
                .await
                .unwrap_or_default()
        };

        let killed = !all_targets.is_empty();
        println!(
            "[BACKGROUND] Killed task {} (pids: {:?}, effective: {})",
            task_id, all_targets, killed
        );
        killed
    }

    /// 清理指定会话的非运行中任务
    pub async fn clear_session_tasks(app: &tauri::AppHandle, session_id: &str) -> usize {
        if let Some(state) = app.try_state::<BackgroundState>() {
            let mut bg = state.0.lock().await;
            let before = bg.tasks.len();
            bg.remove_session_tasks(session_id);
            before - bg.tasks.len()
        } else {
            0
        }
    }
}

/// Tauri 状态包装器，用于注入到应用状态管理
pub struct BackgroundState(pub Arc<Mutex<BackgroundManager>>);

impl Default for BackgroundState {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(BackgroundManager::new())))
    }
}

/// 记录当前正在进行上下文压缩的会话，用于前端 F5 刷新后恢复"压缩中"状态
pub struct CompactingState(pub std::sync::Mutex<std::collections::HashSet<String>>);

impl Default for CompactingState {
    fn default() -> Self {
        Self(std::sync::Mutex::new(std::collections::HashSet::new()))
    }
}

impl CompactingState {
    pub fn is_compacting(&self, session_id: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(session_id)
    }

    /// 原子更新单会话压缩状态；开始时返回 false 表示已有压缩任务占用。
    pub fn set_compacting(&self, session_id: &str, active: bool) -> bool {
        let mut set = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active {
            set.insert(session_id.to_string())
        } else {
            set.remove(session_id)
        }
    }
}

#[cfg(test)]
mod process_tree_tests {
    use super::*;

    #[test]
    fn parses_parent_child_edges_and_skips_malformed_lines() {
        let sample = "100 200\n  200   301 \nbad line\n200 0\n301 401\n";
        assert_eq!(
            parse_process_edges(sample),
            vec![(100, 200), (200, 301), (301, 401)]
        );
    }

    #[test]
    fn collects_multilevel_descendants_and_handles_cycles() {
        let edges = vec![
            (100, 200),
            (100, 201),
            (200, 300),
            (300, 100), // 构造环路验证不死循环
            (999, 888), // 无关进程树不应被误伤
        ];
        let mut descendants = collect_descendants_from_edges(&[100], &edges);
        descendants.sort_unstable();
        assert_eq!(descendants, vec![200, 201, 300]);
    }
}
