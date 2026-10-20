//! 引擎自动重启的第 2/3 层（规格 W3）。
//!
//! 三层分工，别混：
//! * **第 1 层（0.2.9，已在线）**：探活不通时先取证——进程还在写盘/烧 CPU 就判"慢"，
//!   不暂停、不换引擎（`procstat::is_busy`）；
//! * **第 2 层（这里）**：取证判定"真没动"、且连续 `AFTER_PROBES` 次探活失败 ⇒
//!   带退避（5 / 15 / 45 s）自动重启，**预算 3 次**；
//! * **第 3 层（这里）**：预算用尽才报 `engine_unresponsive`——那是"说断开"的唯一时刻。
//!
//! 成功判据**两条都要**（规格 W3.1）：新引擎 `ping()` 通 **且** 60 s 内有进度。
//! 只满足第一条不算——现场见过"起来了但立刻又卡死"。
//!
//! 这一整个模块是**纯逻辑**（时钟由调用方给），所以退避与预算能用假时钟断言，
//! 不必真等 65 秒。

/// 连续几次探活失败才考虑重启（约 30 s：自愈线程一秒一拍）。
pub const AFTER_PROBES: u32 = 3;

/// 最多自动重启几次；用尽才报 `engine_unresponsive`。
pub const BUDGET: u32 = 3;

/// 重启之后给多久证明自己（毫秒）。
pub const VERIFY_MS: u64 = 60_000;

/// 第 `attempt` 次重启前要等多久（`attempt` 从 1 开始）。
pub fn backoff_ms(attempt: u32) -> u64 {
    match attempt {
        1 => 5_000,
        2 => 15_000,
        _ => 45_000,
    }
}

/// 一拍探活之后该做什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 什么都不用做（还没到条件、或正在等退避）。
    None,
    /// 现在就重启，这是第几次（1..=BUDGET）。
    Restart { attempt: u32 },
    /// 重启成功（探活通了**且**有进度）：预算清零，界面可以说"已自动恢复"。
    Recovered { attempt: u32 },
    /// 预算用尽：**此刻**才允许报「下载引擎没有响应」。
    Exhausted,
}

/// 给界面看的进度（`restart_progress` 命令）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// 已经用掉的次数。
    pub attempt: u32,
    /// 总预算。
    pub budget: u32,
    /// `idle` / `waiting`（等退避）/ `verifying`（等进度证明）/ `exhausted`。
    pub state: &'static str,
    /// 当前阶段的起点（等待开始 / 重启完成 / 用尽），界面用来算"还剩几秒"。
    pub since_ms: u64,
    /// 连续探活失败次数（诊断用）。
    pub fails: u32,
}

/// 重启账本。
#[derive(Debug, Default)]
pub struct Gate {
    fails: u32,
    /// 下一次重启动手的时间（等退避）。
    next_at: Option<u64>,
    /// 已经重启过的次数。
    attempt: u32,
    /// 上一次重启完成的时间（等进度证明）。
    verify_at: Option<u64>,
    /// 预算是否已经用尽（用尽后不再自动重启，直到被 `reset`）。
    exhausted: bool,
    since: u64,
}

impl Gate {
    pub fn new() -> Self {
        Gate::default()
    }

    /// 手动重启成功 / 换了交付码 / 应用重启：预算与状态一起清零。
    pub fn reset(&mut self) {
        *self = Gate::default();
    }

    /// 每次重启动作做完（`replace_engine` 返回）之后调用。
    pub fn restart_done(&mut self, now_ms: u64) {
        self.verify_at = Some(now_ms);
        self.since = now_ms;
    }

