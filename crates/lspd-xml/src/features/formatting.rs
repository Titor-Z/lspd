//! 保守格式化：只调整缩进和标签间换行。
//! 不改注释/CDATA/PI/DOCTYPE/`xml:space="preserve"` 内部；混合内容中的文本保持原样。

use serde_json::{Value, json};

use crate::ast::NodeKind;
use crate::features::View;

/// 格式化整份文档。
pub fn format(view: &View<'_>, tab_size: u32, insert_spaces: bool) -> Value {
    let indent_unit = if insert_spaces { " ".repeat(tab_size.max(1) as usize) } else { "\t".to_string() };
    let src = view.source;
    let preserve = preserve_ranges(view);
    let mut out = String::with_capacity(src.len() + 16);
    let mut i = 0usize;
    let bytes = src.as_bytes();
    let mut depth: i32 = 0;
    let mut at_line_start = true;

    while i < bytes.len() {
        if in_ranges(&preserve, i) {
            let end = preserve_end(&preserve, i);
            out.push_str(&src[i..end]);
            i = end;
            at_line_start = out.ends_with('\n');
            continue;
        }

        if at_line_start {
            while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') { i += 1; }
            if i >= bytes.len() { break; }
            if in_ranges(&preserve, i) { continue; }
        }

        if bytes[i] == b'<' {
            if starts_with(bytes, i, b"<!--") {
                let end = find(bytes, i + 4, b"-->").map(|p| p + 3).unwrap_or(bytes.len());
                write_indent(&mut out, depth.max(0) as usize, &indent_unit, &mut at_line_start);
                out.push_str(&src[i..end]);
                i = end;
                continue;
            }
            if starts_with(bytes, i, b"<![CDATA[") {
                let end = find(bytes, i + 9, b"]]>").map(|p| p + 3).unwrap_or(bytes.len());
                write_indent(&mut out, depth.max(0) as usize, &indent_unit, &mut at_line_start);
                out.push_str(&src[i..end]);
                i = end;
                continue;
            }
            if starts_with(bytes, i, b"<?") {
                let end = find(bytes, i + 2, b"?>").map(|p| p + 2).unwrap_or(bytes.len());
                write_indent(&mut out, depth.max(0) as usize, &indent_unit, &mut at_line_start);
                out.push_str(&src[i..end]);
                i = end;
                newline(&mut out, &mut at_line_start);
                continue;
            }
            if starts_with_ci(bytes, i, b"<!DOCTYPE") {
                let end = find_tag_end(bytes, i);
                write_indent(&mut out, depth.max(0) as usize, &indent_unit, &mut at_line_start);
                out.push_str(&normalize_tag(&src[i..end]));
                i = end;
                newline(&mut out, &mut at_line_start);
                continue;
            }
            if starts_with(bytes, i, b"</") {
                let end = find_tag_end(bytes, i);
                depth = (depth - 1).max(0);
                let inline = inline_close(view, i);
                if inline {
                    if at_line_start { write_indent(&mut out, depth as usize, &indent_unit, &mut at_line_start); }
                    out.push_str(&normalize_tag(&src[i..end]));
                } else {
                    write_indent(&mut out, depth as usize, &indent_unit, &mut at_line_start);
                    out.push_str(&normalize_tag(&src[i..end]));
                    newline(&mut out, &mut at_line_start);
                }
                i = end;
                continue;
            }

            let end = find_tag_end(bytes, i);
            let tag = &src[i..end];
            let self_closing = is_self_closing(tag);
            let inline = inline_open(view, i);
            if inline {
                if at_line_start { write_indent(&mut out, depth.max(0) as usize, &indent_unit, &mut at_line_start); }
                out.push_str(&normalize_tag(tag));
            } else {
                write_indent(&mut out, depth.max(0) as usize, &indent_unit, &mut at_line_start);
                out.push_str(&normalize_tag(tag));
                newline(&mut out, &mut at_line_start);
            }
            i = end;
            if !self_closing { depth += 1; }
            continue;
        }

        let start = i;
        while i < bytes.len() && bytes[i] != b'<' && !in_ranges(&preserve, i) { i += 1; }
        let text = &src[start..i];
        if !text.trim().is_empty() {
            if mixed_text(view, start) {
                out.push_str(text);
                at_line_start = text.ends_with('\n');
            } else {
                write_indent(&mut out, depth.max(0) as usize, &indent_unit, &mut at_line_start);
                out.push_str(text.trim());
                newline(&mut out, &mut at_line_start);
            }
        }
    }

    let end_pos = view.lines.position(src.len() as u32);
    json!([{ "range": { "start": {"line":0,"character":0}, "end": {"line":end_pos.0,"character":end_pos.1} }, "newText": out }])
}

fn write_indent(out: &mut String, depth: usize, unit: &str, at_line_start: &mut bool) {
    if !*at_line_start { return; }
    for _ in 0..depth { out.push_str(unit); }
    *at_line_start = false;
}

