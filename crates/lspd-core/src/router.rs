//! 服务器主体：生命周期、消息路由与能力合并。
//!
//! 核心层独占 stdin/stdout，后端只往待发队列里写消息，因此不需要锁，
//! 也不需要处理并发写入。事件循环是单线程的：一条消息处理完再处理后一条。

use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

use crate::backend::Backend;
use crate::docs::{ContentChange, DocumentStore, TextDocumentItem};
use crate::jsonrpc::Message;
use crate::message::{Id, Outgoing, error_code};

/// 服务器生命周期。
///
/// 协议要求：`initialize` 之前不接受任何请求，`shutdown` 之后不再处理业务请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    /// 尚未收到 initialize
    Uninitialized,
    /// 正常工作
    Running,
    /// 已收到 shutdown，等待 exit
    ShuttingDown,
}

/// 单条消息处理后的走向。
pub enum Action {
    /// 继续读下一条消息
    Continue,
    /// 退出进程，携带退出码
    Exit(i32),
}

/// 语言服务器。
pub struct Server {
    /// 已装载的语言后端，顺序即路由优先级
    backends: Vec<Box<dyn Backend>>,
    /// 已打开文档
    documents: DocumentStore,
    /// 生命周期状态
    lifecycle: Lifecycle,
    /// 工作区根 URI，供后端按需使用
    root_uri: Option<String>,
}

impl Server {
    /// 用给定的后端集合构造服务器。
    pub fn new(backends: Vec<Box<dyn Backend>>) -> Self {
        Self {
            backends,
            documents: DocumentStore::default(),
            lifecycle: Lifecycle::Uninitialized,
            root_uri: None,
        }
    }

    /// 工作区根 URI。
    pub fn root_uri(&self) -> Option<&str> {
        self.root_uri.as_deref()
    }

    /// 处理一条入站消息，把待发消息追加到 `out`。
    pub fn handle(&mut self, message: Message<'_>, out: &mut Vec<Outgoing>) -> Action {
        match message {
            // 当前不向客户端发起请求，应答静默丢弃
            Message::Response { .. } => Action::Continue,
            Message::Notification { method, params } => self.on_notification(method, params, out),
            Message::Request { id, method, params } => {
                self.on_request(id, method, params, out);
                Action::Continue
            }
        }
    }

    /// 输入流结束时的退出码：未收到 shutdown 视为异常退出。
    pub fn exit_code_on_eof(&self) -> i32 {
        match self.lifecycle {
            Lifecycle::ShuttingDown => 0,
            _ => 1,
        }
    }

    /// 处理通知。
    fn on_notification(
        &mut self,
        method: &str,
        params: Option<&RawValue>,
        out: &mut Vec<Outgoing>,
    ) -> Action {
        match method {
            "exit" => return Action::Exit(self.exit_code_on_eof()),
            "textDocument/didOpen" => self.on_did_open(params, out),
            "textDocument/didChange" => self.on_did_change(params, out),
            "textDocument/didClose" => self.on_did_close(params, out),
            // initialized / $/cancelRequest / $/setTrace 等：单线程事件循环下无需处理
            _ => {}
        }
        Action::Continue
    }

    /// 处理请求。
    fn on_request(
        &mut self,
        id: Id,
        method: &str,
        params: Option<&RawValue>,
        out: &mut Vec<Outgoing>,
    ) {
        match (self.lifecycle, method) {
            (Lifecycle::Uninitialized, "initialize") => self.on_initialize(id, params, out),
            (Lifecycle::Uninitialized, _) => out.push(Outgoing::err(
                id,
                error_code::SERVER_NOT_INITIALIZED,
                "服务器尚未完成 initialize",
            )),
            (Lifecycle::Running, "initialize") => out.push(Outgoing::err(
                id,
                error_code::INVALID_REQUEST,
                "initialize 只能调用一次",
            )),
            (Lifecycle::Running, "shutdown") => {
                self.lifecycle = Lifecycle::ShuttingDown;
                for backend in &mut self.backends {
                    backend.shutdown();
                }
                out.push(Outgoing::ok(id, Value::Null));
            }
            (Lifecycle::ShuttingDown, _) => out.push(Outgoing::err(
                id,
                error_code::INVALID_REQUEST,
                "服务器正在关闭，不再处理请求",
            )),
            (Lifecycle::Running, method) => self.dispatch(id, method, params, out),
        }
    }

