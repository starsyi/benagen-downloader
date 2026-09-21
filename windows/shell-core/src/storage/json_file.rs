//! 壳的**第一次写盘**：一份 JSON 文件的原子读写（**对齐 macOS 的
//! `macos/Sources/BenagenCoreKit/JsonFileStore.swift`**，逐条判据照抄）。
//!
//! 两条底线在这一层（macOS 侧记的 **E-1 / E-2**，这里逐字保留）：
//!
//!   * **E-1 —— 读失败不是异常路径**：[`JsonFile::read`] 返回 `None` 而不是 `Err`
//!     （最常见的情形就是"第一次运行、文件还不存在"）；
//!   * **E-2 —— 写盘必须原子**：**同目录**临时文件 + `rename`
//!     （与内核 `core/src/state.rs:146-150` 的既有做法同款）。半个文件比没有文件更糟：
//!     半截 JSON 会让下一次启动读到一份"能解析出前半段"的垃圾，而正确的结果是"当空"。
//!     路径只有一个占位：`std::fs::write` 也不是原子的（它先截断再写），
//!     而"跨设备的 rename 不是原子的"是 POSIX 的明文 ⇒ 临时文件**必须与目标同目录**。
//!
//! ## ⚠️ 这一层**不懂 JSON 的语义**：它只搬字节 + 收发 `serde_json`
//!
//! "这份字节还能不能用"由调用方按自己的语义判（`History::load` / `Preferences::load`
//! 各自"坏就当归零"）。在这里替它们判会把"坏文件"和"没有文件"混成同一个 `None`。
//!
//! ## ⚠️ 与 macOS 的一处**形态偏离（W-6）**
//!
//! 上游是 `read(_ url: URL) -> Data?`（**只吐字节**，解析留给调用方），而这里是
//! `read<T: DeserializeOwned>(path) -> Option<T>`（**连解析一起做**）—— 计划任务 3
//! 钉的就是这个签名，照办。两者的可观测差别只有一处：解析失败与读失败在这里
//! **折成同一个 `None`**，而 macOS 那边是两件事。对**本层的两个调用方**没有影响
//! （两边都是"坏就当归零"），但这件事写在这里，免得日后有人以为"读和解析分得开"。
//!
//! ## ⚠️⚠️ **测试一律用临时目录**
//!
//! `read` / `write` 都**接受路径参数**，`storage::dir()` 才指向真机上那个目录。
//! 那里放着人类伙伴**真实在用的**数据 —— 测试必须把句柄指到临时目录去
//! （`crate::storage::test_support::TempDir`）。

// ⚠️ `Write` 是**为了那个 trait 在作用域里**（`file.write_all` / `flush` 是它的方法），
//    不是"顺手 import"。`Read` 不需要：`serde_json::from_slice` 直接吃 `&[u8]`。
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

/// 原子的 JSON 文件读写。**无状态**，可以随便建（对齐 macOS 的 `JsonFileStore`，
/// 那边同样是个 `struct` + 两个方法、没有字段）。
pub struct JsonFile;

impl JsonFile {
    /// 读。**读不出来 ⇒ `None`，绝不 `Err`**（E-1：调用方按"当空"处理）。
    ///
    /// 覆盖：文件不存在、路径是个目录、权限不够、磁盘上是一堆二进制垃圾、字节不是
    /// 合法 JSON、JSON 的形状与 `T` 不符 … 这些在壳里**都不是**故障路径 ——
    /// 它们全部退化成"这次没有历史/没有偏好"。
    ///
    /// ⚠️ 对齐 macOS `JsonFileStore.read(_:) -> Data?` + 调用方那边的一次 `parse`。
    pub fn read<T: DeserializeOwned>(path: &Path) -> Option<T> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// 原子写：**同目录**临时文件 → `rename` 覆盖目标。
    ///
    /// - 目标目录不存在时**先建出来**（首次运行时整条
    ///   `…\BenagenDownloader\` 都可能不存在）—— 对齐 macOS 的
    ///   `createDirectory(withIntermediateDirectories: true)`；
    /// - 任何一步失败都**返回 `Err`**（简报：写失败要能被上报，不能让用户以为存上了）；
    /// - 失败时目标文件**逐字保持原样**（要么是旧的完整内容，要么是新的完整内容，
    ///   永远不会是半截）。
    ///
    /// ⚠️ 对齐 macOS `JsonFileStore.write(_:to:)`。失败时那个临时文件会被清掉
    ///    （除非清不掉 —— 那时它是个残留，但**目标文件仍然是完整的**，那才是承重的那条）。
    pub fn write<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
        // ⚠️ 序列化在**碰盘之前**：连一份能写出去的字节都拿不到时，不该已经动过目标目录。
        let bytes = serde_json::to_vec_pretty(value)
            .map_err(|cause| std::io::Error::new(std::io::ErrorKind::InvalidData, cause))?;

