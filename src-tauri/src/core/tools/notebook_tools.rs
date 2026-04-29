//! # notebook_tools.rs — Jupyter Notebook cell 级编辑工具
//!
//! `.ipynb` 文件本质是 JSON，直接用文本替换容易破坏结构或误改 outputs。
//! 本模块提供 cell 级别的 replace / insert / delete 操作。

use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tauri::{Emitter, Manager};

use super::permission::ensure_path_permission;
use super::registry::ToolDef;
use crate::core::session::checkpoint::{self, FileOperation, OpType};
use crate::core::snapshot_engine::Patch;
use crate::core::SnapshotRegistry;

const IPYNB_INDENT: &[u8] = b" ";

#[derive(Debug, Clone, PartialEq, Eq)]
struct NotebookEditOutcome {
    cell_id: Option<String>,
    cell_type: String,
    language: String,
    edit_mode: String,
}

async fn get_workspace(app: &tauri::AppHandle, session_id: &str) -> Option<PathBuf> {
    if let Some(manager) = app.try_state::<crate::core::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        return ctx.workspace.lock().await.clone();
    }
    None
}

async fn record_operation(app: &tauri::AppHandle, session_id: &str, operation: FileOperation) {
    if let Some(manager) = app.try_state::<crate::core::state::SessionManager>() {
        let ctx = manager.get_or_create(session_id).await;
        let mut ops = ctx.pending_checkpoint.lock().await;
        ops.push(operation);
    }
}

async fn record_patch_to_snapshot(
    app: &tauri::AppHandle,
    session_id: &str,
    patch: Patch,
    message: Option<String>,
) {
    if let Some(registry) = app.try_state::<SnapshotRegistry>() {
        let mgr_result = registry.0.read().await.get_or_create(session_id).await;
        if let Ok(mgr) = mgr_result {
            if let Ok(snapshot) = mgr
                .create_snapshot(vec![patch], message, None, None, None)
                .await
            {
                let _ = app.emit(
                    "snapshot-created",
                    json!({
                        "sessionId": session_id,
                        "snapshotId": snapshot.id
                    }),
                );
            }
        }
    }
}

fn compute_diff(old_text: &str, new_text: &str) -> crate::core::snapshot_engine::patch::TextDiff {
    use crate::core::snapshot_engine::patch::{DiffHunk, DiffLine, TextDiff};
    use similar::{ChangeTag, TextDiff as SimilarDiff};

    let diff = SimilarDiff::from_lines(old_text, new_text);
    let mut hunks = Vec::new();

    for op in diff.ops() {
        let old_start = op.old_range().start as u32;
        let new_start = op.new_range().start as u32;
        let old_len = op.old_range().len() as u32;
        let new_len = op.new_range().len() as u32;

        let mut lines = Vec::new();
        for change in diff.iter_changes(op) {
            let content = change.to_string();
            match change.tag() {
                ChangeTag::Equal => lines.push(DiffLine::Context { content }),
                ChangeTag::Insert => lines.push(DiffLine::Addition { content }),
                ChangeTag::Delete => lines.push(DiffLine::Deletion { content }),
            }
        }

        if !lines.is_empty() {
            hunks.push(DiffHunk {
                old_start,
                old_lines: old_len,
                new_start,
                new_lines: new_len,
                lines,
            });
        }
    }

    TextDiff { hunks }
}

fn resolve_notebook_path(raw_path: &str, workspace: Option<&Path>) -> PathBuf {
    let path = Path::new(raw_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace
            .map(Path::to_path_buf)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
            .join(path)
    }
}

fn has_ipynb_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("ipynb"))
        .unwrap_or(false)
}

fn parse_cell_index(cell_id: &str) -> Option<usize> {
    cell_id
        .strip_prefix("cell-")
        .unwrap_or(cell_id)
        .parse::<usize>()
        .ok()
}

fn find_cell_index(cells: &[Value], cell_id: &str) -> Option<usize> {
    cells
        .iter()
        .position(|cell| cell.get("id").and_then(Value::as_str) == Some(cell_id))
        .or_else(|| parse_cell_index(cell_id).filter(|idx| *idx <= cells.len()))
}

fn notebook_language(notebook: &Value) -> String {
    notebook
        .pointer("/metadata/language_info/name")
        .and_then(Value::as_str)
        .unwrap_or("python")
        .to_string()
}