    /// 处理 initialize：记录工作区根，回告能力。
    fn on_initialize(&mut self, id: Id, params: Option<&RawValue>, out: &mut Vec<Outgoing>) {
        self.root_uri = parse_params::<InitializeParams>(params).and_then(|params| params.root());
        self.lifecycle = Lifecycle::Running;

        let result = json!({
            "capabilities": self.capabilities(),
            "serverInfo": { "name": "lspd", "version": env!("CARGO_PKG_VERSION") },
            // 位置编码固定 UTF-16：协议规定客户端必须支持，无需协商分支
            "positionEncoding": "utf-16",
        });
        out.push(Outgoing::ok(id, result));
    }

    /// 合并所有后端声明的能力。
    fn capabilities(&self) -> Value {
        let mut merged = Map::new();
        // 文本同步由核心层承担，因此无论后端数量都固定声明增量同步
        merged.insert(
            "textDocumentSync".to_string(),
            json!({ "openClose": true, "change": 2 }),
        );
        for backend in &self.backends {
            merge_capabilities(&mut merged, backend.capabilities());
        }
        Value::Object(merged)
    }

    /// 把请求分发给首个愿意处理的后端。
    fn dispatch(
        &mut self,
        id: Id,
        method: &str,
        params: Option<&RawValue>,
        out: &mut Vec<Outgoing>,
    ) {
        let targets = self.targets(params);

        for index in targets {
            let handled = self.backends[index].request(method, params, &self.documents, out);
            if let Some(result) = handled {
                out.push(match result {
                    Ok(value) => Outgoing::ok(id, value),
                    Err(error) => Outgoing::Response {
                        id,
                        result: Err(error),
                    },
                });
                return;
            }
        }

        out.push(Outgoing::err(
            id,
            error_code::METHOD_NOT_FOUND,
            format!("不支持的方法 {method}"),
        ));
    }

    /// 依据参数里的文档 URI 决定候选后端；没有 URI 时按注册顺序全部尝试。
    fn targets(&self, params: Option<&RawValue>) -> Vec<usize> {
        let Some(document) = parse_params::<DocumentOnlyParams>(params) else {
            return (0..self.backends.len()).collect();
        };
        let uri = &document.text_document.uri;

        // 已打开的文档按其语言标识路由，未打开的按扩展名兜底
        if let Some(document) = self.documents.get(uri)
            && let Some(index) = self.backend_by_language(document.language_id())
        {
            return vec![index];
        }

        self.backend_by_extension(uri)
            .map_or_else(|| (0..self.backends.len()).collect(), |index| vec![index])
    }

    /// 按语言标识查找后端。
    fn backend_by_language(&self, language_id: &str) -> Option<usize> {
        self.backends
            .iter()
            .position(|backend| backend.language_ids().contains(&language_id))
    }

    /// 按文件扩展名查找后端。
    fn backend_by_extension(&self, uri: &str) -> Option<usize> {
        let extension = std::path::Path::new(uri).extension()?.to_str()?;
        let dotted = format!(".{}", extension.to_ascii_lowercase());
        self.backends
            .iter()
            .position(|backend| backend.file_extensions().iter().any(|item| *item == dotted))
    }

    /// 打开文档并通知后端。
    fn on_did_open(&mut self, params: Option<&RawValue>, out: &mut Vec<Outgoing>) {
        let Some(params) = parse_params::<DidOpenParams>(params) else {
            return;
        };

        let language_id = params.text_document.language_id.clone();
        let uri = params.text_document.uri.clone();
        self.documents.open(params.text_document);

        if let Some(index) = self.backend_by_language(&language_id)
            && let Some(document) = self.documents.get(&uri)
        {
            let backend = &mut self.backends[index];
            backend.did_open(document, out);
        }
    }

