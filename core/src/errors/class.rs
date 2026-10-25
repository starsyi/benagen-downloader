//! 客户面对的错误只有八类（规格 §2.1）。
//!
//! **为什么要分类**：客户看到的不是 `engine_disconnected` 这种码，而是
//! "下载引擎没有响应 + 一句话解释 + 一个能照做的动作"。八类之外的一切都是内部错误——
//! 那类只给"意料之外的错误 + 复制诊断信息"，**不猜**是哪一步坏的。
//!
//! 三条规矩（都有用例守）：
//! 1. 每类必须有**标题、解释、动作**三句，两种语言各一份（`ALL` 遍历）；
//! 2. 严重度决定多条并存时谁置顶（`severity`）；
//! 3. 分类只由 `class_of` 一处决定——**别在别处再造一张表**。

use crate::i18n::Lang;

/// 客户能理解的九类病灶（0.3.1 增「文件名建不出来」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    /// 连不上服务器（连接失败/超时/被重置）。
    NetworkUnreachable,
    /// 域名解析不出地址。
    DnsFailed,
    /// 服务器回了 4xx/5xx（含 429 限速）。
    ServerRefused,
    /// 目标盘写得比下载慢（0.2.9 的测速探针）。
    DiskSlow,
    /// 空间不足或目录不可写。
    DiskFull,
    /// 引擎不应答，且自动重启的预算用尽。
    EngineUnresponsive,
    /// 引擎进程起不来 / 半路退出。
    EngineCrashed,
    /// 落盘内容与清单的 CRC64/大小不符。
    VerifyMismatch,
    /// 清单里的文件名在 Windows 上建不出来（冒号、通配符、保留设备名…）。
    NameInvalid,
}

/// 判定时的**旁证**：同一个码，旁证不同就是不同的类。
///
/// 例：`delivery_fetch_failed` 既可能是连不上（1 类），也可能是拿到了 404（3 类）。
/// 只凭码分类会撒谎，所以旁证由**知道真相的那一层**给（见 `kernel::op_load_delivery`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// 连不上（连接被拒/超时/被重置）。
    Unreachable,
    /// 域名解析失败。
    Dns,
    /// 服务器回了状态码。
    Http(u16),
    /// 目标盘写得太慢（实测值）。
    SlowDisk,
    /// 空间不足 / 不可写。
    Disk,
    /// 校验不符。
    Verify,
    /// 引擎进程不存在或已退出。
    Crashed,
    /// 引擎在但不回话（重启预算用尽）。
    Unresponsive,
    /// 清单里有 Windows 建不出来的文件名。
    BadName,
}

