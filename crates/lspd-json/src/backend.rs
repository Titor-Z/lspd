//! JSON 后端：语法诊断、文档大纲与折叠区间。
//!
//! 缓存只保留当前文档的一份语法树，按 URI 与版本失效，关闭文档即释放。
//! schema 驱动的补全与校验在 `schema` 侧接入（见 M5.2）。

use lspd_core::docs::{Document, DocumentStore, Position};
use lspd_core::text::{LineIndex, Span};
use lspd_core::{Backend, Outgoing, ResponseError};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::parser::{self, Diagnostic, ParseResult, Severity, Value as Json};
use crate::schema;

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
    /// 从 `$schema` 加载的本地 schema；不可用为 `None`
    schema: Option<Value>,
}

impl JsonBackend {
    /// 确保当前文档已解析并加载 schema，必要时重做。
    fn ensure(&mut self, document: &Document) {
        let fresh = self.cache.as_ref().is_some_and(|cache| cache.uri == document.uri() && cache.version == document.version());

        if fresh {
            return;
        }

        let parsed = parser::parse_with(document.text(), dialect_of(document));
        let schema = schema::load(document.text(), &parsed.value, document.uri());
        self.cache = Some(ParseCache {
            uri: document.uri().to_string(),
            version: document.version(),
            parsed,
            schema,
        });
    }

    /// 发布一份文档的诊断：语法诊断 + schema 校验诊断。
    fn publish(&mut self, document: &Document, out: &mut Vec<Outgoing>) {
        self.ensure(document);

        let cache = self.cache.as_ref().expect("刚写入缓存");
        let lines = LineIndex::new(document.text());
        let mut items: Vec<Value> = cache.parsed.diagnostics.iter().map(|item| diagnostic(&lines, item)).collect();

        if let Some(schema) = &cache.schema {
            items.extend(schema::validate(document.text(), &cache.parsed.value, schema, &lines));
        }

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
        &["json", "jsonc"]
    }

    fn file_extensions(&self) -> &'static [&'static str] {
        &[".json", ".jsonc"]
    }

    fn capabilities(&self) -> Value {
        json!({
            "completionProvider": { "triggerCharacters": ["\"", ":", ",", "{", "[", " "] },
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

                let items = schema::completion(document.text(), &cache.parsed.value, cache.schema.as_ref(), offset);
                Some(Ok(json!({ "isIncomplete": false, "items": items })))
            }
            "textDocument/hover" => {
                let params: PositionParams = serde_json::from_str(params?.get()).ok()?;
                let document = documents.get(&params.text_document.uri)?;
                self.ensure(document);
                let cache = self.cache.as_ref().expect("刚写入缓存");
                let offset = document.offset_at(params.position) as u32;

                let result = schema::hover(&cache.parsed.value, cache.schema.as_ref(), offset)
                    .map(|text| json!({ "contents": { "kind": "markdown", "value": text } }))
                    .unwrap_or(Value::Null);
                Some(Ok(result))
            }
            // 不认识的方法交还核心层，由其他后端或统一的「方法不存在」应答处理
            _ => None,
        }
    }

    fn shutdown(&mut self) {
        self.cache = None;
    }
}

/// 判定文档方言：JSONC 语言标识、`.jsonc` 扩展名与常见的可注释文件按 JSONC 解析。
///
/// 编辑器对 tsconfig.json 的语言标识并不统一（有的报 `jsonc`，有的报 `json`），
/// 只认语言标识会让同一份文件在不同编辑器里表现不一致，因此再按文件名兜底。
fn dialect_of(document: &Document) -> parser::Dialect {
    if document.language_id() == "jsonc" || is_comment_friendly(document.uri()) {
        parser::Dialect::Jsonc
    } else {
        parser::Dialect::Json
    }
}

/// 文件名是否属于允许注释的 JSON 家族。
fn is_comment_friendly(uri: &str) -> bool {
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    let name = path.rsplit('/').next().unwrap_or(path);

    if name.ends_with(".jsonc") {
        return true;
    }

    // 带前缀的家族：tsconfig.json、tsconfig.build.json、jsconfig.json 等。
    if [("tsconfig", ".json"), ("jsconfig", ".json")]
        .iter()
        .any(|(prefix, suffix)| name.starts_with(prefix) && name.ends_with(suffix))
    {
        return true;
    }

    const NAMES: &[&str] = &[
        "settings.json",
        "launch.json",
        "tasks.json",
        "keybindings.json",
        "extensions.json",
        "mcp.json",
        "devcontainer.json",
        "biome.json",
        "deno.json",
        "omnisharp.json",
        ".babelrc",
        ".swcrc",
        ".eslintrc",
        ".eslintrc.json",
        ".prettierrc",
        ".prettierrc.json",
        ".stylelintrc",
        ".stylelintrc.json",
        ".markdownlint.json",
    ];

    NAMES.contains(&name)
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

/// 带位置的请求参数。
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

#[cfg(test)]
mod tests {
    use super::*;
    use lspd_core::docs::TextDocumentItem;

    /// 造一份只用于判定方言的文档。
    fn document(uri: &str, language_id: &str) -> Document {
        Document::new(TextDocumentItem {
            uri: uri.to_string(),
            language_id: language_id.to_string(),
            version: 1,
            text: String::from("{}"),
        })
    }

    /// 语言标识为 jsonc 时一律按 JSONC 解析。
    #[test]
    fn honors_jsonc_language_id() {
        assert_eq!(dialect_of(&document("file:///p/data.json", "jsonc")), parser::Dialect::Jsonc);
        assert_eq!(dialect_of(&document("file:///p/data.json", "json")), parser::Dialect::Json);
    }

    /// 可注释文件族即使被报成 json 也按 JSONC 解析。
    #[test]
    fn recognizes_comment_friendly_files() {
        for uri in [
            "file:///p/tsconfig.json",
            "file:///p/tsconfig.build.json",
            "file:///p/jsconfig.json",
            "file:///p/.vscode/settings.json",
            "file:///p/foo.jsonc",
            "file:///p/.eslintrc",
        ] {
            assert!(is_comment_friendly(uri), "{uri} 应识别为 JSONC");
        }
    }

    /// 普通 JSON 不应被误判为 JSONC。
    #[test]
    fn keeps_plain_json_strict() {
        for uri in ["file:///p/package.json", "file:///p/tsconfig.yaml", "file:///p/data.json"] {
            assert!(!is_comment_friendly(uri), "{uri} 不应识别为 JSONC");
        }
    }
}
