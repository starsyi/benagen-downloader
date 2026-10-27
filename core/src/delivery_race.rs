//! 双域名竞速（规格 W4）：国内 CDN 与国际 CDN 同时解析同一个交付码。
//!
//! 两个事实决定了设计：
//! 1. **同一个交付码只存在于一个域名**（客户在哪边上传的，就只在那边）——
//!    所以这不是"重试"，是"并行找"；
//! 2. 换个网络环境，哪边快可能反过来 —— 所以缓存**不落盘**，只在进程内记 24 小时。
//!
//! 这一层只做三件事：跑两个请求、先成功的赢、把过程记成四句人话给界面。
//! 真正的 HTTP 在做 I/O 的那一层（`kernel::fetch_race`），这里保持纯逻辑以便用假请求测。

use std::time::{Duration, Instant};

use crate::errors::Cause;
use crate::i18n::Lang;

/// 国内 CDN（与 0.2.x 的默认基址同一处）。
pub const DOMESTIC_BASE: &str = crate::delivery::DEFAULT_BASE_URL;
/// 国际 CDN。
pub const INTL_BASE: &str = "http://downloadint.benagen.com";

/// 竞速的"看谁先答"窗口：过了这个点就不再等另一边的**首次响应**。
pub const RACE_TIMEOUT: Duration = Duration::from_secs(8);

/// 一侧已经明确答复（例如国际 CDN 回 403 "我这边没有"）、另一侧**还在下清单**时，
/// 给剩下那一侧的总耐心。
///
/// 现场（2026-10-27）：十万文件的批次清单有几十 MB，国内 CDN 慢时 8 秒连头都还没回来，
/// 而国际 CDN 立刻回 403 ⇒ 竞速在 8 秒就把整次加载判死，界面写"international: HTTP 403"，
/// 其实国内那一路还在正常下载。**慢不等于没有**，所以这里必须留耐心。
///
/// 为什么是 180 秒：`delivery::fetch` 一次尝试（含 5 次重试与 1/2/4/8 秒退避）最坏约 90 秒，
/// 两倍余量够覆盖慢链路把清单下完，又不至于让真断网的客户干等。
pub const RACE_PATIENCE: Duration = Duration::from_secs(180);

/// 缓存保鲜期：24 小时（跨天换网络环境就该重新探）。
pub const CACHE_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

/// 同一个交付码可能在哪个域名下。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Domestic,
    International,
}

impl Origin {
    pub const fn base(self) -> &'static str {
        match self {
            Origin::Domestic => DOMESTIC_BASE,
            Origin::International => INTL_BASE,
        }
    }

    pub const fn wire(self) -> &'static str {
        match self {
            Origin::Domestic => "domestic",
            Origin::International => "international",
        }
    }

    pub fn from_base(base: &str) -> Option<Origin> {
        let b = base.trim_end_matches('/');
        if b == DOMESTIC_BASE {
            Some(Origin::Domestic)
        } else if b == INTL_BASE {
            Some(Origin::International)
        } else {
            None
        }
    }
}

/// 进程内缓存：`code → 上次命中的那一侧`。
///
/// **有界**：最多 [`CACHE_CAP`] 条，满了丢最旧的一条——交付码是客户手里的东西，
/// 一天开十几个是极限，没有理由让它无限长。
#[derive(Debug, Default)]
pub struct Cache {
    entries: Vec<(String, Origin, u64)>,
}

/// 缓存条数上限。
pub const CACHE_CAP: usize = 128;

impl Cache {
    pub fn new() -> Self {
        Cache { entries: Vec::new() }
    }

    /// 查（过期即当没有）。
    pub fn lookup(&mut self, code: &str, now_ms: u64) -> Option<Origin> {
        let entry = self
            .entries
            .iter()
            .find(|(c, _, _)| c == code)
            .map(|(_, o, at)| (*o, *at))?;
        if now_ms.saturating_sub(entry.1) > CACHE_TTL_MS {
            self.entries.retain(|(c, _, _)| c != code);
            return None;
        }
        Some(entry.0)
    }

    /// 记下这一批在哪个域名下。
    pub fn remember(&mut self, code: &str, origin: Origin, now_ms: u64) {
        self.entries.retain(|(c, _, _)| c != code);
        self.entries.push((code.to_string(), origin, now_ms));
        while self.entries.len() > CACHE_CAP {
            let oldest = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, (_, _, at))| *at)
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.entries.remove(oldest);
        }
    }
}

