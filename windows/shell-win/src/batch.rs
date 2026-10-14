//! 「暂停全部 / 一键开始」与**换下载目录**的交接状态（规格 2026-10-12 §5–§6）。
//!
//! 为什么是一个进程级静态而不是 `Session` 的一格：这两样东西（`resume_pending`、搬运进度）
//! 只被 `transfers` 这一条读路径和换目录这一条写路径碰到，放进 `Session` 要同时改它的
//! `Inner`、`view()` 与所有构造点，波及面远大于它承载的东西。且它们**不是内核状态的缓存**：
//! 搬运进度的权威在内核（`migrate_status`），这里只是上一次轮询到的值。
//!
//! ## `resume_pending` 是什么
//!
//! 换下载目录会重启内核，引擎和队列一起没了，传输列表一行都没有——而客户的批次其实还在，
//! 只是**还没补回队列**。这时「一键开始」应当可用，但列表里没有任何一行能证明这一点，
//! 只有壳记着「刚换过目录」。它在 [`mark_handover`] 里置上，在一键开始成功后清掉。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use shell_core::client::CoreClient;
use shell_core::presentation::batch_control::MigrateFailure;
use shell_core::presentation::kernel_death::CallFailure;
use shell_core::protocol::{LoadState, MigrateStatus};

use crate::kernel;
use crate::session::{Session, spawn_connect};

#[derive(Default)]
struct State {
    resume_pending: bool,
    migrate: Option<MigrateStatus>,
    /// 搬运**没能完成**的原因（壳这一侧记下，内核没有 `MigrateStatus` 可以说）。
    migrate_failure: Option<MigrateFailure>,
}

