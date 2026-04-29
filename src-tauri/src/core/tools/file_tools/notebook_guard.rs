//! # notebook_guard.rs — 阻止文本工具直接改写 Jupyter Notebook
//!
//! 识别 `.ipynb` 路径和 notebook-shaped JSON 内容，并生成一致的拒绝提示，避免普通文本写入破坏 Notebook 的 cells、metadata 与 outputs。
//!
//! ## Key Exports
//! - `is_notebook_path()`: 判断路径是否指向 Jupyter Notebook 文件
//! - `looks_like_notebook_json()`: 判断文本内容是否符合 Notebook JSON 结构
//! - `notebook_text_edit_rejection()`: 生成拒绝直接文本编辑 Notebook 的提示
//!
//! ## Dependencies
//! - External: `serde_json`

use std::path::Path;

pub(super) fn is_notebook_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("ipynb"))
        .unwrap_or(false)
}

pub(super) fn looks_like_notebook_json(content: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .map(|value| {
            value
                .get("cells")
                .and_then(|cells| cells.as_array())
                .is_some()
                && value.get("nbformat").and_then(|v| v.as_u64()).is_some()
        })
        .unwrap_or(false)
}

pub(super) fn notebook_text_edit_rejection(path: &str) -> String {
    format!(
        "拒绝文本编辑 Notebook: '{}' 是 .ipynb/Jupyter Notebook 或 notebook-shaped JSON。\
        Notebook 是结构化 JSON，直接使用 edit_file/write_file 可能破坏 cells、metadata、outputs。\
        请改用 notebook_edit 按 cell_id 进行 replace/insert/delete。",
        path
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_notebook_path() {
        assert!(is_notebook_path("analysis.ipynb"));
        assert!(is_notebook_path("analysis.IPYNB"));
        assert!(!is_notebook_path("package.json"));
    }

    #[test]
    fn test_looks_like_notebook_json() {
        let notebook = r#"{"cells":[],"metadata":{},"nbformat":4,"nbformat_minor":5}"#;
        let ordinary_json = r#"{"cells":"not-array","nbformat":4}"#;
        assert!(looks_like_notebook_json(notebook));
        assert!(!looks_like_notebook_json(ordinary_json));
        assert!(!looks_like_notebook_json("not json"));
    }
}