/// 解析过程中的四句话（规格 W4.3 的表；中文逐字，英文另给）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Start,
    Hit(Origin),
    /// 一侧失败，另一侧还在查：`side` 是失败的那一侧。
    OneSideFailed(Origin),
    AllFailed,
    /// 用了缓存（界面不必强调，但日志里要能区分"直连"与"竞速"）。
    CachedHit(Origin),
}

impl Stage {
    pub fn text(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Stage::Start, Lang::Zh) => "正在解析交付码…（同时查国内与国际 CDN）",
            (Stage::Start, Lang::En) => "Resolving the delivery code… (checking both CDNs)",
            (Stage::Hit(Origin::Domestic), Lang::Zh) => "已从国内 CDN 找到",
            (Stage::Hit(Origin::Domestic), Lang::En) => "Found on the domestic CDN",
            (Stage::Hit(Origin::International), Lang::Zh) => "已从国际 CDN 找到",
            (Stage::Hit(Origin::International), Lang::En) => "Found on the international CDN",
            (Stage::OneSideFailed(Origin::Domestic), Lang::Zh) => {
                "国内 CDN 没有找到（国际 CDN 还在查）"
            }
            (Stage::OneSideFailed(Origin::Domestic), Lang::En) => {
                "Not on the domestic CDN (still checking the international one)"
            }
            (Stage::OneSideFailed(Origin::International), Lang::Zh) => {
                "国际 CDN 没有找到（国内 CDN 还在查）"
            }
            (Stage::OneSideFailed(Origin::International), Lang::En) => {
                "Not on the international CDN (still checking the domestic one)"
            }
            (Stage::AllFailed, Lang::Zh) => "两个域名都没有这一批（交付码或网络有问题）",
            (Stage::AllFailed, Lang::En) => "Not found on either CDN",
            (Stage::CachedHit(Origin::Domestic), Lang::Zh) => "已从国内 CDN 找到（上次就在这边）",
            (Stage::CachedHit(Origin::Domestic), Lang::En) => {
                "Found on the domestic CDN (as last time)"
            }
            (Stage::CachedHit(Origin::International), Lang::Zh) => {
                "已从国际 CDN 找到（上次就在这边）"
            }
            (Stage::CachedHit(Origin::International), Lang::En) => {
                "Found on the international CDN (as last time)"
            }
            // `System` 永远不该到这里（内核握手就落定了具体语言）；万一走到给英文。
            (_, Lang::System) => self.text(Lang::En),
        }
    }
}

/// 把一句话追加进"解析过程"（**逐条追加**，不是覆盖；连续重复不重复追加）。
pub fn push_line(lines: &mut Vec<String>, text: &str) {
    if lines.last().map(String::as_str) != Some(text) {
        lines.push(text.to_string());
    }
}

/// 两边都失败时，报**更具体**的那个（规格 W4.1）：DNS > 连接 > 服务器。
///
/// "更具体"= 客户能据此动手的程度：解析不出（换 DNS/网络）> 连不上（换网络）>
/// 服务器回了 4xx/5xx（等一等/找交付方）。
pub fn more_specific(a: Cause, b: Cause) -> Cause {
    fn rank(c: Cause) -> u8 {
        match c {
            Cause::Dns => 3,
            Cause::Unreachable => 2,
            Cause::Http(_) => 1,
            _ => 0,
        }
    }
    if rank(a) >= rank(b) {
        a
    } else {
        b
    }
}

/// 从传输层错误串里读出"更像哪一类"（`delivery::fetch` 给的是人话串）。
pub fn cause_of(err: &str) -> Cause {
    let e = err.to_ascii_lowercase();
    if e.contains("dns") || e.contains("resolve") || err.contains("解析") {
        Cause::Dns
    } else if let Some(code) = crate::delivery::http_status(err) {
        Cause::Http(code)
    } else {
        Cause::Unreachable
    }
}

/// 一次竞速的结果。
#[derive(Debug)]
pub struct Raced {
    pub manifest: crate::delivery::Manifest,
    pub origin: Origin,
    pub lines: Vec<String>,
}

