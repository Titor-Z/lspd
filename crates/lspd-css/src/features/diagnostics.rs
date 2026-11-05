//! 诊断：结构诊断（解析期产出）与数据驱动诊断（未知属性、未知 at 规则）。

use serde_json::Value;

use crate::ast::NodeKind;
use crate::data;
use crate::features::{View, diagnostic};
use crate::parser::{Diagnostic, Severity};

/// 单次发布的诊断上限。
const MAX_DIAGNOSTICS: usize = 200;

/// LSP 严重程度。
mod level {
    /// 错误
    pub const ERROR: u8 = 1;
    /// 警告
    pub const WARNING: u8 = 2;
}

/// SCSS 与 Less 专有的 at 规则，数据表里没有但合法。
const DIALECT_AT_RULES: &[&str] = &[
    "@include", "@mixin", "@if", "@else", "@each", "@for", "@while", "@function", "@return", "@extend", "@use",
    "@forward", "@debug", "@warn", "@error", "@at-root", "@content", "@import", "@plugin", "@reference",
];

/// 汇总一份样式表的全部诊断。
pub fn collect(view: &View<'_>, structural: &[Diagnostic]) -> Vec<Value> {
    let mut diagnostics: Vec<Value> = Vec::with_capacity(structural.len());

    for item in structural {
        let severity = match item.severity {
            Severity::Error => level::ERROR,
            Severity::Warning => level::WARNING,
        };
        diagnostics.push(diagnostic(view, item.span, severity, item.message.clone()));
        if diagnostics.len() >= MAX_DIAGNOSTICS {
            return diagnostics;
        }
    }

    for index in view.ast.nodes_of(NodeKind::Declaration) {
        let node = view.ast.node(index);
        let name = view.text(node.name).trim();

        if name.is_empty() || name.starts_with("--") || name.starts_with('$') || name.starts_with('@') {
            continue;
        }
        if data::is_known_property(name) {
            continue;
        }

        let span = leading_trim(node.name, view.text(node.name));
        diagnostics.push(diagnostic(view, span, level::WARNING, format!("未知属性 `{name}`")));
        if diagnostics.len() >= MAX_DIAGNOSTICS {
            return diagnostics;
        }
    }

    for index in view.ast.nodes_of(NodeKind::AtRule) {
        let node = view.ast.node(index);
        let name = view.text(node.name).to_ascii_lowercase();
        if name.is_empty() || data::at_rule(&name).is_some() || DIALECT_AT_RULES.contains(&name.as_str()) {
            continue;
        }

        diagnostics.push(diagnostic(view, node.name, level::WARNING, format!("未知 at 规则 `{name}`")));
        if diagnostics.len() >= MAX_DIAGNOSTICS {
            return diagnostics;
        }
    }

    diagnostics
}

/// 去掉前导空白，避免诊断下划线画到空格上。
fn leading_trim(span: lspd_core::text::Span, text: &str) -> lspd_core::text::Span {
    let offset = text.len() - text.trim_start().len();
    lspd_core::text::Span { start: span.start + offset as u32, end: span.end }
}
