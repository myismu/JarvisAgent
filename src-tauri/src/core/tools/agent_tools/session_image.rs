//! # core/tools/agent_tools/session_image.rs — 会话历史图片按需回看工具
//!
//! 往轮图片会被 `restore_image_data` 折叠成 `[图片: image/png | filename=...]` 节省 Token。
//! 当用户在后续对话追问历史图片细节时，模型可通过按需工具 `ViewSessionImage` 重新挂载
//! 当前会话内的指定图片（后端强制按当前 `session_id` 隔离校验，禁止跨会话查询）。

use crate::core::session::resource_repository;
use crate::core::tools::framework::ToolCallResult;
use tauri::Manager;

/// 剥离可能带有的 `attachment://` 前缀并清理空白
fn normalize_attachment_name(raw: &str) -> &str {
    let trimmed = raw.trim();
    trimmed
        .strip_prefix("attachment://")
        .or_else(|| trimmed.strip_prefix("ATTACHMENT://"))
        .unwrap_or(trimmed)
}

/// `ViewSessionImage` 执行入口
pub async fn view_session_image(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> ToolCallResult {
    let raw_name = input["filename"]
        .as_str()
        .map(normalize_attachment_name)
        .filter(|s| !s.is_empty());

    // 未传 filename 时，列出当前会话可回看的全部图片附件名
    let Some(target_name) = raw_name else {
        return match resource_repository::list_session_attachments(session_id) {
            Ok(list) if list.is_empty() => {
                ToolCallResult::ok("当前会话中暂无可回看的图片附件。".to_string())
            }
            Ok(list) => {
                let mut lines = vec![format!("当前会话共有 {} 张可回看的图片：", list.len())];
                for (name, mime) in list {
                    lines.push(format!("- {} ({})", name, mime));
                }
                lines.push(
                    "如需查看具体图片细节，请传入 filename 参数再次调用 ViewSessionImage。"
                        .to_string(),
                );
                ToolCallResult::ok(lines.join("\n"))
            }
            Err(err) => ToolCallResult::error(format!("查询会话图片列表失败: {}", err)),
        };
    };

    // 按当前 session_id 强隔离查询，绝不允许访问其他会话的图片
    let (filename, media_type) = match resource_repository::load_session_attachment_meta(
        session_id,
        target_name,
    ) {
        Ok(Some(meta)) => meta,
        Ok(None) => {
            return ToolCallResult::error(format!(
                    "当前会话中未找到图片 '{}'（只能查看本会话内的图片附件；省略 filename 可列出本会话所有图片）。",
                    target_name
                ));
        }
        Err(err) => return ToolCallResult::error(format!("读取会话图片失败: {}", err)),
    };

    if let Some(sm) = app.try_state::<crate::core::runtime::SessionManager>() {
        let ctx = sm.get_or_create(session_id).await;
        ctx.pending_tool_images
            .lock()
            .await
            .push((media_type.clone(), filename.clone()));
    }

    ToolCallResult::ok(format!(
        "已重新加载当前会话图片 {} ({}) 到本轮视觉上下文（若当前模型支持视觉能力即可直接查看；如需展示给用户可用 ![图片](attachment://{})）。",
        filename, media_type, filename
    ))
}

#[cfg(test)]
mod tests {
    use super::normalize_attachment_name;

    #[test]
    fn normalize_attachment_name_strips_scheme_and_whitespace() {
        assert_eq!(
            normalize_attachment_name("attachment://45dc870d_148726fc.jpg"),
            "45dc870d_148726fc.jpg"
        );
        assert_eq!(
            normalize_attachment_name("  ATTACHMENT://abc_123.png  "),
            "abc_123.png"
        );
        assert_eq!(
            normalize_attachment_name("  plain_name.webp\n"),
            "plain_name.webp"
        );
        assert_eq!(normalize_attachment_name("   "), "");
    }
}
