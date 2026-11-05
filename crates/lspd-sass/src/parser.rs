//! 缩进式 Sass 解析器：行首缩进定块，换行即语句边界，无花括号与分号。
//!
//! 产出与 CSS 完全同构的 `lspd_css::ast::Ast`：节点区间都落在原始 `.sass` 字节偏移上，
//! 因此补全、悬停、诊断、跳转、选择区间等能力可直接复用 CSS 侧实现，本模块不重复一套逻辑。

use lspd_core::text::Span;
use lspd_css::ast::{Ast, Node, NodeKind, NONE};
use lspd_css::data::is_known_property;
use lspd_css::parser::{Diagnostic, ParseResult, Severity};

/// 解析一份缩进式 Sass 文档。
pub fn parse(source: &str) -> ParseResult {
    Parser::new(source).run()
}

/// 一行的预处理结果。
struct Line {
    /// 行首缩进（前导空白字符数）
    indent: usize,
    /// 首个非空白字符的字节偏移
    content_start: usize,
    /// 去掉首尾空白与可能存在的行尾分号后的内容
    content: String,
}

/// 单遍按行扫描的解析器。
struct Parser<'a> {
    /// 原文
    source: &'a str,
    /// 语法树
    ast: Ast,
    /// 打开的块：(缩进, 节点索引)
    stack: Vec<(usize, u32)>,
    /// 结构诊断
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    /// 建立解析器。
    fn new(source: &'a str) -> Self {
        Self { source, ast: Ast::default(), stack: Vec::new(), diagnostics: Vec::new() }
    }

    /// 执行解析。
    fn run(mut self) -> ParseResult {
        let lines = preprocess(self.source);

        for (index, line) in lines.iter().enumerate() {
            // 关闭缩进不小于本行的所有块：换行 + 退格即块结束
            while self.stack.last().is_some_and(|&(top, _)| top >= line.indent) {
                let (_, node) = self.stack.pop().expect("刚检查过非空");
                self.finish(node, line.content_start);
            }

            if line.content.starts_with('@') {
                self.open_at_rule(line);
            } else {
                self.open_statement(line, &lines, index);
            }
        }

        self.close_unclosed();
        self.diagnostics.sort_by_key(|item| item.span.start);
        ParseResult { ast: self.ast, diagnostics: self.diagnostics }
    }

    /// 解析一条 at 规则：`@media screen`、`@import "a"` 等，作为块开启者入栈。
    fn open_at_rule(&mut self, line: &Line) {
        let at_start = line.content_start;
        let name_len = 1 + line.content[1..].chars().take_while(|c| is_name_char(*c)).count();
        let name = Span::new(at_start, at_start + name_len);
        let prelude = self.tight(Span::new(at_start + name_len, at_start + line.content.len()));
        let index = self.push(NodeKind::AtRule, Span::new(at_start, at_start + line.content.len()), name, prelude);
        self.stack.push((line.indent, index));
    }

    /// 解析一条声明或选择器：靠冒号与后续缩进区分二者。
    fn open_statement(&mut self, line: &Line, lines: &[Line], index: usize) {
        let content_end = line.content_start + line.content.len();

        let Some(colon) = find_colon(self.source, line.content_start, content_end) else {
            // 无冒号行：已知属性 / 变量（正在输入、还没打冒号）当作叶子声明，使补全能给属性上下文；
            // 其余（元素 / 嵌套选择器）作为块开启者。
            let content_text = &self.source[line.content_start..content_end];
            if is_known_property(content_text) || content_text.starts_with("--") || content_text.starts_with('$') {
                let name = Span::new(line.content_start, content_end);
                let node = self.push(NodeKind::Declaration, name, name, Span::default());
                self.finish(node, content_end);
            } else {
                self.open_rule(line, content_end);
            }
            return;
        };

        let name = self.tight(Span::new(line.content_start, colon));
        let name_text = name.text(self.source);
        let parent_is_rule = self.stack.last().is_some_and(|&(_, node)| self.ast.node(node).kind == NodeKind::Rule);

        // 是否是一条声明：已知属性 / 自定义属性一定算；
        // 未知名只有在"不是块开启者"时才当作声明（拼错的属性），
        // 后随更深缩进的行说明它是带嵌套的选择器。
        let is_declaration = is_known_property(name_text)
            || name_text.starts_with("--")
            || name_text.starts_with('$')
            || (parent_is_rule && is_simple_ident(name_text) && !peek_deeper(lines, index, line.indent));

        if !is_declaration {
            self.open_rule(line, content_end);
            return;
        }

        let value = self.tight(Span::new(colon + 1, content_end));
        let node = self.push(NodeKind::Declaration, Span::new(line.content_start, content_end), name, value);
        self.finish(node, content_end);

        if value.is_empty() {
            self.diagnostics
                .push(Diagnostic { span: name, severity: Severity::Warning, message: format!("`{name_text}` 缺少值") });
        }
    }

    /// 打开一个选择器块。
    fn open_rule(&mut self, line: &Line, content_end: usize) {
        let name = self.tight(Span::new(line.content_start, content_end));
        let index = self.push(NodeKind::Rule, Span::new(line.content_start, content_end), name, Span::default());
        self.stack.push((line.indent, index));
    }

    /// 文件结束：关闭仍打开的块。
    fn close_unclosed(&mut self) {
        let end = self.source.len();
        while let Some((_, index)) = self.stack.pop() {
            self.finish(index, end);
        }
    }

    /// 把区间收缩到不含首尾空白。
    fn tight(&self, span: Span) -> Span {
        let text = span.text(self.source);
        let leading = text.len() - text.trim_start().len();
        let trailing = text.len() - text.trim_end().len();
        Span::new(span.start as usize + leading, (span.end as usize).saturating_sub(trailing))
    }

    /// 补全区间并记录子树结束。
    fn finish(&mut self, index: u32, end: usize) {
        let subtree_end = self.ast.nodes.len() as u32;
        let node = self.ast.node_mut(index);
        node.span.end = end as u32;
        node.subtree_end = subtree_end;
    }

    /// 创建节点并挂到当前块下。
    fn push(&mut self, kind: NodeKind, span: Span, name: Span, value: Span) -> u32 {
        let index = self.ast.nodes.len() as u32;
        self.ast.nodes.push(Node {
            kind,
            span,
            name,
            value,
            parent: NONE,
            first_child: NONE,
            last_child: NONE,
            next_sibling: NONE,
            subtree_end: index + 1,
        });

        match self.stack.last().copied() {
            Some((_, parent)) => {
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
}

/// 把文档切成行，并算好缩进与内容；跳过纯空行与注释行（不参与块结构）。
fn preprocess(source: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut offset = 0usize;

    for segment in source.split('\n') {
        let text = segment.trim_end_matches('\r');
        let lead = text.len() - text.trim_start_matches([' ', '\t']).len();
        let body = text.trim();
        let is_skippable = body.is_empty() || body.starts_with("//") || body.starts_with("/*");

        if !is_skippable {
            lines.push(Line {
                indent: lead,
                content_start: offset + lead,
                content: body.strip_suffix(';').unwrap_or(body).to_string(),
            });
        }

        offset += segment.len() + 1;
    }

    lines
}

/// 下一行（跳过空行与注释）是否比当前更深：更深说明当前行开启了嵌套块。
fn peek_deeper(lines: &[Line], index: usize, indent: usize) -> bool {
    lines[index + 1..].iter().find(|next| !next.content.is_empty()).is_some_and(|next| next.indent > indent)
}

/// 在 `start..end` 内找到括号与字符串之外的首个冒号，返回绝对字节偏移。
fn find_colon(source: &str, start: usize, end: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut index = start;
    let mut depth = 0usize;

    while index < end {
        match bytes[index] {
            b'"' | b'\'' => index = skip_string(bytes, index),
            b'(' => {
                depth += 1;
                index += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b':' if depth == 0 => return Some(index),
            _ => index += 1,
        }
    }

    None
}

/// 跳过字符串，返回结束后的位置。
fn skip_string(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            byte if byte == quote => return index + 1,
            _ => index += 1,
        }
    }
    bytes.len()
}

/// 属性名允许字符（at 规则名亦用此判定边界）。
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_') || c >= '\u{80}'
}

