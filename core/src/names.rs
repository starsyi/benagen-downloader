//! 文件名在 Windows 上能不能建出来（纯判据）。
//!
//! 现场（2026-10-22，交付码 `JCs.TsTJXMJVRfPM8Gt2`）：清单里有一批
//! `SW_vs_SG.gsea.go.GO:0002181.pdf` 这样的名字（GO 号带冒号）。macOS/Linux 建得出来，
//! **Windows 建不出来**——`:` 是 NTFS 的备用数据流分隔符，`< > : " | ? * \` 在名字里
//! 一律是 `ERROR_INVALID_NAME`。客户看到的表现最糟：传输列表"已完成"、校验"通过"、
//! 目录也在，打开却少 40 个文件。
//!
//! 所以判据要**单独成一份**，在两处用同一份：
//! * 开工前（`enqueue`）：直接拒绝，说明白是名字的问题、并给出示例；
//! * 校验时（`verify`）：这类路径单列成 `invalid_names`，不许混进"通过"。
//!
//! 为什么不写在 `engine::safe_rel_path` 里：那是**跨平台**的"清单路径可信吗"判据
//! （越界、控制字符、空段），Windows 的名字规则只在 Windows 上成立。混在一起会让
//! macOS 也拒绝一批本来能下的数据——对客户是纯损失。

/// 文件名里 Windows 不允许出现的字符（`/` 与 `\` 都是分隔符，但清单里的段内
/// 反斜杠同样建不出来，所以一并算）。
pub const FORBIDDEN: [char; 8] = ['<', '>', ':', '"', '|', '?', '*', '\\'];

/// Windows 的设备名：这些名字（带不带扩展名都算）不能当文件名。
pub const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 一个路径段（不含分隔符）为什么建不出来。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reason {
    /// 含 `FORBIDDEN` 里的字符。
    Char(char),
    /// 名字是保留设备名（`CON`、`NUL`、`COM1` …）。
    Reserved,
    /// 段太小写点或空格结尾：资源管理器能显示，但 API 会把它悄悄截掉，之后按原路径就找不到。
    Trailing,
    /// 段是空的（`a//b`）或超出 255 个字符。
    TooLong,
}

impl Reason {
    /// 一句话说明（中文；诊断日志与错误消息都用它，不翻译理由本身——理由里的字符
    /// 是客户要照着改的东西，翻译只会让人对不上）。
    pub fn describe(self) -> String {
        match self {
            Reason::Char(c) => format!("文件名里有 Windows 不允许的字符 `{c}`"),
            Reason::Reserved => "文件名是 Windows 保留的设备名".to_string(),
            Reason::Trailing => "文件名以点或空格结尾".to_string(),
            Reason::TooLong => "文件名的某一段为空或过长".to_string(),
        }
    }
}

/// 检查一个路径段。合法返回 `None`。
pub fn bad_component(seg: &str) -> Option<Reason> {
    if seg.is_empty() || seg.len() > 255 {
        return Some(Reason::TooLong);
    }
    if let Some(c) = seg.chars().find(|c| FORBIDDEN.contains(c)) {
        return Some(Reason::Char(c));
    }
    // 保留名按"第一个点之前的部分"比（`NUL.txt` 同样不能建）。
    let base = seg.split('.').next().unwrap_or(seg).trim_end();
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(base)) {
        return Some(Reason::Reserved);
    }
    if seg.ends_with('.') || seg.ends_with(' ') {
        return Some(Reason::Trailing);
    }
    None
}

/// 整条清单路径里第一个建不出来的段（跨平台可用：判断只看字符，不看当前系统）。
pub fn first_bad(path: &str) -> Option<(String, Reason)> {
    for seg in path.split('/') {
        if let Some(reason) = bad_component(seg) {
            return Some((seg.to_string(), reason));
        }
    }
    None
}

/// 一批路径的扫描结果（只用来措辞，不参与存储）。
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Scan {
    /// 有多少个文件建不出来。
    pub count: usize,
    /// 前三个坏文件（清单路径，逐字，供客户拿去对照/找交付方）。
    pub examples: Vec<String>,
    /// 第一个坏段与原因（措辞里点出"哪一个字符"）。
    pub first: Option<(String, Reason)>,
}

impl Scan {
    /// 是否一个都没有。
    pub fn is_clean(&self) -> bool {
        self.count == 0
    }
}

/// 扫描一批清单路径。
pub fn scan<I, S>(paths: I) -> Scan
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = Scan::default();
    for p in paths {
        let p = p.as_ref();
        if let Some((seg, reason)) = first_bad(p) {
            out.count += 1;
            if out.examples.len() < 3 {
                out.examples.push(p.to_string());
            }
            if out.first.is_none() {
                out.first = Some((seg, reason));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_go_number_that_broke_the_customer_is_caught() {
        let p = "result/06.de_enrich/gene/SW_vs_SG/enrich_All_diff/gsea/SW_vs_SG.plot.go/SW_vs_SG.gsea.go.GO:0002181.pdf";
        let (seg, reason) = first_bad(p).expect("这一条就是现场那个名字，必须命中");
        assert_eq!(seg, "SW_vs_SG.gsea.go.GO:0002181.pdf");
        assert_eq!(reason, Reason::Char(':'));
        assert!(reason.describe().contains('`'), "理由要点出具体字符");
    }

    #[test]
    fn every_forbidden_character_is_reported_as_itself() {
        for c in FORBIDDEN {
            let p = format!("a/b{}c.bin", c);
            assert_eq!(first_bad(&p), Some((format!("b{}c.bin", c), Reason::Char(c))));
        }
    }

    #[test]
    fn ordinary_names_pass() {
        for p in [
            "a/b/c.bin",
            "result/06.de_enrich/SW_vs_SG.plot.go/0002181.pdf",
            "中文/目录/文件.fastq.gz",
            "d/user name/report (2).pdf",
            "x/a.b.c",
        ] {
            assert!(first_bad(p).is_none(), "{p} 不该被判非法");
        }
    }

    #[test]
    fn reserved_device_names_are_caught_with_or_without_extension() {
        assert_eq!(first_bad("a/CON"), Some(("CON".to_string(), Reason::Reserved)));
        assert_eq!(first_bad("a/nul.txt"), Some(("nul.txt".to_string(), Reason::Reserved)));
        assert!(first_bad("a/CONSOLE.txt").is_none(), "只认整名，不做前缀猜测");
    }

    #[test]
    fn trailing_dot_or_space_is_caught() {
        assert_eq!(first_bad("a/b."), Some(("b.".to_string(), Reason::Trailing)));
        assert_eq!(first_bad("a/b "), Some(("b ".to_string(), Reason::Trailing)));
    }

    #[test]
    fn the_scan_counts_all_and_keeps_three_examples() {
        let paths: Vec<String> = (0..5)
            .map(|i| format!("d/f{i}:x.bin"))
            .chain(std::iter::once("d/ok.bin".to_string()))
            .collect();
        let s = scan(&paths);
        assert_eq!(s.count, 5);
        assert_eq!(s.examples, vec!["d/f0:x.bin", "d/f1:x.bin", "d/f2:x.bin"]);
        assert_eq!(s.first.as_ref().map(|(seg, _)| seg.as_str()), Some("f0:x.bin"));
        assert!(!s.is_clean());
        assert!(scan(Vec::<String>::new()).is_clean());
    }
}
