//! # delete.rs — 安全的文件删除
//!
//! 实现 agent 可调用的 `delete_file` 工具：先记录回滚快照，再把目标"软删除"进
//! **应用数据目录**下的回收站（不是真删）。
//!
//! ## 两条互补的恢复通道（2026-09-21 起打通）
//! - **快照内容**：删前把原内容写进 `snapshot_content` 并记录 `Patch::DeleteFile`。
//!   只有"能读成文本"的文件有这一条（`read_text_preserve_encoding` 对目录/二进制会失败）。
//! - **回收站本体**：把目标整个搬进 `data/trash/<会话 id>/`，位置记进 patch 的
//!   `trash_path`。**目录 / 二进制 / 大文件只有这一条**，回滚时由回滚引擎搬回原位。
//!
//! 所以"删除"在这套系统里是可逆的：文本靠快照、其余靠回收站；两条都失效才会真的丢。
//!
//! ## 约束
//! - 回收站固定位于应用数据目录（见 `core::rollback::trash::trash_root`），
//!   **不放在被删对象旁边** —— 放旁边会形成自嵌套。
//! - 跨卷时 `rename` 会失败，由 `trash::move_to` 退化为"复制后删源"。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::rollback::trash;
use crate::core::rollback::Patch;
use crate::core::tools::framework;
use crate::core::tools::framework::permission::ensure_path_permission;

use super::common::{read_text_preserve_encoding, resolve_path};
use super::workspace::{get_workspace, record_patch_to_snapshot, resolve_exec_path, sandbox_missing_hint};

/// 回收站内该对象的目标路径：`<回收站根>/<时间戳>_<原名>`。
///
/// 时间戳前缀是为了同一个名字被多次删除时不互相覆盖。
fn trash_dest(session_id: &str, filename: &str, ts: u64) -> PathBuf {
    trash::trash_root(session_id).join(format!("{}_{}", ts, filename))
}

/// 把底层 IO 错误翻译成模型能用的话。
///
/// 原来的 `删除失败: 参数错误。 (os error 87)` 对模型毫无指导性（听起来像自己参数写错了），
/// 它只能继续换策略瞎试——2026-09-21 那次空转好几分钟就有这句话的份。
/// 这里补上"是什么情况"和"要不要再试"两件事。
fn delete_failure_message(err: &std::io::Error, path: &Path) -> String {
    let (hint, retryable) = match err.raw_os_error() {
        // ERROR_ACCESS_DENIED / ERROR_SHARING_VIOLATION：目标被占用或受保护
        Some(5) | Some(32) => (
            "目标正被其他程序占用（可能是某个进程的工作目录，或文件已被打开）",
            true,
        ),
        // ERROR_INVALID_PARAMETER：路径不合法（含往自身内部移动）
        Some(87) => ("目标路径不合法（不能把目录移进它自己内部）", false),
        // ERROR_NOT_SAME_DEVICE：跨盘——正常应走复制路径，走到这里说明复制也失败了
        Some(17) => ("跨磁盘移动且复制失败", false),
        _ => ("", true),
    };
    let tail = if retryable {
        "可以先确认目标是否被占用；若再次失败，请换方案或向主 Agent 报告受阻点，不要对同一目标反复重试。"
    } else {
        "这属于结构性失败，重试不会成功：请立即停止重试，向主 Agent 报告受阻点，或请用户手动处理。"
    };
    format!(
        "删除失败: {}{}。目标: {}\n{}",
        err,
        if hint.is_empty() {
            String::new()
        } else {
            format!("（{hint}）")
        },
        path.display(),
        tail
    )
}

pub async fn delete_file(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> framework::ToolCallResult {
    let path = resolve_path(input);
    let ws = get_workspace(app, session_id).await;
    if let Err(e) = ensure_path_permission(app, &path, "删除", ws.as_deref()).await {
        return framework::ToolCallResult::error(e);
    }
    // 执行层与校验层对齐：相对路径按沙箱目录解析（而非进程 CWD），否则会越界删除
    let path = resolve_exec_path(&path, ws.as_deref());
    let src = Path::new(&path);

    if !src.exists() {
        return framework::ToolCallResult::error(format!(
            "文件不存在: {}{}",
            path,
            sandbox_missing_hint(ws.as_deref())
        ));
    }

    // 恢复通道一：把原内容存进 snapshot_content（快照只存 hash）。
    // 目录 / 二进制 / 大文件读不出来 → None → 走通道二（回收站本体）。
    let content_hash = read_text_preserve_encoding(&path).ok().map(|d| {
        let hash = Patch::content_hash(&d.content);
        let _ = crate::core::rollback::store::save_content(session_id, &hash, &d.content);
        hash
    });

    let filename = src
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let trash_path = trash_dest(session_id, filename, ts);
    if let Some(dir) = trash_path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return framework::ToolCallResult::error(format!(
                "删除失败：无法创建回收目录 {}：{}",
                dir.display(),
                e
            ));
        }
    }

    // 恢复通道二：把本体搬进回收站
    match trash::move_to(src, &trash_path) {
        Ok(()) => {
            record_patch_to_snapshot(
                app,
                session_id,
                Patch::DeleteFile {
                    path: path.to_string(),
                    content_hash,
                    // 记下本体位置：目录 / 二进制 / 大文件在快照里没有内容，回滚时靠它搬回来
                    trash_path: Some(trash_path.to_string_lossy().to_string()),
                },
                Some(format!("删除 {} → {}", path, trash_path.display())),
            )
            .await;
            framework::ToolCallResult::ok(format!(
                "已删除: {}（已移入回收站 {}）",
                path,
                trash_path.display()
            ))
        }
        Err(e) => framework::ToolCallResult::error(delete_failure_message(&e, src)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 结构性失败必须给出"别再重试"的明确指示 —— 这句是那次空转的直接解药
    #[test]
    fn failure_message_tells_the_model_to_stop_for_structural_errors() {
        let invalid = std::io::Error::from_raw_os_error(87);
        let msg = delete_failure_message(&invalid, Path::new("C:/ws/.jarvis_trash"));
        assert!(msg.contains("立即停止重试"), "应明确叫停：{msg}");
        assert!(msg.contains("目标路径不合法"), "应说明是什么情况：{msg}");

        // 占用类（可能只是一时）不叫停，但要求确认后再决定
        let busy = std::io::Error::from_raw_os_error(32);
        let msg = delete_failure_message(&busy, Path::new("C:/ws/a.txt"));
        assert!(msg.contains("被其他程序占用"), "应说明是什么情况：{msg}");
        assert!(!msg.contains("立即停止重试"), "占用类不该一律叫停：{msg}");
    }

    /// 回收站目标路径的形态：带时间戳前缀、落在应用数据目录下。
    #[test]
    fn trash_dest_carries_timestamp_and_lives_under_the_trash_root() {
        let dest = trash_dest("sess-1", "a.txt", 1789944415);
        assert!(dest.starts_with(trash::trash_root("sess-1")));
        assert_eq!(
            dest.file_name().unwrap().to_string_lossy(),
            "1789944415_a.txt"
        );
    }
}