static STATE: Mutex<State> = Mutex::new(State {
    resume_pending: false,
    migrate: None,
    migrate_failure: None,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn resume_pending() -> bool {
    state().resume_pending
}

pub fn set_resume_pending(on: bool) {
    state().resume_pending = on;
}

/// 上一次轮询到的搬运进度（没搬过 / 已被一键开始清掉 ⇒ `None`）。
pub fn migrate() -> Option<MigrateStatus> {
    state().migrate.clone()
}

pub fn set_migrate(status: Option<MigrateStatus>) {
    state().migrate = status;
}

pub fn migrate_failure() -> Option<MigrateFailure> {
    state().migrate_failure.clone()
}

/// 一键开始成功之后：待办清零，搬运的结果话也不用再留着了。
pub fn clear_after_resume() {
    let mut s = state();
    s.resume_pending = false;
    s.migrate = None;
    s.migrate_failure = None;
}

/// 等重放落地的上限。重放要重新拉清单（最坏 91.5 秒）并做一次全量 plan，所以给得宽。
const REPLAY_DEADLINE: Duration = Duration::from_secs(180);
const POLL: Duration = Duration::from_millis(500);

/// 换目录成功、新内核起来之后调用：等批次重放落地，让新内核把旧目录里的文件搬过来，
/// 并把进度灌进 [`migrate`]。**后台线程，立刻返回。**
///
/// `old_dir` 是换之前向旧内核问到的下载目录；问不到（`None`）⇒ 无从搬运，只置 `resume_pending`。
///
/// 搬运没能完成时**一律说出来**（[`migrate_failure`]，界面那一行照摆原因）：
/// 客户会据此决定要不要自己去旧目录处理，静默收场就是让他以为旧文件已经搬过来了。
/// 但它**不影响下载本身**：`resume_pending` 照样在，「一键开始」照样可用。
///   * 没有批次 ⇒ 没有东西可搬也没有东西可补，`resume_pending` 清掉（不算失败）；
///   * 重放失败 / 超时 ⇒ 不搬，说明原因；
///   * `migrate_start` 被拒（新旧目录相同等）⇒ 不搬，说明原因。
pub fn mark_handover(session: &Arc<Session>, old_dir: Option<String>) {
    set_resume_pending(true);
    set_migrate(None);
    state().migrate_failure = None;
    let session = Arc::clone(session);
    let _ = std::thread::Builder::new()
        .name("dir-handover".to_string())
        .spawn(move || run_handover(&session, old_dir.as_deref()));
}

fn give_up(failure: MigrateFailure) {
    set_migrate(None);
    state().migrate_failure = Some(failure);
}

fn refuse(reason: &str) {
    give_up(MigrateFailure::Refused(reason.to_string()));
}

/// 连续这么多拍取不到搬运进度才放弃（0.5 s 一拍 ≈ 20 s，与重启等待的量级一致）。
const GIVE_UP_AFTER_MISSES: u32 = 40;

fn run_handover(session: &Arc<Session>, old_dir: Option<&str>) {
    if session.load_request().is_none() {
        set_resume_pending(false);
        return;
    }
    let started = Instant::now();
    loop {
        match session.view().load {
            LoadState::Loaded(_) => break,
            LoadState::Failed(_) => {
                refuse("这一批没能重新加载，所以没有开始搬运");
                return;
            }
            LoadState::Idle | LoadState::Loading => {}
        }
        if started.elapsed() >= REPLAY_DEADLINE {
            refuse("这一批没能在期限内重新加载，所以没有开始搬运");
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let Some(old_dir) = old_dir else {
        refuse("没能问到旧的下载目录，所以没有开始搬运");
        return;
    };
    let Some(client) = session.client() else {
        refuse("内核没有连上，所以没有开始搬运");
        return;
    };
    // 让界面立刻看到「搬运开始了」，不必等第一次轮询。
    set_migrate(Some(MigrateStatus {
        running: true,
        ..Default::default()
    }));
    if let Err(why) = kernel::migrate_start(&client, old_dir) {
        refuse(why.text());
        return;
    }
    follow_migrate(session, &client, POLL, GIVE_UP_AFTER_MISSES);
}

/// 跟着内核的搬运进度走，直到它完成或我们放弃。
///
/// 瞬时错误（RPC 慢、一次抖动）**不是**引擎死了：保留上一次的进度、原文照登到 `last_error`，继续跟。
/// 只有内核管道结束（`KernelGone`）才走「内核没了」那条路；连续取不到进度到了上限、或者内核说它既不在跑也没完成，
/// 都判定这次搬运已经丢了。
fn follow_migrate(session: &Arc<Session>, client: &CoreClient, poll: Duration, give_up_after: u32) {
    let mut misses = 0;
    loop {
        match kernel::migrate_status(client) {
            Ok(st) => {
                misses = 0;
                if !st.running && !st.finished {
                    give_up(MigrateFailure::ProgressLost);
                    return;
                }
                let done = !st.running && st.finished;
                set_migrate(Some(st));
                if done {
                    return;
                }
            }
            Err(why) => {
                if let CallFailure::KernelGone { reason, .. } = &why {
                    if session.note_kernel_death(reason.clone()) && session.claim_auto_restart() {
                        // 第一次死亡、且这一轮还没自动重启过 ⇒ 给它一次。
                        spawn_connect(session);
                    }
                    give_up(MigrateFailure::ProgressLost);
                    return;
                }
                session.set_last_error(Some(why.text().to_string()));
                misses += 1;
                if misses >= give_up_after {
                    give_up(MigrateFailure::ProgressLost);
                    return;
                }
            }
        }
        std::thread::sleep(poll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shell_core::client::{ClientError, LineChannel};
    use shell_core::protocol::EngineState;
    use std::collections::VecDeque;

    /// 全局静态，测试之间会互相踩 ⇒ 所有用例串在同一把锁下。
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn clearing_after_a_resume_resets_both_cells() {
        let _g = serial();
        set_resume_pending(true);
        set_migrate(Some(MigrateStatus::default()));
        clear_after_resume();
        assert!(!resume_pending());
        assert!(migrate().is_none());
    }

    /// 没有批次 ⇒ 没东西可搬也没东西可补：`resume_pending` 必须被清掉，
    /// 否则「一键开始」会对着一个空列表永远亮着。
    /// 判别力：删掉 `run_handover` 开头那段 ⇒ 这里 `resume_pending` 仍为真 ⇒ 红。
    #[test]
    fn a_handover_without_a_batch_clears_the_pending_flag() {
        let _g = serial();
        set_resume_pending(true);
        let session = Arc::new(Session::new());
        run_handover(&session, Some("/old"));
        assert!(!resume_pending());
    }

    /// 批次重新加载失败 ⇒ 搬运**不开始**，但这件事必须说出来（原先是静默收场）。
    /// 判别力：把 `give_up` 换回"什么都不写" ⇒ 这一条红。
    #[test]
    fn a_failed_reload_says_the_migration_did_not_happen() {
        let _g = serial();
        state().migrate_failure = None;
        let session = Arc::new(Session::new());
        session.remember_load_request("C24-8".to_string(), "https://user.example".to_string());
        session.set_load(LoadState::Failed("批次加载失败".to_string()));
        run_handover(&session, Some("/old"));
        match migrate_failure().expect("搬运没开始必须有话说") {
            MigrateFailure::Refused(reason) => assert!(reason.contains("没能重新加载"), "{reason}"),
            other => panic!("应当是「没开始」而不是 {other:?}"),
        }
        assert!(migrate().is_none());
    }

    /// 脚本化的一步回话：`Ok` 是 `result` 的 JSON，`Err` 是内核的错误码，`Dead` 是管道结束。
    #[derive(Clone)]
    enum Step {
        Ok(&'static str),
        Err(&'static str),
        Dead,
    }

    /// 一条按脚本回话的假通道：每个请求取一步；脚本用完后一直重复 `tail`。
    struct Script {
        queue: Mutex<VecDeque<Step>>,
        tail: Step,
        sent: Mutex<u64>,
        current: Mutex<Option<Step>>,
    }

    impl LineChannel for Script {
        fn write_line(&self, _line: &str) -> Result<(), ClientError> {
            *self.sent.lock().unwrap() += 1;
            let step = self.queue.lock().unwrap().pop_front().unwrap_or_else(|| self.tail.clone());
            *self.current.lock().unwrap() = Some(step);
            Ok(())
        }

        fn read_line(&self) -> Result<Option<String>, ClientError> {
            let id = *self.sent.lock().unwrap();
            match self.current.lock().unwrap().clone().expect("先写后读") {
                Step::Ok(result) => Ok(Some(format!(r#"{{"id":{id},"ok":true,"result":{result}}}"#))),
                Step::Err(code) => Ok(Some(format!(
                    r#"{{"id":{id},"ok":false,"error":{{"code":"{code}","message":"瞬时的一次失败"}}}}"#
                ))),
                Step::Dead => Ok(None),
            }
        }

        fn close(&self) {}
    }

    fn a_client(steps: Vec<Step>, tail: Step) -> CoreClient {
        CoreClient::new(Box::new(Script {
            queue: Mutex::new(steps.into()),
            tail,
            sent: Mutex::new(0),
            current: Mutex::new(None),
        }))
    }

    const RUNNING: &str = r#"{"running":true,"finished":false,"files_done":1,"files_total":2,"bytes_done":1,"bytes_total":2}"#;
    const DONE: &str = r#"{"running":false,"finished":true,"files_done":2,"files_total":2,"bytes_done":2,"bytes_total":2}"#;
    const GONE: &str = r#"{"running":false,"finished":false,"files_done":0,"files_total":0,"bytes_done":0,"bytes_total":0}"#;

    fn reset() {
        state().migrate_failure = None;
        set_migrate(None);
    }

    /// 判别力：把瞬时错误分支改成 `give_up` ⇒ 第一条红（那就是「一次抖动就弹失败」）。
    #[test]
    fn a_transient_poll_error_keeps_the_progress_and_carries_on() {
        let _g = serial();
        reset();
        let session = Arc::new(Session::new());
        let client = a_client(vec![Step::Ok(RUNNING), Step::Err("invalid_params"), Step::Ok(DONE)], Step::Ok(DONE));

        follow_migrate(&session, &client, Duration::ZERO, 3);

        assert!(migrate().is_some_and(|m| m.finished), "抖动之后要接着跟到完成");
        assert!(migrate_failure().is_none(), "一次抖动不是失败");
        assert!(
            session.view().last_error.unwrap_or_default().contains("瞬时的一次失败"),
            "原文照登到错误格"
        );
    }

    /// 判别力：把 `give_up_after` 的比较改成 `>` 或放弃时不说话 ⇒ 红。
    #[test]
    fn a_run_that_keeps_missing_is_given_up_and_the_loss_is_said() {
        let _g = serial();
        reset();
        let session = Arc::new(Session::new());
        let client = a_client(vec![], Step::Err("invalid_params"));

        follow_migrate(&session, &client, Duration::ZERO, 3);

        assert_eq!(migrate_failure(), Some(MigrateFailure::ProgressLost));
        assert!(migrate().is_none());
    }

    /// 判别力：不在放弃判据里把 `misses` 清零 ⇒ 第一条红（间断的抖动会累加成一次放弃）。
    #[test]
    fn a_success_in_between_resets_the_miss_count() {
        let _g = serial();
        reset();
        let session = Arc::new(Session::new());
        let client = a_client(
            vec![
                Step::Err("invalid_params"),
                Step::Err("invalid_params"),
                Step::Ok(RUNNING),
                Step::Err("invalid_params"),
                Step::Err("invalid_params"),
                Step::Ok(DONE),
            ],
            Step::Ok(DONE),
        );

        follow_migrate(&session, &client, Duration::ZERO, 3);

        assert!(migrate_failure().is_none(), "两次连续抖动之间有成功，不该放弃");
        assert!(migrate().is_some_and(|m| m.finished));
    }

    /// 内核说它既不在跑也没完成 ⇒ 搬运已随内核重启丢了，必须说出来。
    #[test]
    fn a_run_the_kernel_no_longer_has_is_reported_as_lost() {
        let _g = serial();
        reset();
        let session = Arc::new(Session::new());
        let client = a_client(vec![Step::Ok(GONE)], Step::Ok(GONE));

        follow_migrate(&session, &client, Duration::ZERO, 3);

        assert_eq!(migrate_failure(), Some(MigrateFailure::ProgressLost));
    }

    /// 管道结束 ⇒ 内核没了：引擎那一格要翻（横幅），搬运也说出来。
    #[test]
    fn a_dead_kernel_flips_the_engine_and_the_loss_is_said() {
        let _g = serial();
        reset();
        let session = Arc::new(Session::new());
        let client = a_client(vec![Step::Dead], Step::Dead);

        follow_migrate(&session, &client, Duration::ZERO, 3);

        assert!(matches!(session.view().engine, EngineState::Unavailable(_)), "{:?}", session.view().engine);
        assert_eq!(migrate_failure(), Some(MigrateFailure::ProgressLost));
    }
}
