import Foundation

/// 在文件列表里**双击**一行该做什么。
///
/// ⚠️ 为什么要把这条规则抽出来（全局约束 C-6）：它是双击的**语义**，
///    而双击这件事本身无法单测（视图不单测）。抽出来之后"目录进入、文件入队、
///    多个一起入队"这三条都有断言，视图里只剩把结果分派出去。
public enum BrowserPrimaryAction: Equatable, Sendable {
    /// 进入这个目录（它必须是当前层里的目录）。
    case enter(String)
    /// 把这些路径加进下载（内核按前缀展开目录，约束 1：壳不展开）。
    case enqueue(Set<String>)
    /// 空集：什么都没双击到。
    case nothing

    /// - `paths`：`List` 的 `primaryAction` 回调给的那一组（在 macOS 上，双击前
    ///   单击已经把它选出来了，所以这里通常就是"被双击的那一行"，多选时则是整个勾选面）。
    /// - `dirsInCurrentLevel`：**当前这一层**里目录的路径集合。
    ///   判据只能是它 —— 勾选面可以跨目录，而"跨目录的某个路径是不是目录"，
    ///   壳手里没有那份数据（`get_tree` 的 `flat` 只列文件）。
    public static func of(paths: Set<String>, dirsInCurrentLevel: Set<String>) -> BrowserPrimaryAction {
        guard let only = paths.count == 1 ? paths.first : nil else {
            return paths.isEmpty ? .nothing : .enqueue(paths)
        }
        return dirsInCurrentLevel.contains(only) ? .enter(only) : .enqueue(paths)
    }
}
