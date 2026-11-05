//! 样式语言后端：把各能力接到核心层的 `Backend` 契约上。
//!
//! 后端与具体方言解耦：解析逻辑由注入的 `ParseFn` 决定。
//! CSS 家族（css / scss / less）用 `parser::parse` 按 languageId 选方言；
//! 缩进式 Sass 由 `lspd-sass` 传入自己的解析函数，复用同一套能力与缓存。
//! 缓存只保留“当前文档的一份语法树”，按 URI、版本与 languageId 失效，关闭文档即释放。

use lspd_core::docs::{Document, DocumentStore, Position};
use lspd_core::text::{LineIndex, Span};
use lspd_core::{Backend, Outgoing, ResponseError};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{json, Value};

use crate::dialect::Dialect;
use crate::features::{View, color, completion, diagnostics, hover, navigation, symbols};
use crate::parser::{self, ParseResult};

/// 解析策略：由文档的 languageId 决定如何产出语法树与结构诊断。
pub type ParseFn = fn(source: &str, language_id: &str) -> ParseResult;

/// CSS 家族后端工厂：css / scss / less 共用，方言由 languageId 决定。
pub fn backend() -> Box<dyn Backend> {
    Box::new(StyleBackend::new(
        "css",
        &["css", "scss", "less"],
        &[".css", ".scss", ".less"],
        parse_css,
    ))
}

/// 按 languageId 选方言后调用 CSS 解析器。
fn parse_css(source: &str, language_id: &str) -> ParseResult {
    parser::parse(source, Dialect::from_language_id(language_id))
}

/// 一份文档的解析缓存。
struct ParseCache {
    /// 文档 URI
    uri: String,
    /// 解析时的版本
    version: i32,
    /// 解析时的语言标识
    language_id: String,
    /// 解析结果
    parsed: ParseResult,
}

/// 样式语言后端。方言差异全部收敛在注入的 `parse` 里。
pub struct StyleBackend {
    /// 后端名称
    name: &'static str,
    /// 服务的语言标识
    language_ids: &'static [&'static str],
    /// 服务的文件扩展名
    file_extensions: &'static [&'static str],
    /// 解析策略
    parse: ParseFn,
    /// 单文档解析缓存
    cache: Option<ParseCache>,
}

impl StyleBackend {
    /// 构造一个样式后端。
    pub fn new(
        name: &'static str,
        language_ids: &'static [&'static str],
        file_extensions: &'static [&'static str],
        parse: ParseFn,
    ) -> Self {
        Self { name, language_ids, file_extensions, parse, cache: None }
    }

    /// 取当前文档的解析结果，必要时重新解析。
    fn parsed(&mut self, document: &Document) -> &ParseResult {
        let language_id = document.language_id();
        let fresh = self.cache.as_ref().is_some_and(|cache| {
            cache.uri == document.uri() && cache.version == document.version() && cache.language_id == language_id
        });

        if !fresh {
            self.cache = Some(ParseCache {
                uri: document.uri().to_string(),
                version: document.version(),
                language_id: language_id.to_string(),
                parsed: (self.parse)(document.text(), language_id),
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
            json!({ "uri": document.uri(), "version": document.version(), "diagnostics": items }),
        ));
    }
}

