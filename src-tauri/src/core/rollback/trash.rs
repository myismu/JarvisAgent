//! # trash.rs — 回收站（删除对象的存放处，同时是回滚的"本体恢复源"）
//!
//! ## 两个身份（缺一不可）
//! - **软删除的落点**：`DeleteFile` 把目标整体搬进来而不是真删（`file_tools/delete.rs`）。
//! - **回滚的恢复源**：目录 / 二进制 / 大文件在快照里存不下内容（`content_hash = None`），
//!   它们回滚时的**唯一**恢复途径就是从这里把本体搬回原位（`replay.rs` 的恢复阶段）。
//!
//! ## 由此推出的一条硬约束
//! **清理回收站 == 销毁回滚的恢复源。** 任何清理策略都必须与快照生命周期联动
//! （只清"没有任何快照还引用它"的条目），不要加"定时清空/按大小清空"这种一刀切。
//!
//! ## 位置
//! `data/trash/<会话 id>/<时间戳>_<名字>` —— 放**应用数据目录**，不放在被删对象旁边：
//! 放旁边会形成自嵌套（见 [`trash_root`] 的说明）。
//!
//! ## 关键导出
//! - `trash_root()`: 某会话的回收站根目录
//! - `move_to()`: 把对象移进回收站（跨卷时退化为递归复制 + 删源）
//! - `copy_recursively()`: 递归复制（`move_to` 的跨卷退路，也被回滚搬回时复用）

use std::path::{Path, PathBuf};

/// 回收站根目录：应用数据目录下的 `trash/<会话 id>/`。
///
/// **为什么必须放在这里**（2026-09-21 实测事故）：回收站原先放在"被删对象的父目录"下
/// （`<父目录>/.jarvis_trash/`），于是有两个结构性缺陷：
/// 1. 删一个位于 `.jarvis_trash` 里的文件时，回收站会在它内部再建一个 `.jarvis_trash`
///    —— **删除动作本身在制造新残留**（审计日志实锤：删完 7 个文件后出现了
///    `.jarvis_trash/.jarvis_trash`）；
/// 2. 当目标**就是** `.jarvis_trash` 时，回收站落在源目录内部，Windows 的 rename 直接
///    失败（os error 87），该目录从此无法通过任何工具删除，子代理只能反复重试到轮数上限。
///
/// 放到应用数据目录后，目标与回收站永不互相嵌套，上述两个问题一起消失。
pub fn trash_root(session_id: &str) -> PathBuf {
    crate::get_agent_home().join("trash").join(session_id)
}

/// 把对象移到指定位置。
///
/// 先试 `rename`（同一磁盘上是整体搬移，目录也一次搞定）；失败则退化为
/// "递归复制 + 删源" —— 跨卷（Windows os error 17）必须走这条路，因为
/// 回收站在应用数据目录、目标可能在另一个盘。
pub fn move_to(src: &Path, dst: &Path) -> std::io::Result<()> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(rename_err) => {
            copy_recursively(src, dst).map_err(|copy_err| {
                std::io::Error::new(
                    copy_err.kind(),
                    format!("移动失败({rename_err})；改用复制也失败({copy_err})"),
                )
            })?;
            if src.is_dir() {
                std::fs::remove_dir_all(src)
            } else {
                std::fs::remove_file(src)
            }
        }
    }
}

/// 递归复制（目录/文件通吃）。`std::fs::copy` 只认文件，所以目录要自己走一层。
pub fn copy_recursively(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_recursively(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jarvis-trash-test-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn move_to_moves_a_file_and_keeps_content() {
        let root = temp_root("file");
        let src = root.join("a.txt");
        std::fs::write(&src, "hello").unwrap();
        let dst = root.join("trash").join("1_a.txt");

        move_to(&src, &dst).expect("移动应成功");

        assert!(!src.exists(), "源文件应已不在原位");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hello");
    }

    /// 目录也要能整体移走（含嵌套内容）——删除目录壳就靠这条
    #[test]
    fn move_to_moves_a_directory_with_children() {
        let root = temp_root("dir");
        let src = root.join("some-dir");
        std::fs::create_dir_all(src.join("nested")).unwrap();
        std::fs::write(src.join("nested").join("x.txt"), "x").unwrap();
        let dst = root.join("trash").join("2_some-dir");

        move_to(&src, &dst).expect("移动应成功");

        assert!(!src.exists(), "源目录应已不在原位");
        assert_eq!(
            std::fs::read_to_string(dst.join("nested").join("x.txt")).unwrap(),
            "x"
        );
    }

    /// 跨卷时的退路：递归复制既能搬文件也能搬目录
    #[test]
    fn copy_recursively_handles_files_and_nested_dirs() {
        let root = temp_root("copy");
        let src = root.join("src");
        std::fs::create_dir_all(src.join("d1").join("d2")).unwrap();
        std::fs::write(src.join("top.txt"), "top").unwrap();
        std::fs::write(src.join("d1").join("d2").join("deep.txt"), "deep").unwrap();
        let dst = root.join("dst");

        copy_recursively(&src, &dst).expect("复制应成功");

        assert_eq!(std::fs::read_to_string(dst.join("top.txt")).unwrap(), "top");
        assert_eq!(
            std::fs::read_to_string(dst.join("d1").join("d2").join("deep.txt")).unwrap(),
            "deep"
        );
    }
}
