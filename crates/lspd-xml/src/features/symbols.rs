//! documentSymbol 与 foldingRange。

use serde_json::{Value, json};
use crate::ast::NodeKind;
use crate::features::View;

const KIND_FIELD: u8 = 8;

/// 生成文档大纲。
pub fn document_symbols(view: &View) -> Value {
    let mut symbols = Vec::new();
    for &root in &view.ast.roots {
        if let Some(symbol) = visit(view, root) {
            symbols.push(symbol);
        }
    }
    Value::Array(symbols)
}

fn visit(view: &View, index: u32) -> Option<Value> {
    let node = view.ast.node(index);
    if node.kind != NodeKind::Element {
        return None;
    }
    let name = view.source.get(node.name.start as usize..node.name.end as usize).unwrap_or("");
    let mut children = Vec::new();
    for child in view.ast.children(index) {
        if let Some(symbol) = visit(view, child) {
            children.push(symbol);
        }
    }
    Some(json!({
        "name": name,
        "kind": KIND_FIELD,
        "range": view.range(node.span),
        "selectionRange": view.range(node.name),
        "children": children,
    }))
}

/// 生成折叠区间。
pub fn folding_ranges(view: &View) -> Value {
    let mut ranges = Vec::new();
    for index in 0..view.ast.nodes.len() as u32 {
        let node = view.ast.node(index);
        let span = node.span;
        let start = view.lines.position(span.start).0;
        let end = view.lines.position(span.end).0;
        if end > start && matches!(node.kind, NodeKind::Element | NodeKind::Comment | NodeKind::CData | NodeKind::Doctype) {
            ranges.push(json!({ "startLine": start, "endLine": end, "kind": "region" }));
        }
    }
    Value::Array(ranges)
}
