import Foundation

/// 传输页顶部「暂停全部 / 一键开始」两颗按钮的可用性，以及换下载目录时
/// 「搬运已下载文件」那一行的进度话（规格 2026-10-12 §5.2 / §6）。
///
/// **与 Windows 侧 `shell-core/src/presentation/batch_control.rs` 逐条同判据、同文案**
/// （两边的单测用的是同一组断言）。纯计算，无 UI。
///
/// * `canPauseAll`：有任何一行还提供「暂停」（= 活动中的，或排着队没暂停的）；
/// * `canResumeAll`：有任何一行提供「继续」（= 已暂停的），**或**壳记着「刚换过目录、
///   队列还没补回来」（`resumePending`）——后一种情形引擎是空的，列表里一行都没有，
///   只有壳知道还有事要做；
/// * 搬运**进行中**两颗都不可用：搬运和引擎同写一个文件是数据损坏级的风险，内核那边
///   也会拒绝——界面上就不要给一个注定被拒的按钮。
public struct BatchControl: Equatable, Sendable {
    public let canPauseAll: Bool
    public let canResumeAll: Bool
    /// 搬运还在跑。
    public let migrateRunning: Bool
    /// 搬运进行中的进度话；没在搬运时是 `nil`。
    public let migrateText: String?
    /// 搬运的结论话（没开始 / 中途取不到进度 / 结束后的失败与没覆盖）；干净时是 `nil`。
    public let migrateNote: String?

    public static func of(rows: [TransferRow],
                          resumePending: Bool,
                          migrate: MigrateStatus?,
                          migrateFailure: String? = nil,
                          lang: Lang = .zh) -> BatchControl {
        let running = migrate?.running ?? false
        func offers(_ a: TaskAction) -> Bool { rows.contains { $0.availableActions.contains(a) } }
        return BatchControl(
            canPauseAll: !running && offers(.pause),
            canResumeAll: !running && (offers(.unpause) || resumePending),
            migrateRunning: running,
            migrateText: migrate.flatMap { $0.running ? runningText($0, lang: lang) : nil },
            migrateNote: note(migrate: migrate, failure: migrateFailure, lang: lang))
    }

    /// 搬运没能开始（`migrate_start` 被拒或传输断了）。原文照登（约束 3），后果与旧文件位置照说（约束 4）。
    ///
    /// ⚠️ 与 Windows 侧逐字同文案：两边任何一方改了措辞都要一起改。
    public static func migrateStartFailedNote(reason: String, lang: Lang = .zh) -> String {
        lang.tf(.batchMigrateStartFailed, ["reason": reason])
    }

    /// 搬运进度再也取不到（连续失败到上界，或内核重启后任务已不在）。
    /// 不说「搬运完成」也不说「没有搬运」——我们只知道自己不知道。
    public static var migrateProgressLostNote: String { migrateProgressLostNote(lang: .zh) }

    public static func migrateProgressLostNote(lang: Lang) -> String {
        lang.t(.batchMigrateProgressLost)
    }

    private static func runningText(_ m: MigrateStatus, lang: Lang) -> String {
        lang.tf(.batchMigrateRunning, ["done": String(m.filesDone),
                                       "total": String(m.filesTotal),
                                       "doneBytes": ByteFormat.text(m.bytesDone),
                                       "totalBytes": ByteFormat.text(m.bytesTotal)])
    }

    private static func note(migrate: MigrateStatus?, failure: String?, lang: Lang) -> String? {
        var parts: [String] = []
        if let m = migrate, m.finished, !m.running, let text = finishedNote(m, lang: lang) { parts.append(text) }
        if let failure { parts.append(failure) }
        return parts.isEmpty ? nil : parts.joined(separator: " ")
    }

    /// 结束后要说的话。两类各一句，都有才并排；**干净 ⇒ `nil`**（不说「搬运完成」这种
    /// 客户不需要读的话——按钮能点了就是答案）。
    ///
    /// ⚠️ 失败那句点明**后果**（会从头下载）与**文件还在哪**（旧目录）：客户拿到这句话唯一
    /// 要做的决定就是「要不要自己去旧目录处理」，所以两样都得在话里。
    private static func finishedNote(_ m: MigrateStatus, lang: Lang) -> String? {
        var parts: [String] = []
        if let first = m.errors.first {
            parts.append(lang.tf(.batchMigrateErrors, ["count": String(m.errors.count),
                                                       "path": first.path,
                                                       "reason": first.reason]))
        }
        if !m.skipped.isEmpty {
            parts.append(lang.tf(.batchMigrateSkipped, ["count": String(m.skipped.count)]))
        }
        return parts.isEmpty ? nil : parts.joined(separator: " ")
    }
}