fn notebook_uses_cell_ids(notebook: &Value) -> bool {
    let nbformat = notebook
        .get("nbformat")
        .and_then(Value::as_u64)
        .unwrap_or(4);
    let minor = notebook
        .get("nbformat_minor")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    nbformat > 4 || (nbformat == 4 && minor >= 5)
}

fn generate_cell_id() -> String {
    uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(12)
        .collect()
}

fn make_cell(cell_type: &str, source: &str, cell_id: Option<String>) -> Value {
    let mut cell = serde_json::Map::new();
    cell.insert(
        "cell_type".to_string(),
        Value::String(cell_type.to_string()),
    );
    if let Some(id) = cell_id {
        cell.insert("id".to_string(), Value::String(id));
    }
    cell.insert(
        "metadata".to_string(),
        Value::Object(serde_json::Map::new()),
    );
    cell.insert("source".to_string(), Value::String(source.to_string()));
    if cell_type == "code" {
        cell.insert("execution_count".to_string(), Value::Null);
        cell.insert("outputs".to_string(), Value::Array(Vec::new()));
    }
    Value::Object(cell)
}

fn apply_notebook_edit(
    notebook: &mut Value,
    cell_id: Option<&str>,
    new_source: &str,
    cell_type: Option<&str>,
    edit_mode: &str,
) -> Result<NotebookEditOutcome, String> {
    if !matches!(edit_mode, "replace" | "insert" | "delete") {
        return Err("Edit mode must be replace, insert, or delete.".to_string());
    }
    if edit_mode == "insert" && cell_type.is_none() {
        return Err("Cell type is required when using edit_mode=insert.".to_string());
    }
    if matches!(edit_mode, "replace" | "delete") && cell_id.is_none() {
        return Err("Cell ID must be specified when not inserting a new cell.".to_string());
    }
    if let Some(cell_type) = cell_type {
        if !matches!(cell_type, "code" | "markdown") {
            return Err("Cell type must be code or markdown.".to_string());
        }
    }

    let language = notebook_language(notebook);
    let use_cell_ids = notebook_uses_cell_ids(notebook);
    let cells = notebook
        .get_mut("cells")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "Notebook JSON must contain a cells array.".to_string())?;

    let mut index = if let Some(cell_id) = cell_id {
        find_cell_index(cells, cell_id)
            .ok_or_else(|| format!("Cell with ID \"{}\" not found in notebook.", cell_id))?
    } else {
        0
    };

    let mut mode = edit_mode.to_string();
    if mode == "insert" && cell_id.is_some() {
        index += 1;
    }
    if mode == "replace" && index == cells.len() {
        mode = "insert".to_string();
    }
    if index > cells.len() || (mode != "insert" && index >= cells.len()) {
        return Err(format!("Cell index {} does not exist in notebook.", index));
    }

    let mut resulting_cell_id = cell_id.map(str::to_string);
    let resulting_cell_type;

    match mode.as_str() {
        "delete" => {
            let removed = cells.remove(index);
            resulting_cell_type = removed
                .get("cell_type")
                .and_then(Value::as_str)
                .unwrap_or("code")
                .to_string();
        }
        "insert" => {
            let cell_type = cell_type.unwrap_or("code");
            let new_id = use_cell_ids.then(generate_cell_id);
            resulting_cell_id = new_id.clone();
            resulting_cell_type = cell_type.to_string();
            cells.insert(index, make_cell(cell_type, new_source, new_id));
        }
        "replace" => {
            let target = cells[index]
                .as_object_mut()
                .ok_or_else(|| format!("Cell {} is not a JSON object.", index))?;
            if let Some(cell_type) = cell_type {
                target.insert(
                    "cell_type".to_string(),
                    Value::String(cell_type.to_string()),
                );
            }
            target.insert("source".to_string(), Value::String(new_source.to_string()));

            resulting_cell_type = target
                .get("cell_type")
                .and_then(Value::as_str)
                .unwrap_or("code")
                .to_string();

            if resulting_cell_type == "code" {
                target.insert("execution_count".to_string(), Value::Null);
                target.insert("outputs".to_string(), Value::Array(Vec::new()));
            }
            if resulting_cell_id.is_none() && use_cell_ids {
                resulting_cell_id = target.get("id").and_then(Value::as_str).map(str::to_string);
            }
        }
        _ => unreachable!(),
    }

    Ok(NotebookEditOutcome {
        cell_id: resulting_cell_id,
        cell_type: resulting_cell_type,
        language,
        edit_mode: mode,
    })
}

