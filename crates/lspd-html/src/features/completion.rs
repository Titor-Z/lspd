//! 补全：标签名、属性名、属性值、实体。
//!
//! 上下文判定按文本回扫完成，不依赖语法树——补全常发生在半个标签上，
//! 此时语法树只包含一个残缺节点，文本回扫更可靠。

use serde_json::{Value, json};

use crate::data;
use crate::features::View;

/// LSP 的 CompletionItemKind 取值。
const KIND_CLASS: u8 = 7;
/// 属性按属性（Property）呈现。
const KIND_PROPERTY: u8 = 10;

/// 补全项来源上限，避免一次返回过多条目拖慢客户端。
const MAX_ITEMS: usize = 200;

/// 补全上下文。
enum Context {
    /// 标签名位置：`<di|`
    TagName { start: u32 },
    /// 标签内：补属性名
    Attribute { tag: String, start: u32 },
    /// 属性值位置：补允许取值
    AttributeValue {
        tag: String,
        attribute: String,
        start: u32,
    },
    /// 文本位置：补实体的场景较少，暂不处理
    None,
}

/// 计算补全结果。
pub fn complete(view: &View<'_>, offset: u32) -> Value {
    let context = detect_context(view, offset);
    let mut items: Vec<Value> = Vec::new();

    match context {
        Context::TagName { start } => push_tags(view, start, offset, &mut items),
        Context::Attribute { tag, start } => push_attributes(view, &tag, start, offset, &mut items),
        Context::AttributeValue {
            tag,
            attribute,
            start,
        } => push_values(view, &tag, &attribute, start, offset, &mut items),
        Context::None => {}
    }

    json!({ "isIncomplete": false, "items": items })
}

/// 判定光标处在哪种补全上下文。
fn detect_context(view: &View<'_>, offset: u32) -> Context {
    let source = view.source;
    let bytes = source.as_bytes();
    let position = offset as usize;

    // 向前找最近的 `<`，途中遇到 `>` 说明不在标签内部
    let mut cursor = position;
    let mut opening = None;
    while cursor > 0 {
        match bytes[cursor - 1] {
            b'>' => return Context::None,
            b'<' => {
                opening = Some(cursor - 1);
                break;
            }
            _ => cursor -= 1,
        }
    }
    let Some(tag_start) = opening else {
        return Context::None;
    };
    let inside = &source[tag_start..position];

    // 统计引号，判断是否落在属性值里
    if let Some((attribute, value_start)) = open_attribute_value(inside) {
        let tag_name = current_tag_name(inside);
        return Context::AttributeValue {
            tag: tag_name,
            attribute,
            start: (tag_start + value_start) as u32,
        };
    }

    match current_tag_name_end(inside) {
        // 标签名还没写完
        None => Context::TagName {
            start: tag_start as u32 + 1,
        },
        Some(name_end) => Context::Attribute {
            tag: inside[1..name_end].to_ascii_lowercase(),
            start: word_start(source, position) as u32,
        },
    }
}

/// 标签名的结束位置。已出现空白表示标签名写完，返回其下标；
/// 尚未出现空白表示仍在输入标签名，返回 `None`。
fn current_tag_name_end(inside: &str) -> Option<usize> {
    match inside.find(|character: char| character.is_whitespace()) {
        Some(index) if index > 1 => Some(index),
        _ => None,
    }
}

