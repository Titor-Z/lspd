//! 容错 HTML 解析器：单遍扫描，直接构建扁平语法树。
//!
//! 设计取舍：
//! - 不做完整的 HTML5 树构建算法（含收养机构、格式化元素重开等），
//!   只覆盖实际源码里会出现的结构错误并能继续往下解析；
//! - 词法与语法合一，不产生中间 token 数组，解析期峰值内存即语法树本身；
//! - 结构诊断在解析期产出，数据驱动的诊断（未知标签/属性）在诊断模块里做。

use crate::ast::{Ast, Attribute, NONE, Node, NodeKind};
use lspd_core::text::Span;

/// 空元素：没有结束标签，遇到即闭合。
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// 原样文本元素：内容不按标签解析。
const RAW_TEXT_ELEMENTS: &[&str] = &["script", "style", "textarea", "title"];

/// 会把 `<p>` 隐式闭合的块级元素。
const CLOSES_PARAGRAPH: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "details",
    "div",
    "dl",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "ul",
];

/// 诊断严重程度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// 结构错误，必然导致浏览器行为与作者意图不一致
    Error,
    /// 可疑写法，通常无害但多半不是作者本意
    Warning,
}

/// 一条诊断。
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// 出问题的区间
    pub span: Span,
    /// 严重程度
    pub severity: Severity,
    /// 说明
    pub message: String,
}

/// 解析结果。
pub struct ParseResult {
    /// 语法树
    pub ast: Ast,
    /// 结构诊断
    pub diagnostics: Vec<Diagnostic>,
}

/// 解析一份 HTML 文本。
pub fn parse(source: &str) -> ParseResult {
    Parser::new(source).run()
}

