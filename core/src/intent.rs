//! intent —— 「用户想下的文件集」（规格 2026-10-12 §3）。
//!
//! 引擎被换掉（`replace_engine`）或内核被重启（改下载目录）时，aria2 里的队列会一起丢。
//! 这里记下**用户点过下载、且还没下完**的清单路径，换引擎之后据此把队列补回来
//! （`-c` 恒开，已下的部分续传）。
//!
//! ⚠️ 这个文件和 `state.rs` 同性质：是**优化**不是**真相**。读不出来、码对不上、
//! 内容坏了，一律退化成空集合——后果只是「没有自动补回」，不影响任何下载的正确性。
//! 所以读路径**绝不报错**；写失败也不致命（返回 `Err` 让调用方记一行日志即可）。
//!
//! ⚠️ 随交付码绑定：换码即作废（与 `state::State::bind` 同口径）。拿上一批的路径集
//! 去补这一批的队列，是把别的批次的文件名塞给引擎。

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 落盘文件名（与 `settings.json` 同目录）。
pub const FILE_NAME: &str = "intent.json";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct OnDisk {
    code: String,
    paths: BTreeSet<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Intent {
    code: String,
    paths: BTreeSet<String>,
    file: Option<PathBuf>,
}

impl Intent {
    /// 读 `file`。不存在 / 损坏 / 字段不对 ⇒ 空集合（见模块头）。
    pub fn load(file: &Path) -> Intent {
        let disk = std::fs::read(file)
            .ok()
            .and_then(|raw| serde_json::from_slice::<OnDisk>(&raw).ok())
            .unwrap_or_default();
        Intent {
            code: disk.code,
            paths: disk.paths,
            file: Some(file.to_path_buf()),
        }
    }

    /// 绑定到某个交付码。换了码 ⇒ 集合清空（返回 `true` 表示发生了清空）。
    pub fn bind(&mut self, code: &str) -> bool {
        if self.code == code {
            return false;
        }
        self.code = code.to_string();
        let had = !self.paths.is_empty();
        self.paths.clear();
        had
    }

    /// 记下一条。返回是否真的新增（用来决定要不要落盘）。
    pub fn add(&mut self, path: &str) -> bool {
        self.paths.insert(path.to_string())
    }

    /// 去掉一条。返回是否真的去掉了。
    pub fn remove(&mut self, path: &str) -> bool {
        self.paths.remove(path)
    }

    pub fn contains(&self, path: &str) -> bool {
        self.paths.contains(path)
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// 全部路径（有序）。
    pub fn paths(&self) -> impl Iterator<Item = &String> {
        self.paths.iter()
    }

    /// 只留下「不在 `complete` 里」的那些——**完成的不该再被补回队列**。
    pub fn unfinished(&self, complete: &BTreeSet<String>) -> Vec<String> {
        self.paths
            .iter()
            .filter(|p| !complete.contains(*p))
            .cloned()
            .collect()
    }

    /// 原子写回（先写 `.tmp` 再 `rename`）。没有落盘位置（零值）⇒ 什么都不做。
    pub fn save(&self) -> io::Result<()> {
        let Some(file) = self.file.as_deref() else {
            return Ok(());
        };
        if let Some(dir) = file.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        let body = serde_json::to_vec(&OnDisk {
            code: self.code.clone(),
            paths: self.paths.clone(),
        })
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = TempDir::new();
        let file = dir.join(FILE_NAME);
        let mut a = Intent::load(&file);
        a.bind("CODE");
        a.add("x/1.fq");
        a.add("x/2.fq");
        a.save().expect("落盘");

        let mut b = Intent::load(&file);
        assert!(!b.bind("CODE"), "同一个码不该清空");
        assert_eq!(b.paths().cloned().collect::<Vec<_>>(), vec!["x/1.fq", "x/2.fq"]);
    }

    /// 换码即作废：拿旧批次的路径去补新批次的队列是错的。
    /// 判别力：删掉 `bind` 里的 `clear()` ⇒ 红。
    #[test]
    fn binding_a_different_code_clears_the_set() {
        let mut i = Intent::default();
        i.bind("A");
        i.add("a.fq");
        assert!(i.bind("B"), "换码应报告发生了清空");
        assert!(i.is_empty());
    }

    #[test]
    fn corrupt_or_missing_file_reads_as_empty_never_an_error() {
        let dir = TempDir::new();
        let file = dir.join(FILE_NAME);
        assert!(Intent::load(&file).is_empty(), "不存在");
        std::fs::write(&file, b"{not json").unwrap();
        assert!(Intent::load(&file).is_empty(), "损坏");
        std::fs::write(&file, br#"{"code":1,"paths":"x"}"#).unwrap();
        assert!(Intent::load(&file).is_empty(), "字段类型不对");
    }

    /// 完成的文件不该再被补回队列。判别力：把过滤删掉 ⇒ 红。
    #[test]
    fn unfinished_excludes_completed_paths() {
        let mut i = Intent::default();
        i.bind("C");
        i.add("a");
        i.add("b");
        i.add("c");
        assert_eq!(i.unfinished(&set(&["b"])), vec!["a".to_string(), "c".to_string()]);
    }

    #[test]
    fn add_and_remove_report_whether_anything_changed() {
        let mut i = Intent::default();
        assert!(i.add("a"));
        assert!(!i.add("a"), "重复添加不算变化");
        assert!(i.remove("a"));
        assert!(!i.remove("a"));
    }

    /// 原子替换：不留 `.tmp`，且第二次写能覆盖第一次。
    #[test]
    fn save_replaces_atomically_and_leaves_no_temp_file() {
        let dir = TempDir::new();
        let file = dir.join(FILE_NAME);
        let mut i = Intent::load(&file);
        i.bind("C");
        i.add("a");
        i.save().unwrap();
        i.add("b");
        i.save().unwrap();
        assert!(!dir.join("intent.json.tmp").exists());
        assert_eq!(Intent::load(&file).len(), 2);
    }
}