fn stringify_notebook(notebook: &Value, original_content: &str) -> Result<String, String> {
    let mut bytes = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(IPYNB_INDENT);
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, formatter);
    notebook
        .serialize(&mut serializer)
        .map_err(|e| format!("Notebook JSON 序列化失败: {}", e))?;
    let mut text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    text.push('\n');
    if original_content.contains("\r\n") {
        text = text.replace('\n', "\r\n");
    }
    Ok(text)
}

/// Cell 级别编辑 Jupyter Notebook。
pub async fn notebook_edit(
    app: &tauri::AppHandle,
    input: &serde_json::Value,
    session_id: &str,
) -> String {
    let notebook_path = input["notebook_path"].as_str().unwrap_or("");
    let new_source = input["new_source"].as_str().unwrap_or("");
    let cell_id = input["cell_id"].as_str();
    let cell_type = input["cell_type"].as_str();
    let edit_mode = input["edit_mode"].as_str().unwrap_or("replace");

    if notebook_path.trim().is_empty() {
        return "NotebookEdit 错误: notebook_path 不能为空。".to_string();
    }

    let workspace = get_workspace(app, session_id).await;
    let path = resolve_notebook_path(notebook_path, workspace.as_deref());
    if !has_ipynb_extension(&path) {
        return "NotebookEdit 错误: 文件必须是 Jupyter Notebook (.ipynb)。普通文本或普通 JSON 请不要使用 notebook_edit。".to_string();
    }
    if let Err(e) = ensure_path_permission(
        app,
        &path.to_string_lossy(),
        "编辑 Notebook",
        workspace.as_deref(),
    )
    .await
    {
        return e;
    }

    let read_mtime: Option<SystemTime> = std::fs::metadata(&path)
        .ok()
        .and_then(|meta| meta.modified().ok());
    let original_content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) => return format!("NotebookEdit 读取失败: {}", e),
    };

    let mut notebook: Value = match serde_json::from_str(&original_content) {
        Ok(value) => value,
        Err(e) => return format!("NotebookEdit 失败: Notebook 不是合法 JSON: {}", e),
    };

    let outcome =
        match apply_notebook_edit(&mut notebook, cell_id, new_source, cell_type, edit_mode) {
            Ok(outcome) => outcome,
            Err(e) => return format!("NotebookEdit 失败: {}", e),
        };

    let updated_content = match stringify_notebook(&notebook, &original_content) {
        Ok(content) => content,
        Err(e) => return e,
    };

    if let (Some(orig_mtime), Ok(current_meta)) = (read_mtime, std::fs::metadata(&path)) {
        if let Ok(current_mtime) = current_meta.modified() {
            if current_mtime != orig_mtime {
                return format!(
                    "NotebookEdit 中止: 文件 {} 在读取后被外部修改。请重新读取后再编辑。",
                    path.display()
                );
            }
        }
    }

    let branch = checkpoint::get_active_branch(session_id);
    let old_content_hash = Some(checkpoint::content_hash(original_content.as_bytes()));
    let backup_path = checkpoint::backup_file(
        session_id,
        &branch.name,
        &path.to_string_lossy(),
        original_content.as_bytes(),
    );
    let new_content_hash = Some(checkpoint::content_hash(updated_content.as_bytes()));
    let diff_summary = Some(format!(
        "Notebook {} cell {}",
        outcome.edit_mode,
        outcome.cell_id.as_deref().unwrap_or("<new>")
    ));

    let operation = FileOperation {
        op_type: OpType::Edit,
        path: path.to_string_lossy().to_string(),
        old_content_hash,
        backup_path,
        new_content_hash,
        diff_summary,
    };

    match std::fs::write(&path, &updated_content) {
        Ok(_) => {
            record_operation(app, session_id, operation).await;
            let patch = Patch::UpdateFile {
                path: path.to_string_lossy().to_string(),
                old_content: original_content.clone(),
                new_content: updated_content.clone(),
                diff: Some(compute_diff(&original_content, &updated_content)),
            };
            let msg = Some(format!("NotebookEdit {}", path.display()));
            record_patch_to_snapshot(app, session_id, patch, msg).await;

            match outcome.edit_mode.as_str() {
                "replace" => format!(
                    "Updated cell {} ({}, language: {})",
                    outcome.cell_id.unwrap_or_else(|| "<unknown>".to_string()),
                    outcome.cell_type,
                    outcome.language
                ),
                "insert" => format!(
                    "Inserted cell {} ({}, language: {})",
                    outcome.cell_id.unwrap_or_else(|| "<new>".to_string()),
                    outcome.cell_type,
                    outcome.language
                ),
                "delete" => format!(
                    "Deleted cell {} ({}, language: {})",
                    outcome.cell_id.unwrap_or_else(|| "<unknown>".to_string()),
                    outcome.cell_type,
                    outcome.language
                ),
                _ => "NotebookEdit completed".to_string(),
            }
        }
        Err(e) => format!("NotebookEdit 写入失败: {}", e),
    }
}

