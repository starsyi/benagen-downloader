import Testing
@testable import BenagenCoreKit

@Suite("文件列表里双击一行该做什么")
struct BrowserPrimaryActionTests {

    @Test("恰好一个路径、且它是当前层里的目录 ⇒ 进入")
    func aSingleDirectoryEnters() {
        #expect(BrowserPrimaryAction.of(paths: ["d1"], dirsInCurrentLevel: ["d1", "d2"])
                == .enter("d1"))
    }

    @Test("恰好一个路径、它是文件 ⇒ 把它加进下载")
    func aSingleFileEnqueues() {
        #expect(BrowserPrimaryAction.of(paths: ["a.txt"], dirsInCurrentLevel: ["d1"])
                == .enqueue(["a.txt"]))
    }

    @Test("多个路径 ⇒ 全部加进下载（哪怕里面混着目录）")
    func manyPathsEnqueue() {
        #expect(BrowserPrimaryAction.of(paths: ["d1", "a.txt"], dirsInCurrentLevel: ["d1"])
                == .enqueue(["a.txt", "d1"]))
    }

    @Test("空集合 ⇒ 什么都不做")
    func emptyDoesNothing() {
        #expect(BrowserPrimaryAction.of(paths: [], dirsInCurrentLevel: []) == .nothing)
    }
}
