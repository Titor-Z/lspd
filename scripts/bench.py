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
import statistics
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


class StderrTee(threading.Thread):
    """把服务器 stderr 原样写入日志文件，供 TSRS_REGION_LOG 等诊断输出落盘。"""

    def __init__(self, stream, path):
        super().__init__(daemon=True)
        self.stream = stream
        self.path = path

    def run(self):
        with open(self.path, "w", encoding="utf-8", errors="replace") as handle:
            while True:
                line = self.stream.readline()
                if not line:
                    return
                handle.write(line.decode("utf-8", "replace"))
                handle.flush()


class Server:
    """被测服务器进程的封装：消息收发与 RSS 采样。"""

    def __init__(self, command, cwd, env, stderr_log=None):
        self.process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            # 需要采集日志时保留 stderr，否则直接丢弃，避免管道写满阻塞服务器
            stderr=subprocess.PIPE if stderr_log else subprocess.DEVNULL,
            cwd=cwd,
            env=env,
        )
        self.reader = FrameReader(self.process.stdout)
        self.reader.start()
        self.stderr_thread = None
        if stderr_log:
            self.stderr_thread = StderrTee(self.process.stderr, stderr_log)
            self.stderr_thread.start()
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
            # 服务器反向请求（workspace/configuration、registerCapability 等）必须应答，
            # 否则 initialized 里取配置会被取消，整个会话不会真正建立。
            if "id" in message and message.get("method"):
                self.send(
                    {
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "result": {} if message["method"] == "workspace/configuration" else None,
                    }
                )
                continue
            if message.get("id") == request_id:
                return message, time.monotonic() - started
        return None, None

    def notify(self, method, params):
        """发送通知，不等待应答。"""
        self.send({"jsonrpc": "2.0", "method": method, "params": params})

    def drain(self, seconds):
        """在指定时间内消化服务器主动发来的消息，顺带应答其中的反向请求。"""
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                message = self.reader.messages.get(timeout=max(0.01, deadline - time.monotonic()))
            except queue.Empty:
                return
            if message is None:
                return
            if "id" in message and message.get("method"):
                self.send(
                    {
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "result": {} if message["method"] == "workspace/configuration" else None,
                    }
                )

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


def build_initialize_params(root, init_options=None):
    """构造 initialize 参数：以单根工作区打开被测项目。

    init_options 非空时写入 initializationOptions，用于对比服务器在不同客户端
    配置下的行为（例如 disablePushDiagnostics）。
    """
    root_uri = path_to_uri(root)
    params = {
        "processId": os.getpid(),
        "clientInfo": {"name": "lspd-bench"},
        "rootUri": root_uri,
        "workspaceFolders": [{"uri": root_uri, "name": os.path.basename(root)}],
        "capabilities": {
            "general": {"positionEncodings": ["utf-16"]},
            "textDocument": {
                "synchronization": {"didSave": False},
                "publishDiagnostics": {"versionSupport": True},
                # 声明拉取式诊断能力，与真实编辑器保持一致
                "diagnostic": {},
            },
            "workspace": {"workspaceFolders": True},
        },
    }
    if init_options is not None:
        params["initializationOptions"] = init_options
    return params


def path_to_uri(path):
    """把本地路径转成 file:// URI，保留中文与空格。"""
    from urllib.parse import quote

    absolute = os.path.abspath(path)
    return "file://" + quote(absolute)


def format_kb(value):
    """把 KB 格式化成 MB，便于阅读。"""
    return "-" if value is None else "%.1f MB" % (value / 1024.0)


STEP_PROBE_METHOD = {
    "hover": "textDocument/hover",
    "completion": "textDocument/completion",
    "diagnostic": "textDocument/diagnostic",
}


