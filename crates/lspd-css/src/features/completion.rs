//! 补全：属性名、属性值、at 规则、伪类与伪元素、文档内变量。
//!
//! 上下文判定以语法树为准（不依赖花括号/分号的文本扫描），
//! 因此 scss（有括号）与缩进式 Sass（无括号）共用同一套补全。

use serde_json::{Value, json};

use crate::ast::NodeKind;
use crate::data;
use crate::features::{View, find_colon, innermost};
use lspd_core::text::Span;

/// LSP 的 CompletionItemKind 取值。
const KIND_PROPERTY: u8 = 10;
/// 关键字按 Keyword 呈现。
const KIND_KEYWORD: u8 = 14;
/// 伪类与 at 规则按 Module 呈现。
const KIND_MODULE: u8 = 9;

/// 无节点时的哨兵索引。
const NONE_INDEX: u32 = u32::MAX;

/// 单次返回条目上限。
const MAX_ITEMS: usize = 200;

/// CSS 全局关键字，任何属性都接受。
const WIDE_KEYWORDS: &[&str] = &["inherit", "initial", "unset", "revert", "revert-layer"];

/// 常用函数，值位置的补全候选。
const FUNCTIONS: &[&str] = &["calc(", "var(", "rgb(", "hsl(", "min(", "max(", "clamp(", "url("];

/// 一条方言 at 规则关键字：名称、归类、用途。
struct Keyword {
    /// 含 `@` 的名称
    name: &'static str,
    /// 归类（悬停详情）
    detail: &'static str,
    /// 用途说明
    description: &'static str,
}

/// Sass 专有 at 规则关键字（SCSS 与缩进语法共用）。
const SASS_KEYWORDS: &[Keyword] = &[
    Keyword { name: "@mixin", detail: "Sass", description: "定义可复用的样式混入（mixin）。" },
    Keyword { name: "@include", detail: "Sass", description: "在规则中引入一个 mixin。" },
    Keyword { name: "@function", detail: "Sass", description: "定义返回值的 Sass 函数。" },
    Keyword { name: "@extend", detail: "Sass", description: "把一个选择器的样式继承到当前选择器。" },
    Keyword { name: "@use", detail: "Sass", description: "加载模块并暴露其成员（命名空间）。" },
    Keyword { name: "@forward", detail: "Sass", description: "把某个模块的成员转出给使用者。" },
    Keyword { name: "@if", detail: "Sass", description: "条件分支。" },
    Keyword { name: "@else", detail: "Sass", description: "`@if` 的否则分支。" },
    Keyword { name: "@each", detail: "Sass", description: "遍历列表或 map。" },
    Keyword { name: "@for", detail: "Sass", description: "按区间计数循环。" },
    Keyword { name: "@while", detail: "Sass", description: "条件循环。" },
    Keyword { name: "@return", detail: "Sass", description: "从 `@function` 返回结果。" },
    Keyword { name: "@at-root", detail: "Sass", description: "在嵌套中把规则提升到根。" },
    Keyword { name: "@content", detail: "Sass", description: "在 mixin 内渲染 `@include` 传入的块。" },
    Keyword { name: "@debug", detail: "Sass", description: "输出调试信息。" },
    Keyword { name: "@warn", detail: "Sass", description: "发出警告。" },
    Keyword { name: "@error", detail: "Sass", description: "抛出错误。" },
];

/// Less 专有 at 规则关键字。
const LESS_KEYWORDS: &[Keyword] = &[
    Keyword { name: "@plugin", detail: "Less", description: "加载一个 Less 插件。" },
    Keyword { name: "@reference", detail: "Less", description: "引入仅被 mixin 匹配、不直接输出的样式。" },
];

/// 补全上下文。
enum Context {
    /// 块内的属性名位置
    Property,
    /// 块内的值位置
    Value,
    /// 选择器中的伪类或伪元素
    Pseudo { double_colon: bool },
    /// at 规则位置
    AtRule,
    /// 其余位置不补全
    None,
}

/// 计算补全结果。`language_id` 决定叠加哪些方言 at 规则关键字。
pub fn complete(view: &View<'_>, offset: u32, language_id: &str) -> Value {
    let context = detect(view, offset);
    let (start, _) = replace_range(view.source, offset);
    let prefix = &view.source[start as usize..offset as usize];
    let range = view.range(Span { start, end: offset });
    let mut items: Vec<Value> = Vec::new();

    match context {
        Context::Property => {
            push_properties(prefix, &range, &mut items);
            push_variables(view, prefix, &range, &mut items);
        }
        Context::Value => {
            push_values(prefix, &mut items);
            push_variables(view, prefix, &range, &mut items);
        }
        Context::Pseudo { double_colon } => push_pseudos(double_colon, prefix, &range, &mut items),
        Context::AtRule => push_at_rules(language_id, prefix, &range, &mut items),
        Context::None => {}
    }

    json!({ "isIncomplete": false, "items": items })
}