    /// 一拍。
    ///
    /// * `ping_ok`：探活通了；
    /// * `busy`：取证闸门说"进程在动"（第 1 层）——**忙就永不重启**；
    /// * `moving`：有进度（`completedLength` 涨或 `numActive > 0`）。
    pub fn on_probe(&mut self, now_ms: u64, ping_ok: bool, busy: bool, moving: bool) -> Action {
        // ① 正在等"重启是否真的有效"
        if let Some(at) = self.verify_at {
            if ping_ok && moving {
                self.verify_at = None;
                let attempt = self.attempt;
                self.reset();
                return Action::Recovered { attempt };
            }
            if now_ms.saturating_sub(at) > VERIFY_MS {
                // 起来了但一直没进度（或压根没通）：这一次算失败，继续
                self.verify_at = None;
                self.exhausted = self.attempt >= BUDGET;
                if self.exhausted {
                    self.since = now_ms;
                    return Action::Exhausted;
                }
                self.next_at = Some(now_ms + backoff_ms(self.attempt + 1));
                self.since = now_ms;
            }
            return Action::None;
        }

        // ② 平时
        if ping_ok {
            self.fails = 0;
            self.next_at = None;
            return Action::None;
        }
        self.fails = self.fails.saturating_add(1);
        if busy {
            // 第 1 层：在写盘就不动它（也把等待取消，免得"慢"被当成"死"）
            self.next_at = None;
            return Action::None;
        }
        if self.fails < AFTER_PROBES {
            return Action::None;
        }
        if self.exhausted {
            return Action::Exhausted;
        }
        if self.attempt >= BUDGET {
            self.exhausted = true;
            self.since = now_ms;
            return Action::Exhausted;
        }
        match self.next_at {
            None => {
                self.next_at = Some(now_ms + backoff_ms(self.attempt + 1));
                self.since = now_ms;
                Action::None
            }
            Some(at) if now_ms >= at => {
                self.next_at = None;
                self.attempt += 1;
                self.since = now_ms;
                Action::Restart {
                    attempt: self.attempt,
                }
            }
            Some(_) => Action::None,
        }
    }