    /// 应用增量变更并通知后端。
    fn on_did_change(&mut self, params: Option<&RawValue>, out: &mut Vec<Outgoing>) {
        let Some(params) = parse_params::<DidChangeParams>(params) else {
            return;
        };
        let uri = params.text_document.uri.clone();

        if let Some(document) = self.documents.get_mut(&uri) {
            for change in params.content_changes {
                document.apply_change(change);
            }
        }

        let Some(document) = self.documents.get(&uri) else {
            return;
        };
        let Some(index) = self.backend_by_language(document.language_id()) else {
            return;
        };
        let backend = &mut self.backends[index];
        backend.did_change(document, out);
    }

    /// 关闭文档并通知后端释放状态。
    fn on_did_close(&mut self, params: Option<&RawValue>, out: &mut Vec<Outgoing>) {
        let Some(params) = parse_params::<DocumentOnlyParams>(params) else {
            return;
        };
        let uri = params.text_document.uri;

        let Some(index) = self
            .documents
            .get(&uri)
            .and_then(|document| self.backend_by_language(document.language_id()))
        else {
            self.documents.close(&uri);
            return;
        };

        self.backends[index].did_close(&uri, out);
        self.documents.close(&uri);
    }
}

/// 把后端的 capabilities 合并进目标对象。
///
/// 合并规则：对象递归合并、数组取并集、布尔取或、其余保留先声明者。
fn merge_capabilities(target: &mut Map<String, Value>, source: Value) {
    let Value::Object(source) = source else {
        return;
    };

    for (key, incoming) in source {
        match (target.get_mut(&key), incoming) {
            (Some(Value::Object(existing)), Value::Object(incoming)) => {
                merge_capabilities(existing, Value::Object(incoming));
            }
            (Some(Value::Array(existing)), Value::Array(incoming)) => {
                for item in incoming {
                    if !existing.contains(&item) {
                        existing.push(item);
                    }
                }
            }
            (Some(Value::Bool(existing)), Value::Bool(incoming)) => {
                *existing = *existing || incoming
            }
            (Some(_), _) => {}
            (None, incoming) => {
                target.insert(key, incoming);
            }
        }
    }
}

/// 把原始参数解析成具体类型，失败返回 `None`。
fn parse_params<'a, T: Deserialize<'a>>(params: Option<&'a RawValue>) -> Option<T> {
    serde_json::from_str(params?.get()).ok()
}

/// initialize 参数中本层关心的字段。
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct InitializeParams {
    /// 工作区根 URI
    #[serde(default)]
    root_uri: Option<String>,
    /// 多根工作区
    #[serde(default)]
    workspace_folders: Option<Vec<WorkspaceFolder>>,
}

impl InitializeParams {
    /// 取工作区根：优先 rootUri，其次第一个工作区目录。
    fn root(&self) -> Option<String> {
        self.root_uri.clone().or_else(|| {
            self.workspace_folders
                .as_ref()?
                .first()
                .map(|folder| folder.uri.clone())
        })
    }
}

/// 工作区目录。
#[derive(Deserialize)]
struct WorkspaceFolder {
    /// 目录 URI
    uri: String,
}

/// 只带文档标识的参数，多数请求与通知共用。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentOnlyParams {
    /// 文档标识
    text_document: TextIdentifier,
}

/// 文档标识。
#[derive(Deserialize)]
struct TextIdentifier {
    /// 文档 URI
    uri: String,
}

/// didOpen 参数。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DidOpenParams {
    /// 文档初始内容
    text_document: TextDocumentItem,
}

/// didChange 参数。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DidChangeParams {
    /// 文档标识
    text_document: VersionedIdentifier,
    /// 内容变更集合
    content_changes: Vec<ContentChange>,
}

/// 带版本的文档标识。
#[derive(Deserialize)]
struct VersionedIdentifier {
    /// 文档 URI
    uri: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ResponseError;

