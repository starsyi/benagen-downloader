# macOS 客户端

SwiftUI 壳 + Rust 内核（`../core`）+ 内嵌下载引擎的图形界面客户端。

## 系统要求

- **macOS 13.0 或更高**。
  最低版本由包内三个二进制里**最严**的那个决定：壳与内核本身要求的更低，
  而内嵌的下载引擎（`aria2c-macos-*`）要求 **13.0**，所以整包的下限是 13.0。
  构建脚本里的 `check_minos_macos.sh` 会读包内每个二进制的 `minos` 并断言它 ≤ 目标下限 ——
  改部署目标时**多处要一起改**（`Package.swift`、`build_app_macos.sh`、`Info.plist`、
  以及下载引擎自身的编译参数），只改一处不会报错，但客户会打不开。
- Intel（x86_64）与 Apple Silicon（arm64）各出一份。

## 构建

```bash
bash scripts/build_app_macos.sh                          # arm64（默认）→ dist/BenagenDownloader.app
BENAGEN_ARCH=x86_64 bash scripts/build_app_macos.sh      # Intel
bash scripts/build_dmg_macos.sh                          # 打成 DMG（对 dist/ 里现成的 .app 操作）
bash scripts/check_minos_macos.sh                        # 断言包内每个二进制的最低系统版本
```

Intel 那份需要 rustup 提供的 `x86_64-apple-darwin` 标准库：

```bash
export PATH="$HOME/.cargo/bin:$PATH"                  # 少了这一句会看到第三方依赖编译失败（不是本仓的问题）
bash ../core/scripts/preflight_x86_64_toolchain.sh    # 工具链预检
```

`.app` 的装配内容是**壳 + 内核 + 下载引擎 + GPL 许可证 + 品牌资源**；
少了内核或引擎，客户拿到的是一个起不来下载的壳，所以请用脚本装配，不要手工拷 `.build/` 里的可执行文件。

## 测试

```bash
bash scripts/test.sh                       # Swift 测试套件
bash scripts/e2e_shell_macos.sh            # 端到端：真内核 + 真下载引擎 + 桩 HTTP 服务
bash scripts/verify_app_launch_macos.sh    # 启动取证：确认是前台应用且有窗口
```

## 分发注意

- **没有做代码签名，也没有做公证。** 客户首次打开需要手动放行。
  Intel 与 Apple Silicon 两份都是这个情况：arm64 那份由链接器加了 ad-hoc 签名，
  x86_64 那份完全没有签名 —— 两者都不满足 Gatekeeper 的要求。
- **盘里带了一件 `Resources/已损坏修复.command`**（`build_dmg_macos.sh` 装盘时拷进去）。
  客户双击应用提示「已损坏，无法打开」时，双击它即可：装进 `/Applications`、
  清掉 `com.apple.quarantine`、启动。这是**首选**的放行方式。
  ⚠️ 它**不动**系统 Gatekeeper（`spctl --master-disable` 会把整台机器的安全闸门永久关掉，
  而且实测并不能替代清属性），也不重新签名。
- **手动放行按系统版本分**（⚠️ 那句流传很广的「右键 →『打开』」**从 macOS 15 起已失效**，
  Apple 移除了那条路）：
  - **macOS 15 及以上**：先双击一次（会被拦下）→ 系统设置 →「隐私与安全性」→
    在「安全性」里找到那条拦截记录 → 点「仍要打开」。该按钮**只在尝试打开后的约 1 小时内可用**。
  - **macOS 14 及以下**：右键（或按住 Control 点击）→「打开」→ 再确认一次。
  - 或直接在终端执行（`-r` 是递归：应用是目录，属性落在里面每个文件上）：
    `xattr -dr com.apple.quarantine /Applications/BenagenDownloader.app`
- `scripts/verify_fix_script_macos.sh` 是修复脚本的取证：三条路径 + 变异自验，
  不碰真的 `/Applications`（用 `BENAGEN_APP_DEST` 指到临时目录）。
- `dist/` 是**可再生产物，不入库**：源码与脚本才是唯一真相。
