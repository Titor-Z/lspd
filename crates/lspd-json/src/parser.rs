//! 容错 JSON 解析器：递归下降，逐节点保留字节区间，出错时恢复而非中止。
//!
//! 不用 `serde_json` 做解析（它严格、无恢复、不产出逐节点区间），
//! 这里自己扫描：既给编辑期持续可用的语法树，也产出可定位的语法诊断。

use lspd_core::text::Span;

/// 诊断严重程度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// 结构错误
    Error,
    /// 可疑写法
    Warning,
}

/// 一条诊断。
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// 出错区间
    pub span: Span,
    /// 严重程度
    pub severity: Severity,
    /// 说明
    pub message: String,
}

/// 对象的一个成员。
#[derive(Debug, Clone)]
pub struct Member {
    /// 键的整体区间（含引号）
    pub key: Span,
    /// 键名（已解码转义）
    pub key_name: String,
    /// 值节点
    pub value: Value,
}

/// 一个 JSON 值节点。
#[derive(Debug, Clone)]
pub enum Value {
    /// null
    Null { span: Span },
    /// true / false
    Bool { span: Span, value: bool },
    /// 数字（保留原文区间，交由 schema 校验取值）
    Number { span: Span },
    /// 字符串（区间含引号，content 为内部）
    String { span: Span, content: Span },
    /// 数组
    Array { span: Span, items: Vec<Value> },
    /// 对象
    Object { span: Span, members: Vec<Member> },
}

impl Value {
    /// 节点整体区间。
    pub fn span(&self) -> Span {
        match self {
            Value::Null { span }
            | Value::Bool { span, .. }
            | Value::Number { span }
            | Value::String { span, .. }
            | Value::Array { span, .. }
            | Value::Object { span, .. } => *span,
        }
    }
}

/// 解析结果。
pub struct ParseResult {
    /// 根节点
    pub value: Value,
    /// 语法诊断
    pub diagnostics: Vec<Diagnostic>,
}

/// 解析一份 JSON 文档。
pub fn parse(source: &str) -> ParseResult {
    let mut parser = Parser::new(source);
    let value = parser.parse_document();
    ParseResult { value, diagnostics: parser.diagnostics }
}

