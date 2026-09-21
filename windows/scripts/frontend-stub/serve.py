#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""serve.py —— 无头验收用的**最小静态服务**（给 `check_files_screen.sh` 起）。

    python3 serve.py <服务根> <端口> <结果文件>

两件事：
  · `GET`  —— 当普通静态服务（`http.server.SimpleHTTPRequestHandler`，根 = 第一个参数）；
  · `POST /__result` —— 把请求体**原样写进**第三个参数那个文件。

## ⚠️ 为什么要自己写一个，而不是 `python3 -m http.server`
因为结果要**从浏览器里出来**。本仓库实测：这台机器上的 `Microsoft Edge 149` 在
`--headless=new --dump-dom` 下**会挂住**（页面跑完了也不退出、也拿不到 stdout）；
而 `--screenshot` 能退，但截图里的数字**脚本读不到**（那就又回到"靠人看"）。
⇒ 夹具页把 `{ok, fail, lines}` POST 回来，本服务把它落到一个文件上，runner 轮询那个文件。
这样"跑完了没有""过了几条、红的是哪几条"都是**脚本读得到的数**。

⚠️ 它**不是**一个通用服务器：只监听 127.0.0.1、只多接一个 POST 路由、日志关掉
（噪声只在失败时才有用，而失败时 runner 会把夹具页里的 `lines` 全打出来）。
⚠️ 它住在 `windows/scripts/frontend-stub/` —— **不在 `windows/web/` 下**，
所以不会被 `frontendDist` 编进 exe。
"""

import functools
import http.server
import os
import socketserver
import sys

def main() -> int:
    if len(sys.argv) != 4:
        print("用法：python3 serve.py <服务根> <端口> <结果文件>", file=sys.stderr)
        return 2
    root, port, result_path = sys.argv[1], int(sys.argv[2]), sys.argv[3]
    if not os.path.isdir(root):
        print(f"错误：服务根不是一个目录：{root}", file=sys.stderr)
        return 1

    class Handler(http.server.SimpleHTTPRequestHandler):
        def do_POST(self):  # noqa: N802 —— http.server 的命名约定
            if self.path != "/__result":
                self.send_error(404, "only /__result is accepted")
                return
            try:
                n = int(self.headers.get("content-length") or 0)
            except ValueError:
                n = 0
            body = self.rfile.read(n) if n > 0 else b""
            # ⚠️ 先写临时文件再改名：runner 是**轮询这个文件**的，
            #    直接写会让它读到一个写了一半的 JSON（那是"偶发红"，最难查）。
            tmp = result_path + ".part"
            with open(tmp, "wb") as fh:
                fh.write(body)
            os.replace(tmp, result_path)
            self.send_response(204)
            self.end_headers()

        def log_message(self, *args):  # 安静：噪声只在失败时才有用
            pass

    class Server(socketserver.TCPServer):
        allow_reuse_address = True  # 连着跑两轮之间不留 TIME_WAIT 的坑

    handler = functools.partial(Handler, directory=root)
    with Server(("127.0.0.1", port), handler) as httpd:
        httpd.serve_forever()
    return 0

if __name__ == "__main__":
    sys.exit(main())