    /// 测试用后端：记录收到的文档事件，并对指定方法给出固定应答。
    struct EchoBackend {
        /// 负责的语言标识
        languages: &'static [&'static str],
        /// 已打开文档的 URI 列表
        opened: Vec<String>,
    }

    impl Backend for EchoBackend {
        fn name(&self) -> &'static str {
            "echo"
        }

        fn language_ids(&self) -> &'static [&'static str] {
            self.languages
        }

        fn file_extensions(&self) -> &'static [&'static str] {
            &[".html"]
        }

        fn capabilities(&self) -> Value {
            json!({ "documentSymbolProvider": true })
        }

        fn did_open(&mut self, document: &crate::docs::Document, _out: &mut Vec<Outgoing>) {
            self.opened.push(document.uri().to_string());
        }

        fn request(
            &mut self,
            method: &str,
            _params: Option<&RawValue>,
            _documents: &DocumentStore,
            _out: &mut Vec<Outgoing>,
        ) -> Option<Result<Value, ResponseError>> {
            match method {
                "textDocument/documentSymbol" => Some(Ok(json!([]))),
                _ => None,
            }
        }
    }

    /// 构造一条 JSON 正文并解析成消息。
    fn message(text: &str) -> Message<'_> {
        crate::jsonrpc::parse(text.as_bytes()).unwrap()
    }

    /// initialize 之前发请求应收到 -32002。
    #[test]
    fn rejects_request_before_initialize() {
        let mut server = Server::new(vec![]);
        let mut out = Vec::new();
        server.handle(
            message(r#"{"jsonrpc":"2.0","id":1,"method":"textDocument/hover"}"#),
            &mut out,
        );

        let Outgoing::Response {
            result: Err(error), ..
        } = &out[0]
        else {
            panic!("应为错误应答");
        };
        assert_eq!(error.code, error_code::SERVER_NOT_INITIALIZED);
    }

    /// initialize 应答应包含同步能力与后端声明的能力。
    #[test]
    fn initialize_merges_capabilities() {
        let backend = EchoBackend {
            languages: &["html"],
            opened: Vec::new(),
        };
        let mut server = Server::new(vec![Box::new(backend)]);
        let mut out = Vec::new();
        server.handle(
            message(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"rootUri":"file:///p"}}"#),
            &mut out,
        );

        let Outgoing::Response {
            result: Ok(value), ..
        } = &out[0]
        else {
            panic!("应为成功应答");
        };
        let capabilities = &value["capabilities"];
        assert_eq!(capabilities["documentSymbolProvider"], json!(true));
        assert_eq!(capabilities["textDocumentSync"]["change"], json!(2));
        assert_eq!(server.root_uri(), Some("file:///p"));
    }

    /// didOpen 应按 languageId 路由到对应后端，并进入文档存储。
    #[test]
    fn routes_open_to_backend() {
        let backend = EchoBackend {
            languages: &["html"],
            opened: Vec::new(),
        };
        let mut server = Server::new(vec![Box::new(backend)]);
        let mut out = Vec::new();
        server.handle(
            message(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#),
            &mut out,
        );
        server.handle(
            message(
                r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///a.html","languageId":"html","version":1,"text":"<p>你好</p>"}}}"#,
            ),
            &mut out,
        );

        assert_eq!(server.documents.len(), 1);
        assert_eq!(
            server.documents.get("file:///a.html").unwrap().text(),
            "<p>你好</p>"
        );
    }

    /// 请求应路由到能处理的后端；无人处理时报方法不存在。
    #[test]
    fn dispatches_request_or_reports_missing() {
        let backend = EchoBackend {
            languages: &["html"],
            opened: Vec::new(),
        };
        let mut server = Server::new(vec![Box::new(backend)]);
        let mut out = Vec::new();
        server.handle(
            message(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#),
            &mut out,
        );
        out.clear();

        server.handle(
            message(
                r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/documentSymbol","params":{"textDocument":{"uri":"file:///a.html"}}}"#,
            ),
            &mut out,
        );
        assert!(matches!(&out[0], Outgoing::Response { result: Ok(_), .. }));

        out.clear();
        server.handle(
            message(
                r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/unknown","params":{"textDocument":{"uri":"file:///a.html"}}}"#,
            ),
            &mut out,
        );
        let Outgoing::Response {
            result: Err(error), ..
        } = &out[0]
        else {
            panic!("应为错误应答");
        };
        assert_eq!(error.code, error_code::METHOD_NOT_FOUND);
    }

    /// 未收到 shutdown 就收到 exit，退出码为 1。
    #[test]
    fn exits_with_error_without_shutdown() {
        let mut server = Server::new(vec![]);
        let mut out = Vec::new();
        let action = server.handle(message(r#"{"jsonrpc":"2.0","method":"exit"}"#), &mut out);
        assert!(matches!(action, Action::Exit(1)));
    }
}
