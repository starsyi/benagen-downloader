//! browser_primary_action —— 在文件列表里**双击**一行该做什么。
//!
//! 上游：`macos/Sources/BenagenCoreKit/Presentation/BrowserPrimaryAction.swift`（逐字对位）。
//!
//! ⚠️ 为什么要把这条规则抽出来（全局约束 C-6）：它是双击的**语义**，
//!    而双击这件事本身无法单测（触发它的那一层不单测——同 macOS 侧）。抽出来之后
//!    "目录进入、文件入队、多个一起入队"这三条都有断言，剩下那一层只剩把结果分派出去。
//!
//! ⚠️ 集合类型用 `BTreeSet`：判据是**集合相等**，与遍历顺序无关（macOS 侧是 `Set`）。
//!    `BTreeSet` 还顺带让 `Enqueue` 里的路径**有序**，下游照着它做请求时结果稳定。

use std::collections::BTreeSet;

/// 双击一行的结果（上游 `BrowserPrimaryAction.swift:8-27`）。
///
/// macOS 源写的是 `Equatable, Sendable`；这里对位成 `PartialEq, Eq, Debug, Clone`
/// ——`Sendable` 在 Rust 里由类型系统默认保证（没有内部可变性/裸指针就自动成立），
/// 而 `Debug` 是断言失败时要看得见值才加的（`assert_eq!` 需要它）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BrowserPrimaryAction {
    /// 进入这个目录（它必须是当前层里的目录）。
    Enter(String),
    /// 把这些路径加进下载（内核按前缀展开目录，约束 1：壳不展开）。
    Enqueue(BTreeSet<String>),
    /// 空集：什么都没双击到。
    Nothing,
}

impl BrowserPrimaryAction {
    /// - `paths`：`primaryAction` 回调给的那一组（在 macOS 上，双击前单击已经把它选出来了，
    ///   所以这里通常就是"被双击的那一行"，多选时则是整个勾选面）。
    /// - `dirs_in_current_level`：**当前这一层**里目录的路径集合。
    ///   判据只能是它 —— 勾选面可以跨目录，而"跨目录的某个路径是不是目录"，
    ///   壳手里没有那份数据（`get_tree` 的 `flat` 只列文件）。
    pub fn of(paths: &BTreeSet<String>, dirs_in_current_level: &BTreeSet<String>) -> Self {
        // 上游写的是 `paths.count == 1 ? paths.first : nil`；`len() != 1` 时一律走 `None`
        // 那一支，所以这里逐个对位（`BTreeSet` 没有 `first`，取的是最小元素——
        // 但在 `len() == 1` 的前提下"最小元素"就是"那唯一一个元素"）。
        let only = if paths.len() == 1 {
            paths.iter().next()
        } else {
            None
        };
        match only {
            None => {
                if paths.is_empty() {
                    Self::Nothing
                } else {
                    Self::Enqueue(paths.clone())
                }
            }
            Some(only) => {
                if dirs_in_current_level.contains(only) {
                    Self::Enter(only.clone())
                } else {
                    Self::Enqueue(paths.clone())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! 上游：`macos/Tests/BenagenCoreKitTests/BrowserPrimaryActionTests.swift`（4 条，逐条对位）。

    use super::BrowserPrimaryAction;
    use std::collections::BTreeSet;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// 上游 `aSingleDirectoryEnters`：恰好一个路径、且它是当前层里的目录 ⇒ 进入。
    #[test]
    fn a_single_directory_enters() {
        assert_eq!(
            BrowserPrimaryAction::of(&set(&["d1"]), &set(&["d1", "d2"])),
            BrowserPrimaryAction::Enter("d1".to_string())
        );
    }

    /// 上游 `aSingleFileEnqueues`：恰好一个路径、它是文件 ⇒ 把它加进下载。
    #[test]
    fn a_single_file_enqueues() {
        assert_eq!(
            BrowserPrimaryAction::of(&set(&["a.txt"]), &set(&["d1"])),
            BrowserPrimaryAction::Enqueue(set(&["a.txt"]))
        );
    }

    /// 上游 `manyPathsEnqueue`：多个路径 ⇒ 全部加进下载（哪怕里面混着目录）。
    #[test]
    fn many_paths_enqueue() {
        assert_eq!(
            BrowserPrimaryAction::of(&set(&["d1", "a.txt"]), &set(&["d1"])),
            BrowserPrimaryAction::Enqueue(set(&["a.txt", "d1"]))
        );
    }

    /// 上游 `emptyDoesNothing`：空集合 ⇒ 什么都不做。
    #[test]
    fn empty_does_nothing() {
        assert_eq!(
            BrowserPrimaryAction::of(&set(&[]), &set(&[])),
            BrowserPrimaryAction::Nothing
        );
    }
}
