//! 文档大纲、折叠区间与选择区间。
//!
//! 大纲只收录规则与 at 规则（声明太碎，进大纲反而不可用），
//! 名称取选择器或 at 规则前奏，去掉多余空白。

use lspd_core::text::Span;
use serde_json::{Map, json, Value};

use crate::ast::{NONE, NodeKind};
use crate::features::{View, innermost};

/// LSP 的 SymbolKind 取值。
const SYMBOL_NAMESPACE: u8 = 3;

/// 生成文档大纲。
pub fn document_symbols(view: &View<'_>) -> Value {
    let mut symbols = Vec::new();
    for root in view.ast.roots.iter().copied() {
        if let Some(symbol) = build(view, root) {
            symbols.push(symbol);
        }
    }
    Value::Array(symbols)
}

/// 递归构造一个块的符号。
fn build(view: &View<'_>, index: u32) -> Option<Value> {
    let node = view.ast.node(index);
    if !matches!(node.kind, NodeKind::Rule | NodeKind::AtRule) {
        return None;
    }

    let mut children = Vec::new();
    for child in view.ast.children(index) {
        if let Some(symbol) = build(view, child) {
            children.push(symbol);
        }
    }

    let label = label_of(view, index);
    if label.is_empty() {
        return None;
    }

    Some(json!({
        "name": label,
        "kind": SYMBOL_NAMESPACE,
        "range": view.range(node.span),
        "selectionRange": view.range(node.name),
        "children": children,
    }))
}

