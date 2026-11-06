#!/usr/bin/env python3
"""LSP 服务器基准：常驻内存与启动延迟。

用法：
    scripts/bench.py --name tsrs --root ~/some/project -- tsrs --lsp -stdio
    scripts/bench.py --name vtsls --root ~/some/project -- npx vtsls --stdio

行为：以 stdio 启动被测服务器，完成 initialize / didOpen 握手，在三个时点采样
被测进程的 RSS（常驻集），并统计 initialize 往返耗时。全程不修改被测文件。
"""

import argparse
import json
import os
import queue
import subprocess
import sys
import threading
import time

# 参与打开的文件扩展名到 LSP languageId 的映射
LANGUAGE_BY_EXT = {
    ".html": "html",
    ".htm": "html",
    ".css": "css",
    ".scss": "scss",
    ".less": "less",
    ".sass": "sass",
    ".json": "json",
    ".ts": "typescript",
    ".tsx": "typescriptreact",
    ".js": "javascript",
    ".jsx": "javascriptreact",
    ".rs": "rust",
}

# 遍历项目时跳过的目录，避免把依赖与构建产物算进基准
SKIP_DIRS = {
    ".git", "node_modules", "target", "dist", "build", ".venv",
    ".next", "vendor", "bench-out",
}


def collect_files(root, only_exts, max_files):
    """深度优先收集项目内可打开的文件，跳过依赖与构建目录。"""
    picked = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(name for name in dirnames if name not in SKIP_DIRS)
        for filename in sorted(filenames):
            ext = os.path.splitext(filename)[1].lower()
            if ext not in LANGUAGE_BY_EXT:
                continue
            if only_exts and ext not in only_exts:
                continue
            picked.append(os.path.join(dirpath, filename))
            if len(picked) >= max_files:
                return picked
    return picked


class FrameReader(threading.Thread):
    """后台读取服务器 stdout 的 JSON-RPC 分帧，把消息放进队列。"""

    def __init__(self, stream):
        super().__init__(daemon=True)
        self.stream = stream
        self.messages = queue.Queue()

    def run(self):
        while True:
            length = self._read_headers()
            if length is None:
                self.messages.put(None)
                return
            body = self.stream.read(length)
            if not body:
                self.messages.put(None)
                return
            try:
                self.messages.put(json.loads(body))
            except json.JSONDecodeError:
                continue

    def _read_headers(self):
        """解析 Content-Length 头，返回正文字节数；流结束返回 None。"""
        length = None
        while True:
            line = self.stream.readline()
            if not line:
                return None
            if line in (b"\r\n", b"\n"):
                return length
            name, _, value = line.decode("ascii", "replace").partition(":")
            if name.strip().lower() == "content-length":
                length = int(value.strip())


class Server:
    """被测服务器进程的封装：消息收发与 RSS 采样。"""

    def __init__(self, command, cwd, env):
        self.process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            cwd=cwd,
            env=env,
        )
        self.reader = FrameReader(self.process.stdout)
        self.reader.start()
        self.next_id = 0

    def send(self, payload):
        """按 LSP 分帧写入一条消息。"""
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        stdin = self.process.stdin
        assert stdin is not None, "子进程的 stdin 必须是管道"
        stdin.write(b"Content-Length: %d\r\n\r\n" % len(body))
        stdin.write(body)
        stdin.flush()

    def request(self, method, params, timeout):
        """发送请求并等待应答，返回 (结果, 往返秒数)；超时返回 (None, None)。"""
        self.next_id += 1
        request_id = self.next_id
        started = time.monotonic()
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        deadline = started + timeout
        while time.monotonic() < deadline:
            try:
                message = self.reader.messages.get(timeout=deadline - time.monotonic())
            except queue.Empty:
                return None, None
            if message is None:
                return None, None
            if message.get("id") == request_id:
                return message, time.monotonic() - started
        return None, None

    def notify(self, method, params):
        """发送通知，不等待应答。"""
        self.send({"jsonrpc": "2.0", "method": method, "params": params})

    def rss_kb(self):
        """读取被测进程当前常驻集，单位 KB。"""
        try:
            output = subprocess.check_output(
                ["ps", "-o", "rss=", "-p", str(self.process.pid)], text=True
            )
        except subprocess.CalledProcessError:
            return None
        return int(output.strip()) if output.strip() else None

    def close(self):
        """尽力优雅退出，超时则强制结束。"""
        if self.process.poll() is not None:
            return
        try:
            self.notify("exit", None)
            stdin = self.process.stdin
            if stdin is not None:
                stdin.close()
        except (BrokenPipeError, ValueError):
            pass
        try:
            self.process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.process.kill()


