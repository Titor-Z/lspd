//! 命名空间语义（文档内）。
//!
//! 只处理文档中声明的 `xmlns` / `xmlns:prefix`，不抓取外部资源。
//! 解析结果用于 hover/completion/schema 匹配时的 QName 解析。

use crate::ast::{Ast, NONE, NodeKind};

/// XML 命名空间 URI。
pub const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
/// xmlns 命名空间 URI。
pub const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

/// 拆分 QName，返回 (prefix, local)。无前缀时 prefix 为空。
pub fn split_qname(name: &str) -> (&str, &str) {
    match name.split_once(':') {
        Some((prefix, local)) => (prefix, local),
        None => ("", name),
    }
}

/// 解析元素在其作用域内的命名空间 URI。
pub fn element_namespace(ast: &Ast, source: &str, element: u32) -> Option<String> {
    let name = text(ast, source, ast.node(element).name);
    let (prefix, _local) = split_qname(name);
    if prefix == "xmlns" {
        return Some(XMLNS_NS.to_string());
    }
    resolve_prefix(ast, source, element, prefix)
}

/// 解析属性命名空间 URI。无前缀属性属于空命名空间（XML 属性特例）。
pub fn attribute_namespace(ast: &Ast, source: &str, element: u32, attr_name: &str) -> Option<String> {
    let (prefix, _local) = split_qname(attr_name);
    match prefix {
        "" => None,
        "xml" => Some(XML_NS.to_string()),
        "xmlns" => Some(XMLNS_NS.to_string()),
        _ => resolve_prefix(ast, source, element, prefix),
    }
}

/// 从元素开始向上解析前缀。
pub fn resolve_prefix(ast: &Ast, source: &str, mut element: u32, prefix: &str) -> Option<String> {
    loop {
        let node = ast.node(element);
        if node.kind != NodeKind::Element {
            if node.parent == NONE { return default_for(prefix); }
            element = node.parent;
            continue;
        }
        for attr in ast.attributes_of(node) {
            let name = text(ast, source, attr.name);
            let value = attr.value.map(|v| attr_value(source, v)).unwrap_or_default();
            if prefix.is_empty() && name == "xmlns" {
                return Some(value);
            }
            if let Some(rest) = name.strip_prefix("xmlns:") {
                if rest == prefix {
                    return Some(value);
                }
            }
        }
        if node.parent == NONE {
            return default_for(prefix);
        }
        element = node.parent;
    }
}

fn default_for(prefix: &str) -> Option<String> {
    match prefix {
        "xml" => Some(XML_NS.to_string()),
        "xmlns" => Some(XMLNS_NS.to_string()),
        "" => Some(String::new()),
        _ => None,
    }
}

fn text<'a>(ast: &Ast, source: &'a str, span: lspd_core::text::Span) -> &'a str {
    let _ = ast;
    source.get(span.start as usize..span.end as usize).unwrap_or("")
}

/// 去掉属性值引号。
pub fn attr_value(source: &str, span: lspd_core::text::Span) -> String {
    let raw = source.get(span.start as usize..span.end as usize).unwrap_or("");
    if raw.len() >= 2 {
        let bytes = raw.as_bytes();
        if (bytes[0] == b'"' && bytes[raw.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[raw.len() - 1] == b'\'')
        {
            return raw[1..raw.len() - 1].to_string();
        }
    }
    raw.to_string()
}