/// 竞速的本体：`fetch` 由调用方给（真实现是 HTTP，测试里是假请求）。
///
/// 语义：
/// * 两个域名**同时**发；
/// * 谁先成功就返回谁，另一个线程的结果丢弃（它是 daemon 线程，不会拖住调用方）；
/// * 两个都失败 ⇒ 用 [`more_specific`] 选归因；
/// * 超过 [`RACE_TIMEOUT`] 还没人成功 ⇒ 也一样归因（当作"这一边没有"）。
pub fn race<F>(fetch: F, lang: Lang, timeout: Duration) -> Result<Raced, (String, Cause)>
where
    F: Fn(Origin) -> Result<crate::delivery::Manifest, String> + Send + Sync + 'static,
{
    let mut lines = vec![Stage::Start.text(lang).to_string()];
    let (tx, rx) = std::sync::mpsc::channel();
    let fetch = std::sync::Arc::new(fetch);
    for origin in [Origin::Domestic, Origin::International] {
        let tx = tx.clone();
        let f = std::sync::Arc::clone(&fetch);
        std::thread::spawn(move || {
            let _ = tx.send((origin, f(origin)));
        });
    }
    drop(tx);

    let soft = Instant::now() + timeout;
    let hard = Instant::now() + RACE_PATIENCE;
    let mut failures: Vec<(Origin, String)> = Vec::new();
    // 两边都失败时，归因取"更具体"的那个；先给一个最不具体的兜底。
    let mut worst: Cause = Cause::Unreachable;
    let mut all_failed_line = false;
    let mut answered = 0usize;
    while answered < 2 {
        // 前 `timeout` 之内按"先答者赢"收；之后只可能剩下一路还在下（另一路已失败），
        // 这时把窗口放宽到 `RACE_PATIENCE`——它慢，不代表它没有。
        let limit = if Instant::now() < soft { soft } else { hard };
        let left = limit.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok((origin, Ok(m))) => {
                answered += 1;
                push_line(&mut lines, Stage::Hit(origin).text(lang));
                return Ok(Raced {
                    manifest: m,
                    origin,
                    lines,
                });
            }
            Ok((origin, Err(e))) => {
                answered += 1;
                push_line(&mut lines, Stage::OneSideFailed(origin).text(lang));
                worst = more_specific(cause_of(&e), worst);
                failures.push((origin, e));
                if !all_failed_line && failures.len() == 2 {
                    all_failed_line = true;
                    push_line(&mut lines, Stage::AllFailed.text(lang));
                }
            }
            // 软窗口到点但耐心还没用完：不回话的只是"慢"，不是"没有"，继续等。
            Err(_) => {
                if Instant::now() >= hard {
                    break;
                }
            }
        }
    }
    let detail = if failures.is_empty() {
        // 两边都还在跑、耐心用完：不能说"没有"，只能说"没答完"。
        "no answer within the patience window".to_string()
    } else {
        failures
            .iter()
            .map(|(o, e)| format!("{}: {e}", o.wire()))
            .collect::<Vec<_>>()
            .join(" | ")
    };
    push_line(&mut lines, Stage::AllFailed.text(lang));
    Err((detail, worst))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(code: &str, base: &str) -> crate::delivery::Manifest {
        crate::delivery::Manifest {
            code: code.to_string(),
            base_url: base.to_string(),
            created_at: String::new(),
            expires_at: String::new(),
            total_files: 0,
            total_bytes: 0,
            files: Vec::new(),
        }
    }

    /// 判据（规格 W4.1）：两边同时查，**先成功的那边赢**，另一边的慢请求不许拖住调用方。
    #[test]
    fn the_first_side_to_succeed_wins() {
        let r = race(
            |o| match o {
                Origin::Domestic => {
                    std::thread::sleep(Duration::from_millis(100));
                    Err("dns lookup failed".to_string())
                }
                Origin::International => Ok(manifest("X", INTL_BASE)),
            },
            Lang::Zh,
            Duration::from_secs(2),
        )
        .expect("国际那边应当命中");
        assert_eq!(r.origin, Origin::International);
        assert_eq!(r.manifest.base_url, INTL_BASE);
        assert_eq!(r.lines.first().unwrap(), "正在解析交付码…（同时查国内与国际 CDN）");
        assert_eq!(r.lines.last().unwrap(), "已从国际 CDN 找到");
    }

    /// 判据（2026-10-27 现场）：一侧已经回了 403「我这边没有」、另一侧**还在下清单**（慢，
    /// 但没出错）⇒ 必须把耐心留给还在跑的那一侧，不能把整次加载判死。十万文件的清单几十 MB，
    /// 8 秒连响应头都还没回来是常态。
    #[test]
    fn a_403_on_one_side_does_not_abort_the_side_that_is_still_downloading() {
        let r = race(
            |o| match o {
                Origin::Domestic => {
                    // 比"看谁先答"的窗口（下面传的 30 毫秒）慢得多，但仍在耐心之内
                    std::thread::sleep(Duration::from_millis(200));
                    Ok(manifest("X", DOMESTIC_BASE))
                }
                Origin::International => Err("HTTP 403".to_string()),
            },
            Lang::Zh,
            Duration::from_millis(30),
        )
        .expect("国内那一路只是慢，不该被判成没有");
        assert_eq!(r.origin, Origin::Domestic);
        assert_eq!(r.manifest.base_url, DOMESTIC_BASE);
        assert!(
            r.lines
                .iter()
                .any(|l| l == "国际 CDN 没有找到（国内 CDN 还在查）"),
            "过程里要说明国际那边已经答了 403、国内还在查：{:?}",
            r.lines
        );
        assert_eq!(r.lines.last().unwrap(), "已从国内 CDN 找到");
    }

    /// 判据：两边都失败 ⇒ 归因取**更具体**的那个（DNS 盖过连接），
    /// 且过程里既有"单边失败"也有"全失败"。
    #[test]
    fn when_both_fail_the_more_specific_cause_wins() {
        let err = race(
            |o| match o {
                Origin::Domestic => Err("connection refused".to_string()),
                Origin::International => Err("dns lookup failed".to_string()),
            },
            Lang::Zh,
            Duration::from_secs(2),
        )
        .unwrap_err();
        assert_eq!(err.1, Cause::Dns, "解析不出比连不上更具体");
        assert!(err.0.contains("domestic"), "错误串要保留两边的原文: {}", err.0);
    }

    /// 判据（规格 W4.1 的顺序）：DNS > 连接 > 服务器状态码。
    /// 顺序就是"客户能据此动手的程度"——解析不出最该动，状态码最该等。
    #[test]
    fn dns_beats_connection_and_connection_beats_a_status_code() {
        assert_eq!(
            more_specific(Cause::Http(404), Cause::Unreachable),
            Cause::Unreachable
        );
        assert_eq!(
            more_specific(Cause::Http(404), Cause::Dns),
            Cause::Dns
        );
        assert_eq!(
            more_specific(Cause::Unreachable, Cause::Http(503)),
            Cause::Unreachable
        );
    }

    /// 判据（规格 W4.2）：缓存 24 小时内有效、过期当没有、有上限。
    #[test]
    fn the_cache_lives_a_day_and_stays_bounded() {
        let mut c = Cache::new();
        c.remember("CODE1", Origin::International, 1_000);
        assert_eq!(c.lookup("CODE1", 2_000), Some(Origin::International));
        assert_eq!(
            c.lookup("CODE1", 1_000 + CACHE_TTL_MS + 1),
            None,
            "过了 24 小时就当没记过"
        );
        for i in 0..(CACHE_CAP + 5) {
            c.remember(&format!("C{i}"), Origin::Domestic, 10_000 + i as u64);
        }
        assert!(
            c.entries.len() <= CACHE_CAP,
            "缓存必须有上限，实际 {}",
            c.entries.len()
        );
    }

    /// 判据：英文界面里不许出现中文（四句话都要）。
    #[test]
    fn every_stage_has_english_without_cjk() {
        for stage in [
            Stage::Start,
            Stage::Hit(Origin::Domestic),
            Stage::Hit(Origin::International),
            Stage::OneSideFailed(Origin::Domestic),
            Stage::OneSideFailed(Origin::International),
            Stage::AllFailed,
            Stage::CachedHit(Origin::Domestic),
            Stage::CachedHit(Origin::International),
        ] {
            assert!(
                !stage.text(Lang::En).chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                "英文里出现了中文: {:?}",
                stage
            );
            assert!(!stage.text(Lang::Zh).is_empty());
        }
    }

    /// 判据：同一个域名能从 URL 认出来，陌生域名不许被误认。
    #[test]
    fn the_two_bases_are_recognised_and_others_are_not() {
        assert_eq!(Origin::from_base(DOMESTIC_BASE), Some(Origin::Domestic));
        assert_eq!(Origin::from_base(INTL_BASE), Some(Origin::International));
        assert_eq!(Origin::from_base("http://example.com"), None);
    }
}
