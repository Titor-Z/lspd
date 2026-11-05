//! 语言后端契约。
//!
//! 一个后端代表一种或一族语言（如 HTML、CSS 方言族），向核心层声明自己的能力，
//! 并接收与文档、请求相关的事件。核心层不关心后端内部实现，只负责路由与时序。

use serde_json::Value;
use serde_json::value::RawValue;

use crate::docs::{Document, DocumentStore};
use crate::message::{Outgoing, ResponseError};

/// 语言后端。
///
/// 事件分两类：文档事件（已打开文档发生变化）与请求（客户端主动询问）。
/// 两类都要写入 `out`，由核心层统一发送，后端不直接接触 stdout。
pub trait Backend {
    /// 后端名称，用于日志与错误信息。
    fn name(&self) -> &'static str;

    /// 该后端负责的语言标识，例如 `["html"]`。
    fn language_ids(&self) -> &'static [&'static str];

    /// 该后端负责的文件扩展名（含点），用于处理尚未打开的文档请求。
    fn file_extensions(&self) -> &'static [&'static str];

    /// 该后端对外声明的能力，遵循 LSP 的 `ServerCapabilities` 结构。
    fn capabilities(&self) -> Value;

    /// 文档被打开或被整篇替换。
    fn did_open(&mut self, document: &Document, out: &mut Vec<Outgoing>) {
        let _ = (document, out);
    }

    /// 文档发生增量变更。
    fn did_change(&mut self, document: &Document, out: &mut Vec<Outgoing>) {
        let _ = (document, out);
    }

    /// 文档被关闭，后端应释放该文档相关的全部状态。
    fn did_close(&mut self, uri: &str, out: &mut Vec<Outgoing>) {
        let _ = (uri, out);
    }

    /// 处理客户端请求。
    ///
    /// 返回 `None` 表示该后端不处理此方法，核心层会尝试下一个后端；
    /// 返回 `Some` 即为最终应答。
    fn request(
        &mut self,
        method: &str,
        params: Option<&RawValue>,
        documents: &DocumentStore,
        out: &mut Vec<Outgoing>,
    ) -> Option<Result<Value, ResponseError>> {
        let _ = (method, params, documents, out);
        None
    }

    /// 服务端即将退出，后端可在此做收尾。
    fn shutdown(&mut self) {}
}
