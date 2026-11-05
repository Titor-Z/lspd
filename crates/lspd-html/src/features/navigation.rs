//! 导航：标签联动改名、`id`/`class` 的定义与引用。

use serde_json::{Value, json};

use crate::ast::Attribute;
use crate::features::View;
use lspd_core::text::Span;

/// 引用型属性：值指向文档内的 `id`。
const REFERENCE_ATTRIBUTES: &[&str] = &[
    "href",
    "for",
    "form",
    "list",
    "headers",
    "aria-labelledby",
    "aria-describedby",
    "aria-controls",
    "aria-owns",
    "popovertarget",
    "anchor",
];

/// 标签联动改名：光标在开始或结束标签名上时，返回两处名称区间。
pub fn linked_editing(view: &View<'_>, offset: u32) -> Option<Value> {
    let index = view.ast.element_at(offset)?;
    let node = view.ast.node(index);
    let name = view.text(node.name).to_ascii_lowercase();

    let start_span = node.name;
    let end_span = end_tag_name_span(view, node.span, &name)?;

    if !start_span.contains(offset) && !end_span.contains(offset) {
        return None;
    }

    Some(json!({ "ranges": [view.range(start_span), view.range(end_span)] }))
}

/// 在元素区间内定位结束标签的名称区间。
fn end_tag_name_span(view: &View<'_>, span: Span, name: &str) -> Option<Span> {
    let text = view.text(span);
    let lower = text.to_ascii_lowercase();
    let needle = format!("</{name}");
    let position = lower.rfind(&needle)?;
    let start = span.start as usize + position + 2;
    Some(Span::new(start, start + name.len()))
}

/// 转到定义：光标在引用型属性值或 `id` 上时，跳到目标元素。
pub fn definition(view: &View<'_>, offset: u32, uri: &str) -> Value {
    let Some((_, attribute, attribute_name)) = attribute_at(view, offset) else {
        return Value::Null;
    };
    let Some(target) = target_id(view, &attribute, &attribute_name) else {
        return Value::Null;
    };

    let Some(index) = find_element_by_id(view, &target) else {
        return Value::Null;
    };
    let node = view.ast.node(index);

    json!([{ "uri": uri, "range": view.range(node.name) }])
}

/// 查找引用：`id` 查引用点，`class` 查同类元素，引用型属性查目标声明。
pub fn references(view: &View<'_>, offset: u32, uri: &str) -> Value {
    let Some((_, attribute, attribute_name)) = attribute_at(view, offset) else {
        return json!([]);
    };
    let Some(value_span) = attribute.value else {
        return json!([]);
    };
    let value = view.text(value_span);

    let mut locations = Vec::new();

    match attribute_name.as_str() {
        "id" => {
            let id = value.trim();
            for index in view.ast.elements() {
                let node = view.ast.node(index);
                if let Some(span) = attribute_value_span(view, index, "id")
                    && view.text(span) == id
                {
                    locations.push(json!({ "uri": uri, "range": view.range(node.name) }));
                }
                collect_reference_values(view, index, id, uri, &mut locations);
            }
        }
        "class" => {
            let wanted: Vec<&str> = value.split_whitespace().collect();
            for index in view.ast.elements() {
                let node = view.ast.node(index);
                let Some(span) = attribute_value_span(view, index, "class") else {
                    continue;
                };
                let classes: Vec<&str> = view.text(span).split_whitespace().collect();
                if wanted.iter().any(|item| classes.contains(item)) {
                    locations.push(json!({ "uri": uri, "range": view.range(node.name) }));
                }
            }
        }
        _ => {
            if let Some(target) = target_id(view, &attribute, &attribute_name)
                && let Some(index) = find_element_by_id(view, &target)
            {
                locations
                    .push(json!({ "uri": uri, "range": view.range(view.ast.node(index).name) }));
            }
        }
    }

    Value::Array(locations)
}