impl Backend for StyleBackend {
    fn name(&self) -> &'static str {
        self.name
    }

    fn language_ids(&self) -> &'static [&'static str] {
        self.language_ids
    }

    fn file_extensions(&self) -> &'static [&'static str] {
        self.file_extensions
    }

    fn capabilities(&self) -> Value {
        json!({
            "completionProvider": { "triggerCharacters": [":", "-", "@", "$", " "], "resolveProvider": false },
            "hoverProvider": true,
            "documentSymbolProvider": true,
            "foldingRangeProvider": true,
            "documentColorProvider": true,
            "definitionProvider": true,
            "referencesProvider": true,
            "renameProvider": { "prepareProvider": true },
            "selectionRangeProvider": true,
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
            | "textDocument/prepareRename" => {
                let params: PositionParams = parse_params(params)?;
                let document = documents.get(&params.text_document.uri)?;
                let offset = document.offset_at(params.position) as u32;

                let parsed = self.parsed(document);
                let lines = LineIndex::new(document.text());
                let view = View::new(document.text(), &parsed.ast, &lines);

                let result = match method {
                    "textDocument/hover" => hover::hover(&view, offset)?,
                    "textDocument/completion" => completion::complete(&view, offset, document.language_id()),
                    "textDocument/definition" => navigation::definition(&view, offset, document.uri()),
                    "textDocument/references" => navigation::references(&view, offset, document.uri()),
                    _ => navigation::prepare_rename(&view, offset)?,
                };
                Some(Ok(result))
            }
            "textDocument/documentSymbol" | "textDocument/foldingRange" | "textDocument/documentColor" => {
                let params: DocumentParams = parse_params(params)?;
                let document = documents.get(&params.text_document.uri)?;

                let parsed = self.parsed(document);
                let lines = LineIndex::new(document.text());
                let view = View::new(document.text(), &parsed.ast, &lines);

                let result = match method {
                    "textDocument/documentSymbol" => symbols::document_symbols(&view),
                    "textDocument/foldingRange" => symbols::folding_ranges(&view),
                    _ => color::document_colors(&view),
                };
                Some(Ok(result))
            }
            "textDocument/selectionRange" => {
                let params: SelectionRangeParams = parse_params(params)?;
                let document = documents.get(&params.text_document.uri)?;

                let parsed = self.parsed(document);
                let lines = LineIndex::new(document.text());
                let view = View::new(document.text(), &parsed.ast, &lines);

                let ranges = params
                    .positions
                    .iter()
                    .map(|position| symbols::selection_range(&view, document.offset_at(*position) as u32))
                    .collect::<Vec<_>>();
                Some(Ok(Value::Array(ranges)))
            }
            "textDocument/colorPresentation" => {
                let params: ColorParams = parse_params(params)?;
                let document = documents.get(&params.text_document.uri)?;

                let parsed = self.parsed(document);
                let lines = LineIndex::new(document.text());
                let view = View::new(document.text(), &parsed.ast, &lines);

                let span = Span {
                    start: document.offset_at(params.range.start) as u32,
                    end: document.offset_at(params.range.end) as u32,
                };
                let rgba = color::Rgba::from(&params.color);
                Some(Ok(color::presentations(&view, span, &rgba)))
            }
            "textDocument/rename" => {
                let params: RenameParams = parse_params(params)?;
                let document = documents.get(&params.text_document.uri)?;
                let offset = document.offset_at(params.position) as u32;

                let parsed = self.parsed(document);
                let lines = LineIndex::new(document.text());
                let view = View::new(document.text(), &parsed.ast, &lines);
                Some(Ok(navigation::rename(&view, offset, document.uri(), &params.new_name)))
            }
            _ => Some(Err(ResponseError {
                code: lspd_core::message::error_code::METHOD_NOT_FOUND,
                message: format!("css 后端不支持 {method}"),
                data: None,
            })),
        }
    }

    fn shutdown(&mut self) {
        self.cache = None;
    }
}

/// 解析参数；失败返回 `None`。
fn parse_params<'a, T: Deserialize<'a>>(params: Option<&'a RawValue>) -> Option<T> {
    serde_json::from_str(params?.get()).ok()
}

/// 文档标识。
#[derive(Deserialize)]
struct TextIdentifier {
    /// 文档 URI
    uri: String,
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

/// 只带文档标识的参数。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentParams {
    /// 文档标识
    text_document: TextIdentifier,
}

/// 选择区间参数：一次请求可带多个位置。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectionRangeParams {
    /// 文档标识
    text_document: TextIdentifier,
    /// 位置列表
    positions: Vec<Position>,
}

/// 颜色呈现参数。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ColorParams {
    /// 文档标识
    text_document: TextIdentifier,
    /// 颜色分量
    color: ColorValue,
    /// 颜色所在区间
    range: RangeValue,
}

/// 颜色分量，取值 0.0~1.0。
#[derive(Deserialize)]
struct ColorValue {
    /// 红
    red: f64,
    /// 绿
    green: f64,
    /// 蓝
    blue: f64,
    /// 透明度
    alpha: f64,
}

impl From<&ColorValue> for color::Rgba {
    fn from(value: &ColorValue) -> Self {
        Self { red: value.red, green: value.green, blue: value.blue, alpha: value.alpha }
    }
}

/// 区间参数。
#[derive(Deserialize)]
struct RangeValue {
    /// 起点
    start: Position,
    /// 终点
    end: Position,
}

/// 重命名参数。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameParams {
    /// 文档标识
    text_document: TextIdentifier,
    /// 光标位置
    position: Position,
    /// 新名称
    new_name: String,
}
