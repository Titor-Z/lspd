//! TOML 后端：语法诊断、schema 驱动的补全/悬停/校验、大纲与折叠。

use lspd_core::docs::{Document, DocumentStore, Position};
use lspd_core::text::{LineIndex, Span};
use lspd_core::{Backend, Outgoing, ResponseError};
use lspd_json::parser::Value as Json;
use lspd_json::schema;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::parser::{self, Diagnostic, ParseResult};

/// LSP DiagnosticSeverity。
mod level {
    pub const ERROR: u8 = 1;
}

/// LSP SymbolKind。
const KIND_PROPERTY: u8 = 7;
const KIND_ARRAY: u8 = 18;
const KIND_OBJECT: u8 = 19;
const KIND_STRING: u8 = 15;
const KIND_NUMBER: u8 = 16;
const KIND_BOOLEAN: u8 = 17;

/// 创建 TOML 后端工厂。
pub fn backend() -> Box<dyn Backend> {
    Box::new(TomlBackend::default())
}

/// TOML 语言后端。
#[derive(Default)]
pub struct TomlBackend {
    /// 单文档缓存
    cache: Option<ParseCache>,
    schema_store: schema::SchemaStore,
}

/// 解析缓存。
struct ParseCache {
    uri: String,
    version: i32,
    parsed: ParseResult,
    schema: Option<schema::LoadedSchema>,
}

impl TomlBackend {
    /// 确保当前文档已解析。
    fn ensure(&mut self, document: &Document) {
        let fresh = self.cache.as_ref().is_some_and(|cache| {
            cache.uri == document.uri() && cache.version == document.version()
        });
        if fresh { return; }

        let parsed = parser::parse(document.text());
        let schema = schema::load_schema_with_store(document.text(), &parsed.value, document.uri(), &self.schema_store);
        self.cache = Some(ParseCache {
            uri: document.uri().to_string(),
            version: document.version(),
            parsed,
            schema,
        });
    }

    /// 发布诊断。
    fn publish(&mut self, document: &Document, out: &mut Vec<Outgoing>) {
        self.ensure(document);
        let cache = self.cache.as_ref().expect("刚写入缓存");
        let lines = LineIndex::new(document.text());

        let mut items: Vec<Value> = cache
            .parsed
            .diagnostics
            .iter()
            .map(|item| diagnostic(&lines, item))
            .collect();

        if let Some(schema) = &cache.schema {
            items.extend(schema::validate_with_store(
                document.text(),
                &cache.parsed.value,
                schema,
                &self.schema_store,
                &lines,
            ));
        }

        out.push(Outgoing::notify(
            "textDocument/publishDiagnostics",
            json!({ "uri": document.uri(), "version": document.version(), "diagnostics": items }),
        ));
    }
}

impl Backend for TomlBackend {
    fn name(&self) -> &'static str {
        "toml"
    }

    fn language_ids(&self) -> &'static [&'static str] {
        &["toml"]
    }

    fn file_extensions(&self) -> &'static [&'static str] {
        &[".toml"]
    }

    fn capabilities(&self) -> Value {
        json!({
            "completionProvider": { "triggerCharacters": [".", "\"", "'", "=", "[", " "] },
            "hoverProvider": true,
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
                self.ensure(document);
                let cache = self.cache.as_ref().expect("刚写入缓存");
                let lines = LineIndex::new(document.text());
                let result = match method {
                    "textDocument/documentSymbol" => document_symbols(&cache.parsed.value, &lines),
                    _ => folding_ranges(&cache.parsed.value, &lines),
                };
                Some(Ok(result))
            }
            "textDocument/completion" => {
                let params: PositionParams = serde_json::from_str(params?.get()).ok()?;
                let document = documents.get(&params.text_document.uri)?;
                self.ensure(document);
                let cache = self.cache.as_ref().expect("刚写入缓存");
                let offset = document.offset_at(params.position) as u32;
                let items = cache.schema.as_ref().map_or_else(Vec::new, |schema| {
                    schema::completion_with_store(document.text(), &cache.parsed.value, schema, &self.schema_store, offset)
                });
                Some(Ok(json!({ "isIncomplete": false, "items": items })))
            }
            "textDocument/hover" => {
                let params: PositionParams = serde_json::from_str(params?.get()).ok()?;
                let document = documents.get(&params.text_document.uri)?;
                self.ensure(document);
                let cache = self.cache.as_ref().expect("刚写入缓存");
                let offset = document.offset_at(params.position) as u32;
                let result = cache.schema.as_ref()
                    .and_then(|schema| schema::hover_with_store(&cache.parsed.value, schema, &self.schema_store, offset))
                    .map(|text| json!({ "contents": { "kind": "markdown", "value": text } }))
                    .unwrap_or(Value::Null);
                Some(Ok(result))
            }
            _ => None,
        }
    }

    fn shutdown(&mut self) {
        self.cache = None;
    }
}