fn newline(out: &mut String, at_line_start: &mut bool) {
    if !out.ends_with('\n') { out.push('\n'); }
    *at_line_start = true;
}

fn starts_with(bytes: &[u8], pos: usize, needle: &[u8]) -> bool {
    bytes.get(pos..pos + needle.len()) == Some(needle)
}
fn starts_with_ci(bytes: &[u8], pos: usize, needle: &[u8]) -> bool {
    let Some(s) = bytes.get(pos..pos + needle.len()) else { return false };
    s.iter().zip(needle).all(|(a, b)| a.to_ascii_lowercase() == *b)
}
fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || from + needle.len() > bytes.len() { return None; }
    (from..=bytes.len() - needle.len()).find(|&p| &bytes[p..p + needle.len()] == needle)
}
fn find_tag_end(bytes: &[u8], mut i: usize) -> usize {
    let mut quote = 0u8;
    while i < bytes.len() {
        let b = bytes[i];
        i += 1;
        match b {
            b'"' | b'\'' if quote == 0 => quote = b,
            b'"' | b'\'' if quote == b => quote = 0,
            b'>' if quote == 0 => break,
            _ => {}
        }
    }
    i
}
fn is_self_closing(tag: &str) -> bool {
    let t = tag.trim_end();
    t.ends_with("/>") || t.starts_with("<?") || t.starts_with("<!")
}
fn normalize_tag(tag: &str) -> String {
    let bytes = tag.as_bytes();
    let mut out = String::with_capacity(tag.len());
    let mut i = 0usize;
    let mut quote = 0u8;
    let mut last_space = false;
    while i < bytes.len() {
        let b = bytes[i];
        let c = b as char;
        if quote != 0 {
            out.push(c);
            if b == quote { quote = 0; }
            i += 1;
            continue;
        }
        if b == b'"' || b == b'\'' { quote = b; out.push(c); last_space = false; i += 1; continue; }
        if b.is_ascii_whitespace() {
            if !last_space && !out.ends_with('<') && !out.ends_with("</") {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() { j += 1; }
                if j < bytes.len() && (bytes[j] == b'>' || bytes[j] == b'/') { i = j; continue; }
                out.push(' ');
                last_space = true;
            }
            i += 1;
            continue;
        }
        out.push(c);
        last_space = false;
        i += 1;
    }
    out
}

fn preserve_ranges(view: &View<'_>) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for i in 0..view.ast.nodes.len() as u32 {
        let node = view.ast.node(i);
        if !node.is_element() { continue; }
        let mut preserve = false;
        for attr in view.ast.attributes_of(node) {
            let name = view.text(attr.name);
            if name == "xml:space" {
                let value = attr.value.map(|v| view.text(v).trim_matches('"').trim_matches('\'')).unwrap_or("");
                if value == "preserve" { preserve = true; }
            }
        }
        if preserve {
            out.push((node.span.start as usize, node.span.end as usize));
        }
    }
    out
}
fn in_ranges(ranges: &[(usize, usize)], pos: usize) -> bool {
    ranges.iter().any(|(s, e)| pos >= *s && pos < *e)
}
fn preserve_end(ranges: &[(usize, usize)], pos: usize) -> usize {
    ranges.iter().find(|(s, e)| pos >= *s && pos < *e).map(|(_, e)| *e).unwrap_or(pos + 1)
}

fn inline_open(view: &View<'_>, pos: usize) -> bool {
    let Some(i) = view.ast.elements().find(|&i| view.ast.node(i).span.start as usize == pos) else { return false };
    element_is_mixed(view, i)
}
fn inline_close(view: &View<'_>, pos: usize) -> bool {
    // close tag starts at pos, belongs to current innermost open element; approximate by nearest element whose end matches
    let Some(i) = view.ast.elements().find(|&i| {
        let n = view.ast.node(i);
        pos >= n.span.start as usize && pos < n.span.end as usize && view.source.as_bytes().get(pos + 1) == Some(&b'/')
    }) else { return false };
    element_is_mixed(view, i)
}
fn mixed_text(view: &View<'_>, text_pos: usize) -> bool {
    let Some(i) = view.ast.elements().filter(|&i| {
        let n = view.ast.node(i);
        text_pos >= n.span.start as usize && text_pos < n.span.end as usize
    }).max_by_key(|&i| view.ast.node(i).span.start) else { return false };
    element_is_mixed(view, i)
}
fn element_is_mixed(view: &View<'_>, index: u32) -> bool {
    let mut has_text = false;
    let mut has_element = false;
    for child in view.ast.children(index) {
        let node = view.ast.node(child);
        match node.kind {
            NodeKind::Text => {
                let t = view.text(node.span);
                if !t.trim().is_empty() { has_text = true; }
            }
            NodeKind::Element => has_element = true,
            _ => {}
        }
    }
    has_text && has_element
}