/// 当前标签名。
fn current_tag_name(inside: &str) -> String {
    inside
        .trim_start_matches('<')
        .split(|character: char| character.is_whitespace() || character == '>' || character == '/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// 若光标处在未闭合的属性值中，返回属性名与值的起始位置。
fn open_attribute_value(inside: &str) -> Option<(String, usize)> {
    let mut quote: Option<(char, usize)> = None;
    for (index, character) in inside.char_indices() {
        match (quote, character) {
            (None, '"' | '\'') => quote = Some((character, index)),
            (Some((open, _)), current) if current == open => quote = None,
            _ => {}
        }
    }

    let (_, quote_index) = quote?;
    let before = &inside[..quote_index];
    let equals = before.rfind('=')?;
    let name_part = before[..equals].trim_end();
    let name_start = name_part
        .rfind(|character: char| character.is_whitespace())
        .map_or(1, |index| index + 1);
    Some((
        name_part[name_start..].to_ascii_lowercase(),
        quote_index + 1,
    ))
}

/// 从光标处向前取单词起点。
fn word_start(source: &str, position: usize) -> usize {
    let bytes = source.as_bytes();
    let mut start = position;
    while start > 0
        && !matches!(
            bytes[start - 1],
            b' ' | b'\t' | b'\n' | b'\r' | b'<' | b'>' | b'"' | b'\''
        )
    {
        start -= 1;
    }
    start
}

/// 推送标签名补全项。
fn push_tags(view: &View<'_>, start: u32, offset: u32, items: &mut Vec<Value>) {
    let replace = json!({ "start": view.position(start), "end": view.position(offset) });
    let filter = &view.source[start as usize..offset as usize];

    for tag in data::TAGS.iter().take(MAX_ITEMS) {
        if !filter.is_empty() && !tag.name.starts_with(&filter.to_ascii_lowercase()) {
            continue;
        }
        items.push(json!({
            "label": tag.name,
            "kind": KIND_CLASS,
            "detail": "HTML 标签",
            "documentation": { "kind": "markdown", "value": tag.description },
            "textEdit": { "range": replace, "newText": tag.name },
        }));
    }
}

/// 推送属性名补全项：标签专属属性在前，全局属性在后。
fn push_attributes(view: &View<'_>, tag: &str, start: u32, offset: u32, items: &mut Vec<Value>) {
    let replace = json!({ "start": view.position(start), "end": view.position(offset) });
    let filter = view.source[start as usize..offset as usize].to_ascii_lowercase();

    let mut push = |attribute: &'static data::AttributeData, scope: &str| {
        if !filter.is_empty() && !attribute.name.starts_with(&filter) {
            return;
        }
        if items.len() >= MAX_ITEMS {
            return;
        }
        items.push(json!({
            "label": attribute.name,
            "kind": KIND_PROPERTY,
            "detail": scope,
            "documentation": { "kind": "markdown", "value": attribute.description },
            "textEdit": { "range": replace, "newText": attribute.name },
        }));
    };

    if let Some(tag_data) = data::tag(tag) {
        for attribute in tag_data.attributes {
            push(attribute, "标签属性");
        }
    }
    for attribute in data::GLOBAL_ATTRIBUTES {
        push(attribute, "全局属性");
    }
}

/// 推送属性值补全项。
fn push_values(
    view: &View<'_>,
    tag: &str,
    attribute: &str,
    start: u32,
    offset: u32,
    items: &mut Vec<Value>,
) {
    let Some(attribute_data) = data::attribute(tag, attribute) else {
        return;
    };
    let replace = json!({ "start": view.position(start), "end": view.position(offset) });

    for value in attribute_data.values {
        items.push(json!({
            "label": value.name,
            "kind": KIND_PROPERTY,
            "detail": format!("{} 的取值", attribute_data.name),
            "documentation": { "kind": "markdown", "value": value.description },
            "textEdit": { "range": replace, "newText": value.name },
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use lspd_core::text::LineIndex;

    /// 在给定源码与光标位置上求补全标签。
    fn labels(source: &str, offset: u32) -> Vec<String> {
        let parsed = parser::parse(source);
        let lines = LineIndex::new(source);
        let view = View::new(source, &parsed.ast, &lines);
        let result = complete(&view, offset);

        result["items"]
            .as_array()
            .expect("items 应为数组")
            .iter()
            .filter_map(|item| item["label"].as_str().map(str::to_string))
            .collect()
    }

    /// 标签未写完时补标签名，且只保留前缀匹配项。
    #[test]
    fn completes_tag_names() {
        let items = labels("<di", 3);
        assert!(items.contains(&"div".to_string()));
        assert!(items.contains(&"dialog".to_string()));
        assert!(!items.contains(&"span".to_string()));
    }

    /// 标签名之后的空白处补属性名。
    #[test]
    fn completes_attribute_names() {
        let source = "<div ";
        let items = labels(source, source.len() as u32);
        assert!(items.contains(&"class".to_string()));
        assert!(items.contains(&"id".to_string()));
    }

    /// 标签专属属性应出现在该标签的补全里。
    #[test]
    fn completes_tag_specific_attributes() {
        let source = "<a ";
        let items = labels(source, source.len() as u32);
        assert!(items.contains(&"href".to_string()));
    }

    /// 属性值位置补允许取值。
    #[test]
    fn completes_attribute_values() {
        let source = "<input type=\"";
        let items = labels(source, source.len() as u32);
        assert!(items.contains(&"text".to_string()));
        assert!(items.contains(&"checkbox".to_string()));
    }
}
