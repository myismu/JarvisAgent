// --- Shell 命令执行工具模块 ---
// run_shell, git_command, background_run, check_background

use std::process::Command;
use super::permission::request_permission;

/// 执行 PowerShell 命令（阻塞同步）
pub async fn run_shell(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
) -> String {
    let cmd = input["command"].as_str().unwrap_or("");

    // 拦截长周期任务，强制要求使用 background_run
    let lower_cmd = cmd.to_lowercase();
    let is_long_running = lower_cmd.contains("npm run dev") || lower_cmd.contains("npm start") || 
        lower_cmd.contains("yarn dev") || lower_cmd.contains("yarn start") || 
        lower_cmd.contains("pnpm dev") || lower_cmd.contains("pnpm start") || 
        lower_cmd.contains("vite") || lower_cmd.contains("vue-cli-service serve") ||
        (lower_cmd.contains("node ") && (
            lower_cmd.contains("server") || lower_cmd.contains("app.js") || 
            lower_cmd.contains("index.js") || lower_cmd.contains("main.js")
        )) ||
        lower_cmd.contains("python manage.py runserver") ||
        lower_cmd.contains("flask run") || lower_cmd.contains("uvicorn ") ||
        lower_cmd.contains("npx serve") || lower_cmd.contains("http-server");
    if is_long_running {
        return "错误：检测到您正在尝试启动服务或长周期任务。run_shell 是阻塞同步的，会导致对话卡死！请改用 `background_run` 工具来执行此命令。".to_string();
    }
    if lower_cmd.contains("dir node_modules") || lower_cmd.contains("ls node_modules") {
        return "错误：严禁使用 run_shell 查看 node_modules 目录！它会返回数千行无用文本导致系统崩溃。请改用专用的 list_directory 工具，或者假定依赖已安装直接启动服务。".to_string();
    }

    // 高风险命令拦截
    let dangerous_keywords = [
        "del ", "rm ", "format ", "rd ", "rmdir ",
        "remove-item", "clear-content", "stop-process", "kill ",
    ];
    if dangerous_keywords.iter().any(|k| lower_cmd.contains(k)) {
        let msg = format!("警告：高风险命令：{}", cmd);
        let decision = request_permission(app, &msg).await;
        if decision == "reject" {
            return "权限拒绝".to_string();
        }
    }

    // 直接拦截网络下载命令
    let network_keywords = ["invoke-webrequest", "iwr ", "wget ", "curl "];
    if network_keywords.iter().any(|k| lower_cmd.contains(k)) {
        return "错误：禁止在 run_shell 中使用网络下载命令（Invoke-WebRequest/iwr/wget/curl）。这类命令会触发 PowerShell 安全确认框，导致进程在后台永久等待 stdin 而卡死。如需下载，请告知用户手动操作。".to_string();
    }

    let ps_cmd = format!(
        "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; {}",
        cmd
    );
    let project_root = std::env::current_dir().unwrap_or_default();

    match Command::new("powershell")
        .current_dir(&project_root)
        .args(&["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
        .output()
    {
        Ok(output) => {
            let out = String::from_utf8_lossy(&output.stdout);
            let err = String::from_utf8_lossy(&output.stderr);
            format!("STDOUT: {}\nSTDERR: {}", out, err)
        }
        Err(e) => format!("执行失败: {}", e),
    }
}

/// 执行只读 git 命令
pub async fn git_command(
    _app: &tauri::AppHandle,
    input: &serde_json::Value,
) -> String {
    let args_value = input["args"].as_array().unwrap();
    let args: Vec<&str> = args_value.iter().filter_map(|v| v.as_str()).collect();

    let dangerous_git_args = [
        "push", "commit", "rebase", "reset", "revert", "clean", "checkout",
    ];
    if args
        .iter()
        .any(|arg| dangerous_git_args.contains(&arg.to_lowercase().as_str()))
    {
        return format!(
            "安全拦截：git_command 工具仅用于只读操作，禁止执行 '{}'。",
            args.join(" ")
        );
    }

    let project_root = std::env::current_dir().unwrap_or_default();
    match Command::new("git")
        .current_dir(&project_root)
        .args(&args)
        .output()
    {
        Ok(output) => {
            let out = String::from_utf8_lossy(&output.stdout);
            let err = String::from_utf8_lossy(&output.stderr);
            format!("STDOUT: {}\nSTDERR: {}", out, err)
        }
        Err(e) => format!("Git 命令执行失败: {}", e),
    }
}

/// 后台执行长时间运行的命令
pub async fn background_run(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
) -> String {
    let cmd = input["command"].as_str().unwrap_or("");
    let dir = input["dir"].as_str().map(|s| s.to_string());
    crate::core::background::BackgroundManager::run(app.clone(), cmd.to_string(), dir).await
}

/// 检查后台任务状态
pub async fn check_background(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
) -> String {
    let task_id = input["task_id"].as_str().map(|s| s.to_string());
    crate::core::background::BackgroundManager::check(app, task_id).await
}
