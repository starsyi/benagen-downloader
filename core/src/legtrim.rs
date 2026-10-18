//! legtrim —— 跨拍**摘腿判决**：什么时候摘、摘哪几条、补什么、怎么防抖
//! （规格 2026-10-18 §2 的 D6/D7、§3.1）。
//!
//! `core/src/engine/legs.rs` 只回答「这一拍每条腿多快、多慢算该摘」；
//! 本文件回答**跨拍**的问题：一条腿要连续慢几拍才算数、摘掉之后多久不许补回、
//! 一条腿都不许摘到只剩几条。它是**纯判决**：没有时钟、没有 RPC、没有 IO，
//! 拍号 `beat` 由调用方（`core/src/kernel.rs` 的定时拍线程）单调递增地给。
//! 这个分工与 `core/src/autoretry.rs` 同形：那边只判「哪些错该重试、等多久」，
//! 线程与 RPC 在 `kernel.rs`；**别把两者混在一起**，判据能单测，线程和 RPC 只能靠联调。
//!
//! 为什么需要去抖：现场速率是 10 s 一拍的单点采样，一次 GC、一次路由抖动、
//! 一条分片刚好下完，都会让某一拍的速度掉到阈值以下。看见单拍慢就换腿，
//! 会把下载变成「不断地摘了补、补了再摘」，每次 `changeUri` 都要重新建连、
//! 重新铺分片，反而比留着慢腿更慢。
//!
//! 为什么需要「全慢不摘」：所有腿一起慢说明瓶颈不在选边（服务端限速、整条链路
//! 拥塞、冷启动还没跑起来），此时把腿全换成本地候选只是把同一批候选再试一遍，
//! 白烧 RPC、白抖连接（规格 D6 的裁决）。这个判据与 `legs::trim_threshold`
//! 的 128 KiB/s 下限是同一个思路的两道闸：下限挡「中位数本身太小」，
//! 全慢挡「所有人都低于阈值」。
//!
//! 判据对坏数据的态度与 `legs.rs` 一致：**永远不 panic、只让这一拍不动作**
//! （返回 `None`）。摘腿是优化，不是正确性——不能让一次空腿表或一条垃圾 URI
//! 把后台拍线程带走（规格 §5 的 I3）。

use std::collections::BTreeMap;

use crate::engine::legs::{leg_is_ip_literal, median, trim_threshold, Leg};

/// 暖机拍数：前 3 拍只观察、不动作。
///
/// 为什么是 3 拍：引擎刚起时第一拍的 `downloadSpeed` 常常是 0（连接刚建、分片还没铺开），
/// 第二拍也可能只跑了一部分链路；3 拍 ≈ 30 s（一拍 10 s）足够让每条腿都完成一次
/// 握手 + 分片分配。再长就会让一条**真的死了**的腿白占 10 s 以上的带宽；
/// 再短则可能把「还没跑起来」误判成「慢」。
///
/// 暖机只挡**动作**，不挡**计数**：暖机期间仍然按本拍速率更新连续慢计数，
/// 所以一条从第一拍就慢的腿会在第 4 拍（暖机结束后的第一拍）就满 2 拍 streak，
/// 不必再等两拍。这也是 `WARMUP_BEATS` 与 `BAD_BEATS_TO_TRIM` 的关系：
/// 暖机必须 ≥ 去抖拍数，否则暖机结束那一刻 streak 还不够，去抖窗口会白等。
pub const WARMUP_BEATS: u64 = 3;

/// 连续低于阈值多少拍才摘：2 拍。
///
/// 为什么是 2 而不是 1：单拍慢可能只是采样噪声（见文件头）。两拍连续 ≈ 20 s，
/// 足以滤掉一次抖动，又只比单拍多付 10 s——对一条真的只有 100 KB/s 的腿来说，
/// 20 s 少下的数据远小于「整轮下载被它拖住」的代价。
/// 为什么不是 3：每多一拍就多 10 s 的低速窗口；现场实测的坏腿（5–8 倍差距）
/// 在第二拍就已经很明显，第三拍只会拖慢恢复。
pub const BAD_BEATS_TO_TRIM: u8 = 2;

