//! 补全：属性名、属性值、at 规则、伪类与伪元素、文档内变量。

use serde_json::{json, Value};

use crate::ast::NodeKind;
use crate::data;
use crate::features::{View, find_colon, inside_block, statement_start};
use lspd_core::text::Span;

/// LSP 的 CompletionItemKind 取值。
const KIND_PROPERTY: u8 = 10;
/// 关键字按 Keyword 呈现。
const KIND_KEYWORD: u8 = 14;
/// 伪类与 at 规则按 Module 呈现。
const KIND_MODULE: u8 = 9;

/// 单次返回条目上限。
const MAX_ITEMS: usize = 200;

/// CSS 全局关键字，任何属性都接受。
const WIDE_KEYWORDS: &[&str] = &["inherit", "initial", "unset", "revert", "revert-layer"];

/// 常用函数，值位置的补全候选。
const FUNCTIONS: &[&str] = &["calc(", "var(", "rgb(", "hsl(", "min(", "max(", "clamp(", "url("];

/// 补全上下文。
enum Context {
    /// 块内的属性名位置
    Property,
    /// 块内的值位置
    Value,
    /// 选择器中的伪类或伪元素
    Pseudo { double_colon: bool },
    /// at 规则位置
    AtRule,
    /// 其余位置不补全
    None,
}

/// 计算补全结果。
pub fn complete(view: &View<'_>, offset: u32) -> Value {
    let context = detect(view, offset);
    let (start, _) = replace_range(view.source, offset);
    let prefix = &view.source[start as usize..offset as usize];
    let mut items: Vec<Value> = Vec::new();

    match context {
        Context::Property => {
            push_properties(view, prefix, start, offset, &mut items);
            push_variables(view, prefix, start, offset, &mut items);
        }
        Context::Value => push_values(prefix, start, offset, &mut items),
        Context::Pseudo { double_colon } => push_pseudos(view, double_colon, prefix, start, offset, &mut items),
        Context::AtRule => push_at_rules(view, prefix, start, offset, &mut items),
        Context::None => {}
    }

    json!({ "isIncomplete": false, "items": items })
}

/// 判定补全上下文。
fn detect(view: &View<'_>, offset: u32) -> Context {
    let source = view.source;
    let start = statement_start(source, offset);
    let prefix = &source[start as usize..offset as usize];
    let trimmed = prefix.trim_start();

    if !inside_block(source, offset) {
        return if trimmed.starts_with('@') {
            Context::AtRule
        } else if let Some(position) = prefix.rfind(':') {
            Context::Pseudo { double_colon: prefix[..position].ends_with(':') }
        } else {
            Context::None
        };
    }

    // 块内：冒号之前是属性名，之后是值。变量声明 `$x: value` 同样适用
    if find_colon(source, start, offset).is_some() {
        Context::Value
    } else {
        Context::Property
    }
}

/// 计算替换区间：单词起点到光标。
fn replace_range(source: &str, offset: u32) -> (u32, u32) {
    let bytes = source.as_bytes();
    let mut start = (offset as usize).min(bytes.len());
    while start > 0 && is_word_byte(bytes[start - 1]) {
        start -= 1;
    }
    (start as u32, offset)
}

/// 单词字符集。
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') || byte >= 0x80
}

/// 推送属性名补全。
fn push_properties(view: &View<'_>, prefix: &str, start: u32, offset: u32, items: &mut Vec<Value>) {
    let range = text_edit(view, start, offset);
    let prefix = prefix.to_ascii_lowercase();

    for entry in data::PROPERTIES {
        if !prefix.is_empty() && !entry.name.starts_with(&prefix) {
            continue;
        }
        if items.len() >= MAX_ITEMS {
            return;
        }
        items.push(json!({
            "label": entry.name,
            "kind": KIND_PROPERTY,
            "detail": if entry.syntax.is_empty() { "属性".to_string() } else { format!("属性：{}", entry.syntax) },
            "documentation": { "kind": "markdown", "value": entry.description },
            "textEdit": { "range": range, "newText": entry.name },
        }));
    }
}