class RssSampler(threading.Thread):
    """后台采样 RSS，记录峰值。"""

    def __init__(self, server, interval=0.1):
        super().__init__(daemon=True)
        self.server = server
        self.interval = interval
        self.peak_kb = 0
        self.samples = 0

    def run(self):
        while self.server.process.poll() is None:
            current = self.server.rss_kb()
            if current:
                self.peak_kb = max(self.peak_kb, current)
                self.samples += 1
            time.sleep(self.interval)


def build_initialize_params(root):
    """构造 initialize 参数：以单根工作区打开被测项目。"""
    root_uri = path_to_uri(root)
    return {
        "processId": os.getpid(),
        "clientInfo": {"name": "lspd-bench"},
        "rootUri": root_uri,
        "workspaceFolders": [{"uri": root_uri, "name": os.path.basename(root)}],
        "capabilities": {
            "general": {"positionEncodings": ["utf-16"]},
            "textDocument": {
                "synchronization": {"didSave": False},
                "publishDiagnostics": {"versionSupport": True},
            },
            "workspace": {"workspaceFolders": True},
        },
    }


def path_to_uri(path):
    """把本地路径转成 file:// URI，保留中文与空格。"""
    from urllib.parse import quote

    absolute = os.path.abspath(path)
    return "file://" + quote(absolute)


def format_kb(value):
    """把 KB 格式化成 MB，便于阅读。"""
    return "-" if value is None else "%.1f MB" % (value / 1024.0)