    /// 界面要的那几格。
    pub fn progress(&self) -> Progress {
        let state = if self.exhausted {
            "exhausted"
        } else if self.verify_at.is_some() {
            "verifying"
        } else if self.next_at.is_some() {
            "waiting"
        } else {
            "idle"
        };
        Progress {
            attempt: self.attempt,
            budget: BUDGET,
            state,
            since_ms: self.since,
            fails: self.fails,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把时钟一秒一拍往前推，直到真的动手重启；返回第几次。
    /// （验证窗过期、等退避、再动手——这些中间态都会在这条循环里自然走过。）
    fn tick_until_restart(g: &mut Gate, now: &mut u64) -> u32 {
        for _ in 0..100_000 {
            *now += 1_000;
            if let Action::Restart { attempt } = g.on_probe(*now, false, false, false) {
                return attempt;
            }
        }
        panic!("等不到重启：状态 {:?}", g.progress());
    }

    /// 判据（规格 W3.1）：**进程不动**时，连续 3 次探活失败 + 等满 5 秒才动手。
    #[test]
    fn a_dead_engine_is_restarted_after_three_probes_and_a_backoff() {
        let mut g = Gate::new();
        // 前两拍：记录失败，不动手
        assert_eq!(g.on_probe(0, false, false, false), Action::None);
        assert_eq!(g.on_probe(1_000, false, false, false), Action::None);
        // 第三拍：够了，先等 5 秒
        assert_eq!(g.on_probe(2_000, false, false, false), Action::None);
        assert_eq!(g.progress().state, "waiting");
        assert_eq!(g.on_probe(6_999, false, false, false), Action::None, "还没到点");
        assert_eq!(
            g.on_probe(7_000, false, false, false),
            Action::Restart { attempt: 1 },
            "第三次失败后等满 5 秒才重启"
        );
        g.restart_done(7_100);
        assert_eq!(g.progress().state, "verifying");
    }

    /// 判据：三次预算依次用完——第 1 次 5 秒、第 2 次 15 秒、第 3 次 45 秒，
    /// **只有到第三次之后**才 `Exhausted`（第 6 类错误就等于"三次重启都无效"）。
    #[test]
    fn the_budget_runs_out_after_exactly_three_restarts() {
        let mut g = Gate::new();
        let mut now = 0u64;

        assert_eq!(tick_until_restart(&mut g, &mut now), 1);
        g.restart_done(now);

        assert_eq!(tick_until_restart(&mut g, &mut now), 2);
        g.restart_done(now);

        assert_eq!(tick_until_restart(&mut g, &mut now), 3);
        g.restart_done(now);

        // 第 3 次也没证明自己 ⇒ 用尽
        let mut saw = None;
        for _ in 0..100_000 {
            now += 1_000;
            match g.on_probe(now, false, false, false) {
                Action::Exhausted => {
                    saw = Some(now);
                    break;
                }
                Action::Restart { attempt } => panic!("预算用尽后不许再重启：第 {attempt} 次"),
                _ => {}
            }
        }
        assert!(saw.is_some(), "第 3 次无效之后必须报 Exhausted");
        assert_eq!(g.progress().state, "exhausted");
        assert_eq!(g.progress().attempt, BUDGET);
    }

    /// 判据：**每次退避的时长就是规格里那三个数**（5 / 15 / 45 秒）。
    /// 用假时钟精确断言，不真等 65 秒。
    #[test]
    fn the_three_backoffs_are_five_fifteen_and_fortyfive_seconds() {
        assert_eq!(backoff_ms(1), 5_000);
        assert_eq!(backoff_ms(2), 15_000);
        assert_eq!(backoff_ms(3), 45_000);

        let mut g = Gate::new();
        // 第一次：第三拍失败发生在 t=2 000，动手在 t=7 000
        let mut now = 0u64;
        for _ in 0..AFTER_PROBES {
            now += 1_000;
            g.on_probe(now, false, false, false);
        }
        assert_eq!(now, 3_000);
        assert_eq!(g.progress().state, "waiting");
        let first = now + backoff_ms(1) - 1;
        assert_eq!(g.on_probe(first, false, false, false), Action::None);
        assert_eq!(
            g.on_probe(first + 1, false, false, false),
            Action::Restart { attempt: 1 }
        );

        // 第二次：验证窗过期那一拍才排下一次；动手时刻 = 那一拍 + 15 秒
        g.restart_done(now);
        let expire = now + VERIFY_MS + 1;
        assert_eq!(g.on_probe(expire, false, false, false), Action::None);
        assert_eq!(
            g.on_probe(expire + backoff_ms(2) - 1, false, false, false),
            Action::None
        );
        assert_eq!(
            g.on_probe(expire + backoff_ms(2), false, false, false),
            Action::Restart { attempt: 2 }
        );

        // 第三次：45 秒
        g.restart_done(expire + backoff_ms(2));
        let expire2 = expire + backoff_ms(2) + VERIFY_MS + 1;
        assert_eq!(g.on_probe(expire2, false, false, false), Action::None);
        assert_eq!(
            g.on_probe(expire2 + backoff_ms(3) - 1, false, false, false),
            Action::None
        );
        assert_eq!(
            g.on_probe(expire2 + backoff_ms(3), false, false, false),
            Action::Restart { attempt: 3 }
        );
    }

    /// 判据（**0.2.9 的回归压在这里**）：取证闸门说"进程在动" ⇒ 一次都不重启，
    /// 哪怕探活连续失败一百次。
    #[test]
    fn a_busy_engine_is_never_restarted() {
        let mut g = Gate::new();
        for i in 0..100u64 {
            assert_eq!(g.on_probe(i * 1_000, false, true, false), Action::None);
        }
        assert_eq!(g.progress().attempt, 0);
        assert_eq!(g.progress().state, "idle");
    }

    /// 判据（规格 W3.1 成功要**两条**）：探活通了但没进度不算成功；
    /// 通了**且**有进度才算，且算成功后预算清零。
    #[test]
    fn coming_back_without_progress_is_not_success() {
        let mut g = Gate::new();
        let mut now = 0u64;
        assert_eq!(tick_until_restart(&mut g, &mut now), 1);
        g.restart_done(now);
        // 通了但没进度：不算成功，也不立刻算失败（要等满 60 秒）
        assert_eq!(g.on_probe(now + 1_000, true, false, false), Action::None);
        assert_eq!(g.on_probe(now + 30_000, true, false, false), Action::None);
        assert_eq!(g.progress().state, "verifying");
        // 之后有进度 ⇒ 成功，预算清零
        assert_eq!(
            g.on_probe(now + 31_000, true, false, true),
            Action::Recovered { attempt: 1 }
        );
        assert_eq!(g.progress().attempt, 0);
        assert_eq!(g.progress().state, "idle");
        // 清零之后又能从头开始
        for _ in 0..AFTER_PROBES {
            now += 1_000;
            g.on_probe(now, false, false, false);
        }
        assert_eq!(g.progress().state, "waiting");
    }

    /// 判据：退避等待期间**一旦探活通了就取消重启**（别把正在恢复的引擎杀掉）。
    #[test]
    fn a_probe_that_answers_cancels_the_pending_restart() {
        let mut g = Gate::new();
        for i in 0..AFTER_PROBES {
            g.on_probe(i as u64, false, false, false);
        }
        assert_eq!(g.progress().state, "waiting");
        assert_eq!(g.on_probe(1_000, true, false, false), Action::None);
        assert_eq!(g.progress().state, "idle");
        assert_eq!(g.progress().attempt, 0, "没动手就不该记一次");
    }

    /// 判据（规格 W3.3 重置）：手动重启成功 / 换交付码 / 应用重启后，
    /// 用尽的预算必须回到满额——否则客户修好一次之后再也不会有自动恢复。
    #[test]
    fn reset_gives_the_budget_back() {
        let mut g = Gate::new();
        g.restart_done(0);
        g.on_probe(VERIFY_MS + 1, false, false, false);
        g.reset();
        let p = g.progress();
        assert_eq!((p.attempt, p.budget, p.state), (0, BUDGET, "idle"));
    }
}