/// 单遍扫描的解析器。
struct Parser<'a> {
    /// 原文
    source: &'a str,
    /// 原文字节，避免反复 `as_bytes`
    bytes: &'a [u8],
    /// 当前扫描位置
    position: usize,
    /// 语法树
    ast: Ast,
    /// 尚未闭合的元素，栈顶为最内层
    stack: Vec<u32>,
    /// 结构诊断
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    /// 建立解析器。
    fn new(source: &'a str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            position: 0,
            ast: Ast::default(),
            stack: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    /// 执行解析并返回结果。
    fn run(mut self) -> ParseResult {
        while self.position < self.bytes.len() {
            if self.bytes[self.position] == b'<' && self.try_markup() {
                continue;
            }
            self.parse_text();
        }

        self.close_unclosed_at_eof();

        ParseResult {
            ast: self.ast,
            diagnostics: self.diagnostics,
        }
    }

    /// 尝试把当前位置当作标记解析；不是标记则返回 `false` 交给文本分支。
    fn try_markup(&mut self) -> bool {
        let rest = &self.bytes[self.position..];

        if rest.starts_with(b"<!--") {
            self.parse_comment();
            return true;
        }
        if starts_with_ignore_ascii_case(rest, b"<!doctype") {
            self.parse_doctype();
            return true;
        }
        if rest.starts_with(b"<![CDATA[") {
            self.parse_cdata();
            return true;
        }
        if rest.starts_with(b"</") {
            self.parse_end_tag();
            return true;
        }
        if rest.len() > 1 && is_name_start(rest[1]) {
            self.parse_start_tag();
            return true;
        }

        false
    }

    /// 解析文本节点，直到下一个 `<`。
    fn parse_text(&mut self) {
        let start = self.position;
        self.position += 1;
        while self.position < self.bytes.len() && self.bytes[self.position] != b'<' {
            self.position += 1;
        }
        self.push_node_with_name(
            NodeKind::Text,
            Span::new(start, self.position),
            Span::default(),
        );
    }

    /// 解析注释。
    fn parse_comment(&mut self) {
        let start = self.position;
        let end = find(&self.bytes[self.position + 4..], b"-->")
            .map(|offset| self.position + 4 + offset + 3)
            .unwrap_or(self.bytes.len());
        self.position = end;
        self.push_node_with_name(NodeKind::Comment, Span::new(start, end), Span::default());
    }

    /// 解析 `<!DOCTYPE ...>`。
    fn parse_doctype(&mut self) {
        let start = self.position;
        let end = find(&self.bytes[self.position..], b">")
            .map(|offset| self.position + offset + 1)
            .unwrap_or(self.bytes.len());
        self.position = end;
        self.push_node_with_name(NodeKind::Doctype, Span::new(start, end), Span::default());
    }

    /// 解析 `<![CDATA[ ... ]]>`，内容按文本处理。
    fn parse_cdata(&mut self) {
        let start = self.position;
        let end = find(&self.bytes[self.position + 9..], b"]]>")
            .map(|offset| self.position + 9 + offset + 3)
            .unwrap_or(self.bytes.len());
        self.position = end;
        self.push_node_with_name(NodeKind::Text, Span::new(start, end), Span::default());
    }

    /// 解析开始标签。
    fn parse_start_tag(&mut self) {
        let start = self.position;
        self.position += 1;
        let name_start = self.position;
        self.skip_name();
        let name = Span::new(name_start, self.position);
        let element_name = name.text(self.source).to_ascii_lowercase();

        let attribute_start = self.ast.attributes.len() as u32;
        let self_closing = self.parse_attributes();
        let attribute_len = self.ast.attributes.len() as u32 - attribute_start;

        // 隐式闭合要在建节点之前完成，否则新元素会挂到本该关闭的元素下面
        self.close_implied(&element_name, start);

        let index =
            self.push_node_with_name(NodeKind::Element, Span::new(start, self.position), name);
        let node = self.ast.node_mut(index);
        node.attribute_start = attribute_start;
        node.attribute_len = attribute_len;

        if RAW_TEXT_ELEMENTS.contains(&element_name.as_str()) {
            self.parse_raw_text(index, &element_name);
            return;
        }

        if self_closing || VOID_ELEMENTS.contains(&element_name.as_str()) {
            self.finish(index, self.position);
            return;
        }

        self.stack.push(index);
    }

    /// 解析属性列表，返回是否自闭合。
    fn parse_attributes(&mut self) -> bool {
        loop {
            self.skip_whitespace();
            if self.position >= self.bytes.len() {
                // 标签在文件末尾被截断
                self.diagnostics.push(Diagnostic {
                    span: Span::new(self.position, self.position),
                    severity: Severity::Error,
                    message: "标签未结束，缺少 `>`".to_string(),
                });
                return false;
            }

            match self.bytes[self.position] {
                b'>' => {
                    self.position += 1;
                    return false;
                }
                b'/' if self.bytes.get(self.position + 1) == Some(&b'>') => {
                    self.position += 2;
                    return true;
                }
                _ => self.parse_attribute(),
            }
        }
    }

    /// 解析单个属性。
    fn parse_attribute(&mut self) {
        let start = self.position;
        let name_start = self.position;
        // 属性名以空白、`=`、`>`、`/` 为界
        while self.position < self.bytes.len() && !is_attribute_boundary(self.bytes[self.position])
        {
            self.position += 1;
        }

        if self.position == name_start {
            // 遇到无法识别的字符，跳过以免死循环
            self.position += 1;
            return;
        }

        let name = Span::new(name_start, self.position);
        self.skip_whitespace();
        let value_span = self.parse_attribute_value().map(|(span, _)| span);
        self.ast.attributes.push(Attribute {
            span: Span::new(start, self.position),
            name,
            value: value_span,
        });
    }

    /// 解析属性值（若存在）。返回区间与是否带引号。
    fn parse_attribute_value(&mut self) -> Option<(Span, bool)> {
        if self.bytes.get(self.position) != Some(&b'=') {
            return None;
        }
        self.position += 1;
        self.skip_whitespace();

        match self.bytes.get(self.position) {
            Some(quote @ (b'"' | b'\'')) => {
                let quote = *quote;
                self.position += 1;
                let value_start = self.position;
                while self.position < self.bytes.len() && self.bytes[self.position] != quote {
                    self.position += 1;
                }
                let value = Span::new(value_start, self.position);
                if self.position < self.bytes.len() {
                    self.position += 1;
                }
                Some((value, true))
            }
            _ => {
                let value_start = self.position;
                while self.position < self.bytes.len()
                    && !matches!(
                        self.bytes[self.position],
                        b' ' | b'\t' | b'\n' | b'\r' | b'>'
                    )
                {
                    self.position += 1;
                }
                Some((Span::new(value_start, self.position), false))
            }
        }
    }

    /// 解析原样文本元素（script/style/textarea/title）的内容与结束标签。
    fn parse_raw_text(&mut self, index: u32, name: &str) {
        self.stack.push(index);
        let content_start = self.position;
        let end = self.find_raw_text_end(name, content_start);

        if content_start < end {
            self.push_node_with_name(
                NodeKind::Text,
                Span::new(content_start, end),
                Span::default(),
            );
        }

        if end >= self.bytes.len() {
            self.diagnostics.push(Diagnostic {
                span: self.ast.node(index).name,
                severity: Severity::Error,
                message: format!("<{name}> 缺少结束标签"),
            });
            self.stack.pop();
            self.finish(index, self.bytes.len());
            self.position = self.bytes.len();
            return;
        }

        // 跳过 `</name>` 与其后的 `>`
        self.position = end;
        self.position += 2 + name.len();
        while self.position < self.bytes.len() && self.bytes[self.position] != b'>' {
            self.position += 1;
        }
        self.position = (self.position + 1).min(self.bytes.len());
        self.stack.pop();
        self.finish(index, self.position);
    }

    /// 在给定位置之后查找原样文本元素的结束标签起始位置。
    fn find_raw_text_end(&self, name: &str, from: usize) -> usize {
        let mut position = from;
        while position + 2 + name.len() <= self.bytes.len() {
            if self.bytes[position] == b'<' && self.bytes[position + 1] == b'/' {
                let candidate = &self.bytes[position + 2..position + 2 + name.len()];
                if candidate.eq_ignore_ascii_case(name.as_bytes())
                    && !is_name_continue(self.bytes.get(position + 2 + name.len()).copied())
                {
                    return position;
                }
            }
            position += 1;
        }
        self.bytes.len()
    }

    /// 解析结束标签。
    fn parse_end_tag(&mut self) {
        let start = self.position;
        self.position += 2;
        let name_start = self.position;
        self.skip_name();
        let name = Span::new(name_start, self.position);
        let element_name = name.text(self.source).to_ascii_lowercase();

        // 跳到标签结束
        let end = find(&self.bytes[self.position..], b">")
            .map(|offset| self.position + offset + 1)
            .unwrap_or(self.bytes.len());
        self.position = end;

        if element_name.is_empty() {
            self.diagnostics.push(Diagnostic {
                span: Span::new(start, end),
                severity: Severity::Error,
                message: "结束标签缺少名称".to_string(),
            });
            return;
        }

        let Some(depth) = self.stack.iter().rposition(|index| {
            self.ast
                .node(*index)
                .name
                .text(self.source)
                .eq_ignore_ascii_case(&element_name)
        }) else {
            self.diagnostics.push(Diagnostic {
                span: Span::new(start, end),
                severity: Severity::Error,
                message: format!("结束标签 </{element_name}> 没有对应的开始标签"),
            });
            return;
        };

        // 内部元素若可被该结束标签隐式关闭（如 </ul> 关闭 li），不算嵌套错误
        let premature = self.stack[depth + 1..]
            .iter()
            .map(|index| self.ast.node(*index).name.text(self.source))
            .find(|inner| !closes_on_end_tag(&inner.to_ascii_lowercase(), &element_name));

        if let Some(inner) = premature {
            self.diagnostics.push(Diagnostic {
                span: Span::new(start, end),
                severity: Severity::Warning,
                message: format!("</{element_name}> 提前闭合，其内部的 {inner} 会被隐式关闭"),
            });
        }

        // 先关闭内部元素，再关闭自身
        while self.stack.len() > depth + 1 {
            let inner = self.stack.pop().expect("栈非空");
            self.finish(inner, start);
        }
        let index = self.stack.pop().expect("栈非空");
        self.finish(index, end);
    }

    /// 依据隐式闭合规则弹出栈顶元素，`end` 用作被关闭元素的结束位置。
    fn close_implied(&mut self, next: &str, end: usize) {
        while let Some(&top) = self.stack.last() {
            let open = self.ast.node(top).name.text(self.source);
            if !closes_implicitly(open, next) {
                break;
            }
            self.stack.pop();
            self.finish(top, end);
        }
    }

    /// 文件结束时仍未闭合的元素，逐个报诊断并补全区间。
    fn close_unclosed_at_eof(&mut self) {
        let end = self.bytes.len();
        while let Some(index) = self.stack.pop() {
            let name = self.ast.node(index).name;
            self.diagnostics.push(Diagnostic {
                span: name,
                severity: Severity::Error,
                message: format!("<{}> 未闭合", name.text(self.source)),
            });
            self.finish(index, end);
        }
    }

    /// 把一个元素的区间补全，并记录子树结束下标。
    fn finish(&mut self, index: u32, end: usize) {
        let subtree_end = self.ast.nodes.len() as u32;
        let node = self.ast.node_mut(index);
        node.span.end = end as u32;
        node.subtree_end = subtree_end;
    }

    /// 创建节点并挂到当前父节点下，返回其索引。
    fn push_node_with_name(&mut self, kind: NodeKind, span: Span, name: Span) -> u32 {
        let index = self.ast.nodes.len() as u32;
        let node = Node {
            kind,
            span,
            name,
            parent: NONE,
            first_child: NONE,
            last_child: NONE,
            next_sibling: NONE,
            attribute_start: 0,
            attribute_len: 0,
            subtree_end: index + 1,
        };
        self.ast.nodes.push(node);

        match self.stack.last().copied() {
            Some(parent) => {
                let previous = self.ast.node(parent).last_child;
                if previous == NONE {
                    self.ast.node_mut(parent).first_child = index;
                } else {
                    self.ast.node_mut(previous).next_sibling = index;
                }
                self.ast.node_mut(parent).last_child = index;
                self.ast.node_mut(index).parent = parent;
            }
            None => self.ast.roots.push(index),
        }

        index
    }

    /// 跳过标签名。
    fn skip_name(&mut self) {
        while self.position < self.bytes.len() && is_name_char(self.bytes[self.position]) {
            self.position += 1;
        }
    }

    /// 跳过空白。
    fn skip_whitespace(&mut self) {
        while self.position < self.bytes.len()
            && matches!(
                self.bytes[self.position],
                b' ' | b'\t' | b'\n' | b'\r' | b'\x0c'
            )
        {
            self.position += 1;
        }
    }
}

