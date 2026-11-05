//! 各 LSP 能力在语法树上的实现。
//!
//! 所有实现共用一个 `View`：原文、语法树与行索引。
//! 能力函数只读取这三者，不持有状态，因此可以随文档版本缓存或丢弃。

pub mod completion;
pub mod diagnostics;
pub mod hover;
pub mod navigation;
pub mod symbols;

use serde_json::{Value, json};

use crate::ast::Ast;
use lspd_core::text::{LineIndex, Span};

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

    /// 字节偏移换算成 LSP 的 `Position`。
    pub fn position(&self, offset: u32) -> Value {
        let (line, character) = self.lines.position(offset);
        json!({ "line": line, "character": character })
    }

    /// 字节区间换算成 LSP 的 `Range`。
    pub fn range(&self, span: Span) -> Value {
        json!({ "start": self.position(span.start), "end": self.position(span.end) })
    }

    /// 取区间对应的源码文本。
    pub fn text(&self, span: Span) -> &'a str {
        span.text(self.source)
    }
}

/// 构造一条 LSP 诊断。
pub fn diagnostic(view: &View, span: Span, severity: u8, message: impl Into<String>) -> Value {
    json!({
        "range": view.range(span),
        "severity": severity,
        "source": "lspd",
        "message": message.into(),
    })
}
