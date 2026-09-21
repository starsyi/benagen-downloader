//! `engine` 骨架：**路径守卫**（落盘安全面）与**状态模型**（RPC 原始状态 → 领域模型）。
//!
//! 真正的副作用（释放内嵌 aria2c、起子进程、走 JSON-RPC）分别在任务 9/10 落地；
//! 本任务只建立两者共用的那两块纯函数地基。

pub mod daemon;
pub mod rpc;
pub mod status;

use crate::delivery::{File, Manifest};

/// 判断 `manifest.path` 能否安全地交给 aria2。
///
/// 拒绝四类（契约 §3.2）：
///   1. 绝对路径（`/etc/passwd`）：不含 `..`，但 `dir=/etc` 会直接越界
///   2. 含 `..` 段（`a/../../b`）
///   3. 含控制字符（`\n`、`\r`、NUL）
///   4. 空段（`a//b`、结尾斜杠）
///
/// 第 1/2 类的后果是**写到目标目录之外**——不可逆的副作用。
///
/// ⚠️ **第 3 类在 Rust 侧的理由与 Go 侧不同，不要照抄 Go 的说法。**
/// Go 侧的后果是：aria2 的 `-i` 是行式格式，文件名里的换行会成为**选项边界**，
/// 一个叫 `x\n  dir=/Users/you` 的文件能把后续内容注入成 aria2 选项、写到目标目录之外。
/// **Rust 不生成 `-i` 文件**（契约 §8 的 `#5`：Rust 从第一天就走 JSON-RPC，
/// 路径作为结构化字段传递，没有行/空白边界）——**那条注入路径在本内核里不存在**。
/// 第 3 类**照样拒绝**，但理由是另外两条：
///   a. 这份清单来自网络、是不可信输入，控制字符出现在路径里就是**清单已损坏或被构造**的信号；
///   b. 纵深防御：路径还会被用作状态文件的键、用于本地文件系统操作与校验，
///      将来若有人在某处把它序列化成行式文本，守卫已经在了。
/// 按 Go 原样拒绝是**对的**（两边行为一致是逐条移植要保的东西），只是别把理由写错——
/// 写错的理由会让后来者以为守着一道已经不存在的边界。
///
/// 第 4 条与前两条有逻辑重叠（`"/x"` 的首段必为空串，`""` 的 Split 同样是 `[""]`），
/// **无法被单独测出**——留作显式意图与纵深防御：万一将来放宽空段规则，两道守卫依然成立。
pub fn safe_rel_path(p: &str) -> bool {
    if p.is_empty() || p.starts_with('/') {
        return false;
    }
    if p.contains(['\n', '\r', '\0']) {
        return false;
    }
    // 空段（`a//b`、结尾斜杠）与 `..` 都不接受：清单不该产出它们，
    // 出现即说明数据异常，宁可判失败。
    p.split('/').all(|seg| !seg.is_empty() && seg != "..")
}

/// 文件系统元数据里的 mtime，换算成**状态文件记录的量纲**。
///
/// 量纲约定：Unix 纪元起的**秒**，纳秒部分折进小数（`f64`）。
/// 逐位对应 Go 的 `float64(fi.ModTime().UnixNano()) / 1e9`
/// ——**不要换成「秒 + 纳秒/1e9」**，那在 `f64` 上会得到不同的值：
/// Go 实测 `1757855151s + 123000000ns` → `1757855151.1230001` vs `1757855151.1229999`
/// （差 1 ULP，`0x41da31af6bc7df3c` vs `0x41da31af6bc7df3b`）。
///
/// ⚠️ **这个换算在整个内核里只有这一份**（步骤 0 硬要求）：planner **读** mtime、
/// verify **写** mtime，两边必须调同一个函数。两边各写一份的后果是漂移，而漂移的表现
/// 是**该重下的文件被跳过**（静默少交，见契约 §1.7）；反向的表现是每一批都退化成全量重下。
/// Go 侧两边都写着 `float64(...UnixNano()) / 1e9`，但没有任何测试钉得住这个式子
/// ——任务 6 实测：「秒 + 纳秒/1e9」那个变异体在 Go 与 Rust 侧都存活。
/// `mtime_secs_matches_go_unixnano_formula` 是钉住它的那一条（不计入 verify 的 16 条移植）。
pub fn mtime_secs(t: std::time::SystemTime) -> f64 {
    use std::time::UNIX_EPOCH;
    // Go 的 UnixNano 是 int64 纳秒；负数（1970 之前）同样成立。
    fn from_parts(secs: i64, nanos: u32) -> f64 {
        (secs * 1_000_000_000 + i64::from(nanos)) as f64 / 1e9
    }
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => from_parts(d.as_secs() as i64, d.subsec_nanos()),
        Err(e) => {
            let d = e.duration();
            -from_parts(d.as_secs() as i64, d.subsec_nanos())
        }
    }
}