/// 大纲条目的显示名。
fn label_of(view: &View<'_>, index: u32) -> String {
    let node = view.ast.node(index);
    let raw = match node.kind {
        NodeKind::Rule => view.text(node.name),
        _ => {
            // at 规则：名称 + 前奏（前奏可能跨行，压成单行）
            let prelude = view.text(node.value).split_whitespace().collect::<Vec<_>>().join(" ");
            if prelude.is_empty() {
                return view.text(node.name).to_string();
            }
            return format!("{} {}", view.text(node.name), prelude);
        }
    };
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 生成折叠区间：跨行的块按区域折叠。
pub fn folding_ranges(view: &View<'_>) -> Value {
    let mut ranges = Vec::new();

    for index in view.ast.nodes_of(NodeKind::Rule).chain(view.ast.nodes_of(NodeKind::AtRule)) {
        let span = view.ast.node(index).span;
        let start = view.lines.position(span.start).0;
        let end = view.lines.position(span.end).0;
        if end > start {
            ranges.push(json!({ "startLine": start, "endLine": end, "kind": "region" }));
        }
    }

    Value::Array(ranges)
}

/// 生成一个位置的选择区间：由内到外的嵌套链，外层为整份文档。
pub fn selection_range(view: &View<'_>, offset: u32) -> Value {
    let offset = offset.min(view.source.len() as u32);

    let mut chain: Vec<Span> = Vec::new();
    if let Some(index) = innermost(view.ast, offset) {
        let node = view.ast.node(index);
        push(&mut chain, token_of(node, offset));
        push(&mut chain, node.span);

        let mut parent = node.parent;
        while parent != NONE {
            let ancestor = view.ast.node(parent);
            push(&mut chain, ancestor.span);
            parent = ancestor.parent;
        }
    } else {
        // 落在空白或注释：最内层为光标自身的零宽区间
        chain.push(Span::new(offset as usize, offset as usize));
    }

    // 最外层为整份文档
    push(&mut chain, Span::new(0, view.source.len()));

    // 链已由内到外，反向累加 parent
    let mut current: Option<Value> = None;
    for span in chain.iter().rev() {
        let mut object = Map::new();
        object.insert("range".to_string(), view.range(*span));
        if let Some(parent) = current.take() {
            object.insert("parent".to_string(), parent);
        }
        current = Some(Value::Object(object));
    }
    current.unwrap_or(Value::Null)
}

/// 节点内光标所在的具体区间；不在名字或值上时退到节点整体。
fn token_of(node: &crate::ast::Node, offset: u32) -> Span {
    if node.name.contains(offset) {
        return node.name;
    }
    if !node.value.is_empty() && node.value.contains(offset) {
        return node.value;
    }
    node.span
}

/// 追加一个区间；与前一项相等或不严格增大时跳过，保证严格包含。
fn push(chain: &mut Vec<Span>, span: Span) {
    if chain.last().is_some_and(|last| *last == span || span.end - span.start <= last.end - last.start) {
        return;
    }
    chain.push(span);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::Dialect;
    use crate::parser::parse;
    use lspd_core::text::LineIndex;

    /// 把嵌套链拆成逐级的子串，并校验每级严格包含下一级。
    fn chain(view: &View<'_>, needle: &str) -> Vec<String> {
        let offset = view.source.find(needle).expect("寻找子串") as u32;
        let mut current = selection_range(view, offset);
        let mut levels = Vec::new();

        let mut child: Option<(u32, u32)> = None;
        loop {
            let range = &current["range"];
            let start = view.lines.offset(range["start"]["line"].as_u64().unwrap() as u32,
                range["start"]["character"].as_u64().unwrap() as u32);
            let end = view.lines.offset(range["end"]["line"].as_u64().unwrap() as u32,
                range["end"]["character"].as_u64().unwrap() as u32);

            if let Some((child_start, child_end)) = child {
                assert!(start <= child_start && end >= child_end, "选择链必须单调包含");
            }
            child = Some((start as u32, end as u32));
            levels.push(view.source[start as usize..end as usize].to_string());

            match current.get("parent") {
                Some(parent) if !parent.is_null() => current = parent.clone(),
                _ => break,
            }
        }
        levels
    }

    /// 属性、值、选择器、at 规则名的最内层选择应落在对应词上。
    #[test]
    fn selection_lands_on_innermost_token() {
        let source = "@media (min-width: 600px) {\n  a { color: red; }\n}\n";
        let parsed = parse(source, Dialect::Css);
        let lines = LineIndex::new(source);
        let view = View::new(source, &parsed.ast, &lines);

        assert_eq!(chain(&view, "color").first().unwrap(), "color");
        assert_eq!(chain(&view, "red").first().unwrap(), "red");
        assert_eq!(chain(&view, "a { color").first().unwrap(), "a");
        assert_eq!(chain(&view, "@media").first().unwrap(), "@media");
    }

    /// 最外层选择区间应为整份文档。
    #[test]
    fn selection_ends_with_whole_document() {
        let source = "a { color: red; }\n";
        let parsed = parse(source, Dialect::Css);
        let lines = LineIndex::new(source);
        let view = View::new(source, &parsed.ast, &lines);

        assert_eq!(chain(&view, "color").last().unwrap(), source);
    }

    /// 光标落在空白处仍应返回零宽起始、逐层到文档的链。
    #[test]
    fn selection_in_trivia_is_monotonic() {
        let source = "a { color: red; }\n\n/* gap */\nb { margin: 0 }\n";
        let parsed = parse(source, Dialect::Css);
        let lines = LineIndex::new(source);
        let view = View::new(source, &parsed.ast, &lines);

        // 两个规则之间的换行属于空白
        let offset = source.find("\n\n").unwrap() as u32 + 1;
        let mut current = selection_range(&view, offset);
        let mut child: Option<(u32, u32)> = None;
        loop {
            let range = &current["range"];
            let start = view.lines.offset(range["start"]["line"].as_u64().unwrap() as u32,
                range["start"]["character"].as_u64().unwrap() as u32) as u32;
            let end = view.lines.offset(range["end"]["line"].as_u64().unwrap() as u32,
                range["end"]["character"].as_u64().unwrap() as u32) as u32;
            if let Some((child_start, child_end)) = child {
                assert!(start <= child_start && end >= child_end);
            }
            child = Some((start, end));
            match current.get("parent") {
                Some(parent) if !parent.is_null() => current = parent.clone(),
                _ => break,
            }
        }
    }

}
