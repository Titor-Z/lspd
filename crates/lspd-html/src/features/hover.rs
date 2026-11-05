//! 悬停提示：展示标签与属性的文档，属性有枚举取值时一并列出。

use serde_json::{Value, json};

use crate::data;
use crate::features::View;
use lspd_core::text::Span;

/// 计算悬停内容；光标不在标签名或属性上时返回 `None`。
pub fn hover(view: &View<'_>, offset: u32) -> Option<Value> {
    let index = view.ast.element_at(offset)?;
    let node = view.ast.node(index);

    if node.name.contains(offset) {
        let tag_name = view.text(node.name).to_ascii_lowercase();
        let tag = data::tag(&tag_name)?;
        let text = format!("### `<{}>`\n\n{}", tag.name, tag.description);
        return Some(build(view, node.name, text));
    }

    for attribute in view.ast.attributes_of(node) {
        let in_name = attribute.name.contains(offset);
        let in_value = attribute.value.is_some_and(|value| value.contains(offset));
        if !in_name && !in_value {
            continue;
        }

        let tag_name = view.text(node.name).to_ascii_lowercase();
        let attribute_name = view.text(attribute.name).to_ascii_lowercase();
        let Some(attribute_data) = data::attribute(&tag_name, &attribute_name) else {
            continue;
        };

        let mut text = format!(
            "### `{}`\n\n{}",
            attribute_data.name, attribute_data.description
        );
        if !attribute_data.values.is_empty() {
            text.push_str("\n\n允许取值：");
            for value in attribute_data.values {
                text.push_str(&format!("\n- `{}` — {}", value.name, value.description));
            }
        }
        let span = if in_name {
            attribute.name
        } else {
            attribute.span
        };
        return Some(build(view, span, text));
    }

    None
}

/// 组装 LSP 的 Hover 结构。
fn build(view: &View<'_>, span: Span, markdown: String) -> Value {
    json!({
        "contents": { "kind": "markdown", "value": markdown },
        "range": view.range(span),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use lspd_core::text::LineIndex;

    /// 取悬停文本；无悬停返回空串。
    fn hover_text(source: &str, offset: u32) -> String {
        let parsed = parser::parse(source);
        let lines = LineIndex::new(source);
        let view = View::new(source, &parsed.ast, &lines);
        hover(&view, offset)
            .and_then(|value| value["contents"]["value"].as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// 光标在标签名上时给出标签文档。
    #[test]
    fn hovers_tag() {
        let text = hover_text("<div>x</div>", 2);
        assert!(text.contains("<div>"), "实际内容：{text}");
    }

    /// 光标在属性名上时给出属性文档。
    #[test]
    fn hovers_attribute() {
        let text = hover_text("<a href=\"/x\">x</a>", 5);
        assert!(text.contains("href"), "实际内容：{text}");
    }

    /// 光标在无文档的普通文本上不产生悬停。
    #[test]
    fn no_hover_on_text() {
        let text = hover_text("<div>纯文本</div>", 7);
        assert!(text.is_empty());
    }
}
