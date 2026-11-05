//! 文档大纲与折叠区间。
//!
//! 大纲只收录"结构性"元素与带 `id`/`class` 的元素，避免把每个 `<span>` 都塞进树里，
//! 这是编辑器里大纲可用的前提。

use serde_json::{Value, json};

use crate::features::View;

/// LSP 的 SymbolKind 取值。
const SYMBOL_NAMESPACE: u8 = 3;

/// 会进入大纲的结构性标签。
const STRUCTURAL_TAGS: &[&str] = &[
    "html", "head", "body", "main", "header", "footer", "nav", "section", "article", "aside",
    "form", "table", "ul", "ol", "dl", "template", "script", "style", "dialog", "figure", "video",
    "audio", "canvas",
];

/// 生成文档大纲（层级 `DocumentSymbol`）。
pub fn document_symbols(view: &View<'_>) -> Value {
    let mut symbols = Vec::new();
    for root in view.ast.roots.iter().copied() {
        if let Some(symbol) = build_symbol(view, root) {
            symbols.push(symbol);
        }
    }
    Value::Array(symbols)
}

/// 递归构造一个元素的符号；不含任何可展示子树的元素返回 `None`。
fn build_symbol(view: &View<'_>, index: u32) -> Option<Value> {
    let node = view.ast.node(index);
    if !node.is_element() {
        return None;
    }

    let mut children = Vec::new();
    for child in view.ast.children(index) {
        if let Some(symbol) = build_symbol(view, child) {
            children.push(symbol);
        }
    }

    let tag_name = view.text(node.name).to_ascii_lowercase();
    if !is_listed(&tag_name, view, index) && children.is_empty() {
        return None;
    }

    Some(json!({
        "name": display_name(view, index, &tag_name),
        "kind": SYMBOL_NAMESPACE,
        "range": view.range(node.span),
        "selectionRange": view.range(node.name),
        "children": children,
    }))
}

/// 元素是否应出现在大纲里：结构性标签，或带 `id`/`class`。
fn is_listed(tag_name: &str, view: &View<'_>, index: u32) -> bool {
    if STRUCTURAL_TAGS.contains(&tag_name) {
        return true;
    }
    let node = view.ast.node(index);
    view.ast.attributes_of(node).iter().any(|attribute| {
        matches!(
            view.text(attribute.name).to_ascii_lowercase().as_str(),
            "id" | "class"
        )
    })
}

/// 大纲条目的显示名：`标签#id.类名`。
fn display_name(view: &View<'_>, index: u32, tag_name: &str) -> String {
    let node = view.ast.node(index);
    let mut name = tag_name.to_string();

    for attribute in view.ast.attributes_of(node) {
        let attribute_name = view.text(attribute.name).to_ascii_lowercase();
        let Some(value) = attribute.value else {
            continue;
        };
        let value_text = view.text(value);
        match attribute_name.as_str() {
            "id" if !value_text.is_empty() => name.push_str(&format!("#{value_text}")),
            "class" => {
                for token in value_text.split_whitespace() {
                    name.push('.');
                    name.push_str(token);
                }
            }
            _ => {}
        }
    }

    name
}

/// 生成折叠区间：跨行的元素按区域折叠，跨行的注释按注释折叠。
pub fn folding_ranges(view: &View<'_>) -> Value {
    let mut ranges = Vec::new();

    for index in view.ast.elements() {
        let span = view.ast.node(index).span;
        let start = view.lines.position(span.start).0;
        let end = view.lines.position(span.end).0;
        if end > start {
            ranges.push(json!({ "startLine": start, "endLine": end, "kind": "region" }));
        }
    }

    for node in &view.ast.nodes {
        if node.kind != crate::ast::NodeKind::Comment {
            continue;
        }
        let start = view.lines.position(node.span.start).0;
        let end = view.lines.position(node.span.end).0;
        if end > start {
            ranges.push(json!({ "startLine": start, "endLine": end, "kind": "comment" }));
        }
    }

    Value::Array(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use lspd_core::text::LineIndex;

    /// 对源码求大纲。
    fn symbols_of(source: &str) -> Value {
        let parsed = parser::parse(source);
        let lines = LineIndex::new(source);
        let view = View::new(source, &parsed.ast, &lines);
        document_symbols(&view)
    }

    /// 带 id 与 class 的元素应出现在大纲里，名称包含标识信息。
    #[test]
    fn lists_identified_elements() {
        let symbols = symbols_of(
            "<body><div id=\"main\" class=\"box wide\"><span class=\"dot\"></span></div></body>",
        );
        let text = symbols.to_string();
        assert!(text.contains("div#main.box.wide"), "实际内容：{text}");
        assert!(text.contains("span.dot"), "实际内容：{text}");
    }

    /// 普通文本与无标识的内联元素不进大纲。
    #[test]
    fn skips_plain_elements() {
        let symbols = symbols_of("<body><span>纯文本</span></body>");
        assert_eq!(symbols[0]["children"].as_array().map(Vec::len), Some(0));
    }

    /// 跨行元素产生折叠区间。
    #[test]
    fn folds_multiline_elements() {
        let source = "<div>\n  <p>x</p>\n</div>";
        let parsed = parser::parse(source);
        let lines = LineIndex::new(source);
        let view = View::new(source, &parsed.ast, &lines);
        let ranges = folding_ranges(&view);
        assert!(!ranges.as_array().unwrap().is_empty());
    }
}
