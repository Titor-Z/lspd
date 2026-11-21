//! XML 后端：接入核心层 `Backend` 契约。

use lspd_core::docs::{Document, DocumentStore, Position};
use lspd_core::text::LineIndex;
use lspd_core::{Backend, Outgoing, ResponseError};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{json, Value};

use crate::features::{self, View, symbols};
use crate::hooks::{HooksAdapter, NoHooks, XmlHooks};
use crate::parser::{self, ParseResult, Severity};
use crate::schema::SchemaProvider;

/// 创建 XML 后端。
pub fn backend() -> Box<dyn Backend> {
    backend_with("xml", &["xml"], &[".xml", ".xsl", ".xslt", ".xsd", ".svgz"])
}

/// 创建带自定义语言标识的 XML 后端。
pub fn backend_with(
    name: &'static str,
    language_ids: &'static [&'static str],
    file_extensions: &'static [&'static str],
) -> Box<dyn Backend> {
    backend_with_hooks(name, language_ids, file_extensions, NoHooks)
}

/// 创建带方言钩子的 XML 后端。
pub fn backend_with_hooks(
    name: &'static str,
    language_ids: &'static [&'static str],
    file_extensions: &'static [&'static str],
    hooks: impl XmlHooks + 'static,
) -> Box<dyn Backend> {
    backend_with_schema(name, language_ids, file_extensions, HooksAdapter(hooks))
}

/// 创建带 SchemaProvider 的 XML 后端。
pub fn backend_with_schema(
    name: &'static str,
    language_ids: &'static [&'static str],
    file_extensions: &'static [&'static str],
    schema: impl SchemaProvider + 'static,
) -> Box<dyn Backend> {
    Box::new(XmlBackend { name, language_ids, file_extensions, schema: Box::new(schema), cache: None })
}

/// XML 后端。
struct XmlBackend {
    name: &'static str,
    language_ids: &'static [&'static str],
    file_extensions: &'static [&'static str],
    schema: Box<dyn SchemaProvider>,
    cache: Option<Cache>,
}

/// 单文档缓存。
struct Cache {
    uri: String,
    version: i32,
    parsed: ParseResult,
}

impl XmlBackend {
    fn ensure_cache(&mut self, document: &Document) {
        let fresh = self.cache.as_ref().is_some_and(|c| c.uri == document.uri() && c.version == document.version());
        if !fresh {
            self.cache = Some(Cache { uri: document.uri().to_string(), version: document.version(), parsed: parser::parse(document.text()) });
        }
    }

    fn publish(&mut self, document: &Document, out: &mut Vec<Outgoing>) {
        self.ensure_cache(document);
        let cache = self.cache.as_ref().expect("缓存刚写入");
        let lines = LineIndex::new(document.text());
        let view = View::new(document.text(), &cache.parsed.ast, &lines, self.schema.as_ref());
        let diagnostics = cache.parsed.diagnostics.iter().map(|d| {
            let severity = match d.severity { Severity::Error => 1u8 };
            features::diagnostic(&view, d.span, severity, d.message.clone())
        }).collect::<Vec<_>>();
        out.push(Outgoing::notify(
            "textDocument/publishDiagnostics",
            json!({ "uri": document.uri(), "version": document.version(), "diagnostics": diagnostics }),
        ));
    }
}

impl Backend for XmlBackend {
    fn name(&self) -> &'static str { self.name }
    fn language_ids(&self) -> &'static [&'static str] { self.language_ids }
    fn file_extensions(&self) -> &'static [&'static str] { self.file_extensions }

    fn capabilities(&self) -> Value {
        json!({
            "hoverProvider": true,
            "completionProvider": { "triggerCharacters": ["<", " ", "\"", "'", "=", "/"] },
            "documentSymbolProvider": true,
            "foldingRangeProvider": true,
            "documentFormattingProvider": true,
        })
    }

    fn did_open(&mut self, document: &Document, out: &mut Vec<Outgoing>) { self.publish(document, out); }
    fn did_change(&mut self, document: &Document, out: &mut Vec<Outgoing>) { self.publish(document, out); }
    fn did_close(&mut self, uri: &str, _out: &mut Vec<Outgoing>) {
        if self.cache.as_ref().is_some_and(|c| c.uri == uri) { self.cache = None; }
    }

    fn request(
        &mut self,
        method: &str,
        params: Option<&RawValue>,
        documents: &DocumentStore,
        _out: &mut Vec<Outgoing>,
    ) -> Option<Result<Value, ResponseError>> {
        match method {
            "textDocument/documentSymbol" | "textDocument/foldingRange" | "textDocument/formatting" => {
                let params: FormatParams = serde_json::from_str(params?.get()).ok()?;
                let document = documents.get(&params.text_document.uri)?;
                self.ensure_cache(document);
                let cache = self.cache.as_ref().expect("缓存刚写入");
                let lines = LineIndex::new(document.text());
                let view = View::new(document.text(), &cache.parsed.ast, &lines, self.schema.as_ref());
                let result = match method {
                    "textDocument/documentSymbol" => symbols::document_symbols(&view),
                    "textDocument/foldingRange" => symbols::folding_ranges(&view),
                    _ => {
                        let tab_size = params.options.as_ref().and_then(|o| o.tab_size).unwrap_or(2);
                        let insert_spaces = params.options.as_ref().and_then(|o| o.insert_spaces).unwrap_or(true);
                        features::formatting::format(&view, tab_size, insert_spaces)
                    }
                };
                Some(Ok(result))
            }
            "textDocument/hover" | "textDocument/completion" => {
                let params: PositionParams = serde_json::from_str(params?.get()).ok()?;
                let document = documents.get(&params.text_document.uri)?;
                let offset = document.offset_at(params.position) as u32;
                self.ensure_cache(document);
                let cache = self.cache.as_ref().expect("缓存刚写入");
                let lines = LineIndex::new(document.text());
                let view = View::new(document.text(), &cache.parsed.ast, &lines, self.schema.as_ref());
                let result = match method {
                    "textDocument/hover" => features::hover::hover(&view, offset).unwrap_or(Value::Null),
                    _ => features::completion::complete(&view, offset),
                };
                Some(Ok(result))
            }
            _ => None,
        }
    }

    fn shutdown(&mut self) { self.cache = None; }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FormatParams {
    text_document: TextIdentifier,
    options: Option<FormattingOptions>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FormattingOptions {
    tab_size: Option<u32>,
    insert_spaces: Option<bool>,
}

#[derive(Deserialize)]
struct TextIdentifier {
    uri: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PositionParams {
    text_document: TextIdentifier,
    position: Position,
}