impl Class {
    /// 线上字符串（`ErrorBody.class` 与 `error_class=` 日志共用这一处）。
    pub const fn wire(self) -> &'static str {
        match self {
            Class::NetworkUnreachable => "network_unreachable",
            Class::DnsFailed => "dns_failed",
            Class::ServerRefused => "server_refused",
            Class::DiskSlow => "disk_slow",
            Class::DiskFull => "disk_full",
            Class::EngineUnresponsive => "engine_unresponsive",
            Class::EngineCrashed => "engine_crashed",
            Class::VerifyMismatch => "verify_mismatch",
            Class::NameInvalid => "name_invalid",
        }
    }

    /// 严重度：**数大者置顶**。规格 §2.2：`6/7 > 5 > 4 > 8 > 1/2/3`。
    ///
    /// 排在最前的理由是"客户此刻最该先处理它"：引擎不应答时什么都不会动；
    /// 盘的问题次之（换盘/换目录就能继续）；网络类可以自己好，放最后。
    pub const fn severity(self) -> u8 {
        match self {
            Class::EngineUnresponsive => 60,
            Class::EngineCrashed => 59,
            Class::DiskFull => 50,
            Class::DiskSlow => 40,
            Class::VerifyMismatch => 30,
            Class::NameInvalid => 58,
            Class::NetworkUnreachable => 20,
            Class::DnsFailed => 19,
            Class::ServerRefused => 18,
        }
    }

    /// 这条错误该配哪个按钮（界面上的"唯一动作"）。
    pub const fn title(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Class::NetworkUnreachable, Lang::Zh) => "连不上服务器",
            (Class::NetworkUnreachable, Lang::En) => "Cannot reach the server",
            (Class::DnsFailed, Lang::Zh) => "域名解析失败",
            (Class::DnsFailed, Lang::En) => "Domain lookup failed",
            (Class::ServerRefused, Lang::Zh) => "服务器拒绝了这次请求",
            (Class::ServerRefused, Lang::En) => "The server refused the request",
            (Class::DiskSlow, Lang::Zh) => "目标磁盘写得太慢",
            (Class::DiskSlow, Lang::En) => "The target disk is too slow",
            (Class::DiskFull, Lang::Zh) => "目标磁盘空间不足或不可写",
            (Class::DiskFull, Lang::En) => "The target disk is full or not writable",
            (Class::EngineUnresponsive, Lang::Zh) => "下载引擎没有响应",
            (Class::EngineUnresponsive, Lang::En) => "The download engine is not responding",
            (Class::EngineCrashed, Lang::Zh) => "下载引擎启动失败或已退出",
            (Class::EngineCrashed, Lang::En) => {
                "The download engine failed to start or has exited"
            }
            (Class::NameInvalid, Lang::Zh) => "有文件名在 Windows 上建不出来",
            (Class::NameInvalid, Lang::En) => "Some file names cannot be created on Windows",
            (Class::VerifyMismatch, Lang::Zh) => "文件校验没有通过",
            (Class::VerifyMismatch, Lang::En) => "A file failed verification",
            // `Lang::System` 永远不该走到这里（内核握手时就落定了具体语言）；
            // 万一走到，给英文——与 `i18n::text::t` 的兜底同一口径。
            (_, Lang::System) => self.title(Lang::En),
        }
    }

    /// 一句解释：**为什么会这样**，不带责备、不猜。
    pub const fn explain(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Class::NetworkUnreachable, Lang::Zh) => {
                "这台机器没能和下载服务器建立连接：网络、代理或对方短暂无响应都可能。"
            }
            (Class::NetworkUnreachable, Lang::En) => {
                "This machine could not open a connection to the download server: network, proxy, or a brief silence on the other side."
            }
            (Class::DnsFailed, Lang::Zh) => {
                "下载地址没解析出 IP：DNS 配置、代理或当前网络环境的问题。"
            }
            (Class::DnsFailed, Lang::En) => {
                "The download address did not resolve to an IP: a DNS, proxy, or network problem."
            }
            (Class::ServerRefused, Lang::Zh) => {
                "服务器回了拒绝：交付码可能不对、链接可能过期，也可能对方正在限速。"
            }
            (Class::ServerRefused, Lang::En) => {
                "The server refused: the code may be wrong, the link may have expired, or it is rate limiting."
            }
            (Class::DiskSlow, Lang::Zh) => {
                "落盘速度比下载速度慢，下载会跟着慢下来，长时间写不动看起来就像卡住。"
            }
            (Class::DiskSlow, Lang::En) => {
                "Writing is slower than downloading, so the download slows down and can look stuck."
            }
            (Class::DiskFull, Lang::Zh) => {
                "这一批放不下，或者目标目录写不进去（只读、被拔出、没有权限）。"
            }
            (Class::DiskFull, Lang::En) => {
                "This batch does not fit, or the target folder cannot be written to (read-only, unplugged, no permission)."
            }
            (Class::EngineUnresponsive, Lang::Zh) => {
                "自动重启试过几次仍然没有响应。已下载的文件都留在目标目录，可以直接重试。"
            }
            (Class::EngineUnresponsive, Lang::En) => {
                "Automatic restarts have not helped. Files already downloaded are in the target folder; you can retry directly."
            }
            (Class::EngineCrashed, Lang::Zh) => "引擎进程没能起来，或者半路退出了。",
            (Class::EngineCrashed, Lang::En) => {
                "The engine process could not start, or it exited midway."
            }
            (Class::NameInvalid, Lang::Zh) => {
                "清单里有名字带了 Windows 不允许的字符（例如冒号），系统建不出这些文件——文件本身是好的，换到 macOS / Linux 就能下。"
            }
            (Class::NameInvalid, Lang::En) => {
                "The list contains names with characters Windows does not allow (for example a colon), so these files cannot be created. The files are fine; a macOS or Linux client can download them."
            }
            (Class::VerifyMismatch, Lang::Zh) => {
                "落盘的内容和清单里的 CRC64 / 大小对不上，这份文件不完整。"
            }
            (Class::VerifyMismatch, Lang::En) => {
                "What landed on disk does not match the CRC64 / size in the list, so this file is incomplete."
            }
            (_, Lang::System) => self.explain(Lang::En),
        }
    }

    /// 唯一动作。
    pub const fn action(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Class::NetworkUnreachable, Lang::Zh) => "重试",
            (Class::NetworkUnreachable, Lang::En) => "Retry",
            (Class::DnsFailed, Lang::Zh) => "重试（或换网络）",
            (Class::DnsFailed, Lang::En) => "Retry, or switch networks",
            (Class::ServerRefused, Lang::Zh) => "稍后重试",
            (Class::ServerRefused, Lang::En) => "Retry later",
            (Class::DiskSlow, Lang::Zh) => "换内置盘，或调低并行数",
            (Class::DiskSlow, Lang::En) => "Use the internal disk, or lower the parallel count",
            (Class::DiskFull, Lang::Zh) => "换目录",
            (Class::DiskFull, Lang::En) => "Choose another folder",
            (Class::EngineUnresponsive, Lang::Zh) => "重启引擎",
            (Class::EngineUnresponsive, Lang::En) => "Restart the engine",
            (Class::EngineCrashed, Lang::Zh) => "重启引擎",
            (Class::EngineCrashed, Lang::En) => "Restart the engine",
            (Class::NameInvalid, Lang::Zh) => "联系交付方改名",
            (Class::NameInvalid, Lang::En) => "Ask the sender to rename them",
            (Class::VerifyMismatch, Lang::Zh) => "重新下载该文件",
            (Class::VerifyMismatch, Lang::En) => "Download that file again",
            (_, Lang::System) => self.action(Lang::En),
        }
    }
}

