//! 变量导航：`--自定义属性`、SCSS 的 `$变量`、Less 的 `@变量`。
//!
//! 变量是纯文本作用域概念，直接按"名称边界匹配"找全部出现位置，
//! 既覆盖声明与引用，也省掉一套作用域分析。

use serde_json::{json, Value};

use crate::ast::NodeKind;
use crate::data;
use crate::features::{View, word_at};
use lspd_core::text::Span;

/// 转到定义：跳到变量的声明处。
pub fn definition(view: &View<'_>, offset: u32, uri: &str) -> Value {
    let Some((_, name)) = variable_at(view, offset) else {
        return Value::Null;
    };

    let Some(span) = declaration_span(view, &name).or_else(|| occurrences(view, &name).first().copied()) else {
        return Value::Null;
    };

    json!([{ "uri": uri, "range": view.range(span) }])
}

/// 查找引用：列出变量名在全文档的出现位置。
pub fn references(view: &View<'_>, offset: u32, uri: &str) -> Value {
    let Some((_, name)) = variable_at(view, offset) else {
        return json!([]);
    };

    let locations: Vec<Value> =
        occurrences(view, &name).into_iter().map(|span| json!({ "uri": uri, "range": view.range(span) })).collect();

    Value::Array(locations)
}

/// 重命名前的占位信息。
pub fn prepare_rename(view: &View<'_>, offset: u32) -> Option<Value> {
    let (span, _) = variable_at(view, offset)?;
    let placeholder = view.text(span).to_string();
    Some(json!({ "range": view.range(span), "placeholder": placeholder }))
}

/// 重命名：把变量名在全文档的出现位置一起替换。
pub fn rename(view: &View<'_>, offset: u32, uri: &str, new_name: &str) -> Value {
    let Some((_, name)) = variable_at(view, offset) else {
        return Value::Null;
    };

    let edits: Vec<Value> = occurrences(view, &name)
        .into_iter()
        .map(|span| json!({ "range": view.range(span), "newText": new_name }))
        .collect();

    json!({ "changes": { uri: edits } })
}

/// 取光标处的变量名。
fn variable_at(view: &View<'_>, offset: u32) -> Option<(Span, String)> {
    let span = word_at(view.source, offset)?;
    let name = view.text(span);

    let is_variable = name.starts_with("--") || name.starts_with('$') || is_less_variable(name);
    if !is_variable {
        return None;
    }
    Some((span, name.to_string()))
}

/// Less 变量以 `@` 开头，排除 at 规则。
fn is_less_variable(name: &str) -> bool {
    name.starts_with('@') && data::at_rule(&name.to_ascii_lowercase()).is_none()
}

/// 找变量的声明区间（声明节点的属性名部分）。
fn declaration_span(view: &View<'_>, name: &str) -> Option<Span> {
    for index in view.ast.nodes_of(NodeKind::Declaration) {
        let node = view.ast.node(index);
        let text = view.text(node.name);
        let trimmed = text.trim();
        if trimmed != name {
            continue;
        }
        let offset = text.len() - text.trim_start().len();
        return Some(Span { start: node.name.start + offset as u32, end: node.name.start + offset as u32 + name.len() as u32 });
    }
    None
}

/// 按名称边界找出全部出现位置。
fn occurrences(view: &View<'_>, name: &str) -> Vec<Span> {
    let bytes = view.source.as_bytes();
    let needle = name.as_bytes();
    let mut found = Vec::new();
    let mut index = 0usize;

    while index + needle.len() <= bytes.len() {
        if &bytes[index..index + needle.len()] != needle {
            index += 1;
            continue;
        }

        let before_ok = index == 0 || !is_word_byte(bytes[index - 1]);
        let after = bytes.get(index + needle.len()).copied();
        let after_ok = after.is_none_or(|byte| !is_word_byte(byte));

        if before_ok && after_ok {
            found.push(Span::new(index, index + needle.len()));
            index += needle.len();
        } else {
            index += 1;
        }
    }

    found
}

/// 单词字符集。
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') || byte >= 0x80
}
