//! 悬停提示。

use serde_json::{Value, json};
use crate::features::View;
use crate::ns;

/// 计算悬停。
pub fn hover(view: &View<'_>, offset: u32) -> Option<Value> {
    let index = view.ast.element_at(offset)?;
    let node = view.ast.node(index);
    let name = view.text(node.name);
    let ns = ns::element_namespace(view.ast, view.source, index);
    let ns_ref = ns.as_deref();
    let (prefix, local) = ns::split_qname(name);

    if node.name.contains(offset) {
        let mut text = view
            .schema
            .tag_doc(ns_ref, &name.to_ascii_lowercase())
            .map(|d| d.markdown)
            .unwrap_or_else(|| format!("### `<{}>`", name));
        if let Some(uri) = ns_ref {
            if !uri.is_empty() {
                text.push_str(&format!("\n\n命名空间：`{}`", uri));
            }
        }
        if !prefix.is_empty() {
            text.push_str(&format!("\n\n前缀：`{}`，本地名：`{}`", prefix, local));
        }
        return Some(json!({ "contents": {"kind":"markdown","value":text}, "range": view.range(node.name) }));
    }

    for attr in view.ast.attributes_of(node) {
        if !attr.name.contains(offset) && !attr.value.is_some_and(|v| v.contains(offset)) {
            continue;
        }
        let attr_name = view.text(attr.name);
        let attr_ns = ns::attribute_namespace(view.ast, view.source, index, attr_name);
        let mut text = view
            .schema
            .attribute_doc(ns_ref, &name.to_ascii_lowercase(), &attr_name.to_ascii_lowercase())
            .map(|d| d.markdown)
            .unwrap_or_else(|| format!("### `{}`", attr_name));
        if let Some(uri) = attr_ns {
            if !uri.is_empty() {
                text.push_str(&format!("\n\n命名空间：`{}`", uri));
            }
        }
        if let Some(value) = attr.value {
            let raw = view.text(value);
            text.push_str(&format!("\n\n值：`{}`", raw.trim_matches('"').trim_matches('\'')));
        }
        let span = if attr.name.contains(offset) { attr.name } else { attr.span };
        return Some(json!({ "contents": {"kind":"markdown","value":text}, "range": view.range(span) }));
    }

    None
}
