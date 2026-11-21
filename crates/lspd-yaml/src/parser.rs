//! YAML 解析：把 YAML 事件流转成 lspd-json 可复用的值树。
//!
//! 第一版只覆盖单文档、常见标量/映射/序列；锚点/别名降级为空值，
//! 保持语法诊断与 schema hover/completion 可用。

use lspd_core::text::Span;
use lspd_json::parser::{Member, Value};
use saphyr_parser::{Event, Parser, ScalarStyle, ScanError, Span as YSpan};

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
    /// 转换后的值树，根默认对象
    pub value: Value,
    /// 语法诊断
    pub diagnostics: Vec<Diagnostic>,
}

/// 解析 YAML 文档。
pub fn parse(source: &str) -> ParseResult {
    let mut parser = Parser::new_from_str(source);
    let mut events = Vec::new();
    let mut diagnostics = Vec::new();
    for item in &mut parser {
        match item {
            Ok(event) => events.push(event),
            Err(error) => {
                diagnostics.push(diagnostic_from_error(source, &error));
                break;
            }
        }
    }

    let mut builder = Builder {
        source,
        events: events.into_iter(),
        diagnostics,
    };
    let value = builder.parse_stream().unwrap_or(Value::Object {
        span: Span::new(0, source.len()),
        members: Vec::new(),
    });

    ParseResult {
        value,
        diagnostics: builder.diagnostics,
    }
}

/// 把扫描错误转为诊断。
fn diagnostic_from_error(source: &str, error: &ScanError) -> Diagnostic {
    let span = Span::new(
        error.marker().index().min(source.len()),
        source.len().min(error.marker().index() + 1),
    );
    Diagnostic {
        span,
        message: error.info().to_owned(),
    }
}

/// 事件流构建器。
struct Builder<'a, I: Iterator<Item = (Event<'a>, YSpan)>> {
    source: &'a str,
    events: I,
    diagnostics: Vec<Diagnostic>,
}

impl<'a, I: Iterator<Item = (Event<'a>, YSpan)>> Builder<'a, I> {
    /// 解析整个流。
    fn parse_stream(&mut self) -> Option<Value> {
        let mut root = None;
        while let Some((event, span)) = self.events.next() {
            match event {
                Event::StreamStart | Event::Nothing | Event::DocumentEnd | Event::StreamEnd => {}
                Event::DocumentStart(_) => {
                    if root.is_none() {
                        root = self.parse_node(span);
                    } else {
                        // 第一版只取首个文档，避免误导 schema 定位
                        self.diagnostics.push(Diagnostic {
                            span: to_span(span, self.source.len()),
                            message: "暂不支持多文档 YAML，仅解析第一个文档".to_string(),
                        });
                    }
                }
                _ => {
                    if root.is_none() {
                        root = self.parse_event(event, span);
                    }
                }
            }
        }
        root
    }

    /// 解析一个节点。
    fn parse_node(&mut self, _span: YSpan) -> Option<Value> {
        let (event, span) = self.events.next()?;
        self.parse_event(event, span)
    }

    /// 解析具体事件。
    fn parse_event(&mut self, event: Event<'a>, span: YSpan) -> Option<Value> {
        match event {
            Event::Scalar(value, style, _, _) => Some(scalar_value(self.source, &value, style, span)),
            Event::SequenceStart(_, _) => Some(self.parse_sequence(span)),
            Event::MappingStart(_, _) => Some(self.parse_mapping(span)),
            Event::Alias(_) => Some(Value::Null {
                span: to_span(span, self.source.len()),
            }),
            Event::Nothing
            | Event::StreamStart
            | Event::StreamEnd
            | Event::DocumentStart(_)
            | Event::DocumentEnd
            | Event::SequenceEnd
            | Event::MappingEnd => Some(Value::Null {
                span: to_span(span, self.source.len()),
            }),
        }
    }

    /// 解析序列。
    fn parse_sequence(&mut self, start: YSpan) -> Value {
        let mut items = Vec::new();
        let mut end = start;
        while let Some((event, span)) = self.events.next() {
            end = span;
            match event {
                Event::SequenceEnd => break,
                _ => {
                    if let Some(value) = self.parse_event(event, span) {
                        items.push(value);
                    }
                }
            }
        }
        Value::Array {
            span: span_between(start, end, self.source.len()),
            items,
        }
    }

    /// 解析映射。
    fn parse_mapping(&mut self, start: YSpan) -> Value {
        let mut members = Vec::new();
        let mut end = start;
        while let Some((event, span)) = self.events.next() {
            end = span;
            match event {
                Event::MappingEnd => break,
                _ => {
                    let (key_span, key_name) = key_of(self.source, &event, span);
                    let value = self.parse_node(span).unwrap_or(Value::Null {
                        span: to_span(span, self.source.len()),
                    });
                    end = value_span_end(&value, end);
                    members.push(Member {
                        key: key_span,
                        key_name,
                        value,
                    });
                }
            }
        }
        Value::Object {
            span: span_between(start, end, self.source.len()),
            members,
        }
    }
}

