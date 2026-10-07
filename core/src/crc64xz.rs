//! CRC-64/XZ。
//!
//! 为什么不用 `CRC_64_ECMA_182`：TOS 的 `X-Tos-Hash-Crc64ecma` 用的是 CRC-64/XZ
//! （poly=0x42F0E1EBA9EA3693 的反射形式 0xC96C5795D7870F42、init=~0、xorout=~0、反射），
//! 而 `CRC_64_ECMA_182` 虽然 poly 相同，却是 init=0、xorout=0、非反射，两者不是一回事。
//! 2026-09-14 在真实对象上实测确认：算错会让每个文件的校验都失败。
//!
//! 注意 init 与 xorout 都是全 1，因此空输入的输出是 0。

use std::io;
use std::path::Path;

use crc::{Crc, CRC_64_XZ};

/// 表：CRC-64/XZ（poly=0x42F0E1EBA9EA3693 的反射形式、init=~0、xorout=~0、反射）。
///
/// 用 `static` 而不是 `const`：`Crc::digest()` 借用这张表，`static` 天然满足 `'static`，
/// 而 `const` 被引用时是否被提升为 `'static` 取决于常量提升规则——写 `static` 少一个不确定性。
pub static CRC64XZ: Crc<u64> = Crc::<u64>::new(&CRC_64_XZ);

/// 一次性计算。
///
/// ⚠️ **只有测试用它**——生产路径一条都不走它：`verify.rs` 一律走 `sum_file`（流式，
/// 交付文件有 50GB+，见那个函数的注释）。它的读者是 `streaming_matches_one_shot` 与
/// `sum_file_multi_chunk`（拿它当"一次性"的对照实现）。
///
/// ⚠️ **因此生产构建下这里会报一条既有的 `dead_code` 告警，而且它必须留着**：
/// 按 D-4「本任务**不新增**」执行——`#[allow(dead_code)]`、`#[cfg(test)]` 门控、
/// 删符号**三条路都不许**（删它就要把那两条测试改写成直接调 `CRC64XZ.checksum(...)`，
/// 那是把"对照"淹进实现细节，不是清理）。
///
/// 基线账本见 `docs/superpowers/plans/2026-09-18-client-phase-d.md` **全局约束表的 D-4 行**
/// （当前在 `:24`）：它点名了这四条告警（`crc64xz::sum` / `ARIA2_LICENSE` / `license_text` /
/// `Daemon{secret,argv}`），并写明按"本任务**不新增**执行、
/// **不要**用 `#[allow(dead_code)]` 或删符号去按掉它们"。
/// ⚠️ 原指针写的是 `windows/scripts/test.sh` 头部——**那个文件在本仓库里不存在**
/// （`windows/` 不在本工作树），照它去查会扑空；已订正为上面这处。
pub fn sum(data: &[u8]) -> u64 {
    CRC64XZ.checksum(data)
}

/// 流式计算器。交付里最大的文件有 50GB+，必须流式。
///
/// `crc::Digest` 的累加语义与 `sum` 一致（由 `streaming_matches_one_shot` 守住）。
pub fn digest() -> crc::Digest<'static, u64> {
    CRC64XZ.digest()
}

