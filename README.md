# BenagenDownloader

贝纳基因（Benagen）测序数据交付的**客户端**源代码：把一批交付数据完整下载到本地并校验。

一次交付由一个**交付码**标识。客户端解析交付码 → 拉取该批次的清单 → 下载全部文件 → 逐个校验，
最后给出"到底成没成"的结论。

## 组件

| 目录 | 是什么 | 技术栈 |
|---|---|---|
| `core/` | **共用内核**：交付解析、清单、下载编排、校验、状态持久化。同时提供**命令行工具 `benagen-dl`**（单文件、内嵌下载引擎） | Rust |
| `macos/` | macOS 图形界面客户端 | SwiftUI + Rust 内核 |
| `windows/` | Windows 图形界面客户端 | Tauri（Rust 壳 + Web 前端）+ Rust 内核 |
| `extract_tools/` | 交付给客户的**数据包解压工具**（与本流水线运行时无关，独立分发） | Go / Python / Bash 三种实现 |

命令行工具的**面向使用者**的说明见 [`benagen_dl_usage_guide.md`](benagen_dl_usage_guide.md)。

## 系统要求

| 平台 | 要求 |
|---|---|
| macOS | **macOS 13.0 或更高**（最低版本由内嵌的下载引擎决定）。Intel 与 Apple Silicon 都有对应构建 |
| Linux | x86_64，**静态链接**（不依赖系统库）。目前只提供命令行工具 |
| Windows | Windows 10/11，需 **WebView2 运行时**（Windows 11 自带） |

> macOS 与 Windows 的分发包**未做代码签名/公证**。macOS 上首次打开需要手动放行
> （右键 →「打开」，或解除 quarantine 属性），详见命令行工具说明里的同一节。

## 构建

### 内核与命令行工具

```bash
cd core
cargo build --release                 # 内核（benagen-core）与 CLI（benagen-dl）
cargo test                            # 单元测试与端到端测试
```

打包三平台的单文件 CLI（产物落在 `dist_cli/`，**不入库**）：

```bash
bash core/scripts/build_cli_macos.sh                    # arm64（默认）
BENAGEN_ARCH=x86_64 bash core/scripts/build_cli_macos.sh
bash core/scripts/build_cli_linux.sh                    # 需在一台 Linux x86_64 机器上跑
```

> 交叉编 Linux 静态版需要 musl 工具链，并显式指定 `RUSTFLAGS="-C relocation-model=static"`；
> 这些细节都在 `core/scripts/build_cli_linux.sh` 里。

### macOS 客户端

```bash
bash macos/scripts/build_app_macos.sh                   # arm64（默认）→ macos/dist/BenagenDownloader.app
BENAGEN_ARCH=x86_64 bash macos/scripts/build_app_macos.sh
bash macos/scripts/build_dmg_macos.sh                   # 打成 DMG
bash macos/scripts/test.sh                              # Swift 测试套件
```

`build_app_macos.sh` 会把**壳 + Rust 内核 + 下载引擎 + GPL 许可证**装配进一个 `.app`。
Intel 那份需要 rustup 提供的 `x86_64-apple-darwin` 标准库（`export PATH="$HOME/.cargo/bin:$PATH"`）。

### Windows 客户端

```bash
bash windows/scripts/build_windows.sh
```

细节与验收步骤见 `windows/README.md`。

### 客户解包工具

```bash
cd extract_tools && bash build_windows_exe_linux.sh go   # 交叉编译 Windows EXE
bash extract_tool.sh <前缀> [输出目录] [--md5]           # 纯 Bash 版（Linux 客户）
```

## 许可证

本仓库为**专有软件**，版权所有 (c) 2026 贝纳基因（Benagen），保留所有权利。详见 [`LICENSE`](LICENSE)。

仓库内含的第三方组件（下载引擎 aria2 为 **GPLv2**，另有 musl / OpenSSL / WebView2 SDK 等）
依其自身许可证分发，详见 [`THIRD-PARTY.md`](THIRD-PARTY.md)。
