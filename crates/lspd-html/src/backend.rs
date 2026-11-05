//! HTML 后端：把各能力接到核心层的 `Backend` 契约上。
//!
//! 缓存策略：只缓存"当前文档的一份语法树"，以版本号判定失效。
//! 文档关闭即丢弃，因此常驻内存与"打开了几个 HTML 文件"线性相关，
//! 不会随项目规模增长。

use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use lspd_core::docs::{Document, DocumentStore, Position};
use lspd_core::message::error_code;
use lspd_core::{Backend, Outgoing, ResponseError};

use crate::features::{View, completion, diagnostics, hover, navigation, symbols};
use crate::parser::{self, ParseResult};
use lspd_core::text::LineIndex;

/// 创建 HTML 后端的工厂函数。
pub fn backend() -> Box<dyn Backend> {
    Box::new(HtmlBackend::default())
}

/// HTML 语言后端。
#[derive(Default)]
pub struct HtmlBackend {
    /// 单文档解析缓存
    cache: Option<ParseCache>,
}

/// 一份文档的解析结果。
struct ParseCache {
    /// 文档 URI
    uri: String,
    /// 解析时的文档版本
    version: i32,
    /// 解析结果
    parsed: ParseResult,
}

impl HtmlBackend {
    /// 取当前文档的解析结果，必要时重新解析。
    fn parsed(&mut self, document: &Document) -> &ParseResult {
        let fresh = self.cache.as_ref().is_some_and(|cache| {
            cache.uri == document.uri() && cache.version == document.version()
        });

        if !fresh {
            self.cache = Some(ParseCache {
                uri: document.uri().to_string(),
                version: document.version(),
                parsed: parser::parse(document.text()),
            });
        }

        &self.cache.as_ref().expect("缓存刚写入").parsed
    }

    /// 发布一份文档的诊断。
    fn publish(&mut self, document: &Document, out: &mut Vec<Outgoing>) {
        let parsed = self.parsed(document);
        let lines = LineIndex::new(document.text());
        let view = View::new(document.text(), &parsed.ast, &lines);
        let items = diagnostics::collect(&view, &parsed.diagnostics);

        out.push(Outgoing::notify(
            "textDocument/publishDiagnostics",
            json!({
                "uri": document.uri(),
                "version": document.version(),
                "diagnostics": items,
            }),
        ));
    }
}

impl Backend for HtmlBackend {
    fn name(&self) -> &'static str {
        "html"
    }

    fn language_ids(&self) -> &'static [&'static str] {
        &["html"]
    }

    fn file_extensions(&self) -> &'static [&'static str] {
        &[".html", ".htm"]
    }

    fn capabilities(&self) -> Value {
        json!({
            "completionProvider": { "triggerCharacters": ["<", " ", "\"", "'", "=", "/"], "resolveProvider": false },
            "hoverProvider": true,
            "documentSymbolProvider": true,
            "foldingRangeProvider": true,
            "linkedEditingRangeProvider": true,
            "definitionProvider": true,
            "referencesProvider": true,
        })
    }

    fn did_open(&mut self, document: &Document, out: &mut Vec<Outgoing>) {
        self.publish(document, out);
    }

    fn did_change(&mut self, document: &Document, out: &mut Vec<Outgoing>) {
        self.publish(document, out);
    }

    fn did_close(&mut self, uri: &str, _out: &mut Vec<Outgoing>) {
        if self.cache.as_ref().is_some_and(|cache| cache.uri == uri) {
            self.cache = None;
        }
    }

    fn request(
        &mut self,
        method: &str,
        params: Option<&RawValue>,
        documents: &DocumentStore,
        out: &mut Vec<Outgoing>,
    ) -> Option<Result<Value, ResponseError>> {
        let _ = out;

        match method {
            "textDocument/hover"
            | "textDocument/completion"
            | "textDocument/definition"
            | "textDocument/references"
            | "textDocument/linkedEditingRange" => {
                let params: PositionParams = parse_params(params)?;
                let document = documents.get(&params.text_document.uri)?;
                let offset = document.offset_at(params.position) as u32;

                let parsed = self.parsed(document);
                let lines = LineIndex::new(document.text());
                let view = View::new(document.text(), &parsed.ast, &lines);

                let result = match method {
                    "textDocument/hover" => hover::hover(&view, offset)?,
                    "textDocument/completion" => completion::complete(&view, offset),
                    "textDocument/definition" => {
                        navigation::definition(&view, offset, document.uri())
                    }
                    "textDocument/references" => {
                        navigation::references(&view, offset, document.uri())
                    }
                    _ => navigation::linked_editing(&view, offset)?,
                };
                Some(Ok(result))
            }
            "textDocument/documentSymbol" | "textDocument/foldingRange" => {
                let params: DocumentParams = parse_params(params)?;
                let document = documents.get(&params.text_document.uri)?;

                let parsed = self.parsed(document);
                let lines = LineIndex::new(document.text());
                let view = View::new(document.text(), &parsed.ast, &lines);

                let result = match method {
                    "textDocument/documentSymbol" => symbols::document_symbols(&view),
                    _ => symbols::folding_ranges(&view),
                };
                Some(Ok(result))
            }
            _ => Some(Err(ResponseError {
                code: error_code::METHOD_NOT_FOUND,
                message: format!("html 后端不支持 {method}"),
                data: None,
            })),
        }
    }

    fn shutdown(&mut self) {
        self.cache = None;
    }
}

/// 解析参数；失败返回 `None`，由核心层判定为方法未处理。
fn parse_params<'a, T: Deserialize<'a>>(params: Option<&'a RawValue>) -> Option<T> {
    serde_json::from_str(params?.get()).ok()
}

/// 带位置的请求参数。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PositionParams {
    /// 文档标识
    text_document: TextIdentifier,
    /// 光标位置
    position: Position,
}

/// 只带文档标识的请求参数。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentParams {
    /// 文档标识
    text_document: TextIdentifier,
}

/// 文档标识。
#[derive(Deserialize)]
struct TextIdentifier {
    /// 文档 URI
    uri: String,
}