/// 推送文档内变量补全。
fn push_variables(view: &View<'_>, prefix: &str, start: u32, offset: u32, items: &mut Vec<Value>) {
    let range = text_edit(view, start, offset);

    for index in view.ast.nodes_of(NodeKind::Declaration) {
        let name = view.text(view.ast.node(index).name).trim();
        if !(name.starts_with("--") || name.starts_with('$') || name.starts_with('@')) {
            continue;
        }
        if !prefix.is_empty() && !name.starts_with(prefix) {
            continue;
        }
        items.push(json!({
            "label": name,
            "kind": KIND_PROPERTY,
            "detail": "变量",
            "textEdit": { "range": range, "newText": name },
        }));
    }
}

/// 推送值补全：全局关键字、函数、命名颜色。
fn push_values(prefix: &str, start: u32, offset: u32, items: &mut Vec<Value>) {
    let prefix = prefix.to_ascii_lowercase();

    for keyword in WIDE_KEYWORDS {
        if !prefix.is_empty() && !keyword.starts_with(&prefix) {
            continue;
        }
        items.push(json!({ "label": keyword, "kind": KIND_KEYWORD, "detail": "全局关键字" }));
    }
    for function in FUNCTIONS {
        if !prefix.is_empty() && !function.starts_with(&prefix) {
            continue;
        }
        items.push(json!({ "label": function, "kind": KIND_KEYWORD, "detail": "函数" }));
    }

    for color in data::NAMED_COLORS {
        if !prefix.is_empty() && !color.name.starts_with(&prefix) {
            continue;
        }
        if items.len() >= MAX_ITEMS {
            return;
        }
        items.push(json!({
            "label": color.name,
            "kind": KIND_PROPERTY,
            "detail": format!("颜色 #{:02x}{:02x}{:02x}", color.red, color.green, color.blue),
            "documentation": {
                "kind": "markdown",
                "value": format!("```css\n{}\n```", color.name),
            },
        }));
    }

    let _ = (start, offset);
}

/// 推送伪类或伪元素补全。
fn push_pseudos(view: &View<'_>, double_colon: bool, prefix: &str, start: u32, offset: u32, items: &mut Vec<Value>) {
    let range = text_edit(view, start, offset);
    let table = if double_colon { data::PSEUDO_ELEMENTS } else { data::PSEUDO_CLASSES };

    for entry in table {
        let name = entry.name.trim_start_matches(':');
        let label = if double_colon { format!("::{name}") } else { format!(":{name}") };
        if !prefix.is_empty() && !label.starts_with(prefix) {
            continue;
        }
        if items.len() >= MAX_ITEMS {
            return;
        }
        items.push(json!({
            "label": label,
            "kind": KIND_MODULE,
            "detail": if double_colon { "伪元素" } else { "伪类" },
            "documentation": { "kind": "markdown", "value": entry.description },
            "textEdit": { "range": range, "newText": label },
        }));
    }
}

/// 推送 at 规则补全。
fn push_at_rules(view: &View<'_>, prefix: &str, start: u32, offset: u32, items: &mut Vec<Value>) {
    let range = text_edit(view, start, offset);
    let prefix = prefix.to_ascii_lowercase();

    for entry in data::AT_RULES {
        if !prefix.is_empty() && !entry.name.starts_with(&prefix) {
            continue;
        }
        if items.len() >= MAX_ITEMS {
            return;
        }
        items.push(json!({
            "label": entry.name,
            "kind": KIND_MODULE,
            "detail": "at 规则",
            "documentation": { "kind": "markdown", "value": entry.description },
            "textEdit": { "range": range, "newText": entry.name },
        }));
    }
}

/// 构造替换区间。
fn text_edit(view: &View<'_>, start: u32, offset: u32) -> Value {
    view.range(Span { start, end: offset })
}
