# 第三方组件

本仓库包含以下第三方组件。**它们依各自的许可证分发，不受本仓库 LICENSE（专有）约束。**

## 1. aria2（下载引擎）— GNU GPL v2 或更高

- **版本**：1.37.0（未作任何修改）
- **来源**：https://github.com/aria2/aria2
- **源码**：https://github.com/aria2/aria2/releases （1.37.0 的源码包）
- **许可证全文**：随二进制一同分发，见
  `core/assets/COPYING-GPLv2.txt` 与 `downloader/internal/engine/assets/COPYING-GPLv2.txt`
- **在本项目中的形态**：以**独立的可执行文件**形式随附（`core/assets/aria2c-*`），
  由本项目的程序在运行时释放到临时目录并以子进程方式调用（`--rpc-secret` 等参数）。
  本项目**没有修改** aria2 的源代码，也没有把它链接进本项目自己的代码。
- 应用内的「开源许可」界面会显示 aria2 的 GPLv2 许可证全文。

> 如需 aria2 的源码，请按上述地址获取；我们随附的二进制与上游 1.37.0 一致，未作修改。

## 2. musl（C 标准库，仅 Linux 静态构建）

- **许可证**：MIT
- **来源**：https://musl.libc.org/
- **用途**：Linux 版 `benagen-dl` 以 `x86_64-unknown-linux-musl` 静态链接构建。

## 3. OpenSSL（TLS，仅随 aria2 的静态构建）

- **许可证**：Apache License 2.0
- **来源**：https://www.openssl.org/
- **用途**：随 aria2 一同静态编译，供 HTTPS 下载使用。

## 4. webview2-com-sys（仅 Windows 客户端，已 vendor）

- **许可证**：MIT
- **来源**：https://github.com/wravery/webview2-rs
- **位置**：`windows/vendor/webview2-com-sys`（含 Microsoft WebView2 SDK 的头文件与加载器）

## 5. Rust / Swift 生态的依赖

各 crates 与 Swift Package 依赖的许可证，可在各自的包管理器中查询
（`core/Cargo.lock`、`windows/Cargo.lock`、Swift Package 的 `Package.resolved`）。
