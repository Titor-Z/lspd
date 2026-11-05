//! 语言服务器通用层。
//!
//! 职责边界：
//! - 传输：LSP over stdio 的 Content-Length 分帧，单线程事件循环，不引入异步运行时。
//! - 文档：正文只保留一份，增量编辑与行列换算在此完成。
//! - 路由：按 languageId 与文件扩展名把请求分发给对应的语言后端。
//! - 能力：把多个后端声明的 capabilities 合并成一份对外声明。

pub mod backend;
pub mod docs;
pub mod jsonrpc;
pub mod message;
pub mod router;
pub mod text;

use std::io;

pub use backend::Backend;
pub use message::{Id, Outgoing, ResponseError};
pub use router::Server;

use jsonrpc::{MessageReader, MessageWriter};
use router::Action;

/// 以标准输入输出运行语言服务器，返回进程退出码。
///
/// 事件循环保持单线程：读一条、处理一条、写完再读下一条。
/// LSP 的请求粒度足够小，单线程即可满足响应性，且省掉线程栈与同步开销。
pub fn run(backends: Vec<Box<dyn Backend>>) -> i32 {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = MessageReader::new(stdin.lock());
    let mut writer = MessageWriter::new(stdout.lock());
    let mut server = Server::new(backends);
    let mut pending: Vec<Outgoing> = Vec::new();

    loop {
        // 这一块持有读取缓冲的借用，处理完立即释放，供下一轮复用
        let action = {
            let body = match reader.next_message() {
                Ok(Some(body)) => body,
                Ok(None) => break,
                Err(error) => {
                    eprintln!("lspd: 读取消息失败：{error}");
                    break;
                }
            };

            match jsonrpc::parse(body) {
                Ok(message) => server.handle(message, &mut pending),
                Err(error) => {
                    // 无法解析的正文也拿不到请求标识，按协议只能记录后继续
                    eprintln!("lspd: 消息解析失败：{error}");
                    Action::Continue
                }
            }
        };

        for message in pending.drain(..) {
            if let Err(error) = writer.send(&message) {
                eprintln!("lspd: 写出消息失败：{error}");
                return 1;
            }
        }

        if let Action::Exit(code) = action {
            return code;
        }
    }

    server.exit_code_on_eof()
}
