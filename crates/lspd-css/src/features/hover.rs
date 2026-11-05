//! 悬停：属性、at 规则、伪类/伪元素、命名颜色的文档。

use serde_json::{json, Value};

use crate::data;
use crate::features::{View, word_at};
use lspd_core::text::Span;

/// 计算悬停内容。
pub fn hover(view: &View<'_>, offset: u32) -> Option<Value> {
    let word = word_at(view.source, offset)?;
    let text = view.text(word);
    let lower = text.to_ascii_lowercase();

    let markdown = if let Some(entry) = data::property(&lower) {
        property_markdown(entry)
    } else if let Some(entry) = data::at_rule(&lower) {
        format!("### `{}`\n\n{}", entry.name, entry.description)
    } else if let Some(entry) = data::pseudo_element(&lower) {
        format!("### `{}`\n\n{}", entry.name, entry.description)
    } else if let Some(entry) = data::pseudo_class(&lower) {
        format!("### `{}`\n\n{}", entry.name, entry.description)
    } else if let Some(color) = data::named_color(&lower) {
        format!(
            "### `{}`\n\n颜色 `#{:02x}{:02x}{:02x}`",
            color.name, color.red, color.green, color.blue
        )
    } else {
        return None;
    };

    Some(json!({
        "contents": { "kind": "markdown", "value": markdown },
        "range": view.range(trim_span(word, text)),
    }))
}

/// 属性文档：说明 + 值语法。
fn property_markdown(entry: &data::EntryData) -> String {
    let mut markdown = format!("### `{}`\n\n{}", entry.name, entry.description);
    if !entry.syntax.is_empty() {
        markdown.push_str(&format!("\n\n值语法：`{}`", entry.syntax));
    }
    if !entry.at_rule.is_empty() {
        markdown.push_str(&format!("\n\n仅用于 `{}`", entry.at_rule));
    }
    markdown
}

/// 去掉首尾空白，避免悬停下划线覆盖空格。
fn trim_span(span: Span, text: &str) -> Span {
    let start = text.len() - text.trim_start().len();
    let end = span.start + text.trim_end().len() as u32;
    Span { start: span.start + start as u32, end }
}