/// 交给 aria2 的一项任务。
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedFile {
    pub url: String,
    pub dir: String, // 相对目标目录；顶层文件用 "."
    pub out: String,
}

/// 由交付文件构造 aria2 任务项。路径不安全时返回 `None`，调用方应把它计入失败并
/// 在报告中列出（对应 Go 的 `ok=false`）。
///
/// ⚠️ `dir` 由 `manifest.path` 拆出且**不含随机码**——随机码只是传输前缀与访问凭据，
/// 三种下载方式的落盘基准必须一致（阶段 1 裁决）。
/// ⚠️ **不得规范化**（契约 §3.1）：不调 `Path::clean`、不折叠 `./`、不合并双斜杠。
///     `Join`/`clean` 过的路径会与 manifest 对不上，**让整棵树的四态静默错位**。
pub fn new_planned_file(m: &Manifest, f: &File) -> Option<PlannedFile> {
    if !safe_rel_path(&f.path) {
        return None;
    }
    // 只做「按最后一个 `/` 切开」这一件事：不 Join、不 clean、不折叠 `./`。
    let (parent, name) = split_path(&f.path);
    let dir = if parent.is_empty() { "." } else { parent };
    Some(PlannedFile {
        // URL 用 `manifest.path` **原文**逐段编码，与清单里的 `urls.txt` 同源。
        url: m.file_url(&f.path),
        dir: dir.to_string(),
        out: name.to_string(),
    })
}

