//! TypeScript / JavaScript 语言服务。
//!
//! 实现来自 `compiler/`（源自 oxc-project/tsrs，Apache-2.0）的 `tsrs_lsp`。
//! 该实现的 LSP 层已完备，本模块只做进程装配：把标准输入输出交给它的服务器，
//! 不重复实现协议层，也不与其争夺 stdin/stdout。
//!
//! 与 HTML/CSS 分离为不同进程的原因：TypeScript 的 program 跨文件常驻且无法按需释放，
//! 独立进程边界使它可被单独重启与回收，且不影响其余语言服务。

use std::sync::Arc;
use std::time::Duration;

use tsrs_core::context::Context;
use tsrs_vfs::{bundled, osvfs, FS};

/// 启动 TypeScript / JavaScript 语言服务器，返回进程退出码。
///
/// 直接把 stdio 接入 `tsrs_lsp`，因此调用方不得再读写标准输入输出。
pub fn run() -> i32 {
    let fs: Arc<dyn FS> = Arc::new(bundled::wrap_fs(osvfs::fs()));
    let default_library_path = bundled::default_lib_path();
    let typings_location = osvfs::get_global_typings_cache_location();

    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd.to_string_lossy().into_owned(),
        Err(error) => {
            eprintln!("lspd ts: 无法获取当前目录：{error}");
            return 1;
        }
    };

    // 取消句柄交给信号线程：编辑器终止进程时让服务器收尾，而不是直接断掉。
    let (context, stop) = Context::background().with_cancel();
    spawn_signal_handler(stop);

    let server = tsrs_lsp::new_server(tsrs_lsp::ServerOptions {
        in_: tsrs_lsp::to_reader(std::io::stdin()),
        out: tsrs_lsp::to_writer(std::io::stdout()),
        err: Box::new(std::io::stderr()),
        cwd,
        fs: Some(fs),
        default_library_path,
        typings_location,
        parse_cache: None,
        npm_install: Some(Arc::new(npm_install)),
        // 不使用 content mapper（内联模板映射），无需拉起子进程
        spawn: None,
        progress_delay: Duration::from_millis(250),
        set_parent_process_id: None,
    });

    match server.run(&context) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("lspd ts: {error}");
            1
        }
    }
}

/// 依赖缺失时的自动安装回退，行为与上游 CLI 一致。
fn npm_install(cwd: &str, args: &[String]) -> Result<Vec<u8>, String> {
    let output = std::process::Command::new("npm")
        .args(args)
        .current_dir(cwd)
        .stderr(std::process::Stdio::piped())
        .output();

    match output {
        Ok(output) if output.status.success() => Ok(output.stdout),
        Ok(output) => Err(format!("exit status {}", output.status.code().unwrap_or(-1))),
        Err(error) => Err(error.to_string()),
    }
}

/// 收到中断或终止信号时取消服务器上下文。
///
/// 只用最少的平台接口（Unix 的 signal），Windows 上不注册，服务器随进程退出。
#[cfg(unix)]
fn spawn_signal_handler(stop: tsrs_core::context::CancelFunc) {
    use std::os::raw::c_int;

    unsafe extern "C" {
        fn signal(signum: c_int, handler: extern "C" fn(c_int)) -> usize;
    }

    const SIGINT: c_int = 2;
    const SIGTERM: c_int = 15;

    // 信号处理函数只能调用异步信号安全操作；这里用静态指针把取消句柄交给处理函数。
    static mut STOP: Option<tsrs_core::context::CancelFunc> = None;
    extern "C" fn on_signal(_signum: c_int) {
        // SAFETY: 句柄在注册信号前写入，之后只读取一次并复制，不再修改。
        if let Some(stop) = unsafe { (*std::ptr::addr_of!(STOP)).clone() } {
            stop.call();
        }
    }

    // SAFETY: 写入发生在注册处理函数之前，此时没有并发访问者。
    unsafe {
        STOP = Some(stop);
        signal(SIGINT, on_signal);
        signal(SIGTERM, on_signal);
    }
}

/// 非 Unix 平台不注册信号处理。
#[cfg(not(unix))]
fn spawn_signal_handler(_stop: tsrs_core::context::CancelFunc) {}