/// 无论多慢，至少留这么多条腿：2 条。
///
/// 为什么不能摘到 1 条：`aria2` 的 `split=N` 是「最多 N 条腿、每条腿 1 条连接」
/// （`legs.rs` 文件头），只剩 1 条腿等于放弃并行；更糟的是，那条唯一的腿一旦卡住，
/// 整个任务就卡住——没有第二条腿可以兜住分片，也没有域名腿可以靠 DNS 换 IP 复活。
/// 留 2 条时至少还有一条快的在跑，慢的那条摘掉才真正有意义。
///
/// 摘到只剩 2 条就停，也保证「摘腿」不会退化成「把整张腿表换掉」：
/// 后者与「全慢不摘」要防的是同一件事。
pub const MIN_LEGS: usize = 2;

/// 被摘掉的 URI 在多少拍内不许再补回：12 拍 ≈ 120 s。
///
/// 为什么需要冷却：候选池（例如按地区下发的 IP 列表）常常把刚被摘掉的 IP 又发回来；
/// 没有冷却就会「这一拍摘掉、下一拍补回、再下一拍又摘掉」，每次 `changeUri`
/// 都重新建连、重新铺分片，振荡的成本比老老实实留着慢腿更高。
///
/// 为什么是 12 拍：它要**长于**一次完整的测量周期（暖机 3 拍 + 去抖 2 拍 = 5 拍），
/// 否则新补进来的腿还没被公平地测过就又被摘；也要长于常见的路由/DNS 抖动恢复时间。
/// 12 拍 ≈ 120 s 覆盖「新候选跑起来 → 被观察 2 拍 → 再决定」的完整闭环，
/// 又不至于长到让一个真的恢复了的好 IP 被永久拉黑。
///
/// 冷却表记的是 `beat + COOLDOWN_BEATS`，语义是**「到这一拍为止都不许补回」**
/// （含端点）：在第 B 拍摘掉，B+1 … B+12 这 12 拍都不许补回，B+13 才解禁。
/// 判据因此写成 `beat <= cooled_until`，而不是 `cooled_until > beat`——
/// 后者在离散拍号上实际只挡 11 拍（B+1 … B+11），与「12 拍 ≈ 120 s」差一拍。
pub const COOLDOWN_BEATS: u64 = 12;

/// 一条腿被摘掉的行动：删这些 URI，加这些新候选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub del: Vec<String>,
    pub add: Vec<String>,
}

/// 跨拍状态。纯数据，没有时钟：拍号由调用方给。
///
/// `streak` 是「连续低于阈值」的拍数，**按 URI 记**（腿可能在这一拍出现、下一拍消失，
/// 位置不能当身份）；`cooled_until` 记「到哪一拍为止不许补回」，同样按 URI 记。
/// 三张表都只在 `plan` 里读写，没有别的入口。
#[derive(Debug, Default)]
pub struct TrimState {
    /// 已经走过的拍数，用来实现暖机。调用方每拍 +1，单调递增。
    observations: u64,
    /// URI → 连续低于阈值的拍数。摘掉时清零；本拍变快也清零。
    streak: BTreeMap<String, u8>,
    /// URI → 最后一个仍处于冷却的拍号（含端点）。
    cooled_until: BTreeMap<String, u64>,
}