/// 诊断转 LSP。
fn diagnostic(lines: &LineIndex<'_>, item: &Diagnostic) -> Value {
    json!({
        "range": range(lines, item.span),
        "severity": level::ERROR,
        "source": "lspd",
        "message": item.message,
    })
}

/// 字节区间转 Range。
fn range(lines: &LineIndex<'_>, span: Span) -> Value {
    let (sl, sc) = lines.position(span.start);
    let (el, ec) = lines.position(span.end);
    json!({ "start": {"line": sl, "character": sc}, "end": {"line": el, "character": ec} })
}

/// 生成文档大纲。
fn document_symbols(root: &Json, lines: &LineIndex<'_>) -> Value {
    let symbols = match root {
        Json::Object { members, .. } => members
            .iter()
            .map(|member| {
                symbol(
                    &member.key_name,
                    member.key,
                    KIND_PROPERTY,
                    &member.value,
                    lines,
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    Value::Array(symbols)
}

/// 构造符号。
fn symbol(name: &str, selection: Span, kind: u8, value: &Json, lines: &LineIndex<'_>) -> Value {
    let children: Vec<Value> = match value {
        Json::Object { members, .. } => members
            .iter()
            .map(|member| {
                symbol(
                    &member.key_name,
                    member.key,
                    KIND_PROPERTY,
                    &member.value,
                    lines,
                )
            })
            .collect(),
        Json::Array { items, .. } => items
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item, Json::Object { .. } | Json::Array { .. }))
            .map(|(index, item)| {
                symbol(&index.to_string(), item.span(), kind_of(item), item, lines)
            })
            .collect(),
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

/// 值类型对应 SymbolKind。
fn kind_of(value: &Json) -> u8 {
    match value {
        Json::Object { .. } => KIND_OBJECT,
        Json::Array { .. } => KIND_ARRAY,
        Json::String { .. } => KIND_STRING,
        Json::Number { .. } => KIND_NUMBER,
        Json::Bool { .. } => KIND_BOOLEAN,
        Json::Null { .. } => KIND_STRING,
    }
}

/// 生成折叠区间。
fn folding_ranges(root: &Json, lines: &LineIndex<'_>) -> Value {
    let mut ranges = Vec::new();
    collect_folds(root, lines, &mut ranges);
    Value::Array(ranges)
}

/// 递归收集折叠区间。
fn collect_folds(value: &Json, lines: &LineIndex<'_>, ranges: &mut Vec<Value>) {
    let (span, children) = match value {
        Json::Object { span, members } => {
            (*span, members.iter().map(|m| &m.value).collect::<Vec<_>>())
        }
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

/// 文档请求参数。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentParams {
    text_document: TextIdentifier,
}

/// 位置请求参数。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PositionParams {
    text_document: TextIdentifier,
    position: Position,
}

/// 文档标识。
#[derive(Deserialize)]
struct TextIdentifier {
    uri: String,
}