/// 流式计算文件的 CRC-64/XZ。
///
/// 不要"为了简单"先 `fs::read` 再 `sum`：最大的交付文件有 50GB+。
pub fn sum_file(path: &Path) -> io::Result<u64> {
    use std::io::Read;

    let mut f = std::fs::File::open(path)?;
    let mut d = digest();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        d.update(&buf[..n]);
    }
    Ok(d.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crc::CRC_64_ECMA_182;

    // 临时目录夹具已收敛到 `crate::testutil`（任务 4 步骤 5b），
    // 这里只保留名字，行为与原先那份私有副本相同。
    use crate::testutil::TempDir;

    #[test]
    fn standard_vector() {
        // CRC-64/XZ 的标准校验值，来源见规格 §3
        let got = sum(b"123456789");
        const WANT: u64 = 0x995DC9BBDF1939FA;
        assert_eq!(got, WANT, "CRC-64/XZ(\"123456789\") = {got:#x}，期望 {WANT:#x}");
    }

    #[test]
    fn known_tos_values() {
        // 这个值来自 2026-09-14 在真实 TOS 对象上实测的 X-Tos-Hash-Crc64ecma
        let cases: [(&[u8], u64); 1] = [(b"readme", 5432380796884633278)];
        for (data, want) in cases {
            assert_eq!(
                sum(data),
                want,
                "sum({:?}) 期望 {want}",
                String::from_utf8_lossy(data)
            );
        }
    }

    #[test]
    fn empty() {
        // 空输入：init 与 xorout 都是全 1，两者相消
        assert_eq!(sum(&[]), 0, "空输入应当得到 0");
    }

    #[test]
    fn not_equal_ecma_182() {
        // 守住"不能用 ECMA 变体"这条：两者必须不同，否则说明实现退化成了 ECMA。
        //
        // Go 侧的对应测试（TestNotEqualGoECMA）比的是 crc64.ECMA 常量，对照值即
        // 0x6C40DF5F0B497347。Rust 侧没有那个常量，同 poly 的对照物是 CRC_64_ECMA_182，
        // 守的是同一个陷阱。
        const GO_ECMA_CHECKSUM: u64 = 0x6C40DF5F0B497347;
        let ecma_182 = Crc::<u64>::new(&CRC_64_ECMA_182);
        // 先确认对照物确实是 Go 源码里的那个常量——否则 assert_ne 可能拿自己比自己，
        // 永远不红。
        assert_eq!(
            ecma_182.checksum(b"123456789"),
            GO_ECMA_CHECKSUM,
            "CRC_64_ECMA_182 与 Go 源码里的 crc64.ECMA 对不上，本条测试的前提已失效"
        );
        assert_ne!(
            sum(b"123456789"),
            GO_ECMA_CHECKSUM,
            "实现退化成了非反射 ECMA-182，这不是 TOS 用的变体"
        );
    }

    /// 以规格 §3 的定义（poly=0xC96C5795D7870F42、init=~0、xorout=~0、反射）
    /// 逐位实现一个参照，逐长度对拍。
    ///
    /// 加它的理由：`not_equal_ecma_182` 判别力不足——把 init/xorout 取成 0 的实现
    /// （即把 init/xorout 的双重取反折算反了的那个错法）同样能让它通过，
    /// 却会让标准向量与真实 TOS 值双双对不上。这里用独立参照把这条路堵死。
    #[test]
    fn matches_bitwise_definition() {
        const REFL_POLY: u64 = 0xC96C5795D7870F42;
        fn reference(data: &[u8]) -> u64 {
            let mut crc = !0u64; // init
            for &b in data {
                crc ^= u64::from(b);
                for _ in 0..8 {
                    if crc & 1 != 0 {
                        crc = (crc >> 1) ^ REFL_POLY;
                    } else {
                        crc >>= 1;
                    }
                }
            }
            crc ^ !0u64 // xorout
        }

        // Go 侧的 nil 与 "" 是同一个值，Rust 里合并为一项。
        let fixed: [&[u8]; 5] = [
            b"",
            b"a",
            b"123456789",
            b"readme",
            "C24-8_×_25WS024".as_bytes(), // 真实数据里就有这种目录名
        ];
        for input in fixed {
            assert_eq!(
                sum(input),
                reference(input),
                "sum({:?}) 与逐位参照不一致",
                String::from_utf8_lossy(input)
            );
        }

        // 覆盖 0..299 字节：含空输入、非 8 字节对齐的尾巴。
        // 固定种子，可复现。
        let mut buf: Vec<u8> = Vec::new();
        let mut seed: u64 = 0x9E3779B97F4A7C15;
        for n in 0..300usize {
            buf.clear();
            for _ in 0..n {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                buf.push((seed >> 33) as u8);
            }
            assert_eq!(
                sum(&buf),
                reference(&buf),
                "长度 {n} 的伪随机输入：与逐位参照不一致"
            );
        }
    }

    #[test]
    fn streaming_matches_one_shot() {
        let data = b"streaming-vs-oneshot-1234567890";
        let mut d = digest();
        // 分几次写入，结果必须与一次性 sum 相同
        d.update(&data[..7]);
        d.update(&data[7..]);
        assert_eq!(d.finalize(), sum(data), "流式与一次性结果不一致");
    }

    #[test]
    fn streaming_empty() {
        assert_eq!(digest().finalize(), 0, "空输入的流式结果应当是 0");
    }

    #[test]
    fn sum_file() {
        let dir = TempDir::new();
        let p = dir.join("a.bin");
        std::fs::write(&p, b"readme").expect("写测试文件失败");
        // 用 `super::` 限定：本测试名与被测函数同名，裸名会解析到测试自己。
        let got = super::sum_file(&p).expect("sum_file 不应失败");
        // 与真实 TOS 实测值一致（规格 §7.1）
        assert_eq!(got, 5432380796884633278, "sum_file 与实测 TOS 值不一致");
    }

    #[test]
    fn sum_file_missing() {
        let dir = TempDir::new();
        assert!(
            super::sum_file(&dir.join("nope.bin")).is_err(),
            "文件不存在应当报错"
        );
    }

    /// 用一个远大于读缓冲的文件，逼 `sum_file` 真的走"多次 Read → 逐段累加"这条路。
    ///
    /// 加它的理由：`sum_file` 用的 "readme" 只有 6 字节，单次 Read 就读完了，
    /// 因此"每段都重新 new 一个计算器 / 覆盖累加器"这类流式实现错误它抓不到——
    /// 而那正是 50GB+ 文件上会算错、小文件上却看着没事的错法。
    ///
    /// 数据量是 3MiB + 4KiB，不是 Go 侧的 1MiB：`sum_file` 的读缓冲是 1MiB，
    /// 1MiB 的文件只会触发**一次** Read，那条测试就退化成了"跑通即过"，
    /// 抓不到它本来要抓的错法（变异检查里验证过）。所以这里取缓冲的 3 倍多，
    /// 末尾那 4KiB 顺带覆盖"最后一段不满缓冲"的尾巴。
    #[test]
    fn sum_file_multi_chunk() {
        const LEN: usize = (3 << 20) + 4096;
        let mut data: Vec<u8> = Vec::with_capacity(LEN);
        let mut seed: u64 = 0x9E3779B97F4A7C15;
        for _ in 0..LEN {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            data.push((seed >> 33) as u8);
        }
        let dir = TempDir::new();
        let p = dir.join("big.bin");
        std::fs::write(&p, &data).expect("写测试文件失败");
        let got = super::sum_file(&p).expect("sum_file 不应失败");
        assert_eq!(got, sum(&data), "多段文件：sum_file 与一次性 sum 不一致");
    }
}