/// 把映射 key 事件转成 key 名称。
fn key_of(source: &str, event: &Event<'_>, span: YSpan) -> (Span, String) {
    match event {
        Event::Scalar(value, _, _, _) => (to_span(span, source.len()), value.to_string()),
        Event::Alias(_) => (to_span(span, source.len()), "*alias".to_string()),
        _ => (to_span(span, source.len()), String::new()),
    }
}

/// 标量转值树。
fn scalar_value(source: &str, value: &str, style: ScalarStyle, span: YSpan) -> Value {
    let span = to_span(span, source.len());
    if style != ScalarStyle::Plain {
        return Value::String {
            span,
            content: string_content(span, style),
        };
    }

    match value {
        "true" => Value::Bool { span, value: true },
        "false" => Value::Bool { span, value: false },
        "null" | "~" => Value::Null { span },
        _ => {
            if looks_like_number(value) {
                Value::Number { span }
            } else {
                Value::String {
                    span,
                    content: string_content(span, style),
                }
            }
        }
    }
}

/// 粗略判断数字标量。
fn looks_like_number(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0usize;
    if matches!(bytes[0], b'-' | b'+') {
        i += 1;
    }
    let mut has_digit = false;
    let mut has_dot = false;
    while i < bytes.len() {
        match bytes[i] {
            b'0'..=b'9' => has_digit = true,
            b'.' if !has_dot => has_dot = true,
            b'e' | b'E' => {
                i += 1;
                if i < bytes.len() && matches!(bytes[i], b'-' | b'+') {
                    i += 1;
                }
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    has_digit = true;
                    i += 1;
                }
                return i == bytes.len() && has_digit;
            }
            _ => return false,
        }
        i += 1;
    }
    has_digit
}

/// 字符串内容区间：引号标量去掉首尾引号，其余使用整个 span。
fn string_content(span: Span, style: ScalarStyle) -> Span {
    match style {
        ScalarStyle::SingleQuoted | ScalarStyle::DoubleQuoted
            if span.end.saturating_sub(span.start) >= 2 =>
        {
            Span::new(span.start as usize + 1, span.end as usize - 1)
        }
        _ => span,
    }
}

/// saphyr span 转 lspd span。
fn to_span(span: YSpan, len: usize) -> Span {
    let start = span.start.index().min(len);
    let end = span.end.index().min(len).max(start);
    Span::new(start, end)
}

/// 合并两个 span。
fn span_between(start: YSpan, end: YSpan, len: usize) -> Span {
    let s = to_span(start, len);
    let e = to_span(end, len);
    Span::new(s.start as usize, e.end.max(s.end) as usize)
}

/// 取值末尾。
fn value_span_end(value: &Value, fallback: YSpan) -> YSpan {
    let _ = value;
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 基本映射应转成对象。
    #[test]
    fn parses_mapping() {
        let result = parse("name: demo\ncount: 1\n");
        assert!(result.diagnostics.is_empty());
        let Value::Object { members, .. } = result.value else { panic!("root") };
        assert_eq!(members[0].key_name, "name");
    }

    /// 语法错误应给出诊断。
    #[test]
    fn reports_error() {
        let result = parse("a: [1, 2\n");
        assert!(!result.diagnostics.is_empty());
    }
}