        // ⚠️ `parent()` 对**光杆文件名**（`"a.json"`）返回的是空路径，那时"建目录"没有意义
        //    （`create_dir_all("")` 会报错）。macOS 那边不需要这条守卫，因为 `URL` 总有目录。
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }

        let tmp = temporary_path(path)?;

        // 写临时文件 —— 失败就清掉它再抛（`File` 在这里被 drop，句柄先关掉再删）。
        let written = write_and_sync(&tmp, &bytes);
        if let Err(cause) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(cause);
        }

        if let Err(cause) = std::fs::rename(&tmp, path) {
            // 目标没被碰过，但临时文件留下了 ⇒ 清掉（不然它会永远躺在那里）。
            let _ = std::fs::remove_file(&tmp);
            return Err(cause);
        }
        Ok(())
    }
}

/// 临时文件的名字：**与目标同目录**、点开头、以 `.tmp` 结尾
/// （对齐 macOS 的 `JsonFileStore.temporaryName(for:)`，逐字同形）。
///
/// ⚠️ **点开头是有意的**：它是"这个名字是我们的、删它安全"的标记
///    （macOS 那条注释的理由），也是 Unix 上 `ls` 默认不显示的那一类。
/// ⚠️ 名字**从 `file_name()` 派生**而不是拿整条路径拼：拿整条路径拼出来的名字里带
///    目录分隔符，`File::create` 会去建一个子路径 —— 那是另一个（更糟的）失败形态。
fn temporary_path(path: &Path) -> std::io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("这份 JSON 的路径没有文件名（{}），拼不出临时文件名", path.display()),
        )
    })?;
    let mut tmp = std::ffi::OsString::from(".");
    tmp.push(name);
    tmp.push(".tmp");
    Ok(path.with_file_name(tmp))
}

