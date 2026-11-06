#!/usr/bin/env python3
"""TypeScript 语言服务能力自检：确认 tsbridge 接线后各项能力真实可用。

用法：
    scripts/verify_ts.py --root /path/to/ts-project -- ./target/release/lspd ts

行为：以 stdio 启动被测服务器，完成 initialize / didOpen 握手，逐一发请求并
核对响应，输出每一项的通过与否，全部通过时退出码为 0。

注意：诊断走 LSP 3.17 的 pull 模式（`textDocument/diagnostic`）。push 模式
（`textDocument/publishDiagnostics`）只承载 project 级诊断，不能用它判断
文件级诊断是否可用——这是一个曾导致误判的坑。
"""

import argparse
import json
import os
import select
import subprocess
import sys
import time

# 被测项目根下用于自检的临时文件名；内容故意含语法与类型两类错误
PROBE_FILE = "lspd_probe.ts"
PROBE_SOURCE = """const probe: number = "not a number";
function probeFn( {
"""


def frame(payload):
    """把一条 JSON-RPC 消息包装成 Content-Length 分帧。"""
    body = json.dumps(payload).encode()
    return b"Content-Length: %d\r\n\r\n" % len(body) + body


class Conn:
    """极简 JSON-RPC 连接：只实现本脚本需要的收发与请求应答。"""

    def __init__(self, command, root):
        self.root = root
        self.file = os.path.join(root, PROBE_FILE)
        self.proc = subprocess.Popen(
            command,
            cwd=root,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            bufsize=0,
            env=dict(os.environ, RUST_BACKTRACE="1"),
        )
        self.buf = b""

    def send(self, payload):
        """发送一条消息。"""
        assert self.proc.stdin is not None
        self.proc.stdin.write(frame(payload))
        self.proc.stdin.flush()

    def _read_available(self, timeout):
        """在 stdout 上等待并读取一段数据，超时返回空。"""
        assert self.proc.stdout is not None
        ready, _, _ = select.select([self.proc.stdout], [], [], timeout)
        if not ready:
            return b""
        return os.read(self.proc.stdout.fileno(), 65536)

    def _read_message(self, timeout):
        """读一条完整的 JSON-RPC 消息，超时返回 None。"""
        deadline = time.time() + timeout
        while time.time() < deadline:
            if b"\r\n\r\n" in self.buf:
                header, rest = self.buf.split(b"\r\n\r\n", 1)
                length = _content_length(header)
                if length is None:
                    self.buf = rest
                    continue
                while len(rest) < length:
                    chunk = self._read_available(max(0.1, deadline - time.time()))
                    if not chunk:
                        return None
                    rest += chunk
                self.buf = rest[length:]
                return json.loads(rest[:length])
            chunk = self._read_available(max(0.1, deadline - time.time()))
            if not chunk:
                return None
            self.buf += chunk
        return None

    def wait_response(self, want_id, timeout=90.0):
        """等待指定 id 的响应；期间应答服务器发来的请求。"""
        deadline = time.time() + timeout
        while time.time() < deadline:
            message = self._read_message(max(0.1, deadline - time.time()))
            if message is None:
                continue
            if "id" in message and message.get("method"):
                # 服务器反向请求（workspace/configuration 等）必须应答，否则会话卡住
                result = {} if message["method"] == "workspace/configuration" else None
                self.send({"jsonrpc": "2.0", "id": message["id"], "result": result})
                continue
            if message.get("id") == want_id:
                return message
        return None

    def close(self):
        """结束进程。"""
        if self.proc.poll() is None:
            self.proc.kill()


def _content_length(header):
    """从分帧头里取出 Content-Length。"""
    for line in header.split(b"\r\n"):
        if line.lower().startswith(b"content-length"):
            return int(line.split(b":", 1)[1])
    return None


def check(name, ok, detail=""):
    """打印一行检查结果并返回是否通过。"""
    print("  %s %-14s %s" % ("通过" if ok else "失败", name, detail))
    return ok


def main():
    parser = argparse.ArgumentParser(description="TypeScript 语言服务能力自检")
    parser.add_argument("--root", required=True, help="被测 TypeScript 项目根目录")
    parser.add_argument("--timeout", type=float, default=90.0, help="单个请求的超时秒数")
    parser.add_argument("command", nargs=argparse.REMAINDER, help="-- 之后的服务器命令")
    args = parser.parse_args()

    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("缺少服务器命令，请在 -- 之后给出")

    root = os.path.abspath(args.root)
    uri = "file://" + os.path.join(root, PROBE_FILE)
    conn = Conn(command, root)
    results = []

    try:
        conn.send({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "processId": None,
                "rootUri": "file://" + root,
                "workspaceFolders": [{"uri": "file://" + root, "name": "probe"}],
                "capabilities": {
                    "workspace": {"configuration": True},
                    "textDocument": {"diagnostic": {}},
                },
            },
        })
        response = conn.wait_response(1, args.timeout)
        if response is None:
            print("initialize 无响应")
            return 1

        caps = response["result"]["capabilities"]
        print("== 能力协商 ==")
        for key in ("completionProvider", "hoverProvider", "definitionProvider",
                    "referencesProvider", "renameProvider", "diagnosticProvider"):
            results.append(check(key, key in caps))

        conn.send({"jsonrpc": "2.0", "method": "initialized", "params": {}})
        conn.send({
            "jsonrpc": "2.0", "method": "textDocument/didOpen",
            "params": {"textDocument": {
                "uri": uri, "languageId": "typescript", "version": 1,
                "text": PROBE_SOURCE,
            }},
        })

        print("== 请求能力 ==")
        hover = conn.wait_response(3, args.timeout) if conn.send({
            "jsonrpc": "2.0", "id": 3, "method": "textDocument/hover",
            "params": {"textDocument": {"uri": uri}, "position": {"line": 0, "character": 6}},
        }) is None else None
        hover_text = ((hover or {}).get("result") or {}).get("contents") or {}
        results.append(check("hover", bool(hover_text),
                             hover_text.get("value", "") if isinstance(hover_text, dict) else ""))

        conn.send({
            "jsonrpc": "2.0", "id": 4, "method": "textDocument/diagnostic",
            "params": {"textDocument": {"uri": uri}},
        })
        diagnostic = conn.wait_response(4, args.timeout)
        # pull 模式的响应形如 {"kind": "full", "items": [...]}
        items = ((diagnostic or {}).get("result") or {}).get("items", [])
        codes = sorted({item.get("code") for item in items if item.get("code")})
        results.append(check("diagnostic", bool(items),
                             "%d 条，code=%s" % (len(items), codes)))
    finally:
        conn.close()

    passed = all(results)
    print("\n结论：%s" % ("全部通过" if passed else "存在失败项"))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