def main():
    parser = argparse.ArgumentParser(description="LSP 服务器内存与启动延迟基准")
    parser.add_argument("--name", required=True, help="被测服务器名称，用于输出")
    parser.add_argument("--root", required=True, help="被测项目根目录")
    parser.add_argument("--max-files", type=int, default=32, help="最多打开的文件数")
    parser.add_argument(
        "--only",
        default="",
        help="只看这些扩展名，逗号分隔，例如 .html,.css",
    )
    parser.add_argument("--settle", type=float, default=3.0, help="打开文件后的等待秒数")
    parser.add_argument("--timeout", type=float, default=120.0, help="initialize 超时秒数")
    parser.add_argument("--json-out", default="", help="把结果追加写入该 JSON 文件")
    parser.add_argument(
        "--probe",
        action="store_true",
        help="didOpen 后发一次 completion 请求，触发懒加载的语言服务（program/类型检查）再采样",
    )
    parser.add_argument(
        "--close-all",
        action="store_true",
        help="probe 后依次 didClose 所有已打开文件再采样，验证会话收缩时的内存回落",
    )
    parser.add_argument(
        "--release-wait",
        type=float,
        default=0.0,
        help="probe 后静置的秒数，到时再采样一次 RSS，用于观察空闲后是否归还内存；0 表示不测",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER, help="-- 之后的服务器命令")
    args = parser.parse_args()

    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("缺少服务器命令，请在 -- 之后给出")

    only_exts = {item.strip().lower() for item in args.only.split(",") if item.strip()}
    files = collect_files(args.root, only_exts, args.max_files)
    if not files:
        parser.error("在 %s 下没有找到可打开的文件" % args.root)

    env = dict(os.environ)
    env.setdefault("RUST_BACKTRACE", "1")

    server = Server(command, args.root, env)
    sampler = RssSampler(server)
    sampler.start()

    result = {"name": args.name, "command": command, "root": os.path.abspath(args.root),
              "files": len(files), "opened_bytes": 0}

    response, roundtrip = server.request("initialize", build_initialize_params(args.root), args.timeout)
    result["initialize_ms"] = None if roundtrip is None else round(roundtrip * 1000, 1)
    if response is None:
        result["status"] = "initialize 无响应"
        result["idle_mb"] = round((server.rss_kb() or 0) / 1024.0, 1)
        server.close()
        report(result, args.json_out)
        return 0

    result["status"] = "ok"
    server.notify("initialized", {})
    time.sleep(args.settle)
    result["idle_mb"] = round((server.rss_kb() or 0) / 1024.0, 1)

    opened_bytes = 0
    for path in files:
        text = read_text(path)
        opened_bytes += len(text.encode("utf-8"))
        server.notify(
            "textDocument/didOpen",
            {
                "textDocument": {
                    "uri": path_to_uri(path),
                    "languageId": LANGUAGE_BY_EXT[os.path.splitext(path)[1].lower()],
                    "version": 1,
                    "text": text,
                }
            },
        )
    result["opened_bytes"] = opened_bytes

    # 懒加载的服务器（如 tsrs）在 didOpen 后并不构建 program，
    # 发一次真实请求才能测到语言服务真正工作时的常驻内存。
    if args.probe and files:
        first = files[0]
        text = read_text(first)
        line = text.count("\n") // 2
        server.request(
            "textDocument/completion",
            {
                "textDocument": {"uri": path_to_uri(first)},
                "position": {"line": line, "character": 0},
            },
            args.timeout,
        )

    time.sleep(args.settle)
    result["open_mb"] = round((server.rss_kb() or 0) / 1024.0, 1)
    result["peak_mb"] = round(sampler.peak_kb / 1024.0, 1)

    # 关闭全部文档：program 只有在没有打开的文件时才可能收缩。
    if args.close_all:
        for path in files:
            server.notify(
                "textDocument/didClose",
                {"textDocument": {"uri": path_to_uri(path)}},
            )
        time.sleep(args.settle)
        result["closed_mb"] = round((server.rss_kb() or 0) / 1024.0, 1)

    # 静置后重新采样：验证服务器在空闲时是否把已释放的内存归还给操作系统。
    if args.release_wait > 0:
        time.sleep(args.release_wait)
        result["relaxed_mb"] = round((server.rss_kb() or 0) / 1024.0, 1)

    server.request("shutdown", None, 10.0)
    server.close()
    report(result, args.json_out)
    return 0


def read_text(path):
    """按 UTF-8 读取文件，解码失败时用替换字符，避免基准因编码中断。"""
    with open(path, "rb") as handle:
        return handle.read().decode("utf-8", "replace")


def report(result, json_out):
    """输出人类可读表格，并按需追加 JSON 记录。"""
    print("== %s ==" % result["name"])
    print("  命令        : %s" % " ".join(result["command"]))
    print("  项目        : %s" % result["root"])
    print("  打开文件    : %d 个，%.1f KB" % (result["files"], result["opened_bytes"] / 1024.0))
    print("  启动耗时    : %s ms" % result.get("initialize_ms"))
    print("  空闲 RSS    : %s" % format_mb(result.get("idle_mb")))
    print("  打开后 RSS  : %s" % format_mb(result.get("open_mb")))
    print("  峰值 RSS    : %s" % format_mb(result.get("peak_mb")))
    if "closed_mb" in result:
        print("  关闭后 RSS  : %s" % format_mb(result.get("closed_mb")))
    if "relaxed_mb" in result:
        print("  静置后 RSS  : %s" % format_mb(result.get("relaxed_mb")))
    print("  状态        : %s" % result["status"])
    if json_out:
        with open(json_out, "a+", encoding="utf-8") as handle:
            handle.write(json.dumps(result, ensure_ascii=False) + "\n")


def format_mb(value):
    """MB 数值格式化。"""
    return "-" if value is None else "%.1f MB" % value


if __name__ == "__main__":
    sys.exit(main())
