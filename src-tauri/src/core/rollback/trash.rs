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
    trash_root_in(crate::get_agent_home(), session_id)
}

/// 回收站根目录的参数化版本：给定数据根，拼出 `trash/<会话 id>/`。
///
/// 拆出来是因为 `trash_root` 依赖全局数据目录单例（`AGENT_HOME_DIR`）：
/// 测试直接调它会 panic（未初始化），或迫使测试先 `set` 全局，模块之间
/// 因此形成"谁先跑谁负责初始化"的隐式依赖（单独跑本模块测试必挂）。
/// 路径拼接本身是纯逻辑，参数化后可以独立验证。
fn trash_root_in(data_root: &Path, session_id: &str) -> PathBuf {
    data_root.join("trash").join(session_id)
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

/// 删除某会话回收站里**没有被任何快照引用**的条目，返回删除个数。
///
/// `referenced` = "仍被快照引用的回收站路径"集合（即 `Patch::DeleteFile.trash_path` 的全集）。
/// 这是回收站唯一的存活判据：**按引用清，不按时间/会话清** —— 清早了会让回滚缺本体，
/// 而那属于"事后无法补救"（只能靠回滚时的告警告诉用户）。
///
/// 比较只用**文件名**：回收站是平铺的（一次删除 = 一个条目，文件名带时间戳前缀，唯一），
/// 而路径形态（分隔符、大小写）在不同来源下不保证一致，比文件名字段更稳。
pub fn remove_unreferenced(
    session_id: &str,
    referenced: &std::collections::HashSet<String>,
) -> usize {
    remove_unreferenced_in(&trash_root(session_id), referenced)
}

/// 按引用清理回收站（根目录由调用方给出）。
///
/// 与 `remove_unreferenced` 的分工同 [`trash_root_in`]：把"根目录从哪来"
/// （依赖全局）与"怎么清"（纯逻辑）分开，后者可以直接对着临时目录测。
fn remove_unreferenced_in(root: &Path, referenced: &std::collections::HashSet<String>) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0; // 回收站目录不存在 = 没有可清的
    };
    let referenced_names: std::collections::HashSet<&str> = referenced
        .iter()
        .filter_map(|p| Path::new(p).file_name().and_then(|n| n.to_str()))
        .collect();

    let mut removed = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if referenced_names.contains(name) {
            continue; // 仍被快照引用 → 留给回滚
        }
        let path = entry.path();
        let ok = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if ok.is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 清空某会话的回收站（会话被**删除**时用）。
///
/// 会话删了就没有任何回滚渠道了，回收站里的本体再没有任何用处，留着只是占空间。
pub fn purge_session(session_id: &str) {
    let _ = std::fs::remove_dir_all(trash_root(session_id));
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

    /// 按引用清：被引用的留下、没被引用的清掉。
    ///
    /// 这里是"删早了会让回滚缺本体"的那条判据，必须锁住。
    #[test]
    fn remove_unreferenced_keeps_referenced_entries_only() {
        // 参数化：用临时目录而不是全局数据目录，本测试不再依赖别处的初始化
        let root = std::env::temp_dir().join(format!("jarvis_trash_gc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let kept = root.join("111_kept.txt");
        let stale_file = root.join("222_stale.txt");
        let stale_dir = root.join("333_stale-dir");
        std::fs::write(&kept, "x").unwrap();
        std::fs::write(&stale_file, "y").unwrap();
        std::fs::create_dir_all(&stale_dir).unwrap();

        let referenced: std::collections::HashSet<String> =
            [kept.to_string_lossy().to_string()].into_iter().collect();
        let removed = remove_unreferenced_in(&root, &referenced);

        assert!(kept.exists(), "被引用的条目必须留下（回滚要用）");
        assert!(!stale_file.exists() && !stale_dir.exists(), "没被引用的条目应被清掉");
        assert_eq!(removed, 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 回收站目录不存在时不能报错（没删过东西是常态）
    #[test]
    fn remove_unreferenced_is_noop_without_a_trash_dir() {
        let missing =
            std::env::temp_dir().join(format!("jarvis_trash_missing_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);
        let removed = remove_unreferenced_in(&missing, &std::collections::HashSet::new());
        assert_eq!(removed, 0);
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