/// 单文档解析器。
struct Parser<'a> {
    /// 原文
    source: &'a str,
    /// 字节
    bytes: &'a [u8],
    /// 游标
    position: usize,
    /// 诊断
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    /// 建立解析器。
    fn new(source: &'a str) -> Self {
        Self { source, bytes: source.as_bytes(), position: 0, diagnostics: Vec::new() }
    }

    /// 解析整篇文档：一个值，之后不容许多余内容。
    fn parse_document(&mut self) -> Value {
        self.skip_ws();

        if self.eof() {
            return Value::Null { span: Span::new(self.position, self.position) };
        }

        let value = self.parse_value();
        self.skip_ws();

        if !self.eof() {
            self.error(Span::new(self.position, self.bytes.len()), "文档尾部有多余内容");
        }

        value
    }

    /// 解析一个值。
    fn parse_value(&mut self) -> Value {
        self.skip_ws();

        if self.eof() {
            self.error(Span::new(self.position, self.position), "缺少值");
            return Value::Null { span: Span::new(self.position, self.position) };
        }

        match self.bytes[self.position] {
            b'{' => self.parse_object(),
            b'[' => self.parse_array(),
            b'"' => {
                let (span, content) = self.parse_string();
                Value::String { span, content }
            }
            b't' => Value::Bool { span: self.parse_word("true"), value: true },
            b'f' => Value::Bool { span: self.parse_word("false"), value: false },
            b'n' => Value::Null { span: self.parse_word("null") },
            byte if byte == b'-' || byte.is_ascii_digit() => self.parse_number(),
            _ => {
                let start = self.position;
                self.position += 1;
                self.error(Span::new(start, self.position), "意外的字符");
                Value::Null { span: Span::new(start, start) }
            }
        }
    }

    /// 解析对象。
    fn parse_object(&mut self) -> Value {
        let start = self.position;
        self.position += 1; // '{'
        let mut members = Vec::new();

        loop {
            self.skip_ws();

            if self.eof() {
                self.error(Span::new(start, start + 1), "未闭合的 `{`");
                break;
            }

            if self.peek() == b'}' {
                self.position += 1;
                break;
            }

            if self.peek() != b'"' {
                self.error(Span::new(self.position, self.position + 1), "键必须用双引号");
                self.skip_to_member_boundary();
                if !self.at_object_end() {
                    self.position += 1; // 跳过分隔的 `,`
                }
                continue;
            }

            let (key_span, content) = self.parse_string();
            let key_name = decode_string(self.source, content);

            self.skip_ws();
            if self.peek() == b':' {
                self.position += 1;
            } else {
                self.error(Span::new(self.position, self.position), "键后缺少 `:`");
            }

            let value = self.parse_value();
            members.push(Member { key: key_span, key_name, value });

            self.skip_ws();
            match self.peek() {
                b',' => {
                    self.position += 1;
                    self.skip_ws();
                    if self.peek() == b'}' {
                        self.warn(Span::new(self.position - 1, self.position), "尾随逗号");
                    }
                }
                b'}' => {
                    self.position += 1;
                    break;
                }
                _ => {
                    if self.eof() {
                        self.error(Span::new(start, start + 1), "未闭合的 `{`");
                        break;
                    }
                    self.error(Span::new(self.position, self.position), "缺少 `,` 或 `}`");
                    self.skip_to_member_boundary();
                    if !self.at_object_end() && !self.eof() {
                        self.position += 1;
                    }
                }
            }
        }

        Value::Object { span: Span::new(start, self.position), members }
    }

    /// 解析数组。
    fn parse_array(&mut self) -> Value {
        let start = self.position;
        self.position += 1; // '['
        let mut items = Vec::new();

        loop {
            self.skip_ws();

            if self.eof() {
                self.error(Span::new(start, start + 1), "未闭合的 `[`");
                break;
            }

            if self.peek() == b']' {
                self.position += 1;
                break;
            }

            items.push(self.parse_value());
            self.skip_ws();

            match self.peek() {
                b',' => {
                    self.position += 1;
                    self.skip_ws();
                    if self.peek() == b']' {
                        self.warn(Span::new(self.position - 1, self.position), "尾随逗号");
                    }
                }
                b']' => {
                    self.position += 1;
                    break;
                }
                _ => {
                    if self.eof() {
                        self.error(Span::new(start, start + 1), "未闭合的 `[`");
                        break;
                    }
                    self.error(Span::new(self.position, self.position), "缺少 `,` 或 `]`");
                    self.skip_to_array_boundary();
                    if !self.eof() && self.peek() == b']' {
                        self.position += 1;
                        break;
                    }
                }
            }
        }

        Value::Array { span: Span::new(start, self.position), items }
    }

    /// 解析字符串，返回整体区间与内部内容区间。
    fn parse_string(&mut self) -> (Span, Span) {
        let start = self.position;
        self.position += 1; // 开引号
        let content_start = self.position;

        while !self.eof() {
            match self.bytes[self.position] {
                b'\\' => {
                    let escape = self.position;
                    self.position += 2;
                    match self.bytes.get(escape + 1) {
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' | b'u') => {}
                        Some(_) => self.warn(Span::new(escape, escape + 2), "非法转义"),
                        None => self.position = self.bytes.len(),
                    }
                }
                b'"' => {
                    let content_end = self.position;
                    self.position += 1; // 闭引号
                    return (Span::new(start, self.position), Span::new(content_start, content_end));
                }
                b'\n' => {
                    self.error(Span::new(start, self.position), "未闭合的字符串");
                    self.position += 1;
                    return (Span::new(start, self.position), Span::new(content_start, content_start));
                }
                _ => self.position += 1,
            }
        }

        self.error(Span::new(start, self.position), "未闭合的字符串");
        (Span::new(start, self.position), Span::new(content_start, self.position))
    }

    /// 校验 `true` / `false` / `null` 字面量，返回其区间；不符时跳到边界以免死循环。
    fn parse_word(&mut self, word: &str) -> Span {
        let start = self.position;

        if self.source[start..].starts_with(word) {
            self.position += word.len();
        } else {
            self.skip_to_member_boundary();
            self.error(Span::new(start, self.position), format!("应为 `{word}`"));
        }

        Span::new(start, self.position)
    }

    /// 解析数字。
    fn parse_number(&mut self) -> Value {
        let start = self.position;
        while !self.eof() && matches!(self.bytes[self.position], b'-' | b'+' | b'0'..=b'9' | b'.' | b'e' | b'E') {
            self.position += 1;
        }

        let span = Span::new(start, self.position);
        if !is_valid_number(span.text(self.source)) {
            self.error(span, "非法的数字");
        }
        Value::Number { span }
    }

    // ---- 辅助 ----

    /// 是否越界。
    fn eof(&self) -> bool {
        self.position >= self.bytes.len()
    }

    /// 当前字节（越界返回 0）。
    fn peek(&self) -> u8 {
        self.bytes.get(self.position).copied().unwrap_or(0)
    }

    /// 跳过空白。
    fn skip_ws(&mut self) {
        while !self.eof() && matches!(self.bytes[self.position], b' ' | b'\t' | b'\r' | b'\n') {
            self.position += 1;
        }
    }

    /// 跳到成员分隔边界：`, } ]` 或文件结束。
    fn skip_to_member_boundary(&mut self) {
        while !self.eof() && !matches!(self.bytes[self.position], b',' | b'}' | b']') {
            self.position += 1;
        }
    }

    /// 跳到数组元素分隔边界。
    fn skip_to_array_boundary(&mut self) {
        self.skip_to_member_boundary();
    }

    /// 当前是否处在对象收尾（`}` 或 EOF）。
    fn at_object_end(&self) -> bool {
        self.eof() || self.peek() == b'}'
    }

    /// 记录错误。
    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic { span, severity: Severity::Error, message: message.into() });
    }

    /// 记录警告。
    fn warn(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic { span, severity: Severity::Warning, message: message.into() });
    }
}

