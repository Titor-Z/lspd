//! 各 LSP 能力实现。

pub mod completion;
pub mod formatting;
pub mod hover;
pub mod symbols;

use serde_json::{Value, json};
use crate::ast::Ast;
use lspd_core::text::{LineIndex, Span};

/// 文档视图。
pub struct View<'a> {
    /// 原文
    pub source: &'a str,
    /// 语法树
    pub ast: &'a Ast,
    /// 行索引
    pub lines: &'a LineIndex<'a>,
    /// schema 提供者
    pub schema: &'a dyn crate::SchemaProvider,
}

impl<'a> View<'a> {
    /// 构造视图。
    pub fn new(source: &'a str, ast: &'a Ast, lines: &'a LineIndex<'a>, schema: &'a dyn crate::SchemaProvider) -> Self {
        Self { source, ast, lines, schema }
    }

    /// 偏移转 Position。
    pub fn position(&self, offset: u32) -> Value {
        let (line, character) = self.lines.position(offset);
        json!({ "line": line, "character": character })
    }

    /// 区间转 Range。
    pub fn range(&self, span: Span) -> Value {
        json!({ "start": self.position(span.start), "end": self.position(span.end) })
    }

    /// 取区间文本。
    pub fn text(&self, span: Span) -> &'a str {
        self.source.get(span.start as usize..span.end as usize).unwrap_or("")
    }
}

/// 诊断 JSON。
pub fn diagnostic(view: &View, span: Span, severity: u8, message: impl Into<String>) -> Value {
    json!({
        "range": view.range(span),
        "severity": severity,
        "source": "lspd",
        "message": message.into(),
    })
}
