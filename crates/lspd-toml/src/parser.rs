//! TOML 解析：用 `toml_edit` 得到带 span 的文档，再转成 JSON 后端可复用的值树。
//!
//! `lspd-json` 的 schema/补全/悬停只依赖 `parser::Value/Member` 的 span 与 key 名称，
//! 因此 TOML 只需在词法层转一次值树，就能复用同一套 schema 逻辑。

use lspd_core::text::Span;
use lspd_json::parser::{Member, Value};
use toml_edit::Item;

/// 一条语法诊断。
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// 诊断范围
    pub span: Span,
    /// 信息
    pub message: String,
}

/// 解析结果。
pub struct ParseResult {
    /// 转换后的值树，根始终是对象
    pub value: Value,
    /// 语法诊断
    pub diagnostics: Vec<Diagnostic>,
}

/// 解析 TOML 文档。
pub fn parse(source: &str) -> ParseResult {
    match toml_edit::Document::parse(source.to_string()) {
        Ok(document) => ParseResult {
            value: convert_item(document.as_item(), source.len()),
            diagnostics: Vec::new(),
        },
        Err(error) => ParseResult {
            value: Value::Object {
                span: Span::new(0, source.len()),
                members: Vec::new(),
            },
            diagnostics: vec![Diagnostic {
                span: error_span(&error, source.len()),
                message: error.message().to_string(),
            }],
        },
    }
}

/// 把 toml_edit 错误跨度转成字节区间。
fn error_span(error: &toml_edit::TomlError, len: usize) -> Span {
    match error.span() {
        Some(range) => Span::new(range.start, range.end.min(len)),
        None => Span::new(0, len.min(1)),
    }
}

/// 把 toml_edit 节点转成通用值树。
fn convert_item(item: &Item, len: usize) -> Value {
    match item {
        Item::Table(table) => convert_table(table, len),
        Item::ArrayOfTables(tables) => Value::Array {
            span: item_span(item, len),
            items: tables
                .iter()
                .map(|table| convert_table(table, len))
                .collect(),
        },
        Item::Value(value) => convert_value(value, len),
        Item::None => Value::Null {
            span: Span::new(0, 0),
        },
    }
}

/// 转换表。
fn convert_table(table: &toml_edit::Table, len: usize) -> Value {
    let members = table
        .iter()
        .map(|(key, item)| {
            let key_repr = table.key(key);
            Member {
                key: key_span(key_repr.and_then(|repr| repr.span()), key, len),
                key_name: key.to_string(),
                value: convert_item(item, len),
            }
        })
        .collect();

    Value::Object {
        span: table_span(table, len),
        members,
    }
}

/// 转换标量/内联值。
fn convert_value(value: &toml_edit::Value, len: usize) -> Value {
    let span = value
        .span()
        .map(|range| Span::new(range.start, range.end.min(len)))
        .unwrap_or_else(|| Span::new(0, 0));

    match value {
        toml_edit::Value::String(text) => Value::String {
            span,
            content: string_content(span, text.value()),
        },
        toml_edit::Value::Integer(_) => Value::Number { span },
        toml_edit::Value::Float(_) => Value::Number { span },
        toml_edit::Value::Boolean(flag) => Value::Bool {
            span,
            value: *flag.value(),
        },
        toml_edit::Value::Datetime(_) => Value::String {
            span,
            content: Span {
                start: span.start,
                end: span.end,
            },
        },
        toml_edit::Value::Array(array) => Value::Array {
            span,
            items: array.iter().map(|item| convert_value(item, len)).collect(),
        },
        toml_edit::Value::InlineTable(table) => {
            let members = table
                .iter()
                .map(|(key, item)| Member {
                    key: key_span(table.key(key).and_then(|repr| repr.span()), key, len),
                    key_name: key.to_string(),
                    value: convert_value(item, len),
                })
                .collect();
            Value::Object { span, members }
        }
    }
}

/// 表的 span：合并表头与全部成员，保证 locate 能下钻到表内键。
fn table_span(table: &toml_edit::Table, len: usize) -> Span {
    let mut start = len;
    let mut end = 0usize;

    if let Some(range) = table.span() {
        start = start.min(range.start);
        end = end.max(range.end.min(len));
    }

    for (key, item) in table.iter() {
        let key_span = key_span(table.key(key).and_then(|repr| repr.span()), key, len);
        let item_span = item_span(item, len);
        start = start
            .min(key_span.start as usize)
            .min(item_span.start as usize);
        end = end.max(key_span.end as usize).max(item_span.end as usize);
    }

    match end > start {
        true => Span::new(start, end),
        false => Span::new(0, len),
    }
}

/// 节点 span，缺失时退化为空区间。
fn item_span(item: &Item, len: usize) -> Span {
    item.span()
        .map(|range| Span::new(range.start, range.end.min(len)))
        .unwrap_or_else(|| Span::new(0, 0))
}

/// key span；toml_edit 有时不给 key span，这里退化为零宽。
fn key_span(span: Option<std::ops::Range<usize>>, _key: &str, len: usize) -> Span {
    match span {
        Some(range) => Span::new(range.start, range.end.min(len)),
        None => Span::new(0, 0),
    }
}

/// 字符串内容区间：去掉首尾引号，便于复用 JSON 的 decode/hover。
fn string_content(span: Span, _value: &str) -> Span {
    if span.end.saturating_sub(span.start) >= 2 {
        Span::new((span.start as usize) + 1, (span.end as usize) - 1)
    } else {
        Span {
            start: span.start,
            end: span.end,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 基本 TOML 应转成对象树。
    #[test]
    fn parses_table() {
        let result = parse("[pkg]\nname = \"demo\"\nversion = 1\n");
        assert!(result.diagnostics.is_empty());
        let Value::Object { members, .. } = result.value else {
            panic!("root")
        };
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].key_name, "pkg");
        let Value::Object { members: inner, .. } = &members[0].value else {
            panic!("pkg")
        };
        assert_eq!(inner[0].key_name, "name");
    }

    /// 语法错误应给出诊断。
    #[test]
    fn reports_syntax_error() {
        let result = parse("[pkg\nname='x'");
        assert!(!result.diagnostics.is_empty());
    }
}