def step_probe_params(kind, path, text):
    """构造每步探针的参数：诊断只需文档标识，其余需要位置。"""
    uri = path_to_uri(path)
    if kind == "diagnostic":
        return {"textDocument": {"uri": uri}}
    return {
        "textDocument": {"uri": uri},
        "position": {"line": max(0, text.count("\n") // 2), "character": 0},
    }


def report_steps(steps, csv_path):
    """打印阶梯曲线与增量统计，并把原始数据写成 CSV。

    三个汇总数各自回答一个问题：
    - 首个增量：首次打开是否包含一次性开销（lib.d.ts、checker 池）
    - 中位增量：日常每多开一个文件的成本
    - 末尾中位：增长是否在收敛，还是持续线性
    """
    print("\n  阶梯采样（每打开一个文件后的 RSS）")
    print("    %-4s %-28s %9s %9s" % ("#", "文件", "RSS(MiB)", "增量"))
    previous = None
    deltas = []
    for step in steps:
        delta = "" if previous is None else "%+8.2f" % (step["rss_mb"] - previous)
        if previous is not None:
            deltas.append(step["rss_mb"] - previous)
        print("    %-4d %-28s %9.2f %9s" % (step["index"], step["file"], step["rss_mb"], delta))
        previous = step["rss_mb"]

    if deltas:
        tail = deltas[-10:] if len(deltas) >= 10 else deltas
        monotonic = all(value > -0.5 for value in deltas)
        print("\n  首文件增量            : %+.2f MiB" % deltas[0])
        print("  中位增量              : %+.2f MiB" % statistics.median(deltas))
        print("  末 10 个中位增量      : %+.2f MiB" % statistics.median(tail))
        print("  是否单调增长          : %s" % ("是" if monotonic else "否"))
        print("  总计增长              : %+.2f MiB" % (steps[-1]["rss_mb"] - steps[0]["rss_mb"]))

    if csv_path:
        with open(csv_path, "w", encoding="utf-8") as handle:
            handle.write("index,file,rss_mb,elapsed_ms\n")
            for step in steps:
                handle.write(
                    "%d,%s,%.2f,%.1f\n"
                    % (step["index"], step["file"], step["rss_mb"], step["elapsed_ms"])
                )
        print("\n  CSV 已写入 %s" % csv_path)


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
    parser.add_argument(
        "--step",
        action="store_true",
        help="逐个 didOpen，每打开一个采样一次 RSS，输出阶梯曲线，用于定位每文件增量",
    )
    parser.add_argument(
        "--step-csv",
        default="",
        help="把阶梯数据写入该 CSV：index,file,rss_mb,elapsed_ms",
    )
    parser.add_argument(
        "--init-options",
        default="",
        help='传给 initialize 的 initializationOptions，JSON 字符串，例如 {"disablePushDiagnostics":true}',
    )
    parser.add_argument(
        "--stderr-log",
        default="",
        help="把服务器 stderr 落盘，配合 TSRS_REGION_LOG=1 采集 program 生死日志",
    )
    parser.add_argument(
        "--step-probe",
        choices=["none", "hover", "completion", "diagnostic"],
        default="none",
        help="--step 模式下每打开一个文件后发的请求；none 只打开不请求，用来测纯惰性路径",
    )
    parser.add_argument(
        "--step-wait",
        type=float,
        default=0.6,
        help="--step 模式下每打开一个文件后的等待秒数，需大于服务器的诊断防抖（默认 0.6）",
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

    init_options = None
    if args.init_options:
        try:
            init_options = json.loads(args.init_options)
        except json.JSONDecodeError as error:
            parser.error("--init-options 不是合法 JSON：%s" % error)

    env = dict(os.environ)
    env.setdefault("RUST_BACKTRACE", "1")

    server = Server(command, args.root, env, stderr_log=args.stderr_log or None)
    sampler = RssSampler(server)
    sampler.start()

    result = {"name": args.name, "command": command, "root": os.path.abspath(args.root),
              "files": len(files), "opened_bytes": 0}

    response, roundtrip = server.request(
        "initialize", build_initialize_params(args.root, init_options), args.timeout
    )
    result["initialize_ms"] = None if roundtrip is None else round(roundtrip * 1000, 1)
    if response is None:
        result["status"] = "initialize 无响应"
        result["idle_mb"] = round((server.rss_kb() or 0) / 1024.0, 1)
        server.close()
        report(result, args.json_out)
        return 0

    result["status"] = "ok"
    server.notify("initialized", {})
    server.drain(args.settle)
    result["idle_mb"] = round((server.rss_kb() or 0) / 1024.0, 1)

    opened_bytes = 0
    steps = []
    for index, path in enumerate(files, start=1):
        text = read_text(path)
        opened_bytes += len(text.encode("utf-8"))
        started = time.monotonic()
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
        if args.step:
            # 真实编辑器打开文件后会发请求；这里可按需模拟，以触发 program 构建与类型检查
            if args.step_probe != "none":
                server.request(
                    STEP_PROBE_METHOD[args.step_probe],
                    step_probe_params(args.step_probe, path, text),
                    args.timeout,
                )
            # 每打开一个就等一小段再采样，让防抖的诊断与 program 更新跑完
            server.drain(args.step_wait)
            steps.append(
                {
                    "index": index,
                    "file": os.path.basename(path),
                    "rss_mb": round((server.rss_kb() or 0) / 1024.0, 2),
                    "elapsed_ms": round((time.monotonic() - started) * 1000, 1),
                }
            )
    result["opened_bytes"] = opened_bytes
    if steps:
        result["steps"] = steps
        report_steps(steps, args.step_csv)

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

    server.drain(args.settle)
    result["open_mb"] = round((server.rss_kb() or 0) / 1024.0, 1)
    result["peak_mb"] = round(sampler.peak_kb / 1024.0, 1)

    # 关闭全部文档：program 只有在没有打开的文件时才可能收缩。
    if args.close_all:
        for path in files:
            server.notify(
                "textDocument/didClose",
                {"textDocument": {"uri": path_to_uri(path)}},
            )
        server.drain(args.settle)
        result["closed_mb"] = round((server.rss_kb() or 0) / 1024.0, 1)

    # 静置后重新采样：验证服务器在空闲时是否把已释放的内存归还给操作系统。
    if args.release_wait > 0:
        server.drain(args.release_wait)
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
