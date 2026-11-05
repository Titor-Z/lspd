//! 容错 CSS/SCSS/Less 解析器：单遍扫描，直接构建扁平语法树。
//!
//! 不产出中间 token 数组：扫描到 `{`、`;`、`}` 即切分语句，
//! 语句再按首个括号外的冒号拆成属性与值。这样解析期峰值内存就是语法树本身。

use lspd_core::text::Span;

use crate::ast::{Ast, Node, NodeKind, NONE};
use crate::dialect::Dialect;

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
    /// 出问题的区间
    pub span: Span,
    /// 严重程度
    pub severity: Severity,
    /// 说明
    pub message: String,
}

/// 解析结果。
pub struct ParseResult {
    /// 语法树
    pub ast: Ast,
    /// 结构诊断
    pub diagnostics: Vec<Diagnostic>,
}

/// 解析一份样式表。
pub fn parse(source: &str, dialect: Dialect) -> ParseResult {
    Parser::new(source, dialect).run()
}

/// 语句的结束方式。
enum Boundary {
    /// `{`：进入块
    Brace,
    /// `;`：语句结束
    Semicolon,
    /// `}`：当前块结束
    CloseBrace,
    /// 文件结束
    End,
}

/// 单遍扫描的解析器。
struct Parser<'a> {
    /// 原文
    source: &'a str,
    /// 原文字节
    bytes: &'a [u8],
    /// 方言
    dialect: Dialect,
    /// 当前扫描位置
    position: usize,
    /// 语法树
    ast: Ast,
    /// 未闭合的块
    stack: Vec<u32>,
    /// 诊断
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    /// 建立解析器。
    fn new(source: &'a str, dialect: Dialect) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            dialect,
            position: 0,
            ast: Ast::default(),
            stack: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    /// 执行解析。
    fn run(mut self) -> ParseResult {
        loop {
            self.skip_trivia();
            if self.position >= self.bytes.len() {
                break;
            }

            match self.bytes[self.position] {
                b'}' => self.close_block(),
                b'@' => self.parse_at_rule(),
                _ => self.parse_statement(),
            }
        }

        self.close_unclosed();
        self.diagnostics.sort_by_key(|item| item.span.start);
        ParseResult { ast: self.ast, diagnostics: self.diagnostics }
    }

    /// 关闭当前块；没有可关闭的块时报错。
    fn close_block(&mut self) {
        let start = self.position;
        self.position += 1;

        match self.stack.pop() {
            Some(index) => self.finish(index, self.position),
            None => self.error(Span::new(start, start + 1), "多余的 `}`"),
        }
    }

    /// 解析 at 规则。
    fn parse_at_rule(&mut self) {
        let start = self.position;
        self.position += 1;
        while self.position < self.bytes.len() && is_name_char(self.bytes[self.position]) {
            self.position += 1;
        }
        let name = Span::new(start, self.position);

        let (boundary, cursor) = self.scan_boundary();

        match boundary {
            Boundary::Brace => {
                let prelude = self.tight(Span::new(self.position, cursor));
                let index = self.push(NodeKind::AtRule, Span::new(start, cursor + 1), name, prelude);
                self.position = cursor + 1;
                self.stack.push(index);
            }
            Boundary::Semicolon | Boundary::CloseBrace | Boundary::End => {
                let prelude = self.tight(Span::new(self.position, cursor));
                let index = self.push(NodeKind::AtRule, Span::new(start, cursor), name, prelude);
                self.finish(index, cursor);
                self.position = if matches!(boundary, Boundary::Semicolon) { cursor + 1 } else { cursor };
            }
        }
    }

    /// 解析一条规则或一条声明。
    fn parse_statement(&mut self) {
        let start = self.position;
        let (boundary, cursor) = self.scan_boundary();

        match boundary {
            Boundary::Brace => {
                let selector = self.tight(Span::new(start, cursor));
                if selector.is_empty() {
                    self.error(Span::new(cursor, cursor + 1), "缺少选择器");
                }
                let index = self.push(NodeKind::Rule, Span::new(start, cursor + 1), selector, Span::default());
                self.position = cursor + 1;
                self.stack.push(index);
            }
            Boundary::Semicolon => {
                self.parse_declaration(start, cursor, cursor + 1);
                self.position = cursor + 1;
            }
            Boundary::CloseBrace | Boundary::End => {
                self.parse_declaration(start, cursor, cursor);
                // 停止在边界字符上，由外层循环交给 close_block 或结束解析
                self.position = cursor;
            }
        }
    }

    /// 把 `start..end` 解析为一条声明。
    fn parse_declaration(&mut self, start: usize, end: usize, span_end: usize) {
        if self.source[start..end].trim().is_empty() {
            return;
        }

        let Some(colon) = self.find_colon(start, end) else {
            self.error(Span::new(start, end), "声明缺少 `:`");
            return;
        };

        let property = self.tight(Span::new(start, colon));
        let value = self.tight(Span::new(colon + 1, end));
        let index = self.push(NodeKind::Declaration, Span::new(start, span_end), property, value);
        self.finish(index, span_end);

        if value.is_empty() {
            self.warn(property, format!("`{}` 缺少值", property.text(self.source)));
        } else if self.find_colon(colon + 1, end).is_some() {
            self.warn(property, "疑似缺少分号");
        }
    }

    /// 在不进入字符串与括号的前提下，找到属性与值的分隔冒号。
    fn find_colon(&self, start: usize, end: usize) -> Option<usize> {
        let mut index = start;
        let mut depth = 0usize;

        while index < end {
            match self.bytes[index] {
                b'"' | b'\'' => index = self.skip_string(index).0,
                b'(' => {
                    depth += 1;
                    index += 1;
                }
                b')' => {
                    depth = depth.saturating_sub(1);
                    index += 1;
                }
                b':' if depth == 0 => return Some(index),
                _ => index += 1,
            }
        }

        None
    }

    /// 扫描到语句边界，返回边界类型与位置。
    fn scan_boundary(&self) -> (Boundary, usize) {
        let mut index = self.position;
        let mut depth = 0usize;

        while index < self.bytes.len() {
            match self.bytes[index] {
                b'"' | b'\'' => index = self.skip_string(index).0,
                b'/' if self.bytes.get(index + 1) == Some(&b'*') => index = self.skip_block_comment(index),
                b'/' if self.dialect.has_line_comments() && self.bytes.get(index + 1) == Some(&b'/') => {
                    index = self.skip_line_comment(index)
                }
                b'(' => {
                    depth += 1;
                    index += 1;
                }
                b')' => {
                    depth = depth.saturating_sub(1);
                    index += 1;
                }
                b'{' if depth == 0 => return (Boundary::Brace, index),
                b';' if depth == 0 => return (Boundary::Semicolon, index),
                b'}' if depth == 0 => return (Boundary::CloseBrace, index),
                _ => index += 1,
            }
        }

        (Boundary::End, self.bytes.len())
    }

    /// 跳过空白与注释。
    fn skip_trivia(&mut self) {
        loop {
            while self.position < self.bytes.len() && self.bytes[self.position].is_ascii_whitespace() {
                self.position += 1;
            }

            if self.bytes.get(self.position) == Some(&b'/') && self.bytes.get(self.position + 1) == Some(&b'*') {
                self.position = self.skip_block_comment(self.position);
                continue;
            }
            if self.bytes.get(self.position) == Some(&b'/')
                && self.bytes.get(self.position + 1) == Some(&b'/')
                && self.dialect.has_line_comments()
            {
                self.position = self.skip_line_comment(self.position);
                continue;
            }
            return;
        }
    }

    /// 跳过字符串，返回结束位置与是否正常闭合。
    fn skip_string(&self, start: usize) -> (usize, bool) {
        let quote = self.bytes[start];
        let mut index = start + 1;
        while index < self.bytes.len() {
            match self.bytes[index] {
                b'\\' => index += 2,
                byte if byte == quote => return (index + 1, true),
                _ => index += 1,
            }
        }
        (self.bytes.len(), false)
    }

    /// 跳过块注释。
    fn skip_block_comment(&self, start: usize) -> usize {
        find(&self.bytes[start + 2..], b"*/")
            .map(|offset| start + 2 + offset + 2)
            .unwrap_or(self.bytes.len())
    }

    /// 跳行注释。
    fn skip_line_comment(&self, start: usize) -> usize {
        find(&self.bytes[start + 2..], b"\n").map(|offset| start + 2 + offset + 1).unwrap_or(self.bytes.len())
    }

    /// 文件结束时仍未闭合的块逐个报错。
    fn close_unclosed(&mut self) {
        let end = self.bytes.len();
        while let Some(index) = self.stack.pop() {
            let span = self.ast.node(index).name;
            let label = span.text(self.source).trim().to_string();
            self.error(span, format!("`{label}` 的块未闭合"));
            self.finish(index, end);
        }
    }

    /// 补全节点区间并记录子树结束位置。
    fn finish(&mut self, index: u32, end: usize) {
        let subtree_end = self.ast.nodes.len() as u32;
        let node = self.ast.node_mut(index);
        node.span.end = end as u32;
        node.subtree_end = subtree_end;
    }

    /// 把区间收缩到不含首尾空白的部分，使语法树里的名字区间可直接使用。
    fn tight(&self, span: Span) -> Span {
        let text = span.text(self.source);
        let leading = text.len() - text.trim_start().len();
        let trailing = text.len() - text.trim_end().len();
        Span::new(span.start as usize + leading, (span.end as usize).saturating_sub(trailing))
    }

    /// 创建节点并挂到当前块下。
    fn push(&mut self, kind: NodeKind, span: Span, name: Span, value: Span) -> u32 {
        let index = self.ast.nodes.len() as u32;
        self.ast.nodes.push(Node {
            kind,
            span,
            name,
            value,
            parent: NONE,
            first_child: NONE,
            last_child: NONE,
            next_sibling: NONE,
            subtree_end: index + 1,
        });

        match self.stack.last().copied() {
            Some(parent) => {
                let previous = self.ast.node(parent).last_child;
                if previous == NONE {
                    self.ast.node_mut(parent).first_child = index;
                } else {
                    self.ast.node_mut(previous).next_sibling = index;
                }
                self.ast.node_mut(parent).last_child = index;
                self.ast.node_mut(index).parent = parent;
            }
            None => self.ast.roots.push(index),
        }

        index
    }

    /// 记录一条错误。
    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic { span, severity: Severity::Error, message: message.into() });
    }

    /// 记录一条警告。
    fn warn(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic { span, severity: Severity::Warning, message: message.into() });
    }
}

