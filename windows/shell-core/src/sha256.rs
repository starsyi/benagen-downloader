//! SHA-256（FIPS 180-4）—— **两份内嵌二进制（内核 exe / 未来可能的其它资产）的身份判据**。
//!
//! ## 为什么是手写的（理由与内核那一条**逐字同源**，见 `core/src/engine/daemon.rs`
//! 的 `sha256`）
//!
//! 用到它的那条语义是「**绝不使用来路不明的文件**」：释放内嵌的内核时，名字由内容哈希
//! 派生、哈希相符才复用。这要求一个**密码学**哈希 —— 碰撞构造成本极低的校验和
//! （CRC 之类）在这里等于把"被篡改的文件"判成"相符"。
//!
//! 而本 workspace 的依赖纪律是"不新增依赖"（`core/Cargo.toml` 为同一条理由拒绝过
//! `sha2`；`windows/scripts/test.sh` 第 0 步还有一道**直接依赖白名单**守卫）。
//! ⇒ 照内核的成例：**手写一份，并用独立来源的标准向量把它钉住**
//! （[`tests::sha256_known_answer_vectors`]，向量取自 FIPS 180-4 的公开示例）。
//!
//! ## ⚠️ 这份实现同时被**构建脚本**用
//!
//! `shell-win/build.rs` 需要在内核 exe 被拷进 `OUT_DIR` 的那一刻算出它的 sha256（好把
//! 哈希当成**编译期常量**交给运行时，并使构建脚本能自验"包里那份内核确实是这一份"）。
//! 构建脚本**不能**依赖本 crate 的代码（build script 是独立编译的、没有依赖图），
//! 所以它是用 `#[path = "../shell-core/src/sha256.rs"] mod sha256;` **把本文件原文**
//! 编进去的 —— 那是"同一件事只有一份实现"的做法；复制第二份会在改算法/改宽度时静默分叉，
//! 而分叉的表现是**两个哈希对不上、释放路径每次都重写**（不报错，只是白做功）。
//!
//! ⇒ **本文件必须保持自足**：只许用 `std`，不许 `use crate::…`（build script 那一侧
//!    没有这个 crate 的模块树）。

/// SHA-256 的摘要（32 字节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Digest(pub [u8; 32]);

impl Digest {
    /// 小写十六进制（与 `sha256sum(1)` / Go 的 `hex.EncodeToString` 同形）。
    pub fn to_hex(self) -> String {
        hex(&self.0)
    }
}

/// 算 `data` 的 SHA-256。
///
/// 一次算完（不做流式）：调用点的数据要么是内嵌常量（[crate 里的 `CORE_BIN`]），
/// 要么是整个文件（十几 MB），一次读进来最简单，也**没有**一个"分块喂"的状态可写错。
pub fn sha256(data: &[u8]) -> Digest {
    /// SHA-256 的轮常数：前 64 位圆周率小数部分的前 32 位（FIPS 180-4 §4.2.2）。
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    // 初始状态：前 8 个素数平方根小数部分的前 32 位（FIPS 180-4 §5.3.3）。
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // 填充：0x80 → 若干 0 → 64 位大端比特长度。填充后的长度必是 64 的整数倍。
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity(data.len() + 72);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d) = (h[0], h[1], h[2], h[3]);
        let (mut e, mut f, mut g, mut hh) = (h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(v);
        }
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    Digest(out)
}

/// `data` 的 sha256 十六进制串（小写，64 个字符）。
pub fn sha256_hex(data: &[u8]) -> String {
    sha256(data).to_hex()
}

/// 小写十六进制。
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::{sha256, sha256_hex};

    /// **标准向量**（FIPS 180-4 的公开示例 / NIST 的 SHA-256 测试向量）。
    ///
    /// ⚠️ 手写哈希**没有别的东西能证明它是对的** —— 所以这一条不是"顺手补的覆盖率"，
    ///    它是这份实现唯一的判别力来源（内核那侧的同名测试也是这个理由）。
    ///    向量是**独立来源**（标准文档），不是从本实现里跑出来再抄回来的：
    ///    抄回来的期望值只能证明"它每次跑都一样"，证明不了"它算的是 SHA-256"。
    #[test]
    fn sha256_known_answer_vectors() {
        let cases: [(&[u8], &str); 4] = [
            (
                b"",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                b"abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (
                &[b'a'; 1_000_000], // 跨块 + 多轮 padding
                "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(
                sha256_hex(input),
                want,
                "输入 {} 字节的 sha256 与标准向量对不上 —— 手写实现的判别力全在这一条上",
                input.len()
            );
        }
    }

    /// 十六进制那一半：宽度、大小写、逐字节对应。
    ///
    /// 判别力：`to_hex` 少补一个前导 0（`{:x}` 而不是 `{:02x}` 那种写法）会在这里红 ——
    /// 而它一旦发生，哈希串会短一位，**名字与比较逻辑都还是"能跑"的**（静默错位）。
    #[test]
    fn the_hex_form_is_lowercase_and_zero_padded() {
        let h = sha256_hex(b"abc");
        assert_eq!(h.len(), 64, "sha256 的十六进制形式恒为 64 个字符");
        assert!(
            h.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "必须是小写十六进制：{h}"
        );
        // 摘要的首字节必须原样出现在串的开头（期望值来自上面那条 KAT 的向量本身：
        // 空串的摘要是 `e3b0c442…`）。
        assert_eq!(&sha256_hex(b"")[..2], "e3");
        assert_eq!(&h[..2], "ba", "abc 的摘要是 ba7816bf…");
        // ⚠️ 这一条要的是"**每个字节**都写成两位"：`Digest::to_hex` 走的正是 `hex()`，
        //    而 `hex()` 是逐字节 `{:02x}` 语义。首字节恰好是 0x0f 的摘要不好造，
        //    所以直接钉 `Digest::to_hex` 与 `hex()` 的同一性：32 字节的摘要必须
        //    展开成**恰好 64** 个字符（少补前导 0 的那种写法会得到更短的串）。
        assert_eq!(sha256(b"abc").to_hex().len(), 64);
        assert_eq!(sha256(b"abc"), sha256(b"abc"), "纯函数：同样的输入同样的摘要");
        assert_ne!(
            sha256(b"abc"),
            sha256(b"abd"),
            "这是密码学哈希，不是恒等映射（判别力下限）"
        );
    }
}