/// 把字符串内容区间解码为文本（处理常见转义，`\u` 取码点）。
pub fn decode_string(source: &str, content: Span) -> String {
    let text = content.text(source);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();

    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Ok(code) = u32::from_str_radix(&hex, 16)
                    && let Some(ch) = char::from_u32(code) {
                        out.push(ch);
                    }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }

    out
}

/// 校验一段文本是否为合法 JSON 数字。
fn is_valid_number(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return false;
    }

    let mut index = 0usize;
    if bytes[index] == b'-' {
        index += 1;
    }

    // 整数部分
    match bytes.get(index) {
        Some(b'0') => index += 1,
        Some(byte) if byte.is_ascii_digit() && *byte != b'0' => {
            while matches!(bytes.get(index), Some(b) if b.is_ascii_digit()) {
                index += 1;
            }
        }
        _ => return false,
    }

    // 小数部分
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let begin = index;
        while matches!(bytes.get(index), Some(b) if b.is_ascii_digit()) {
            index += 1;
        }
        if index == begin {
            return false;
        }
    }

    // 指数部分
    if matches!(bytes.get(index), Some(b'e') | Some(b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+') | Some(b'-')) {
            index += 1;
        }
        let begin = index;
        while matches!(bytes.get(index), Some(b) if b.is_ascii_digit()) {
            index += 1;
        }
        if index == begin {
            return false;
        }
    }

    index == bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合法文档不应有诊断，并给出可导航的值树。
    #[test]
    fn parses_valid_document() {
        let result = parse(r#"{ "a": 1, "b": [true, null] }"#);
        assert!(result.diagnostics.is_empty(), "诊断：{:?}", result.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>());

        let Value::Object { members, .. } = result.value else { panic!("根应为对象") };
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].key_name, "a");
        let Value::Array { items, .. } = &members[1].value else { panic!("b 应为数组") };
        assert_eq!(items.len(), 2);
    }

    /// 尾随逗号应给警告。
    #[test]
    fn flags_trailing_comma() {
        let result = parse(r#"{ "a": 1, }"#);
        assert!(result.diagnostics.iter().any(|d| d.message.contains("尾随逗号")));
    }

    /// 缺冒号与未闭合都应被记录。
    #[test]
    fn flags_missing_colon_and_unclosed() {
        let result = parse(r#"{ "a" 1 "#);
        assert!(result.diagnostics.iter().any(|d| d.message.contains("缺少 `:`")));
        assert!(result.diagnostics.iter().any(|d| d.message.contains("未闭合")));
    }

    /// 数组元素间缺逗号应报错。
    #[test]
    fn flags_missing_comma_in_array() {
        let result = parse("[1 2]");
        assert!(result.diagnostics.iter().any(|d| d.message.contains("缺少 `,`")));
    }

    /// 未闭合字符串应报错且不吞掉解析。
    #[test]
    fn flags_unterminated_string() {
        let result = parse(r#"{ "a": "abc }"#);
        assert!(result.diagnostics.iter().any(|d| d.message.contains("未闭合的字符串")));
    }

    /// 非法数字应报错。
    #[test]
    fn flags_invalid_number() {
        let result = parse("[01, 1., +2]");
        assert!(result.diagnostics.iter().filter(|d| d.message.contains("非法的数字")).count() >= 2);
    }

    /// 解析器不应因坏输入死循环：各种残缺输入都要收敛。
    #[test]
    fn always_terminates() {
        for source in ["{", "[", "{\"a\"", "[1,", "\"abc", "tru", "1 2", "{,}", "[,]"] {
            let _ = parse(source);
        }
    }
}