/// 按**最后一个** `/` 把路径拆成 `(父目录, 文件名)`。对应 Go 的 `splitPath`。
///
/// 无 `/` 时父目录为空串（由调用方决定顶层文件怎么表示）。
fn split_path(p: &str) -> (&str, &str) {
    match p.rfind('/') {
        None => ("", p),
        Some(i) => (&p[..i], &p[i + 1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery::{File, Manifest};

    /// **步骤 0 的硬要求**：把 mtime 的换算公式钉死（**不计入 verify 的 16 条移植**，
    /// 按全局约束 6 的例外显式记账）。
    ///
    /// 期望值是**真实 Go 取值**，不是从 Rust 实现反推的：
    /// `float64(time.Unix(1757855151, 123000000).UnixNano()) / 1e9`
    /// `%.17g` = `1757855151.1230001`，`math.Float64bits` = `0x41da31af6bc7df3c`。
    /// 探针（`/tmp` 里的 Go 程序）原文见任务 7 报告。
    ///
    /// 判别力：把实现换成另一种常见写法「秒 as f64 + 纳秒 as f64 / 1e9」会得到
    /// `1757855151.1229999`（`0x41da31af6bc7df3b`，差 1 ULP）——
    /// 而 planner 读 / verify 写只要有一边是这种写法，状态条目就永远匹配不上，
    /// 该跳过的文件被重下（每一批 50 GB 白传），或该重下的被跳过（静默少交）。
    /// Go 侧**没有任何测试**钉得住这一点（任务 6 实测该变异体存活），所以这条测试是新加的。
    #[test]
    fn mtime_secs_matches_go_unixnano_formula() {
        let t = std::time::UNIX_EPOCH + std::time::Duration::new(1_757_855_151, 123_000_000);
        assert_eq!(
            mtime_secs(t),
            1757855151.1230001_f64,
            "换算公式与 Go 的 float64(UnixNano())/1e9 不一致"
        );
        // 字面量本身也必须落到**同一个 double** 上（Rust 的十进制解析是正确舍入的）。
        // 这条同时挡住「把上面的字面量改宽/改窄一位」——那种改动会让测试变成同义反复前的
        // 假绿：解析到相邻的 double 仍然「相等」判据下自洽，但已不是 Go 的那个值。
        assert_eq!(
            mtime_secs(t).to_bits(),
            0x41da_31af_6bc7_df3c,
            "与 Go 的 math.Float64bits 不一致"
        );
    }

    /// 对应 Go 的 `&delivery.Manifest{Code: "CODE", BaseURL: "http://x"}`：
    /// Go 未写的字段取零值，这里显式补齐。
    fn manifest() -> Manifest {
        Manifest {
            code: "CODE".to_string(),
            base_url: "http://x".to_string(),
            created_at: String::new(),
            expires_at: String::new(),
            total_files: 0,
            total_bytes: 0,
            files: Vec::new(),
        }
    }

    /// 对应 Go 的 `delivery.File{Path: p}`：`Size`/`CRC64` 取零值。
    fn file(path: &str) -> File {
        File {
            path: path.to_string(),
            size: 0,
            crc64: String::new(),
            source_mtime: String::new(),
        }
    }

    /// 对应 Go `TestPlannedFileFromManifestPath`。
    ///
    /// dir/out 必须由 `manifest.path` 拆出，且**不含随机码**——
    /// 三种下载方式的落盘基准必须一致（阶段 1 已定）。
    #[test]
    fn planned_file_from_manifest_path() {
        let m = manifest();
        let pf = new_planned_file(&m, &file("23999-JF-test/a/b/c/deep.txt"))
            .expect("正常路径不应被拒");
        assert_eq!(
            (pf.dir.as_str(), pf.out.as_str()),
            ("23999-JF-test/a/b/c", "deep.txt"),
            "拆解错误"
        );
        assert!(
            !pf.dir.contains("CODE"),
            "dir 不得含随机码，实际: {:?}",
            pf.dir
        );

        // 顶层文件用 "."
        let pf2 = new_planned_file(&m, &file("top.txt")).expect("顶层文件不应被拒");
        assert_eq!((pf2.dir.as_str(), pf2.out.as_str()), (".", "top.txt"));

        // ⚠️ **不得规范化**（契约 §3.1，承重）：`dir`/`out`/`url` 三个出口都必须用
        // `manifest.path` **原文**。上面两条用的都是规范化路径，把实现换成 `Path::clean`
        // 也照样绿——只有「安全但非规范」的路径能把这条钉住（`./x.txt`、`a/./b.txt`）。
        // 只钉其中一个出口会漏掉另外两个，所以三条断言缺一不可。
        // 后果的形状是**静默错位、不报错**：`a/./b.txt` 被折成 `a/b.txt` 后，落盘基准
        // 与清单对不上，整棵树的四态会显示错的状态而没有任何一处报错。
        let pf3 = new_planned_file(&m, &file("a/./b.txt")).expect("非规范但安全的路径不应被拒");
        assert_eq!(
            (pf3.dir.as_str(), pf3.out.as_str()),
            ("a/.", "b.txt"),
            "不得规范化 path"
        );
        assert_eq!(pf3.url, "http://x/CODE/a/./b.txt", "URL 也必须用 manifest.path 原文");
    }

    /// 对应 Go `TestPlannedFileURLKeepsCodePrefix`。
    ///
    /// URL 必须带上随机码。上面那条测试一个字段都没断言 `pf.URL`——
    /// 就算 `new_planned_file` 交回空 URL，它也照样绿，而那样一个文件都下不下来。
    #[test]
    fn planned_file_url_keeps_code_prefix() {
        let m = manifest();
        let pf = new_planned_file(&m, &file("23999-JF-test/a/b.txt")).expect("正常路径不应被拒");
        assert_eq!(
            pf.url, "http://x/CODE/23999-JF-test/a/b.txt",
            "URL 不对"
        );
    }

    /// 对应 Go `TestPlannedFileRejectsUnsafePaths`。
    ///
    /// 契约 §3.2 的四类都必须拒绝：绝对路径、`..` 段、控制字符、空段。
    /// `SafeRelPath` 的拒绝面由本条守——它和路径守卫是同一件事的两面。
    #[test]
    fn planned_file_rejects_unsafe_paths() {
        let m = manifest();
        let bad: [(&str, &str); 8] = [
            ("../escape.txt", "含 `..` 段——会写到目标目录之外（不可逆）"),
            (
                "a/../../b.txt",
                "含 `..` 段——同上，只是更显眼",
            ),
            ("..", "整个路径就是 `..`"),
            (
                "/etc/passwd",
                "绝对路径——不含 `..`，但 dir=/etc 直接越界",
            ),
            // 下面两条的**载荷**逐字取自 Go 侧测试（拒绝行为必须与 Go 一致）；
            // 但理由不同：Rust 不生成 `-i` 文件，**不存在**选项边界那条注入路径
            // （契约 §8 的 `#5`）。这里拒的是「清单已损坏或被构造」的信号 + 纵深防御。
            (
                "x\n  dir=/Users/someone",
                "换行——清单是不可信输入，控制字符出现即说明数据已损坏或被构造",
            ),
            (
                "x\r  dir=/tmp",
                "回车——同上（纵深防御：路径还会被用作状态键与本地文件系统操作）",
            ),
            ("a//b", "空段"),
            ("a/b/", "结尾斜杠（空段）"),
        ];
        for (p, why) in bad {
            assert!(
                new_planned_file(&m, &file(p)).is_none(),
                "应当拒绝 {p:?}（{why}）"
            );
        }

        // 正常路径不得被误拒——尤其是名字里带点的
        for p in [
            "a/..hidden.txt",
            "23999-JF-test/C24-8_×_25WS024/Figure/QC 图.png",
            "top.txt",
        ] {
            assert!(
                new_planned_file(&m, &file(p)).is_some(),
                "正常路径 {p:?} 不应被拒"
            );
        }
    }
}