/// 八类，供"表必须写全"的遍历用例使用。
pub const ALL: &[Class] = &[
    Class::NetworkUnreachable,
    Class::DnsFailed,
    Class::ServerRefused,
    Class::DiskSlow,
    Class::DiskFull,
    Class::EngineUnresponsive,
    Class::EngineCrashed,
    Class::VerifyMismatch,
    Class::NameInvalid,
];

/// **唯一**的分类判据。
///
/// `cause` 是知道真相的那一层给的旁证；给不出就按码的默认归类（宁可粗，不要撒谎）。
/// 返回 `None` = 内部错误：不面向客户，界面只显示"意料之外的错误 + 复制诊断信息"。
pub fn class_of(code: &str, cause: Option<Cause>) -> Option<Class> {
    use crate::kcodes;
    // 旁证优先：同一个码，旁证不同就是不同的类
    if let Some(c) = cause {
        return Some(match c {
            Cause::Unreachable => Class::NetworkUnreachable,
            Cause::Dns => Class::DnsFailed,
            Cause::Http(code) if code >= 500 || code == 429 || (400..500).contains(&code) => {
                Class::ServerRefused
            }
            Cause::Http(_) => Class::ServerRefused,
            Cause::SlowDisk => Class::DiskSlow,
            Cause::Disk => Class::DiskFull,
            Cause::Verify => Class::VerifyMismatch,
            Cause::Crashed => Class::EngineCrashed,
            Cause::Unresponsive => Class::EngineUnresponsive,
            Cause::BadName => Class::NameInvalid,
        });
    }
    match code {
        kcodes::INVALID_FILENAME => Some(Class::NameInvalid),
        kcodes::PREFLIGHT_FAILED => Some(Class::DiskFull),
        kcodes::ENGINE_START_FAILED => Some(Class::EngineCrashed),
        kcodes::ENGINE_DISCONNECTED => Some(Class::EngineUnresponsive),
        // 拉清单失败：没有旁证时按"连不上"归类（最常见），有旁证时以旁证为准
        kcodes::DELIVERY_FETCH_FAILED => Some(Class::NetworkUnreachable),
        // 内部错误：码保留、界面不猜
        kcodes::UNKNOWN_METHOD
        | kcodes::NO_DELIVERY
        | kcodes::ENGINE_NOT_STARTED
        | kcodes::ENGINE_RPC_FAILED
        | kcodes::PATH_NOT_FOUND => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_cjk(c: char) -> bool {
        ('\u{4e00}'..='\u{9fff}').contains(&c)
    }

    /// 判据（规格 §2.2）：三句都必须有，且英文里**一个汉字都不能有**。
    /// 少写一类的任何一种语言 ⇒ 这里红（`match` 已经挡住了"漏写一类"，
    /// 这条守的是"漏写一句"——比如 en 的 explain 忘了填，只能靠遍历发现）。
    #[test]
    fn every_class_has_three_sentences_in_both_languages() {
        for c in ALL {
            for lang in [Lang::Zh, Lang::En] {
                assert!(!c.title(lang).is_empty(), "{:?} 缺标题（{lang:?}）", c.wire());
                assert!(!c.explain(lang).is_empty(), "{:?} 缺解释（{lang:?}）", c.wire());
                assert!(!c.action(lang).is_empty(), "{:?} 缺动作（{lang:?}）", c.wire());
            }
            assert!(
                !c.title(Lang::En)
                    .chars()
                    .chain(c.explain(Lang::En).chars())
                    .chain(c.action(Lang::En).chars())
                    .any(is_cjk),
                "英文界面里蹦出了中文: {}",
                c.wire()
            );
        }
    }

    /// 判据：**线上字符串是九个稳定的单词**——壳按它做分支，改名等于改协议。
    /// 第九个（`name_invalid`）是 0.3.1 现场补的：老壳不认识它，会退回"只显示原文"
    /// 的兜底路径（`Class::of_wire` 返回 None），不会崩也不会误分类。
    #[test]
    fn the_wire_words_are_exactly_the_spec() {
        let words: Vec<&str> = ALL.iter().map(|c| c.wire()).collect();
        assert_eq!(
            words,
            vec![
                "network_unreachable",
                "dns_failed",
                "server_refused",
                "disk_slow",
                "disk_full",
                "engine_unresponsive",
                "engine_crashed",
                "verify_mismatch",
                "name_invalid",
            ]
        );
    }

    /// 判据（规格 §2.2 的排序）：多条并存时置顶顺序 `6/7 > 5 > 4 > 8 > 1/2/3`。
    #[test]
    fn severity_puts_engine_first_and_network_last() {
        let s = |c: Class| c.severity();
        assert!(s(Class::EngineUnresponsive) > s(Class::DiskFull));
        assert!(s(Class::DiskFull) > s(Class::DiskSlow));
        assert!(s(Class::DiskSlow) > s(Class::VerifyMismatch));
        assert!(s(Class::VerifyMismatch) > s(Class::NetworkUnreachable));
        assert!(s(Class::NetworkUnreachable) > s(Class::ServerRefused));
        // 排序本身要能区分全部八类（相等就会"谁置顶"变得随机）
        let mut seen: Vec<u8> = ALL.iter().map(|c| c.severity()).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), ALL.len(), "八类的严重度必须两两不同");
    }

    /// 判据：**只凭码**时，三个"内部错误"必须留在 `None`（不面向客户）。
    /// 把它们硬塞进某一类，客户会看到一句与事实无关的解释。
    #[test]
    fn internal_codes_are_not_classified() {
        use crate::kcodes;
        for code in [
            kcodes::UNKNOWN_METHOD,
            kcodes::NO_DELIVERY,
            kcodes::ENGINE_NOT_STARTED,
            kcodes::ENGINE_RPC_FAILED,
            kcodes::PATH_NOT_FOUND,
        ] {
            assert_eq!(class_of(code, None), None, "{code} 不该有客户可见的类");
        }
    }

    /// 判据：码能归的按码归；**旁证能改判**（404 不是"连不上"，是"服务器拒绝"）。
    #[test]
    fn a_cause_overrides_the_code_default() {
        use crate::kcodes;
        assert_eq!(
            class_of(kcodes::PREFLIGHT_FAILED, None),
            Some(Class::DiskFull)
        );
        assert_eq!(
            class_of(kcodes::DELIVERY_FETCH_FAILED, None),
            Some(Class::NetworkUnreachable),
            "没有旁证时按最常见的归类"
        );
        assert_eq!(
            class_of(kcodes::DELIVERY_FETCH_FAILED, Some(Cause::Http(404))),
            Some(Class::ServerRefused),
            "拿到 404 就不是连不上"
        );
        assert_eq!(
            class_of(kcodes::DELIVERY_FETCH_FAILED, Some(Cause::Dns)),
            Some(Class::DnsFailed)
        );
        assert_eq!(
            class_of(kcodes::ENGINE_DISCONNECTED, None),
            Some(Class::EngineUnresponsive)
        );
    }
}