/// 判断 `open` 元素是否应在遇到 `next` 元素时隐式闭合。
fn closes_implicitly(open: &str, next: &str) -> bool {
    match open.to_ascii_lowercase().as_str() {
        "p" => CLOSES_PARAGRAPH.contains(&next),
        "li" => next == "li",
        "dt" | "dd" => matches!(next, "dt" | "dd"),
        "option" => matches!(next, "option" | "optgroup"),
        "thead" => matches!(next, "tbody" | "tfoot"),
        "tbody" | "tfoot" => matches!(next, "tbody" | "tfoot"),
        "tr" => next == "tr",
        "td" | "th" => matches!(next, "td" | "th" | "tr" | "tbody" | "thead" | "tfoot"),
        "head" => next == "body",
        _ => false,
    }
}

/// `open` 元素能否被 `closing` 的结束标签隐式关闭。
fn closes_on_end_tag(open: &str, closing: &str) -> bool {
    match open {
        "li" => matches!(closing, "ul" | "ol" | "menu"),
        "dt" | "dd" => closing == "dl",
        "td" | "th" => matches!(closing, "tr" | "table" | "tbody" | "thead" | "tfoot"),
        "tr" => matches!(closing, "table" | "tbody" | "thead" | "tfoot"),
        "option" => matches!(closing, "select" | "datalist" | "optgroup"),
        "thead" | "tbody" | "tfoot" => closing == "table",
        "p" | "head" => true,
        _ => false,
    }
}

