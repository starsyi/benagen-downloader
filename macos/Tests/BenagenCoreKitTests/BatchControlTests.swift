import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 「暂停全部 / 一键开始」与换目录搬运的呈现判据（规格 2026-10-12）。
// 与 Windows 侧 `shell-core/src/presentation/batch_control.rs` 的单测逐条同断言。
// ---------------------------------------------------------------------------

private func rows(_ items: [TransferItem]) -> [TransferRow] { items.map(TransferRow.init) }

private func migrate(running: Bool, finished: Bool,
                     skipped: [MigrateProblem] = [], errors: [MigrateProblem] = []) -> MigrateStatus {
    MigrateStatus(running: running, finished: finished, filesDone: 2, filesTotal: 5,
                  bytesDone: 1024, bytesTotal: 4096, skipped: skipped, errors: errors)
}

/// 暂停只看「有没有行提供暂停」，继续只看「有没有行提供继续」。
@Test func availabilityFollowsWhatTheRowsOffer() {
    let active = rows([.fixture(state: .active, rawStatus: "active")])
    let a = BatchControl.of(rows: active, resumePending: false, migrate: nil)
    #expect(a.canPauseAll && !a.canResumeAll)

    let paused = rows([.fixture(state: .waiting, rawStatus: "paused")])
    let p = BatchControl.of(rows: paused, resumePending: false, migrate: nil)
    #expect(!p.canPauseAll && p.canResumeAll)

    let done = rows([.fixture(state: .complete, rawStatus: "complete")])
    let d = BatchControl.of(rows: done, resumePending: false, migrate: nil)
    #expect(!d.canPauseAll && !d.canResumeAll, "已完成的行两个都不给")
}

/// 换完目录后引擎是空的（一行都没有），「一键开始」只能靠壳记着的 `resumePending`。
@Test func resumeIsOfferedOnAnEmptyListWhenAHandOverIsPending() {
    let c = BatchControl.of(rows: [], resumePending: true, migrate: nil)
    #expect(c.canResumeAll && !c.canPauseAll)
    #expect(!BatchControl.of(rows: [], resumePending: false, migrate: nil).canResumeAll)
}

/// 搬运进行中两颗都灰掉，并给出进度话。
@Test func nothingIsOfferedWhileFilesAreBeingMoved() {
    let both = rows([.fixture(gid: "a", state: .active, rawStatus: "active"),
                     .fixture(gid: "b", state: .waiting, rawStatus: "paused")])
    let c = BatchControl.of(rows: both, resumePending: true,
                            migrate: migrate(running: true, finished: false))
    #expect(!c.canPauseAll && !c.canResumeAll && c.migrateRunning)
    #expect(c.migrateText?.contains("2/5") == true)
    #expect(c.migrateNote == nil)
}

@Test func aCleanFinishSaysNothing() {
    let c = BatchControl.of(rows: [], resumePending: true,
                            migrate: migrate(running: false, finished: true))
    #expect(c.migrateText == nil && c.migrateNote == nil)
    #expect(c.canResumeAll, "搬完了就能开始")
}

/// 失败要说清后果与文件在哪；跳过的另说一句。
@Test func failuresAndSkipsAreReportedWithConsequenceAndLocation() throws {
    let c = BatchControl.of(
        rows: [], resumePending: false,
        migrate: migrate(running: false, finished: true,
                         skipped: [MigrateProblem(path: "c.fq", reason: "x")],
                         errors: [MigrateProblem(path: "a/b.fq", reason: "磁盘已满")]))
    let note = try #require(c.migrateNote)
    #expect(note.contains("从头下载") && note.contains("旧文件仍在原目录"))
    #expect(note.contains("a/b.fq") && note.contains("磁盘已满"))
    #expect(note.contains("没有覆盖"))
}

/// 搬运没能开始：那一句话要原样进到结论位，且不冒充「在搬」或「搬完」。
@Test func aMigrationThatNeverStartedSaysWhatWasLostAndWhere() {
    let why = BatchControl.migrateStartFailedNote(reason: "还没有加载交付清单（先调 load_delivery）")
    #expect(why == "旧目录里已下载的文件没能搬到新目录，会从头下载（旧文件仍在原目录）。原因：还没有加载交付清单（先调 load_delivery）")

    let c = BatchControl.of(rows: [], resumePending: true, migrate: nil, migrateFailure: why)
    #expect(c.migrateNote == why)
    #expect(c.migrateText == nil, "没在搬就不该有进度话")
    #expect(!c.migrateRunning)
}

@Test func aLostMigrationProgressIsSaidAsUnknownNotAsDone() {
    let c = BatchControl.of(rows: [], resumePending: true, migrate: nil,
                            migrateFailure: BatchControl.migrateProgressLostNote)
    #expect(c.migrateNote == "没能取到搬运进度，旧文件可能没有搬完。")
    #expect(c.canResumeAll, "搬运已经不在跑，「一键开始」照常可用")
}

@Test func aFinishedMigrationWithErrorsAndAFailedStartAreBothSaid() {
    let finished = migrate(running: false, finished: true,
                           errors: [MigrateProblem(path: "a.bin", reason: "磁盘已满")])
    let why = BatchControl.migrateStartFailedNote(reason: "上一次搬运还没结束")
    let c = BatchControl.of(rows: [], resumePending: false, migrate: finished, migrateFailure: why)
    let note = c.migrateNote ?? ""
    #expect(note.contains("没能搬到新目录"), "结束后的错误话还在")
    #expect(note.contains(why), "两句都在，不互相覆盖")
}
