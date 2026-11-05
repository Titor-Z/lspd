//! JSON 后端：语法诊断、文档大纲与折叠区间。
//!
//! 缓存只保留当前文档的一份语法树，按 URI 与版本失效，关闭文档即释放。
//! schema 驱动的补全与校验在 `schema` 侧接入（见 M5.2）。

use lspd_core::docs::{Document, DocumentStore};
use lspd_core::text::{LineIndex, Span};
use lspd_core::{Backend, Outgoing, ResponseError};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::parser::{self, Diagnostic, ParseResult, Severity, Value as Json};

/// LSP SymbolKind 取值。
const KIND_PROPERTY: u8 = 7;
const KIND_ARRAY: u8 = 18;
const KIND_OBJECT: u8 = 19;
const KIND_STRING: u8 = 15;
const KIND_NUMBER: u8 = 16;
const KIND_BOOLEAN: u8 = 17;
const KIND_NULL: u8 = 21;

/// LSP DiagnosticSeverity 取值。
mod level {
    pub const ERROR: u8 = 1;
    pub const WARNING: u8 = 2;
}

/// 创建 JSON 后端的工厂函数。
pub fn backend() -> Box<dyn Backend> {
    Box::new(JsonBackend::default())
}

/// JSON 语言后端。
#[derive(Default)]
pub struct JsonBackend {
    /// 单文档解析缓存
    cache: Option<ParseCache>,
}

/// 一份文档的解析缓存。
struct ParseCache {
    uri: String,
    version: i32,
    parsed: ParseResult,
}

impl JsonBackend {
    /// 取当前文档的解析结果，必要时重新解析。
    fn parsed(&mut self, document: &Document) -> &ParseResult {
        let fresh = self.cache.as_ref().is_some_and(|cache| cache.uri == document.uri() && cache.version == document.version());

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
        let items: Vec<Value> = parsed.diagnostics.iter().map(|item| diagnostic(&lines, item)).collect();

        out.push(Outgoing::notify(
            "textDocument/publishDiagnostics",
            json!({ "uri": document.uri(), "version": document.version(), "diagnostics": items }),
        ));
    }
}

impl Backend for JsonBackend {
    fn name(&self) -> &'static str {
        "json"
    }

    fn language_ids(&self) -> &'static [&'static str] {
        &["json"]
    }

    fn file_extensions(&self) -> &'static [&'static str] {
        &[".json"]
    }

    fn capabilities(&self) -> Value {
        json!({
            "documentSymbolProvider": true,
            "foldingRangeProvider": true,
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
        _out: &mut Vec<Outgoing>,
    ) -> Option<Result<Value, ResponseError>> {
        match method {
            "textDocument/documentSymbol" | "textDocument/foldingRange" => {
                let params: DocumentParams = serde_json::from_str(params?.get()).ok()?;
                let document = documents.get(&params.text_document.uri)?;
                let parsed = self.parsed(document);
                let lines = LineIndex::new(document.text());

                let result = match method {
                    "textDocument/documentSymbol" => document_symbols(&parsed.value, &lines),
                    _ => folding_ranges(&parsed.value, &lines),
                };
                Some(Ok(result))
            }
            _ => Some(Err(ResponseError {
                code: lspd_core::message::error_code::METHOD_NOT_FOUND,
                message: format!("json 后端不支持 {method}"),
                data: None,
            })),
        }
    }

    fn shutdown(&mut self) {
        self.cache = None;
    }
}

/// 一条诊断转为 LSP `Diagnostic`。
fn diagnostic(lines: &LineIndex<'_>, item: &Diagnostic) -> Value {
    let severity = match item.severity {
        Severity::Error => level::ERROR,
        Severity::Warning => level::WARNING,
    };

    json!({
        "range": range(lines, item.span),
        "severity": severity,
        "source": "lspd",
        "message": item.message,
    })
}

/// 字节区间转为 LSP `Range`。
fn range(lines: &LineIndex<'_>, span: Span) -> Value {
    let (start_line, start_char) = lines.position(span.start);
    let (end_line, end_char) = lines.position(span.end);
    json!({
        "start": { "line": start_line, "character": start_char },
        "end": { "line": end_line, "character": end_char },
    })
}

/// 生成文档大纲：根为对象/数组的成员，标量根给单个符号。
fn document_symbols(root: &Json, lines: &LineIndex<'_>) -> Value {
    let symbols: Vec<Value> = match root {
        Json::Object { members, .. } => {
            members.iter().map(|member| symbol(&member.key_name, member.key, KIND_PROPERTY, &member.value, lines)).collect()
        }
        Json::Array { items, .. } => container_symbols(items, lines),
        scalar => vec![symbol("<value>", scalar.span(), kind_of(scalar), scalar, lines)],
    };
    Value::Array(symbols)
}

/// 构造一个符号，递归携带子节点。
fn symbol(name: &str, selection: Span, kind: u8, value: &Json, lines: &LineIndex<'_>) -> Value {
    let children: Vec<Value> = match value {
        Json::Object { members, .. } => {
            members.iter().map(|member| symbol(&member.key_name, member.key, KIND_PROPERTY, &member.value, lines)).collect()
        }
        Json::Array { items, .. } => container_symbols(items, lines),
        _ => Vec::new(),
    };

    json!({
        "name": name,
        "kind": kind,
        "range": range(lines, value.span()),
        "selectionRange": range(lines, selection),
        "children": children,
    })
}

/// 数组中的容器元素（对象/数组）转为符号，标量元素不占大纲。
fn container_symbols(items: &[Json], lines: &LineIndex<'_>) -> Vec<Value> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| matches!(item, Json::Object { .. } | Json::Array { .. }))
        .map(|(index, item)| symbol(&index.to_string(), item.span(), kind_of(item), item, lines))
        .collect()
}

/// 值类型对应的 SymbolKind。
fn kind_of(value: &Json) -> u8 {
    match value {
        Json::Object { .. } => KIND_OBJECT,
        Json::Array { .. } => KIND_ARRAY,
        Json::String { .. } => KIND_STRING,
        Json::Number { .. } => KIND_NUMBER,
        Json::Bool { .. } => KIND_BOOLEAN,
        Json::Null { .. } => KIND_NULL,
    }
}

/// 生成折叠区间：跨行的对象/数组按区域折叠。
fn folding_ranges(root: &Json, lines: &LineIndex<'_>) -> Value {
    let mut ranges = Vec::new();
    collect_folds(root, lines, &mut ranges);
    Value::Array(ranges)
}

/// 递归收集容器节点的折叠区间。
fn collect_folds(value: &Json, lines: &LineIndex<'_>, ranges: &mut Vec<Value>) {
    let (span, children) = match value {
        Json::Object { span, members } => (*span, members.iter().map(|member| &member.value).collect::<Vec<_>>()),
        Json::Array { span, items } => (*span, items.iter().collect::<Vec<_>>()),
        _ => return,
    };

    let start = lines.position(span.start).0;
    let end = lines.position(span.end).0;
    if end > start {
        ranges.push(json!({ "startLine": start, "endLine": end, "kind": "region" }));
    }

    for child in children {
        collect_folds(child, lines, ranges);
    }
}

/// 只带文档标识的参数。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentParams {
    text_document: TextIdentifier,
}

/// 文档标识。
#[derive(Deserialize)]
struct TextIdentifier {
    uri: String,
}