impl TrimState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 走一拍。`beat` 单调递增（调用方每次 +1）；`legs` 是这一拍的腿表；
    /// `candidates` 是可补进来的新 URI（调用方已经排除在用的腿）。
    /// 返回 `None` 表示这一拍不动作（暖机、去抖、全慢、无候选……）。
    pub fn plan(&mut self, beat: u64, legs: &[Leg], candidates: &[String]) -> Option<Plan> {
        self.observations = self.observations.saturating_add(1);

        // 阈值必须用**所有**腿（含域名腿）算：域名腿也是这条链路的一部分，
        // 把它排除会让「一批 IP 腿 + 一条域名腿」的中位数偏高或偏低，误判 IP 腿。
        let speeds: Vec<u64> = legs.iter().map(|leg| leg.speed).collect();
        let threshold = trim_threshold(median(&speeds));

        // 先按本拍结果更新连续计数，再做判决：这样「这一拍」也算进 streak，
        // 暖机期间慢的腿在暖机结束那一拍就已经满 2 拍，不必多等一拍。
        self.update_streak(legs, threshold);

        // 冷却表按拍号自然过期（保留「到这一拍为止仍冷却」的条目，含端点）。
        // 不清理的话，候选池里每个被摘过的 URI 都会永久留一行。
        self.cooled_until.retain(|_, until| *until >= beat);

        // 暖机：只观察、只记账，绝不动作。
        if self.observations <= WARMUP_BEATS {
            return None;
        }

        let mut to_trim: Vec<&Leg> = legs
            .iter()
            .filter(|leg| {
                leg_is_ip_literal(&leg.uri)
                    && leg.speed < threshold
                    && self.streak.get(&leg.uri).copied().unwrap_or(0) >= BAD_BEATS_TO_TRIM
                    && !self.is_cooling(&leg.uri, beat)
            })
            .collect();

        if to_trim.is_empty() {
            return None;
        }

        // 全慢不摘：**所有**腿（含域名腿）都低于阈值 ⇒ 没有任何一条是快的。
        // 此时瓶颈不在选边（服务端限速 / 整条链路拥塞），把腿换成本地候选
        // 只是把同一批候选再试一遍。判据必须是「所有腿」而不是「所有腿都在 to_trim 里」：
        // 产线形状永远是「域名腿 + 若干 IP 腿」，而域名腿**永不进 to_trim**
        // （`leg_is_ip_literal` 门），后一种写法在产线上永远不成立——那等于没有这道闸。
        if legs.iter().all(|leg| leg.speed < threshold) {
            return None;
        }

        // 从最慢的开始摘。`sort_by_key` 是稳定排序，同速腿按腿表顺序取，
        // 结果可复现（测试里同速的两条腿才能断言“摘的是哪一条”）。
        to_trim.sort_by_key(|leg| leg.speed);

        // 至少留 MIN_LEGS 条：3 条腿 2 条该摘时只摘 1 条（最慢的那条）。
        // `saturating_sub` 覆盖「本来就不足 2 条」的情形：配额 0，整轮不动作。
        let room = legs.len().saturating_sub(MIN_LEGS);
        to_trim.truncate(room);
        if to_trim.is_empty() {
            return None;
        }

        let del: Vec<String> = to_trim.iter().map(|leg| leg.uri.clone()).collect();

        // 补候选：按调用方给的顺序，跳过仍在用的、已经在 del 里的、冷却中的，
        // 以及本拍已经选过的（防止同一 URI 在 add 里出现两次）。最多补 del.len() 条；
        // 候选不够就少补——**摘了不补也比留着慢腿好**，所以 del 不受 add 影响。
        let mut add = Vec::with_capacity(del.len());
        for candidate in candidates {
            if add.len() == del.len() {
                break;
            }
            if legs.iter().any(|leg| leg.uri == candidate.as_str())
                || del.contains(candidate)
                || add.contains(candidate)
                || self.is_cooling(candidate, beat)
            {
                continue;
            }
            add.push(candidate.clone());
        }

        // 动作之后才登记冷却并清零 streak：本拍的 add 选择必须还看得见
        // 「这一拍正在被摘的 URI 仍在用」这一事实（`legs` 里有它，第 6 步会跳过）。
        for uri in &del {
            self.cooled_until
                .insert(uri.clone(), beat.saturating_add(COOLDOWN_BEATS));
            self.streak.insert(uri.clone(), 0);
        }

        Some(Plan { del, add })
    }

    /// 按本拍速率更新每条腿的连续低速计数：低于阈值 +1（饱和），否则清零。
    fn update_streak(&mut self, legs: &[Leg], threshold: u64) {
        for leg in legs {
            let streak = self.streak.entry(leg.uri.clone()).or_insert(0);
            if leg.speed < threshold {
                *streak = streak.saturating_add(1);
            } else {
                *streak = 0;
            }
        }
    }

    /// 这个 URI 这一拍是否还在冷却里。`cooled_until` 是**最后一个**仍冷却的拍号，
    /// 所以用 `beat <= until`（含端点）：第 B 拍摘掉、冷却 12 拍 ⇒ B+1 … B+12 全挡，
    /// B+13 解禁，正好 12 拍 ≈ 120 s。
    fn is_cooling(&self, uri: &str, beat: u64) -> bool {
        self.cooled_until
            .get(uri)
            .map_or(false, |&until| beat <= until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: u64 = 10_000_000;
    const SLOW: u64 = 100_000;

    fn leg(last: u8, speed: u64) -> Leg {
        Leg {
            uri: format!("http://10.9.0.{last}/f"),
            speed,
        }
    }

    fn slow_ip(last: u8) -> Leg {
        leg(last, SLOW)
    }

    fn fast_ip(last: u8) -> Leg {
        leg(last, FAST)
    }

    fn domain(speed: u64) -> Leg {
        Leg {
            uri: "http://download.benagen.com/f".into(),
            speed,
        }
    }

    fn candidates() -> Vec<String> {
        vec!["http://5.6.7.8/new".to_string()]
    }

    /// 暖机期间只观察、不动作；暖机结束后若仍连续慢，才在下一拍动作。
    ///
    /// 这里故意让腿在第一拍就慢：暖机挡的是**动作**，不是**计数**。
    /// 如果实现把暖机也当成「不计数」，第 4 拍 streak 只有 1，得再等一拍，
    /// 这条测试会立刻红。
    #[test]
    fn warmup_observes_but_does_not_act() {
        let mut state = TrimState::new();
        let legs = vec![slow_ip(1), slow_ip(2), fast_ip(3)];
        let candidates = candidates();
        for beat in 1..=WARMUP_BEATS {
            assert!(
                state.plan(beat, &legs, &candidates).is_none(),
                "暖机第 {beat} 拍只观察不动作：引擎刚起时速率统计还没稳定"
            );
        }
        let plan = state
            .plan(WARMUP_BEATS + 1, &legs, &candidates)
            .expect("暖机结束后仍连续慢，第 4 拍应开始摘腿");
        assert_eq!(
            plan.del.len(),
            legs.len() - MIN_LEGS,
            "3 条腿里 2 条慢，但至少留 {MIN_LEGS} 条，只能摘 {} 条",
            legs.len() - MIN_LEGS
        );
    }

    /// 单拍慢不动作；变快一拍会把 streak 清零，之后再慢一拍仍不动作。
    #[test]
    fn a_single_bad_beat_does_not_trim() {
        let mut state = TrimState::new();
        let good = vec![leg(1, 5_000_000), fast_ip(2), leg(3, 8_000_000)];
        let bad = vec![slow_ip(1), fast_ip(2), leg(3, 8_000_000)];
        for beat in 1..=WARMUP_BEATS {
            assert!(
                state.plan(beat, &good, &[]).is_none(),
                "暖机第 {beat} 拍不动作"
            );
        }
        assert!(
            state.plan(4, &bad, &[]).is_none(),
            "慢 1 拍不动作：单拍抖动可能只是采样噪声"
        );
        assert!(
            state.plan(5, &good, &[]).is_none(),
            "变快的一拍必须把连续慢计数清零，否则下一次抖动会接着上一次的计数"
        );
        assert!(
            state.plan(6, &bad, &[]).is_none(),
            "清零后只慢 1 拍仍不动作（若不复位，这里会因为累积到 2 拍而误摘）"
        );
    }

    /// 暖机后连续 2 拍低于阈值 ⇒ 摘该 URI，并从候选补一条。
    #[test]
    fn two_consecutive_bad_beats_trim() {
        let mut state = TrimState::new();
        let good = vec![leg(1, 5_000_000), fast_ip(2), leg(3, 8_000_000)];
        let bad = vec![slow_ip(1), fast_ip(2), leg(3, 8_000_000)];
        let candidates = candidates();
        for beat in 1..=WARMUP_BEATS {
            assert!(
                state.plan(beat, &good, &candidates).is_none(),
                "暖机第 {beat} 拍不动作"
            );
        }
        assert!(
            state.plan(4, &bad, &candidates).is_none(),
            "第 1 拍慢只记账不动作"
        );
        let plan = state
            .plan(5, &bad, &candidates)
            .expect("连续第 2 拍慢应摘腿");
        assert_eq!(
            plan.del,
            vec!["http://10.9.0.1/f".to_string()],
            "只摘连续 2 拍低于阈值的 IP 腿"
        );
        assert_eq!(plan.add, candidates, "摘 1 条就从候选里按顺序补 1 条");
    }

    /// 所有腿同速偏慢 ⇒ 瓶颈不在选边（服务端限速的形状），整轮不摘。
    #[test]
    fn all_slow_legs_are_not_trimmed() {
        let mut state = TrimState::new();
        let legs = vec![slow_ip(1), slow_ip(2), slow_ip(3)];
        let candidates = candidates();
        for beat in 1..=6 {
            assert!(
                state.plan(beat, &legs, &candidates).is_none(),
                "第 {beat} 拍：所有腿都低于阈值＝全部该摘＝全慢，换腿无益，不得动作"
            );
        }
    }

    /// **产线形状**的全慢：域名腿 + 几条 IP 腿全都低于阈值 ⇒ 一条都不摘。
    ///
    /// 与上一条的区别是这条带域名腿——域名腿永不进 `to_trim`（`leg_is_ip_literal` 门），
    /// 所以「所有腿都在 to_trim 里」这种判据在产线上**永远不成立**，等于没有这道闸。
    /// 服务端统一限速时（客户现场的 1.7 MB/s 就有这一档嫌疑），这条是唯一挡住
    /// 「每 2 拍摘一次、每 12 拍再抖一次」的东西。
    #[test]
    fn all_slow_with_a_domain_leg_is_not_trimmed() {
        let mut state = TrimState::new();
        let legs = vec![domain(60_000), slow_ip(1), slow_ip(2)];
        let candidates = candidates();
        for beat in 1..=8 {
            assert!(
                state.plan(beat, &legs, &candidates).is_none(),
                "第 {beat} 拍：域名腿 + IP 腿全都低于阈值＝服务端限速的形状，换腿无益，不得动作"
            );
        }
    }

    /// 域名腿即使最慢也不进 `del`；同一拍里真正的 IP 慢腿照摘。
    #[test]
    fn domain_leg_is_never_deleted() {
        let mut state = TrimState::new();
        let legs = vec![domain(50_000), slow_ip(1), fast_ip(2)];
        let candidates = candidates();
        for beat in 1..=WARMUP_BEATS {
            assert!(
                state.plan(beat, &legs, &candidates).is_none(),
                "暖机第 {beat} 拍不动作"
            );
        }
        let plan = state
            .plan(WARMUP_BEATS + 1, &legs, &candidates)
            .expect("域名腿不摘，但同拍的 IP 慢腿仍应摘");
        assert_eq!(
            plan.del,
            vec!["http://10.9.0.1/f".to_string()],
            "del 里只允许出现 IP 字面量腿"
        );
        assert!(
            !plan
                .del
                .iter()
                .any(|uri| uri == "http://download.benagen.com/f"),
            "域名腿是唯一「IP 变了还能活」的腿，即使最慢也永不摘"
        );
    }

    /// 3 条腿、2 条该摘 ⇒ 只摘 1 条，绝不摘到只剩 1 条。
    #[test]
    fn never_trims_below_two_legs() {
        let mut state = TrimState::new();
        let legs = vec![slow_ip(1), slow_ip(2), fast_ip(3)];
        // 候选给 2 条：若配额算错、把 2 条慢腿都摘了，`add` 也会是 2 条且剩 1 条腿，
        // 这条测试就能同时抓住「摘太多」和「补太多」。
        let candidates = vec!["http://5.6.7.8/new".to_string(), "http://5.6.7.9/new".to_string()];
        for beat in 1..=WARMUP_BEATS {
            assert!(
                state.plan(beat, &legs, &candidates).is_none(),
                "暖机第 {beat} 拍不动作"
            );
        }
        let plan = state
            .plan(WARMUP_BEATS + 1, &legs, &candidates)
            .expect("有快腿在时可以摘慢腿");
        assert_eq!(
            plan.del.len(),
            1,
            "2 条该摘但只剩 1 条配额（MIN_LEGS = 2），只能摘最慢的 1 条"
        );
        assert_eq!(
            plan.add.len(),
            1,
            "补的条数跟摘的条数走：摘 1 条只补 1 条"
        );
        assert_eq!(
            legs.len() - plan.del.len(),
            MIN_LEGS,
            "摘完必须仍剩 MIN_LEGS 条腿"
        );
    }

    /// 被摘的 URI 进冷却：接下来 12 拍即使有别的腿在摘，也不许把它补回；
    /// 第 13 拍解禁。
    ///
    /// 构造：暖机时让候选池里的 30 条慢腿都留下 streak（暖机挡动作但仍计数），
    /// 之后每一拍只把其中一对「新慢腿」放进腿表，于是每拍都有东西可摘，
    /// 可以在**有 `del` 的拍**上真正验证 `add` 是否跳过了冷却 URI，
    /// 而不是只验证「没有动作的拍里没有 add」。
    #[test]
    fn deleted_uri_enters_cooldown() {
        let mut state = TrimState::new();
        let mut warmup_legs = vec![fast_ip(100), fast_ip(101)];
        warmup_legs.extend((1..=30).map(slow_ip));
        for beat in 1..=WARMUP_BEATS {
            assert!(
                state.plan(beat, &warmup_legs, &[]).is_none(),
                "暖机第 {beat} 拍不动作"
            );
        }

        let first = "http://10.9.0.1/f".to_string();
        let trimmed = state
            .plan(
                WARMUP_BEATS + 1,
                &[fast_ip(100), fast_ip(101), slow_ip(1)],
                &[],
            )
            .expect("暖机结束后第一拍应摘掉唯一一条慢腿");
        assert_eq!(
            trimmed.del,
            vec![first.clone()],
            "先摘掉 10.9.0.1 以建立冷却"
        );

        let candidates = vec![first.clone()];
        let mut next = 2u8;
        for delta in 1..=COOLDOWN_BEATS {
            let legs = vec![fast_ip(100), fast_ip(101), slow_ip(next), slow_ip(next + 1)];
            next += 2;
            let plan = state
                .plan(WARMUP_BEATS + 1 + delta, &legs, &candidates)
                .expect("冷却期内新出现的慢腿仍要照摘，否则冷却无从验证");
            assert!(
                !plan.add.contains(&first),
                "冷却第 {delta} 拍（共 {COOLDOWN_BEATS} 拍）不得把刚摘的 URI 补回"
            );
        }

        let legs = vec![fast_ip(100), fast_ip(101), slow_ip(next), slow_ip(next + 1)];
        let plan = state
            .plan(WARMUP_BEATS + 1 + COOLDOWN_BEATS + 1, &legs, &candidates)
            .expect("冷却期满后新慢腿仍应摘");
        assert_eq!(
            plan.add,
            vec![first],
            "冷却期满后的第一拍应允许补回（候选中它排第一）"
        );
    }

    /// 摘 1 条、候选 5 条 ⇒ `add` 恰好 1 条，且按候选顺序取第一条。
    #[test]
    fn adds_only_as_many_as_deleted() {
        let mut state = TrimState::new();
        let good = vec![leg(1, 5_000_000), fast_ip(2), leg(3, 8_000_000)];
        let bad = vec![slow_ip(1), fast_ip(2), leg(3, 8_000_000)];
        let candidates: Vec<String> = (1..=5).map(|i| format!("http://5.6.7.{i}/new")).collect();
        for beat in 1..=WARMUP_BEATS {
            assert!(
                state.plan(beat, &good, &candidates).is_none(),
                "暖机第 {beat} 拍不动作"
            );
        }
        assert!(
            state.plan(4, &bad, &candidates).is_none(),
            "第 1 拍慢不动作"
        );
        let plan = state
            .plan(5, &bad, &candidates)
            .expect("连续第 2 拍慢应摘腿");
        assert_eq!(plan.del.len(), 1, "只摘 1 条");
        assert_eq!(
            plan.add,
            vec![candidates[0].clone()],
            "摘 1 条就只补 1 条，且按候选顺序取第一条（不是把 5 条全塞进去）"
        );
    }
}