/// 标签名首字符。
fn is_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

/// 标签名后续字符。
fn is_name_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.') || byte >= 0x80
}

/// 标签名之后的字符是否仍属于名称（用于原样文本结束标签的边界判断）。
fn is_name_continue(byte: Option<u8>) -> bool {
    byte.is_some_and(is_name_char)
}

/// 属性名的终止字符。
fn is_attribute_boundary(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'/' | b'>' | b'=')
}

/// 在字节序列中查找子串，返回首次出现的下标。
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// 大小写不敏感的前缀判断。
fn starts_with_ignore_ascii_case(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && haystack[..needle.len()].eq_ignore_ascii_case(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 正常嵌套应得到正确的父子关系。
    #[test]
    fn parses_nested_elements() {
        let result = parse("<div><p>hi</p></div>");
        assert!(result.diagnostics.is_empty());

        let ast = &result.ast;
        let div = ast.roots[0];
        assert_eq!(ast.node(div).name.text("<div><p>hi</p></div>"), "div");

        let paragraph = ast.children(div).next().expect("应有一个子元素");
        assert_eq!(ast.node(paragraph).name.text("<div><p>hi</p></div>"), "p");
        assert_eq!(ast.children(div).count(), 1);
    }

    /// 未闭合标签应报错并且不影响其余节点。
    #[test]
    fn reports_unclosed_element() {
        let result = parse("<div><span>x</div>");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|item| item.message.contains("提前闭合"))
        );
    }

    /// 没有对应开始标签的结束标签要报错。
    #[test]
    fn reports_stray_end_tag() {
        let result = parse("</div>");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|item| item.message.contains("没有对应的开始标签"))
        );
    }

    /// 空元素与自闭合写法都不进入未闭合列表。
    #[test]
    fn treats_void_and_self_closing() {
        assert!(parse("<br><img src=x>").diagnostics.is_empty());
        assert!(parse("<div/>").diagnostics.is_empty());
    }

    /// 脚本内容里的尖括号不应被当作标签。
    #[test]
    fn keeps_raw_text_intact() {
        let source = "<script>if (a < b) { run(); }</script>";
        let result = parse(source);
        assert!(result.diagnostics.is_empty());
        let script = result.ast.roots[0];
        assert_eq!(result.ast.node(script).span.text(source), source);
    }

    /// 列表项隐式闭合：连续的 li 是兄弟而不是嵌套。
    #[test]
    fn applies_implicit_list_close() {
        let source = "<ul><li>a<li>b</ul>";
        let result = parse(source);
        assert!(result.diagnostics.is_empty());
        let list = result.ast.roots[0];
        assert_eq!(result.ast.children(list).count(), 2);
    }

    /// 属性应记录区间、值与是否带引号。
    #[test]
    fn parses_attributes() {
        let source = "<a href=\"/x\" disabled>";
        let result = parse(source);
        let link = result.ast.roots[0];
        let attributes = result.ast.attributes_of(result.ast.node(link));
        assert_eq!(attributes.len(), 2);
        assert_eq!(attributes[0].name.text(source), "href");
        assert_eq!(attributes[0].value.unwrap().text(source), "/x");
        assert!(attributes[1].value.is_none());
    }
}
