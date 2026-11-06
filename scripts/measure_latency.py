#!/usr/bin/env python3
"""语言服务请求往返延迟测量：hover 与 completion 的 P50/P95。

用法：
    scripts/measure_latency.py --root /path/to/project --file a.ts -- ./lspd ts

行为：以 stdio 启动被测服务器，完成 initialize / didOpen 后先预热一次，
再在同一文件的不同位置交替发 hover 与 completion 请求，统计往返耗时。
输出毫秒级的 P50 / P95 / 最大值，便于与其他服务器横向比较。
"""

import argparse
import json
import os
import select
import statistics
import subprocess
import sys
import time


def frame(payload):
    """把一条 JSON-RPC 消息包装成 Content-Length 分帧。"""
    body = json.dumps(payload).encode()
    return b"Content-Length: %d\r\n\r\n" % len(body) + body


class Client:
    """极简 JSON-RPC 客户端：负责收发、应答反向请求与计时。"""

    def __init__(self, command, root):
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
        self.next_id = 0

    def send(self, payload):
        """发送一条消息。"""
        assert self.proc.stdin is not None
        self.proc.stdin.write(frame(payload))
        self.proc.stdin.flush()

    def _read_available(self, timeout):
        """等待并读取一段 stdout 数据。"""
        assert self.proc.stdout is not None
        ready, _, _ = select.select([self.proc.stdout], [], [], timeout)
        if not ready:
            return b""
        return os.read(self.proc.stdout.fileno(), 65536)

    def _read_message(self, deadline):
        """读一条完整消息，超时返回 None。"""
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

    def request(self, method, params, timeout):
        """发请求并等待其响应，返回 (响应, 往返秒数)。"""
        self.next_id += 1
        want = self.next_id
        started = time.perf_counter()
        self.send({"jsonrpc": "2.0", "id": want, "method": method, "params": params})
        deadline = time.time() + timeout
        while time.time() < deadline:
            message = self._read_message(deadline)
            if message is None:
                continue
            if "id" in message and message.get("method"):
                result = {} if message["method"] == "workspace/configuration" else None
                self.send({"jsonrpc": "2.0", "id": message["id"], "result": result})
                continue
            if message.get("id") == want:
                return message, time.perf_counter() - started
        return None, time.perf_counter() - started

    def close(self):
        """结束进程。"""
        if self.proc.poll() is None:
            self.proc.kill()


def _content_length(header):
    """从分帧头取出 Content-Length。"""
    for line in header.split(b"\r\n"):
        if line.lower().startswith(b"content-length"):
            return int(line.split(b":", 1)[1])
    return None


def summarize(samples):
    """把一组耗时（秒）汇总成毫秒级统计。"""
    values = sorted(v * 1000 for v in samples)
    if not values:
        return None
    return {
        "p50": round(statistics.median(values), 2),
        "p95": round(values[min(len(values) - 1, int(len(values) * 0.95))], 2),
        "max": round(values[-1], 2),
    }


def main():
    parser = argparse.ArgumentParser(description="语言服务请求延迟测量")
    parser.add_argument("--root", required=True, help="项目根目录")
    parser.add_argument("--file", required=True, help="用于测量的文件（相对项目根）")
    parser.add_argument("--rounds", type=int, default=20, help="每种请求的测量次数")
    parser.add_argument("--timeout", type=float, default=60.0, help="单次请求超时秒数")
    parser.add_argument("--json-out", default="", help="把结果追加写入该 JSON 文件")
    parser.add_argument("command", nargs=argparse.REMAINDER, help="-- 之后的服务器命令")
    args = parser.parse_args()

    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("缺少服务器命令，请在 -- 之后给出")

    root = os.path.abspath(args.root)
    path = os.path.join(root, args.file)
    uri = "file://" + path
    text = open(path, encoding="utf-8", errors="replace").read()
    lines = text.count("\n")

    client = Client(command, root)
    try:
        client.request("initialize", {
            "processId": None,
            "clientInfo": {"name": "lspd-latency"},
            "rootUri": "file://" + root,
            "workspaceFolders": [{"uri": "file://" + root, "name": os.path.basename(root)}],
            "capabilities": {
                "general": {"positionEncodings": ["utf-16"]},
                "textDocument": {"diagnostic": {}},
                "workspace": {"workspaceFolders": True, "configuration": True},
            },
        }, args.timeout)

        client.send({"jsonrpc": "2.0", "method": "initialized", "params": {}})
        client.send({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": uri, "languageId": "typescript", "version": 1, "text": text}}})

        # 预热：第一次请求往往包含 program 构建，不计入统计
        client.request("textDocument/hover",
                       {"textDocument": {"uri": uri}, "position": {"line": 0, "character": 0}},
                       args.timeout)

        hover, completion, diagnostic = [], [], []
        for index in range(args.rounds):
            line = (index * 7) % max(1, lines)
            params = {"textDocument": {"uri": uri}, "position": {"line": line, "character": 0}}
            _, elapsed = client.request("textDocument/hover", params, args.timeout)
            hover.append(elapsed)

            _, elapsed = client.request("textDocument/completion", params, args.timeout)
            completion.append(elapsed)

            _, elapsed = client.request("textDocument/diagnostic",
                                        {"textDocument": {"uri": uri}}, args.timeout)
            diagnostic.append(elapsed)
    finally:
        client.close()

    result = {
        "name": " ".join(command),
        "file": args.file,
        "rounds": args.rounds,
        "hover_ms": summarize(hover),
        "completion_ms": summarize(completion),
        "diagnostic_ms": summarize(diagnostic),
    }
    print("== 请求延迟（%d 次）==" % args.rounds)
    for key, label in (("hover_ms", "hover"), ("completion_ms", "completion"),
                       ("diagnostic_ms", "diagnostic")):
        stats = result[key]
        print("  %-12s P50 %6.2f ms   P95 %6.2f ms   max %6.2f ms"
              % (label, stats["p50"], stats["p95"], stats["max"]))
    if args.json_out:
        with open(args.json_out, "a+", encoding="utf-8") as handle:
            handle.write(json.dumps(result, ensure_ascii=False) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