/// 取光标所在的属性及其小写名称。
fn attribute_at(view: &View<'_>, offset: u32) -> Option<(u32, Attribute, String)> {
    let index = view.ast.element_at(offset)?;
    let node = view.ast.node(index);

    for attribute in view.ast.attributes_of(node) {
        let in_value = attribute.value.is_some_and(|value| value.contains(offset));
        if in_value || attribute.name.contains(offset) {
            let name = view.text(attribute.name).to_ascii_lowercase();
            return Some((index, *attribute, name));
        }
    }

    None
}

/// 从属性值推断目标 `id`：支持 `#id` 与裸 `id` 两种写法。
fn target_id(view: &View<'_>, attribute: &Attribute, attribute_name: &str) -> Option<String> {
    if !REFERENCE_ATTRIBUTES.contains(&attribute_name) {
        return None;
    }
    let value = view.text(attribute.value?).trim();
    if value.is_empty() {
        return None;
    }
    Some(value.trim_start_matches('#').to_string())
}

/// 找到 `id` 等于给定值的元素。
fn find_element_by_id(view: &View<'_>, id: &str) -> Option<u32> {
    view.ast.elements().find(|index| {
        attribute_value_span(view, *index, "id").is_some_and(|span| view.text(span).trim() == id)
    })
}

/// 取某元素指定属性的值区间。
fn attribute_value_span(view: &View<'_>, index: u32, name: &str) -> Option<Span> {
    let node = view.ast.node(index);
    view.ast
        .attributes_of(node)
        .iter()
        .find(|attribute| view.text(attribute.name).eq_ignore_ascii_case(name))
        .and_then(|attribute| attribute.value)
}

/// 收集指向某个 `id` 的引用型属性所在元素的名称区间。
fn collect_reference_values(
    view: &View<'_>,
    index: u32,
    id: &str,
    uri: &str,
    locations: &mut Vec<Value>,
) {
    let node = view.ast.node(index);
    for attribute in view.ast.attributes_of(node) {
        let name = view.text(attribute.name).to_ascii_lowercase();
        if !REFERENCE_ATTRIBUTES.contains(&name.as_str()) {
            continue;
        }
        let Some(value) = attribute.value else {
            continue;
        };
        if view.text(value).trim().trim_start_matches('#') == id {
            locations.push(json!({ "uri": uri, "range": view.range(node.name) }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use lspd_core::text::LineIndex;

    /// 构造视图并执行回调，避免每个用例重复三行准备。
    fn with_view<T>(source: &str, action: impl FnOnce(&View<'_>) -> T) -> T {
        let parsed = parser::parse(source);
        let lines = LineIndex::new(source);
        let view = View::new(source, &parsed.ast, &lines);
        action(&view)
    }

    /// 光标在开始标签名上时给出两处名称区间。
    #[test]
    fn links_open_and_close_tag_names() {
        let ranges = with_view("<div>x</div>", |view| linked_editing(view, 2));
        let ranges = ranges.expect("应有联动区间");
        assert_eq!(ranges["ranges"].as_array().map(Vec::len), Some(2));
    }

    /// 引用型属性值指向的 `id` 元素即定义处。
    #[test]
    fn resolves_definition_from_href() {
        let source = "<a href=\"#x\">go</a><section id=\"x\"></section>";
        let offset = source.find("#x").unwrap() as u32 + 1;
        let value = with_view(source, |view| definition(view, offset, "file:///a.html"));

        let locations = value.as_array().expect("应返回位置数组");
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0]["range"]["start"]["character"], 20);
    }

    /// `class` 的引用应列出所有同类元素。
    #[test]
    fn finds_class_references() {
        let source = "<i class=\"dot\"></i><b class=\"dot other\"></b><b></b>";
        let offset = source.find("dot").unwrap() as u32;
        let value = with_view(source, |view| references(view, offset, "file:///a.html"));
        assert_eq!(value.as_array().map(Vec::len), Some(2));
    }

    /// `id` 的引用应包含声明点与指向它的引用点。
    #[test]
    fn finds_id_references() {
        let source = "<a href=\"#x\">go</a><section id=\"x\"></section>";
        let offset = source.find("id=\"x").unwrap() as u32 + 4;
        let value = with_view(source, |view| references(view, offset, "file:///a.html"));
        assert_eq!(value.as_array().map(Vec::len), Some(2));
    }
}