/// 简单标识符：字母或 `-` 开头，仅含 `[A-Za-z0-9_-]`，用于在规则块内识别拼错的属性名。
fn is_simple_ident(text: &str) -> bool {
    !text.is_empty()
        && text.starts_with(|c: char| c.is_ascii_alphabetic() || c == '-')
        && text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lspd_css::ast::NodeKind;

    /// 缩进即嵌套：规则与两条声明形成父子关系。
    #[test]
    fn indentation_defines_nesting() {
        let source = "a\n  color: red\n  margin: 0\n";
        let result = parse(source);
        assert_eq!(result.ast.roots.len(), 1);
        let rule = result.ast.roots[0];
        assert_eq!(result.ast.node(rule).kind, NodeKind::Rule);
        assert_eq!(result.ast.node(rule).name.text(source), "a");
        assert_eq!(result.ast.children(rule).count(), 2);
    }

    /// 已知属性即使在顶层也按声明建节点。
    #[test]
    fn known_property_is_declaration() {
        let source = "color: red";
        let result = parse(source);
        assert_eq!(result.ast.roots.len(), 1);
        assert_eq!(result.ast.node(result.ast.roots[0]).kind, NodeKind::Declaration);
    }

    /// 规则块内拼错的属性名当作声明，使未知属性诊断仍能触发。
    #[test]
    fn unknown_property_inside_rule_is_declaration() {
        let source = "a\n  colr: red\n";
        let result = parse(source);
        let rule = result.ast.roots[0];
        let decl = result.ast.children(rule).next().unwrap();
        assert_eq!(result.ast.node(decl).kind, NodeKind::Declaration);
        assert_eq!(result.ast.node(decl).name.text(source), "colr");
    }

    /// `span:hover` 这类带冒号、后随更深缩进的行是嵌套选择器，不是声明。
    #[test]
    fn selector_with_pseudo_opens_block() {
        let source = "a\n  span:hover\n    color: red\n";
        let result = parse(source);
        let rule = result.ast.roots[0];
        let nested = result.ast.children(rule).next().unwrap();
        assert_eq!(result.ast.node(nested).kind, NodeKind::Rule);
        assert_eq!(result.ast.node(nested).name.text(source), "span:hover");
        assert_eq!(result.ast.children(nested).count(), 1);
    }

    /// at 规则作为块开启者。
    #[test]
    fn at_rule_opens_block() {
        let source = "@media screen\n  a\n    color: red\n";
        let result = parse(source);
        let at = result.ast.roots[0];
        assert_eq!(result.ast.node(at).kind, NodeKind::AtRule);
        assert_eq!(result.ast.node(at).name.text(source), "@media");
    }

    /// 行尾分号可写可不写，都不进入属性值。
    #[test]
    fn trailing_semicolon_optional() {
        for source in ["a\n  color: red;\n", "a\n  color: red\n"] {
            let result = parse(source);
            let rule = result.ast.roots[0];
            let decl = result.ast.children(rule).next().unwrap();
            assert_eq!(result.ast.node(decl).value.text(source), "red", "原文：{source}");
        }
    }

    /// 退格回到更浅缩进时关闭上一个块。
    #[test]
    fn dedent_closes_block() {
        let source = "a\n  color: red\nb\n  margin: 0\n";
        let result = parse(source);
        assert_eq!(result.ast.roots.len(), 2);
        assert_eq!(result.ast.children(result.ast.roots[0]).count(), 1);
        assert_eq!(result.ast.children(result.ast.roots[1]).count(), 1);
    }

    /// 变量定义 `$x: value` 应入 AST 为声明（否则变量补全与引用均失效）。
    #[test]
    fn variable_definition_is_declaration() {
        let source = "$width: 100px\na\n  margin: $width\n";
        let result = parse(source);
        let top = result.ast.roots[0];
        assert_eq!(result.ast.node(top).kind, NodeKind::Declaration);
        assert_eq!(result.ast.node(top).name.text(source), "$width");
    }
}