/// 属性名与选择器里允许出现的字符。
fn is_name_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'$' | b'@' | b':') || byte >= 0x80
}

/// 在字节序列中查找子串。
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 规则与声明应形成父子关系。
    #[test]
    fn parses_rule_with_declarations() {
        let source = "a { color: red; }\nb { margin: 0 }";
        let result = parse(source, Dialect::Css);
        assert!(result.diagnostics.is_empty(), "诊断：{:?}", result.diagnostics);

        assert_eq!(result.ast.roots.len(), 2);
        let first = result.ast.roots[0];
        assert_eq!(result.ast.node(first).name.text(source), "a");
        assert_eq!(result.ast.children(first).count(), 1);

        let declaration = result.ast.children(first).next().unwrap();
        assert_eq!(result.ast.node(declaration).name.text(source), "color");
        assert_eq!(result.ast.node(declaration).value.text(source), "red");
    }

    /// at 规则带块与不带块都要能解析。
    #[test]
    fn parses_at_rules() {
        let source = "@import \"a.css\";\n@media (min-width: 600px) { a { color: red } }";
        let result = parse(source, Dialect::Css);
        assert!(result.diagnostics.is_empty(), "诊断：{:?}", result.diagnostics);

        let rules: Vec<u32> = result.ast.nodes_of(NodeKind::AtRule).collect();
        assert_eq!(rules.len(), 2);
        assert_eq!(result.ast.node(rules[0]).name.text(source), "@import");
        assert_eq!(result.ast.node(rules[1]).name.text(source), "@media");
    }

    /// 未闭合的块要报错。
    #[test]
    fn reports_unclosed_block() {
        let result = parse("a { color: red;", Dialect::Css);
        assert!(result.diagnostics.iter().any(|item| item.message.contains("未闭合")));
    }

    /// 多余的右花括号要报错。
    #[test]
    fn reports_extra_brace() {
        let result = parse("a { color: red; }\n}", Dialect::Css);
        assert!(result.diagnostics.iter().any(|item| item.message.contains("多余")));
    }

    /// SCSS 的 `//` 行注释不参与解析。
    #[test]
    fn skips_line_comments_in_scss() {
        let source = "// 注释 { 假规则\na { color: red; }";
        let result = parse(source, Dialect::Scss);
        assert!(result.diagnostics.is_empty(), "诊断：{:?}", result.diagnostics);
        assert_eq!(result.ast.roots.len(), 1);
    }

    /// 值里出现的 URL 与括号不影响属性/值拆分。
    #[test]
    fn keeps_parenthesised_values_intact() {
        let source = "a { background: url(data:image/png;base64,AAA); }";
        let result = parse(source, Dialect::Css);
        let rule = result.ast.roots[0];
        let declaration = result.ast.children(rule).next().unwrap();
        assert_eq!(result.ast.node(declaration).name.text(source).trim(), "background");
        assert!(result.ast.node(declaration).value.text(source).contains("base64"));
    }

    /// 嵌套规则（SCSS）作为子节点出现。
    #[test]
    fn parses_nested_rules() {
        let source = ".a { color: red; .b { color: blue; } }";
        let result = parse(source, Dialect::Scss);
        let outer = result.ast.roots[0];
        let kinds: Vec<NodeKind> = result.ast.children(outer).map(|index| result.ast.node(index).kind).collect();
        assert_eq!(kinds, vec![NodeKind::Declaration, NodeKind::Rule]);
    }
}