/// 写 + `flush` + `sync_all`。
///
/// ⚠️ **`sync_all` 不是装饰**：少了它，"rename 完成"与"字节真的落到盘上"之间还有一段窗口，
///    而断电恰好落在那段窗口里时，目标是**一个完整的空文件** —— 那正是 E-2 要拦的形状
///    （比"没有文件"更糟：它看起来是成功的）。
fn write_and_sync(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_support::TempDir;

    /// ⚠️ **E-1**：文件不存在、内容坏了 —— 两件事都**不是**错误路径，都退化成 `None`。
    #[test]
    fn read_failure_is_none_not_an_error() {
        // macOS 侧：读失败返回 nil 不抛（当空处理）
        let dir = TempDir::new("read");
        assert_eq!(JsonFile::read::<serde_json::Value>(&dir.path().join("nope.json")), None);
        std::fs::write(dir.path().join("broken.json"), b"{not json").unwrap();
        assert_eq!(JsonFile::read::<serde_json::Value>(&dir.path().join("broken.json")), None);
        // 形状不符（`T` 要对象、盘上是个数组）同样是"当空"，不是错误。
        std::fs::write(dir.path().join("shape.json"), b"[1,2]").unwrap();
        #[derive(serde::Deserialize)]
        struct Wanted {
            #[allow(dead_code)]
            v: i64,
        }
        assert!(JsonFile::read::<Wanted>(&dir.path().join("shape.json")).is_none());
        // 路径是**目录**也是"当空"（`fs::read` 会失败）。
        std::fs::create_dir(dir.path().join("a_dir")).unwrap();
        assert_eq!(JsonFile::read::<serde_json::Value>(&dir.path().join("a_dir")), None);
    }

    /// 写出去的能原样读回来（`T` 的形状对上）。
    #[test]
    fn a_written_file_reads_back_as_the_same_value() {
        let dir = TempDir::new("roundtrip");
        let path = dir.path().join("a.json");
        JsonFile::write(&path, &serde_json::json!({"v": 1, "s": "中文"})).unwrap();
        let back = JsonFile::read::<serde_json::Value>(&path).expect("刚写下去的一定读得回来");
        assert_eq!(back, serde_json::json!({"v": 1, "s": "中文"}));
    }

    /// ⚠️ **E-2**：写成功之后同目录里**不该有 `.tmp` 残留**（它必须被 `rename` 掉）。
    ///
    /// 判别力：把 `rename` 换成 `std::fs::copy` + 删临时文件，这一条会红 ——
    /// 而那正是"跨设备的 rename 不是原子的"那条纪律要拦的写法。
    #[test]
    fn write_is_atomic_and_reports_failure_to_the_caller() {
        // macOS 侧：同目录临时文件 + rename；失败**抛给调用方**
        //（供「历史写盘失败」那条常驻提示行显示 —— 见 presentation 的既有语义）
        let dir = TempDir::new("write");
        JsonFile::write(&dir.path().join("a.json"), &serde_json::json!({"v": 1})).unwrap();
        assert!(dir.path().join("a.json").exists());
        // 同目录下不该留下 .tmp
        let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "临时文件必须被 rename 掉");
    }

    /// ⚠️ **首次运行时整条目录都可能不存在** ⇒ `write` 要**先把它建出来**
    ///    （对齐 macOS 的 `createDirectory(withIntermediateDirectories: true)`）。
    ///
    /// 少了它，第一次启动的历史/偏好**一条都存不下来**，而错误只会在"写盘失败"
    /// 那条常驻提示行上出现一次 —— 真机上归因成本极高。
    #[test]
    fn write_creates_the_whole_directory_chain() {
        let dir = TempDir::new("mkdir");
        let path = dir.path().join("BenagenDownloader").join("deep").join("a.json");
        JsonFile::write(&path, &serde_json::json!({"v": 1})).expect("目录不存在时要自己建");
        assert!(path.exists());
    }

    /// ⚠️⚠️ **写失败时目标文件逐字保持原样**（E-2 的下半条）——这是本文件里最要紧的一条。
    ///
    /// 造一次**真实的**失败：把临时文件那个名字占成一个**目录**（`File::create` 会
    /// `EISDIR`）。这与 macOS `JsonFileStoreTests.aFailedWriteThrowsAndLeavesThePreviousFileIntact`
    /// 用的是同一个手法（那边也把临时文件名占成目录）。
    ///
    /// 判别力：把 `write` 改成直接 `std::fs::write(path, …)`（先截断再写），
    /// 这一条立刻红 —— 而真机上的表现是"一次失败把用户的历史清空了"。
    #[test]
    fn a_failed_write_reports_the_failure_and_leaves_the_previous_file_intact() {
        let dir = TempDir::new("failwrite");
        let path = dir.path().join("a.json");
        JsonFile::write(&path, &serde_json::json!({"v": 1})).expect("先写一份好的");

        // 占住临时文件名（`.a.json.tmp`）⇒ 下一次写必然失败。
        std::fs::create_dir(dir.path().join(".a.json.tmp")).expect("占住临时名");

        let failed = JsonFile::write(&path, &serde_json::json!({"v": 2}));
        assert!(failed.is_err(), "写盘失败必须报给调用方（不许静默当成功）");

        let back = JsonFile::read::<serde_json::Value>(&path).expect("旧文件必须还在");
        assert_eq!(back, serde_json::json!({"v": 1}), "失败的写把旧内容改掉了");
    }

    /// 临时文件名：**同目录**、点开头、以 `.tmp` 结尾（对齐 macOS 的 `temporaryName(for:)`）。
    ///
    /// 判别力：名字里若带上目录（拿整条路径拼），`write` 会去建一条子路径 ——
    /// 这一条会红，而真机上的表现是"历史写在了一个莫名其妙的子目录里"。
    #[test]
    fn the_temporary_name_lives_beside_the_target() {
        let tmp = temporary_path(Path::new("/x/y/a.json")).expect("有文件名");
        assert_eq!(tmp, PathBuf::from("/x/y/.a.json.tmp"));
        // ⚠️ **同目录**是承重的（跨设备的 `rename` 不是原子的）——这一条钉住它。
        assert_eq!(tmp.parent(), Some(Path::new("/x/y")));
    }

    /// 畸形输入（没有文件名）**不 panic**，只是报一个错。
    #[test]
    fn a_path_without_a_file_name_is_an_error_not_a_panic() {
        assert!(temporary_path(Path::new("/")).is_err());
    }
}
