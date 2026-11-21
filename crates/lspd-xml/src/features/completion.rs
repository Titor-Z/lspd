//! 补全：基于当前文档已见标签/属性的简单补全。

use serde_json::{Value, json};
use std::collections::BTreeSet;
use crate::ast::NodeKind;
use crate::features::View;

const KIND_CLASS: u8 = 7;
const KIND_PROPERTY: u8 = 10;

/// 计算补全。
pub fn complete(view: &View<'_>, offset: u32) -> Value {
    let bytes = view.source.as_bytes();
    let pos = offset as usize;
    let mut i = pos;
    let mut lt = None;
    while i > 0 {
        match bytes[i - 1] {
            b'>' => break,
            b'<' => { lt = Some(i - 1); break; }
            _ => i -= 1,
        }
    }
    let Some(lt) = lt else { return json!({"isIncomplete":false,"items":[]}); };
    let inside = &view.source[lt..pos];
    let mut items = Vec::new();

    if !inside.contains(|c: char| c.is_whitespace()) {
        let start = lt + 1;
        let prefix = view.source[start..pos].to_ascii_lowercase();
        let mut names = BTreeSet::new();
        for id in view.ast.elements() {
            let n = view.text(view.ast.node(id).name).to_ascii_lowercase();
            if !n.is_empty() { names.insert(n); }
        }
        for name in view.schema.complete_tags(None, &prefix) { names.insert(name); }
        for name in names {
            if !prefix.is_empty() && !name.starts_with(&prefix) { continue; }
            items.push(json!({"label":name,"kind":KIND_CLASS,"textEdit":{"range":{"start":view.position(start as u32),"end":view.position(offset)},"newText":name}}));
        }
        return json!({"isIncomplete":false,"items":items});
    }

    let tag = inside.trim_start_matches('<').split(|c: char| c.is_whitespace() || c == '/' || c == '>').next().unwrap_or("").to_ascii_lowercase();
    let start = word_start(bytes, pos) as u32;
    let prefix = view.source[start as usize..pos].to_ascii_lowercase();
    let mut attrs = BTreeSet::new();
    for id in view.ast.elements() {
        let node = view.ast.node(id);
        if node.kind != NodeKind::Element { continue; }
        let n = view.text(node.name).to_ascii_lowercase();
        if !tag.is_empty() && n != tag { continue; }
        for attr in view.ast.attributes_of(node) {
            let a = view.text(attr.name).to_string();
            if !a.is_empty() { attrs.insert(a); }
        }
    }
    for a in ["xmlns", "xmlns:xlink", "xml:lang", "xml:space", "id", "class"] { attrs.insert(a.to_string()); }
    for a in view.schema.complete_attributes(None, &tag, &prefix) { attrs.insert(a); }
    for name in attrs {
        if !prefix.is_empty() && !name.to_ascii_lowercase().starts_with(&prefix) { continue; }
        items.push(json!({"label":name,"kind":KIND_PROPERTY,"textEdit":{"range":{"start":view.position(start),"end":view.position(offset)},"newText":name}}));
    }
    json!({"isIncomplete":false,"items":items})
}

fn word_start(bytes: &[u8], pos: usize) -> usize {
    let mut s = pos;
    while s > 0 {
        let b = bytes[s - 1];
        if b.is_ascii_whitespace() || matches!(b, b'<' | b'>' | b'"' | b'\'' | b'=') { break; }
        s -= 1;
    }
    s
}
