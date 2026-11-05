//! 各 LSP 能力在样式表语法树上的实现。

pub mod color;
pub mod completion;
pub mod diagnostics;
pub mod hover;
pub mod navigation;
pub mod symbols;

use lspd_core::text::{LineIndex, Span};
use serde_json::{json, Value};

use crate::ast::Ast;

/// 一次请求所需的文档视图。
pub struct View<'a> {
    /// 原文
    pub source: &'a str,
    /// 语法树
    pub ast: &'a Ast,
    /// 行索引
    pub lines: &'a LineIndex<'a>,
}

impl<'a> View<'a> {
    /// 建立视图。
    pub fn new(source: &'a str, ast: &'a Ast, lines: &'a LineIndex<'a>) -> Self {
        Self { source, ast, lines }
    }

    /// 字节偏移换算成 LSP `Position`。
    pub fn position(&self, offset: u32) -> Value {
        let (line, character) = self.lines.position(offset);
        json!({ "line": line, "character": character })
    }

    /// 字节区间换算成 LSP `Range`。
    pub fn range(&self, span: Span) -> Value {
        json!({ "start": self.position(span.start), "end": self.position(span.end) })
    }

    /// 取区间对应的源码文本。
    pub fn text(&self, span: Span) -> &'a str {
        span.text(self.source)
    }
}

/// 构造一条 LSP 诊断。
pub fn diagnostic(view: &View<'_>, span: Span, severity: u8, message: impl Into<String>) -> Value {
    json!({
        "range": view.range(span),
        "severity": severity,
        "source": "lspd",
        "message": message.into(),
    })
}

/// 光标处的"单词"区间：属性名、颜色名、变量名都按这套字符集判定。
pub fn word_at(source: &str, offset: u32) -> Option<Span> {
    let bytes = source.as_bytes();
    let position = (offset as usize).min(bytes.len());

    let mut start = position;
    while start > 0 && is_word_byte(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = position;
    while end < bytes.len() && is_word_byte(bytes[end]) {
        end += 1;
    }

    // 变量与伪元素需要把前缀符号一起纳入
    if start > 0 && matches!(bytes[start - 1], b'-' | b'$' | b'@' | b':') {
        start -= 1;
        if start > 0 && bytes[start - 1] == b'-' && bytes[start] == b'-' {
            start -= 1;
        }
        if start > 0 && bytes[start - 1] == b':' && bytes[start] == b':' {
            start -= 1;
        }
    }

    if start == end {
        return None;
    }
    Some(Span::new(start, end))
}

/// 单词字符集。
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') || byte >= 0x80
}

/// 包含给定位置且跨度最小的节点，即最深的节点。
pub fn innermost(ast: &Ast, offset: u32) -> Option<u32> {
    let mut best: Option<(u32, u32)> = None;
    for (index, node) in ast.nodes.iter().enumerate() {
        if node.span.contains(offset) {
            let width = node.span.end - node.span.start;
            if best.is_none_or(|(_, previous)| width < previous) {
                best = Some((index as u32, width));
            }
        }
    }
    best.map(|(index, _)| index)
}

/// 在 `start..end` 内找到括号外的冒号。
pub fn find_colon(source: &str, start: u32, end: u32) -> Option<u32> {
    let bytes = source.as_bytes();
    let mut index = start as usize;
    let mut depth = 0i32;

    while index < (end as usize).min(bytes.len()) {
        match bytes[index] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b':' if depth == 0 => return Some(index as u32),
            _ => {}
        }
        index += 1;
    }

    None
}
