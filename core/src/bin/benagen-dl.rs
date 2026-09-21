//! `benagen-dl` 入口 —— **只剩两行**。
//!
//! 编排（`run()` + 三个帮手）在库的 `cli/mod.rs` 里，与图形客户端的后端
//! （`benagen-core`）共用同一份内核编排（`kernel::dispatch`）。
//! 本文件刻意不做任何事：判据长在库里，`cargo test` 一视同仁地跑它；
//! 入口里写的东西只有端到端测试碰得到。
//!
//! ⚠️ `std::env::args()` 收的是**进程的原始 argv**（含程序名），这里 `skip(1)` 之后
//! 交给 `cli::run`；`run` 自己不做任何 `std::env` 的读取之外的副作用。
//!
//! ⚠️ **退出码必须经 `ExitCode::code()`**（它是对外契约，spec §3），
//! 不要在这里另写一个 `exit(2)` 之类的兜底——那会让契约出现第二份真相。

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(benagen_core::cli::run(argv).code());
}