/// 依据语法树与当前行判定补全上下文。
fn detect(view: &View<'_>, offset: u32) -> Context {
    let source = view.source;
    let line_start = start_of_line(source, offset);

    // 当前行首为 `@`：at 规则关键字位置（scss 与缩进语法皆适用）
    if source[line_start as usize..offset as usize].trim_start().starts_with('@') {
        return Context::AtRule;
    }

    match innermost(view.ast, offset) {
        Some(index) => {
            let node = view.ast.node(index);
            match node.kind {
                NodeKind::Declaration => {
                    if node.name.contains(offset) {
                        Context::Property
                    } else if node.value.contains(offset) || find_colon(source, line_start, offset).is_some() {
                        Context::Value
                    } else {
                        Context::Property
                    }
                }
                NodeKind::Rule => {
                    if node.name.contains(offset) {
                        selector_context(view, index, line_start, offset)
                    } else {
                        block_body_context(source, line_start, offset)
                    }
                }
                NodeKind::AtRule => {
                    if node.name.contains(offset) || node.value.contains(offset) {
                        Context::None
                    } else {
                        block_body_context(source, line_start, offset)
                    }
                }
            }
        }
        None => selector_context(view, NONE_INDEX, line_start, offset),
    }
}

/// 块体内：冒号之前是属性名，之后是值。
fn block_body_context(source: &str, line_start: u32, offset: u32) -> Context {
    if find_colon(source, line_start, offset).is_some() {
        Context::Value
    } else {
        Context::Property
    }
}

/// 选择器位置的上下文。
///
/// 缩进语法里未写冒号的属性行会被解析成"无子节点的选择器"；当光标位于选择器结束之后的
/// 行（即块内）时按块内处理给属性。其余情况：冒号之前给伪类/伪元素，无冒号不补全。
fn selector_context(view: &View<'_>, index: u32, line_start: u32, offset: u32) -> Context {
    if index != NONE_INDEX {
        let node = view.ast.node(index);
        let is_leaf = view.ast.children(index).next().is_none();
        if is_leaf && line_start >= node.name.end {
            return block_body_context(view.source, line_start, offset);
        }
    }

    if let Some(position) = find_colon(view.source, line_start, offset) {
        let before = &view.source[line_start as usize..position as usize];
        Context::Pseudo { double_colon: before.trim_end().ends_with(':') }
    } else {
        Context::None
    }
}

/// 行首字节偏移。
fn start_of_line(source: &str, offset: u32) -> u32 {
    let bytes = source.as_bytes();
    let mut index = (offset as usize).min(bytes.len());
    while index > 0 && bytes[index - 1] != b'\n' {
        index -= 1;
    }
    index as u32
}

/// 计算替换区间：单词起点到光标；`@` 开头的词把 `@` 一并纳入。
fn replace_range(source: &str, offset: u32) -> (u32, u32) {
    let bytes = source.as_bytes();
    let mut start = (offset as usize).min(bytes.len());
    while start > 0 && (is_word_byte(bytes[start - 1]) || bytes[start - 1] == b'@') {
        start -= 1;
    }
    (start as u32, offset)
}

/// 单词字符集。
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') || byte >= 0x80
}

/// 推送属性名补全。
fn push_properties(prefix: &str, range: &Value, items: &mut Vec<Value>) {
    let prefix = prefix.to_ascii_lowercase();

    for entry in data::PROPERTIES {
        if !prefix.is_empty() && !entry.name.starts_with(&prefix) {
            continue;
        }
        if items.len() >= MAX_ITEMS {
            return;
        }
        items.push(json!({
            "label": entry.name,
            "kind": KIND_PROPERTY,
            "detail": if entry.syntax.is_empty() { "属性".to_string() } else { format!("属性：{}", entry.syntax) },
            "documentation": { "kind": "markdown", "value": entry.description },
            "textEdit": { "range": range.clone(), "newText": entry.name },
        }));
    }
}

/// 推送文档内变量补全。仅在变量前缀（`--` / `$` / Less `@`）出现时给，避免刷屏。
fn push_variables(view: &View<'_>, prefix: &str, range: &Value, items: &mut Vec<Value>) {
    if !(prefix.starts_with("--") || prefix.starts_with('$') || prefix.starts_with('@')) {
        return;
    }

    for index in view.ast.nodes_of(NodeKind::Declaration) {
        let name = view.text(view.ast.node(index).name);
        if !(name.starts_with("--") || name.starts_with('$') || name.starts_with('@')) {
            continue;
        }
        if name == prefix || !name.starts_with(prefix) {
            continue;
        }
        items.push(json!({
            "label": name,
            "kind": KIND_PROPERTY,
            "detail": "变量",
            "textEdit": { "range": range.clone(), "newText": name },
        }));
    }
}