// --- 工具注册 ---
crate::define_tools! {
    pub fn register_tools(registry) {
        ToolDef {
            name: "notebook_edit",
            description: "Cell 级别编辑 Jupyter Notebook，避免直接文本修改 .ipynb JSON",
            search_hint: "notebook edit jupyter ipynb cell json replace insert delete",
            schema: json!({
                "name": "notebook_edit",
                "description": "Cell-level editor for Jupyter Notebook (.ipynb) files. Use this tool instead of edit_file or write_file for any .ipynb notebook or notebook-shaped JSON. It preserves notebook JSON structure, targets a single cell, can replace/insert/delete cells, and clears execution_count/outputs when code cells are modified.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "notebook_path": {
                            "type": "string",
                            "description": "Path to the Jupyter notebook file to edit (.ipynb). Absolute paths and workspace-relative paths are supported."
                        },
                        "cell_id": {
                            "type": "string",
                            "description": "Cell ID to edit. Actual notebook cell IDs are preferred. Numeric indexes or cell-N forms are also accepted. For insert, the new cell is inserted after this cell; omit to insert at the beginning."
                        },
                        "new_source": {
                            "type": "string",
                            "description": "The new source for the cell. Required for replace and insert; ignored for delete."
                        },
                        "cell_type": {
                            "type": "string",
                            "enum": ["code", "markdown"],
                            "description": "The cell type. Required for insert. Optional for replace; when provided it changes the target cell type."
                        },
                        "edit_mode": {
                            "type": "string",
                            "enum": ["replace", "insert", "delete"],
                            "description": "The edit operation. Defaults to replace."
                        }
                    },
                    "required": ["notebook_path", "new_source"]
                }
            }),
            should_defer: true,
            is_read_only: false,
            is_concurrency_safe: false,
            is_enabled: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_notebook() -> Value {
        json!({
            "cells": [
                {
                    "cell_type": "code",
                    "id": "abc",
                    "metadata": {},
                    "source": "print(1)",
                    "execution_count": 3,
                    "outputs": [{"name": "stdout"}]
                },
                {
                    "cell_type": "markdown",
                    "id": "def",
                    "metadata": {},
                    "source": "hello"
                }
            ],
            "metadata": {
                "language_info": { "name": "python" }
            },
            "nbformat": 4,
            "nbformat_minor": 5
        })
    }

    #[test]
    fn test_replace_code_cell_clears_outputs() {
        let mut notebook = sample_notebook();
        let outcome =
            apply_notebook_edit(&mut notebook, Some("abc"), "print(2)", None, "replace").unwrap();
        assert_eq!(outcome.edit_mode, "replace");
        let cell = &notebook["cells"][0];
        assert_eq!(cell["source"], "print(2)");
        assert!(cell["execution_count"].is_null());
        assert_eq!(cell["outputs"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_insert_after_cell_id() {
        let mut notebook = sample_notebook();
        let outcome = apply_notebook_edit(
            &mut notebook,
            Some("abc"),
            "new markdown",
            Some("markdown"),
            "insert",
        )
        .unwrap();
        assert_eq!(outcome.edit_mode, "insert");
        assert_eq!(notebook["cells"].as_array().unwrap().len(), 3);
        assert_eq!(notebook["cells"][1]["source"], "new markdown");
        assert!(outcome.cell_id.is_some());
    }

    #[test]
    fn test_delete_cell_by_index_alias() {
        let mut notebook = sample_notebook();
        let outcome =
            apply_notebook_edit(&mut notebook, Some("cell-1"), "", None, "delete").unwrap();
        assert_eq!(outcome.edit_mode, "delete");
        assert_eq!(notebook["cells"].as_array().unwrap().len(), 1);
        assert_eq!(outcome.cell_type, "markdown");
    }
}
