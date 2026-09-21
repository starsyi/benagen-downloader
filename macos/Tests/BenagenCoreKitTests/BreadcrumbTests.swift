import Testing
import Foundation
@testable import BenagenCoreKit

// ---------------------------------------------------------------------------
// 面包屑与目录导航的呈现模型（全局约束 8：`Presentation/` 里每条纯计算都要有单测）
//
// ⚠️ 路径一律是**清单原文**（约束 3）：不解码 `×`、不折叠 `//`、不改空白、
//    不 `standardizingPath`。下面那些带 `×` 与空格的夹具就是这条约束的现场 ——
//    夹具里出现的 `C24-8_×_25WS024` / `QC 图.png` 是**真清单里的原文**
//    （内核夹具：`core/src/delivery.rs` 的 `file_url_escapes_like_server`）。
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Segments
// ---------------------------------------------------------------------------

@Test func splitsPathIntoSegmentsForTheCrumbs() {
    let b = Breadcrumb(path: "client-test/C24-8_×_25WS024/Figure")

    #expect(b.segments.map(\.name) == ["client-test", "C24-8_×_25WS024", "Figure"])
    // 每一段带的是**到它为止**的完整路径（点了就能跳过去）。
    #expect(b.segments.map(\.path) == ["client-test",
                                       "client-test/C24-8_×_25WS024",
                                       "client-test/C24-8_×_25WS024/Figure"])
    #expect(b.path == "client-test/C24-8_×_25WS024/Figure", "crumbs 记着的是它自己那条原文路径")
}

@Test func rootIsTheEmptyPath() {
    // 内核把根目录的 `path` 回成**空串**，不是 `"/"`（`ListDirResult.path` 的注释）。
    #expect(Breadcrumb(path: "").segments.isEmpty)
}

@Test func nonASCIIAndSpacesSurviveUntouched() {
    // 约束 3：× 与空格是清单原文，不得解码、不得转义。
    #expect(Breadcrumb(path: "C24-8_×_25WS024/Figure/QC 图.png")
              .segments.last?.path == "C24-8_×_25WS024/Figure/QC 图.png")
    // 反向的一半：**没有**被拼成 %C3%97 / %20 那一类转义产物（否则上面那条
    // 只要"整串照抄"就能过，`×` 是否被处理过就看不出来了）。
    #expect(Breadcrumb(path: "C24-8_×_25WS024").segments.map(\.name) == ["C24-8_×_25WS024"])
    #expect(Breadcrumb(path: "QC 图.png").segments.map(\.name) == ["QC 图.png"])
}

@Test func repeatedSlashesAreNotCollapsed() {
    // 约束 3 的「不折叠 //」。`split(omittingEmptySubsequences: true)` /
    // `components(separatedBy: "/") + 过滤空串` 这类写法都会把 `a//b` 折成两段，
    // 于是这一段在界面上凭空消失一格 —— 折叠就是"壳替内核改了路径"。
    let b = Breadcrumb(path: "a//b")
    #expect(b.segments.count == 3, "空段也是一个段（不折叠）")
    #expect(b.segments.last?.path == "a//b", "逐段拼回去必须**逐字**等于原文")
}

// ---------------------------------------------------------------------------
// 上一级 / 拼接
// ---------------------------------------------------------------------------

@Test func parentOfATopLevelEntryIsRoot() {
    #expect(Breadcrumb.parentPath(ofCurrent: "a") == "")
    #expect(Breadcrumb.parentPath(ofCurrent: "a/b/c") == "a/b")
    // 根自己没有上一级（返回它自己），否则「返回上一层」在根上会造出一个 "/"。
    #expect(Breadcrumb.parentPath(ofCurrent: "") == "")
}

@Test func joiningADirEntryRebuildsItsPath() {
    // ⚠️ `list_dir` 的目录项**没有 `path` 键**（判别键是 `"type"`，没有 `is_dir` 布尔）——
    //    目录的路径只能由壳自己拼：父路径 + "/" + name。
    #expect(Breadcrumb.join(parent: "a/b", name: "子目录") == "a/b/子目录")
    #expect(Breadcrumb.join(parent: "", name: "top") == "top")    // 根下不得出现前导 "/"
    // 名字里带空格与非 ASCII 时逐字保真（不得转义）。
    #expect(Breadcrumb.join(parent: "client-test", name: "C24-8_×_25WS024")
            == "client-test/C24-8_×_25WS024")
}

// ---------------------------------------------------------------------------
// `list_dir` 失败之后：停在哪儿、说什么（简报点名的 path_not_found 回退）
// ---------------------------------------------------------------------------

@Test func pathNotFoundGoesUpOneLevel() {
    // ⚠️ 内核在**路径指向文件**时也报 `path_not_found`（`op_list_dir` 只在子树里找目录）。
    //    所以这个错误不是"停在错误页"的理由 —— 退回上一层并说明，才是简报要的处置。
    let e = CoreError.rpc(code: "path_not_found", message: "清单里没有目录 \"a/b/c\"")
    let f = DirLoadFailure.of(e, currentPath: "a/b/c")

    #expect(f.path == "a/b", "退回上一层")
    #expect(f.message == "清单里没有目录 \"a/b/c\"", "内核原文逐字照登（约束 3）")
    #expect(f.notice == "已返回上一层", "退回去了就得说一声，否则用户不知道自己怎么换了地方")
    #expect(f.didFallBack)
}

@Test func pathNotFoundAtTheRootStaysAtTheRoot() {
    // 根上无路可退：必须停在根，不得拼出一个前导 "/" 的假路径（那会让下一次
    // `list_dir` 换一个新错误回来，用户看到的是错误在变、位置不变）。
    let f = DirLoadFailure.of(.rpc(code: "path_not_found", message: "清单里没有目录 \"\""),
                              currentPath: "")
    #expect(f.path == "")
    #expect(f.didFallBack == false, "没退成就不该说「已返回上一层」")
}

@Test func otherErrorsStayPutAndKeepTheKernelWording() {
    // 其余错误码一律**原地不动**：传输断了、参数错了，都跟"这一层存不存在"无关，
    // 把用户弹到上一层是壳在替内核解释错误（约束 1）。
    for e in [CoreError.rpc(code: "engine_disconnected", message: "下载引擎已断开"),
              .rpc(code: "invalid_params", message: "参数不合法"),
              .transport("内核进程已退出（管道结束）"),
              .malformedResponse("id 7 的 result 与 ListDirResult.self 的形状不符")] {
        let f = DirLoadFailure.of(e, currentPath: "a/b")
        #expect(f.path == "a/b", "\(e) 不该把用户弹走")
        #expect(f.notice == nil)
        #expect(f.didFallBack == false)
        #expect(f.message == AppModel.message(of: e), "原文逐字（约束 3）")
    }
}
