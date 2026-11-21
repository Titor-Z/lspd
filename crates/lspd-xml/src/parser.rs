//! 容错 XML 解析器：单遍扫描，直接构建扁平语法树。
//!
//! 设计取舍：
//! - LSP 场景优先"尽量产出可用树"，错误后继续扫描；
//! - 不做 DTD/XSD 校验，不解析外部实体；
//! - 词法与语法合一，不分配中间 token 数组。

use crate::ast::{Ast, Attribute, NONE, Node, NodeKind};
use lspd_core::text::Span;

/// 诊断严重程度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// 结构错误
    Error,
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

/// 解析结果。
pub struct ParseResult {
    /// 语法树
    pub ast: Ast,
    /// 结构诊断
    pub diagnostics: Vec<Diagnostic>,
}

/// 解析 XML 文本。
pub fn parse(source: &str) -> ParseResult {
    Parser::new(source).run()
}

/// 单遍扫描解析器。
struct Parser<'a> {
    source: &'a str,
    bytes: &'a [u8],
    position: usize,
    ast: Ast,
    stack: Vec<u32>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            position: 0,
            ast: Ast::default(),
            stack: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn run(mut self) -> ParseResult {
        while self.position < self.bytes.len() {
            if self.bytes[self.position] == b'<' && self.try_markup() {
                continue;
            }
            self.parse_text();
        }

        while let Some(open) = self.stack.pop() {
            let name = self.ast.node(open).name;
            let message = format!("缺少结束标签 </{}>", self.text(name));
            self.diagnostics.push(Diagnostic { span: name, severity: Severity::Error, message });
            self.close_node(open, self.source.len());
        }

        ParseResult { ast: self.ast, diagnostics: self.diagnostics }
    }

    fn try_markup(&mut self) -> bool {
        let Some(next) = self.peek(1) else { return false };
        match next {
            b'!' => {
                if self.starts_with(b"<!--") {
                    self.parse_comment();
                } else if self.starts_with(b"<![CDATA[") {
                    self.parse_cdata();
                } else if self.starts_with_ci(b"<!DOCTYPE") {
                    self.parse_doctype();
                } else {
                    return false;
                }
                true
            }
            b'?' => { self.parse_processing_instruction(); true }
            b'/' => self.parse_close_tag(),
            b if is_name_start(b) => { self.parse_open_tag(); true }
            _ => false,
        }
    }

    fn parse_text(&mut self) {
        let start = self.position;
        while self.position < self.bytes.len() {
            if self.bytes[self.position] == b'<' && self.looks_like_markup() {
                break;
            }
            self.position += 1;
        }
        if start == self.position {
            self.position += 1;
            return;
        }
        let index = self.push_node(NodeKind::Text, Span::new(start, self.position), Span::new(start, start), 0);
        self.attach(index);
        self.close_node(index, self.position);
    }

    fn parse_comment(&mut self) {
        let start = self.position;
        self.position += 4;
        let end = self.find_bytes(b"-->").map(|p| p + 3).unwrap_or(self.bytes.len());
        if end == self.bytes.len() {
            self.diagnostics.push(Diagnostic { span: Span::new(start, start + 4), severity: Severity::Error, message: "注释未闭合".to_string() });
        }
        self.position = end;
        let index = self.push_node(NodeKind::Comment, Span::new(start, end), Span::new(start, start), 0);
        self.attach(index);
        self.close_node(index, end);
    }

    fn parse_cdata(&mut self) {
        let start = self.position;
        self.position += 9;
        let end = self.find_bytes(b"]]>").map(|p| p + 3).unwrap_or(self.bytes.len());
        if end == self.bytes.len() {
            self.diagnostics.push(Diagnostic { span: Span::new(start, start + 9), severity: Severity::Error, message: "CDATA 未闭合".to_string() });
        }
        self.position = end;
        let index = self.push_node(NodeKind::CData, Span::new(start, end), Span::new(start, start), 0);
        self.attach(index);
        self.close_node(index, end);
    }

    fn parse_doctype(&mut self) {
        let start = self.position;
        self.position += 2; // <!
        while self.position < self.bytes.len() && self.bytes[self.position].is_ascii_whitespace() {
            self.position += 1;
        }
        let name_start = self.position;
        self.scan_name();
        let name = Span::new(name_start, self.position);
        let mut depth = 0u32;
        let mut quote = 0u8;
        while self.position < self.bytes.len() {
            let b = self.bytes[self.position];
            self.position += 1;
            match b {
                b'"' | b'\'' if quote == 0 => quote = b,
                b'"' | b'\'' if quote == b => quote = 0,
                b'[' if quote == 0 => depth += 1,
                b']' if quote == 0 => depth = depth.saturating_sub(1),
                b'>' if quote == 0 && depth == 0 => break,
                _ => {}
            }
        }
        let end = self.position;
        let index = self.push_node(NodeKind::Doctype, Span::new(start, end), name, 0);
        self.attach(index);
        self.close_node(index, end);
    }

    fn parse_processing_instruction(&mut self) {
        let start = self.position;
        self.position += 2; // <?
        let name_start = self.position;
        self.scan_name();
        let name = Span::new(name_start, self.position);
        let end = self.find_bytes(b"?>").map(|p| p + 2).unwrap_or(self.bytes.len());
        if end == self.bytes.len() {
            self.diagnostics.push(Diagnostic { span: Span::new(start, start + 2), severity: Severity::Error, message: "处理指令未闭合".to_string() });
        }
        self.position = end;
        let index = self.push_node(NodeKind::ProcessingInstruction, Span::new(start, end), name, 0);
        self.attach(index);
        self.close_node(index, end);
    }

    fn parse_open_tag(&mut self) {
        let start = self.position;
        self.position += 1;
        let name_start = self.position;
        self.scan_name();
        let name = Span::new(name_start, self.position);
        if name.is_empty() {
            return;
        }
        let attribute_start = self.ast.attributes.len() as u32;
        self.parse_attributes();
        self.skip_space();
        let self_closing = self.eat(b'/');
        let closed = self.eat(b'>');
        if !closed {
            self.diagnostics.push(Diagnostic { span: name, severity: Severity::Error, message: "开始标签未闭合".to_string() });
            self.recover_to_tag_boundary();
        }
        let end = self.position;
        let attr_len = self.ast.attributes.len() as u32 - attribute_start;
        let index = self.push_node(NodeKind::Element, Span::new(start, end), name, attribute_start);
        self.ast.node_mut(index).attribute_len = attr_len;
        self.attach(index);
        if self_closing || !closed {
            self.close_node(index, end);
        } else {
            self.stack.push(index);
        }
    }

    fn parse_close_tag(&mut self) -> bool {
        let start = self.position;
        if !self.starts_with(b"</") {
            return false;
        }
        let Some(next) = self.peek(2) else { return false };
        if !is_name_start(next) {
            return false;
        }
        self.position += 2;
        let name_start = self.position;
        self.scan_name();
        let name = Span::new(name_start, self.position);
        self.skip_space();
        if !self.eat(b'>') {
            self.diagnostics.push(Diagnostic { span: Span::new(start, self.position), severity: Severity::Error, message: "结束标签未闭合".to_string() });
            self.recover_to_tag_boundary();
        }
        let end = self.position;
        let close_name = self.text(name);

        if let Some(pos) = self.stack.iter().rposition(|open| self.text(self.ast.node(*open).name) == close_name) {
            while self.stack.len() - 1 > pos {
                let open = self.stack.pop().expect("栈非空");
                let open_name = self.ast.node(open).name;
                self.diagnostics.push(Diagnostic {
                    span: open_name,
                    severity: Severity::Error,
                    message: format!("缺少结束标签 </{}>", self.text(open_name)),
                });
                self.close_node(open, start);
            }
            let open = self.stack.pop().expect("匹配元素存在");
            self.close_node(open, end);
        } else {
            self.diagnostics.push(Diagnostic { span: Span::new(start, end), severity: Severity::Error, message: format!("未匹配的结束标签 </{}>", close_name) });
        }
        true
    }

    fn parse_attributes(&mut self) {
        loop {
            self.skip_space();
            if self.position >= self.bytes.len() {
                break;
            }
            let b = self.bytes[self.position];
            if b == b'>' || (b == b'/' && self.peek(1) == Some(b'>')) {
                break;
            }
            let start = self.position;
            if !is_name_start(b) {
                self.position += 1;
                continue;
            }
            let name_start = self.position;
            self.scan_name();
            let name = Span::new(name_start, self.position);
            self.skip_space();
            let value = if self.eat(b'=') {
                self.skip_space();
                Some(self.scan_attr_value())
            } else {
                None
            };
            self.ast.attributes.push(Attribute { span: Span::new(start, self.position), name, value });
        }
    }

    fn scan_attr_value(&mut self) -> Span {
        let start = self.position;
        if self.position >= self.bytes.len() {
            return Span::new(start, start);
        }
        let quote = self.bytes[self.position];
        if quote == b'"' || quote == b'\'' {
            self.position += 1;
            while self.position < self.bytes.len() && self.bytes[self.position] != quote {
                self.position += 1;
            }
            if self.position < self.bytes.len() {
                self.position += 1;
            } else {
                self.diagnostics.push(Diagnostic { span: Span::new(start, start + 1), severity: Severity::Error, message: "属性值引号未闭合".to_string() });
            }
            return Span::new(start, self.position);
        }
        while self.position < self.bytes.len() {
            let b = self.bytes[self.position];
            if b.is_ascii_whitespace() || b == b'>' {
                break;
            }
            if b == b'/' && self.peek(1) == Some(b'>') {
                break;
            }
            self.position += 1;
        }
        Span::new(start, self.position)
    }

    fn looks_like_markup(&self) -> bool {
        let Some(next) = self.peek(1) else { return false };
        next == b'!' || next == b'?' || next == b'/' || is_name_start(next)
    }

    fn push_node(&mut self, kind: NodeKind, span: Span, name: Span, attribute_start: u32) -> u32 {
        let index = self.ast.nodes.len() as u32;
        self.ast.nodes.push(Node {
            kind,
            span,
            name,
            parent: NONE,
            first_child: NONE,
            last_child: NONE,
            next_sibling: NONE,
            attribute_start,
            attribute_len: 0,
            subtree_end: index + 1,
        });
        index
    }

    fn attach(&mut self, child: u32) {
        match self.stack.last().copied() {
            Some(parent) => {
                self.ast.node_mut(child).parent = parent;
                let last = self.ast.node(parent).last_child;
                if last == NONE {
                    self.ast.node_mut(parent).first_child = child;
                } else {
                    self.ast.node_mut(last).next_sibling = child;
                }
                self.ast.node_mut(parent).last_child = child;
            }
            None => self.ast.roots.push(child),
        }
    }

    fn close_node(&mut self, index: u32, end: usize) {
        let subtree_end = self.ast.nodes.len() as u32;
        let node = self.ast.node_mut(index);
        node.span.end = end as u32;
        node.subtree_end = subtree_end;
    }

    fn scan_name(&mut self) {
        while self.position < self.bytes.len() && is_name_char(self.bytes[self.position]) {
            self.position += 1;
        }
    }

    fn skip_space(&mut self) {
        while self.position < self.bytes.len() && self.bytes[self.position].is_ascii_whitespace() {
            self.position += 1;
        }
    }

    fn recover_to_tag_boundary(&mut self) {
        while self.position < self.bytes.len() {
            let b = self.bytes[self.position];
            if b == b'>' {
                self.position += 1;
                break;
            }
            if b == b'<' {
                break;
            }
            self.position += 1;
        }
    }

    fn find_bytes(&self, needle: &[u8]) -> Option<usize> {
        self.source[self.position..].find(std::str::from_utf8(needle).ok()?).map(|p| self.position + p)
    }

    fn starts_with(&self, needle: &[u8]) -> bool {
        self.bytes.get(self.position..self.position + needle.len()) == Some(needle)
    }

    fn starts_with_ci(&self, needle: &[u8]) -> bool {
        let Some(slice) = self.bytes.get(self.position..self.position + needle.len()) else { return false };
        slice.iter().zip(needle).all(|(a, b)| a.to_ascii_lowercase() == *b)
    }

    fn peek(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.position + offset).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.bytes.get(self.position).copied() == Some(byte) {
            self.position += 1;
            return true;
        }
        false
    }

    fn text(&self, span: Span) -> &str {
        span.text(self.source)
    }
}

fn is_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b':')
}

fn is_name_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'-' | b'.')
}