/// 推送值补全：全局关键字、函数、命名颜色。
fn push_values(prefix: &str, items: &mut Vec<Value>) {
    let prefix = prefix.to_ascii_lowercase();

    for keyword in WIDE_KEYWORDS {
        if !prefix.is_empty() && !keyword.starts_with(&prefix) {
            continue;
        }
        items.push(json!({ "label": keyword, "kind": KIND_KEYWORD, "detail": "全局关键字" }));
    }
    for function in FUNCTIONS {
        if !prefix.is_empty() && !function.starts_with(&prefix) {
            continue;
        }
        items.push(json!({ "label": function, "kind": KIND_KEYWORD, "detail": "函数" }));
    }

    for color in data::NAMED_COLORS {
        if !prefix.is_empty() && !color.name.starts_with(&prefix) {
            continue;
        }
        if items.len() >= MAX_ITEMS {
            return;
        }
        items.push(json!({
            "label": color.name,
            "kind": KIND_PROPERTY,
            "detail": format!("颜色 #{:02x}{:02x}{:02x}", color.red, color.green, color.blue),
            "documentation": {
                "kind": "markdown",
                "value": format!("```css\n{}\n```", color.name),
            },
        }));
    }
}

/// 推送伪类或伪元素补全。
fn push_pseudos(double_colon: bool, prefix: &str, range: &Value, items: &mut Vec<Value>) {
    let table = if double_colon { data::PSEUDO_ELEMENTS } else { data::PSEUDO_CLASSES };

    for entry in table {
        let name = entry.name.trim_start_matches(':');
        let label = if double_colon { format!("::{name}") } else { format!(":{name}") };
        if !prefix.is_empty() && !label.starts_with(prefix) {
            continue;
        }
        if items.len() >= MAX_ITEMS {
            return;
        }
        items.push(json!({
            "label": label,
            "kind": KIND_MODULE,
            "detail": if double_colon { "伪元素" } else { "伪类" },
            "documentation": { "kind": "markdown", "value": entry.description },
            "textEdit": { "range": range.clone(), "newText": label },
        }));
    }
}

/// 推送 at 规则补全：标准表 + 当前方言的专有关键字。
fn push_at_rules(language_id: &str, prefix: &str, range: &Value, items: &mut Vec<Value>) {
    let prefix = prefix.to_ascii_lowercase();
    let mut seen: Vec<&'static str> = Vec::new();

    for entry in data::AT_RULES {
        if push_rule(items, &mut seen, range, &prefix, entry.name, "at 规则", entry.description) {
            break;
        }
    }

    let keywords: &[Keyword] = match language_id {
        "scss" | "sass" => SASS_KEYWORDS,
        "less" => LESS_KEYWORDS,
        _ => &[],
    };

    for keyword in keywords {
        let documentation = format!("{}｜{}", keyword.detail, keyword.description);
        if push_rule(items, &mut seen, range, &prefix, keyword.name, keyword.detail, &documentation) {
            break;
        }
    }
}

/// 追加一条 at 规则/关键字；去重，命中上限返回 true。
fn push_rule<'a>(
    items: &mut Vec<Value>,
    seen: &mut Vec<&'a str>,
    range: &Value,
    prefix: &str,
    name: &'a str,
    detail: &str,
    description: &str,
) -> bool {
    if !prefix.is_empty() && !name.starts_with(prefix) {
        return false;
    }
    if seen.contains(&name) {
        return false;
    }
    if items.len() >= MAX_ITEMS {
        return true;
    }
    seen.push(name);
    items.push(json!({
        "label": name,
        "kind": KIND_MODULE,
        "detail": detail,
        "documentation": { "kind": "markdown", "value": description },
        "textEdit": { "range": range.clone(), "newText": name },
    }));
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::Dialect;
    use crate::parser::parse;
    use lspd_core::text::LineIndex;

    /// 在源文本中定位偏移后运行补全，返回候选标签。
    fn labels(source: &str, offset: u32, language_id: &str) -> Vec<String> {
        let parsed = parse(source, Dialect::Scss);
        let lines = LineIndex::new(source);
        let view = View::new(source, &parsed.ast, &lines);
        let result = complete(&view, offset, language_id);
        result["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["label"].as_str().unwrap().to_string())
            .collect()
    }

    /// 块内未写完的属性名给属性候选。
    #[test]
    fn property_within_block() {
        let source = "a {\n  col\n}";
        let offset = (source.find("col").unwrap() + 3) as u32;
        assert!(labels(source, offset, "scss").contains(&"color".to_string()));
    }

    /// 冒号之后给值候选（命名颜色）。
    #[test]
    fn value_after_colon() {
        let source = "a {\n  color: red\n}";
        let offset = (source.find("red").unwrap() + 3) as u32;
        assert!(labels(source, offset, "scss").contains(&"red".to_string()));
    }

    /// `@m` 在 scss 下应给出 Sass 专有的 `@mixin`。
    #[test]
    fn sass_at_keyword() {
        let source = "@m";
        assert!(labels(source, 2, "scss").contains(&"@mixin".to_string()));
    }

    /// 同一前缀在标准 css 下不应出现 Sass 专有关键字。
    #[test]
    fn css_excludes_sass_keyword() {
        let source = "@m";
        assert!(!labels(source, 2, "css").contains(&"@mixin".to_string()));
    }

    /// less 下 `@p` 给出 Less 专有的 `@plugin`。
    #[test]
    fn less_at_keyword() {
        let source = "@p";
        assert!(labels(source, 2, "less").contains(&"@plugin".to_string()));
    }
}
