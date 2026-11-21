//! Markdown 行扫描解析。
//!
//! 只提取语言服务需要的信息：标题（含 GitHub 风格锚点）、行内链接目标、代码围栏。
//! 不追求 CommonMark 完整合规——正文结构对大纲、折叠、跳转没有额外价值，
//! 因此这里是一份"够用的块级切片"，而非语法树。
//!
//! 链接识别三种形式：行内 `[文本](目标)`、引用式 `[文本][标签]` 与
//! 快捷引用 `[标签]`（仅当定义表里存在该标签时才算链接）。

use lspd_core::text::Span;

/// 一级或以上的标题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// 级别，1–6
    pub level: u8,
    /// 标题正文（不含 `#` 与行尾空白）的字节区间
    pub text_span: Span,
    /// GitHub 风格锚点（已去重）
    pub slug: String,
    /// 所在行号（0 起）
    pub line: u32,
}

/// 一个行内链接。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// 目标文本的字节区间：诊断标红用
    pub target_span: Span,
    /// 覆盖链接文本与目标的完整区间：definition 命中用
    pub full_span: Span,
    /// 目标原文，形如 `../a.md#小节`
    pub target: String,
    /// 链接语法：行内、`[[wikilink]]` 或引用式
    pub kind: LinkKind,
    /// 所在行号（0 起）
    pub line: u32,
}

/// 一行内一个链接的扫描结果。
struct LinkTarget {
    /// 目标区间（行内相对）
    target: (usize, usize),
    /// 完整区间（行内相对）
    full: (usize, usize),
    kind: LinkKind,
}

/// 链接的书写形式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    /// `[文本](目标)`
    Inline,
    /// `[[目标]]`，目标可含 `#锚点` 与 `|别名`
    Wiki,
    /// `[文本][标签]`，目标在定义表里查
    Reference(String),
}

/// 链接目标里锚点部分的区间，用于重命名与快速修复。
///
/// 引用式链接的锚点写在定义行，无法从链接处定位，返回 `None`。
pub fn anchor_span(link: &Link, definitions: &[LinkDefinition]) -> Option<Span> {
    // 引用式链接的锚点写在定义行，链接处只有标签，因此要到定义表里取目标区间
    let (target, base) = match &link.kind {
        LinkKind::Reference(label) => {
            let definition = definitions.iter().find(|item| &item.label == label)?;
            (&definition.target, definition.target_span.start as usize)
        }
        _ => (&link.target, link.target_span.start as usize),
    };

    let hash = target.find('#')?;
    let rest = &target[hash + 1..];
    let anchor = rest.split('|').next().unwrap_or(rest);
    let start = base + hash + 1;

    Some(Span::new(start, start + anchor.len()))
}

/// 引用式链接的定义：`[标签]: 目标`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkDefinition {
    /// 标签（不含方括号）
    pub label: String,
    /// 目标原文
    pub target: String,
    /// 目标在文档中的字节区间（重命名锚点时要改这一处，而不是链接处的标签）
    pub target_span: Span,
}

/// 代码围栏。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fence {
    /// 起始行（含）
    pub start_line: u32,
    /// 结束行（含）；未闭合时为文档末行
    pub end_line: u32,
}

/// 一份文档的解析结果。
#[derive(Debug, Default)]
pub struct Parsed {
    pub headings: Vec<Heading>,
    pub links: Vec<Link>,
    /// 引用式链接的定义表，按标签去重（后出现者覆盖前者，与 CommonMark 一致）
    pub definitions: Vec<LinkDefinition>,
    pub fences: Vec<Fence>,
    /// 总行数
    pub line_count: u32,
}

/// 解析整篇文档。
pub fn parse(source: &str) -> Parsed {
    let mut headings = Vec::new();
    let mut links = Vec::new();
    let mut definitions = Vec::new();
    let mut fences = Vec::new();
    // 已出现的锚点及其重复次数，用于 GitHub 的 `-1`、`-2` 去重
    let mut taken: Vec<(String, u32)> = Vec::new();

    let mut offset = 0usize;
    let mut line: u32 = 0;
    // 打开中的围栏：起始行、围栏字符、反引号/波浪线个数
    let mut open: Option<(u32, u8, usize)> = None;
    // 上一个段落行（setext 标题的正文来源）：行号、起始偏移、原文
    let mut paragraph: Option<(u32, usize, &str)> = None;
    // 快捷引用候选，等定义表收齐后再决定是否算链接
    let mut shortcuts: Vec<(u32, Span, Span, String)> = Vec::new();

    for raw in source.split_inclusive('\n') {
        let text = raw.strip_suffix('\n').unwrap_or(raw).trim_end_matches('\r');
        let base = offset;
        offset += raw.len();

        if let Some((start, marker, length)) = open {
            // 围栏内不产出标题与链接
            if is_closing_fence(text, marker, length) {
                fences.push(Fence {
                    start_line: start,
                    end_line: line,
                });
                open = None;
            }
            line += 1;
            continue;
        }

        if let Some((marker, length)) = opening_fence(text) {
            open = Some((line, marker, length));
            line += 1;
            continue;
        }

        // setext 标题：`===` / `---` 紧跟在段落行之后
        if let Some(level) = setext_underline(text)
            && let Some((paragraph_line, paragraph_base, paragraph_text)) = paragraph.take()
        {
            let start = paragraph_text.len() - paragraph_text.trim_start().len();
            let end = paragraph_text.trim_end().len();
            headings.push(Heading {
                level,
                text_span: Span::new(paragraph_base + start, paragraph_base + end),
                slug: unique_slug(&paragraph_text[start..end], &mut taken),
                line: paragraph_line,
            });
            line += 1;
            continue;
        }

        let definition = link_definition_at(text);
        if let Some((label, target, (start, end))) = &definition {
            definitions.push(LinkDefinition {
                label: label.clone(),
                target: target.clone(),
                target_span: Span::new(base + start, base + end),
            });
        }

        if let Some((level, start, end)) = heading_at(text) {
            let body = &text[start..end];
            headings.push(Heading {
                level,
                text_span: Span::new(base + start, base + end),
                slug: unique_slug(body, &mut taken),
                line,
            });
        }

        // 定义行本身不是链接（`[标签]: 目标` 的方括号不能被当成快捷引用）
        let (found, shortcut) = match definition.is_some() {
            true => (Vec::new(), Vec::new()),
            false => link_targets(text),
        };
        for item in found {
            links.push(Link {
                target_span: Span::new(base + item.target.0, base + item.target.1),
                full_span: Span::new(base + item.full.0, base + item.full.1),
                target: text[item.target.0..item.target.1].to_string(),
                kind: item.kind,
                line,
            });
        }
        for (target, full, label) in shortcut {
            shortcuts.push((
                line,
                Span::new(base + target.0, base + target.1),
                Span::new(base + full.0, base + full.1),
                label,
            ));
        }

        paragraph = paragraph_line(text, definition.is_some()).then_some((line, base, text));
        line += 1;
    }

    // 未闭合的围栏折叠到文档末尾
    if let Some((start, _, _)) = open {
        fences.push(Fence {
            start_line: start,
            end_line: line.saturating_sub(1),
        });
    }

    // 快捷引用只有在定义表里存在对应标签时才算链接
    for (line, target_span, full_span, label) in shortcuts {
        if definitions
            .iter()
            .any(|definition| definition.label == label)
        {
            links.push(Link {
                target_span,
                full_span,
                target: label.clone(),
                kind: LinkKind::Reference(label),
                line,
            });
        }
    }

    Parsed {
        headings,
        links,
        definitions,
        fences,
        line_count: line,
    }
}

/// GitHub 风格锚点：转小写，空白转连字符，其余标点丢弃（中英文均保留）。
pub fn slugify(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '-' || ch == '_' {
            slug.extend(ch.to_lowercase());
        } else if ch.is_whitespace() {
            slug.push('-');
        }
    }
    slug
}

/// 取当前文档内唯一的锚点。重复标题依次追加 `-1`、`-2`。
fn unique_slug(text: &str, taken: &mut Vec<(String, u32)>) -> String {
    let base = slugify(text);

    match taken.iter_mut().find(|(slug, _)| *slug == base) {
        Some((_, count)) => {
            *count += 1;
            format!("{base}-{count}")
        }
        None => {
            taken.push((base.clone(), 0));
            base
        }
    }
}

/// 识别 ATX 标题（`#` 开头）。返回级别与正文区间。
fn heading_at(line: &str) -> Option<(u8, usize, usize)> {
    // 四空格或制表符缩进属于缩进代码块
    if line.starts_with("    ") || line.starts_with('\t') {
        return None;
    }

    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }

    let end = line.trim_end().len();
    match line.as_bytes().get(hashes) {
        // `###` 后无正文
        None => Some((hashes as u8, hashes, hashes)),
        Some(b' ') | Some(b'\t') => {
            let start = hashes + (line[hashes..].len() - line[hashes..].trim_start().len());
            Some((hashes as u8, start.min(end), end))
        }
        // `#标签` 不是标题
        _ => None,
    }
}

/// 识别 setext 标题的下划线行，返回标题级别。
fn setext_underline(line: &str) -> Option<u8> {
    let trimmed = line.trim();
    let marker = *trimmed.as_bytes().first()?;
    if marker != b'=' && marker != b'-' {
        return None;
    }
    if !trimmed.bytes().all(|byte| byte == marker) {
        return None;
    }

    Some(if marker == b'=' { 1 } else { 2 })
}

/// 当前行能否作为 setext 标题的正文：排除空行、标题、列表、引用、表格与定义行。
fn paragraph_line(line: &str, is_definition: bool) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || is_definition {
        return false;
    }
    if trimmed.starts_with('#')
        || trimmed.starts_with('>')
        || trimmed.starts_with('|')
        || trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("+ ")
    {
        return false;
    }

    let digits = trimmed
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    !(digits > 0 && trimmed[digits..].starts_with(". "))
}

/// 识别围栏起始行，返回围栏字符与其长度。
fn opening_fence(line: &str) -> Option<(u8, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }

    let marker = *trimmed.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }

    let length = trimmed.bytes().take_while(|byte| *byte == marker).count();
    (length >= 3).then_some((marker, length))
}

/// 是否是某段围栏的闭合行。
fn is_closing_fence(line: &str, marker: u8, length: usize) -> bool {
    let trimmed = line.trim();
    let count = trimmed.bytes().take_while(|byte| *byte == marker).count();
    count >= length && count == trimmed.len()
}

/// 识别引用式链接定义 `[标签]: 目标`，返回标签、目标与目标在本行内的区间。
fn link_definition_at(line: &str) -> Option<(String, String, (usize, usize))> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let trimmed = &line[indent..];
    if !trimmed.starts_with('[') {
        return None;
    }

    let close = trimmed.find(']')?;
    let colon = trimmed[close + 1..].find(':')? + close + 1;
    let label = trimmed[1..close].to_string();
    if label.is_empty() {
        return None;
    }

    // 跳过冒号后的空白，记录目标真正开始的位置
    let after = &trimmed[colon + 1..];
    let spaces = after.len() - after.trim_start().len();
    let rest = &after[spaces..];
    let end = rest
        .find(|ch: char| ch.is_ascii_whitespace())
        .unwrap_or(rest.len());
    let target = rest[..end].to_string();
    if target.is_empty() {
        return None;
    }

    let start = indent + colon + 1 + spaces;
    Some((label, target, (start, start + end)))
}

/// 扫描一行的行内链接目标。
///
/// 返回确定形式的链接，以及快捷引用候选（`[标签]`）——后者要等定义表收齐才能定性。
/// 跳过行内代码（反引号包裹）内的伪链接，避免把文档里的示例当成真链接。
fn link_targets(
    line: &str,
) -> (
    Vec<LinkTarget>,
    Vec<((usize, usize), (usize, usize), String)>,
) {
    let bytes = line.as_bytes();
    let mut targets = Vec::new();
    let mut shortcuts = Vec::new();
    let mut index = 0usize;
    let mut in_code = false;
    // 最近一个未配对的 `[`，用于拼出完整链接区间
    let mut last_open: Option<usize> = None;

    while index < bytes.len() {
        match bytes[index] {
            b'`' => {
                in_code = !in_code;
                index += 1;
            }
            b'[' if !in_code => {
                // `[[目标]]`，目标可含 `#锚点` 与 `|别名`
                if bytes.get(index + 1) == Some(&b'[')
                    && let Some(close) = bytes[index + 2..].iter().position(|byte| *byte == b']')
                    && bytes.get(index + 2 + close + 1) == Some(&b']')
                {
                    let start = index + 2;
                    let end = index + 2 + close;
                    targets.push(LinkTarget {
                        target: (start, end),
                        full: (index, end + 2),
                        kind: LinkKind::Wiki,
                    });
                    index = end + 2;
                    continue;
                }
                last_open = Some(index);
                index += 1;
            }
            b']' if !in_code => {
                // `[文本](目标)`
                if bytes.get(index + 1) == Some(&b'(') {
                    let text_start = last_open.unwrap_or(index);
                    let (found, resume) = link_target_at(line, index + 2);
                    if let Some(span) = found {
                        targets.push(LinkTarget {
                            target: span,
                            full: (text_start, resume + 1),
                            kind: LinkKind::Inline,
                        });
                    }
                    index = resume;
                    last_open = None;
                    continue;
                }
                // `[文本][标签]`
                if bytes.get(index + 1) == Some(&b'[')
                    && let Some(close) = bytes[index + 2..].iter().position(|byte| *byte == b']')
                {
                    let label_start = index + 2;
                    let label_end = label_start + close;
                    let label = &line[label_start..label_end];
                    if !label.is_empty() {
                        let text_start = last_open.unwrap_or(index);
                        targets.push(LinkTarget {
                            target: (label_start, label_end),
                            full: (text_start, label_end + 1),
                            kind: LinkKind::Reference(label.to_string()),
                        });
                    }
                    index = label_end + 1;
                    last_open = None;
                    continue;
                }
                // `[标签]`：先记作候选，定义表收齐后再决定
                if let Some(open) = last_open.take() {
                    let label = &line[open + 1..index];
                    if !label.is_empty() && !label.starts_with('[') {
                        shortcuts.push(((open + 1, index), (open, index + 1), label.to_string()));
                    }
                }
                index += 1;
            }
            _ => index += 1,
        }
    }

    (targets, shortcuts)
}

/// 从 `](` 之后开始取链接目标，返回命中结果与继续扫描的位置。
fn link_target_at(line: &str, from: usize) -> (Option<(usize, usize)>, usize) {
    let bytes = line.as_bytes();
    let mut index = from;
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }

    // `<路径 允许含空格>` 形式
    if bytes.get(index) == Some(&b'<') {
        let start = index + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end] != b'>' {
            end += 1;
        }
        return match end < bytes.len() {
            true => (Some((start, end)), end),
            // 尖括号未闭合，按普通目标重试
            false => plain_target(line, start),
        };
    }

    plain_target(line, index)
}

/// 普通目标：直到空白或 `)` 为止。
fn plain_target(line: &str, from: usize) -> (Option<(usize, usize)>, usize) {
    let bytes = line.as_bytes();
    let mut end = from;
    while end < bytes.len() && !bytes[end].is_ascii_whitespace() && bytes[end] != b')' {
        end += 1;
    }

    match end > from {
        true => (Some((from, end)), end),
        false => (None, from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_follow_github_rules() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("中文 标题"), "中文-标题");
        assert_eq!(
            slugify("snake_case 与 kebab-case"),
            "snake_case-与-kebab-case"
        );
        assert_eq!(slugify("**Bold**"), "bold");
    }

    #[test]
    fn duplicate_headings_get_suffix() {
        let parsed = parse("# A\n# A\n# A\n");
        let slugs: Vec<&str> = parsed
            .headings
            .iter()
            .map(|item| item.slug.as_str())
            .collect();
        assert_eq!(slugs, ["a", "a-1", "a-2"]);
    }

    #[test]
    fn heading_levels_and_text() {
        let source = "# 一级\n\n### 三级  \n";
        let parsed = parse(source);
        assert_eq!(parsed.headings.len(), 2);
        assert_eq!(parsed.headings[0].level, 1);
        assert_eq!(parsed.headings[1].level, 3);
        assert_eq!(parsed.headings[1].text_span.text(source), "三级");
    }

    #[test]
    fn ignores_hashes_without_space_and_indented_code() {
        let parsed = parse("#tag\n    # 缩进代码\n####### 七个\n");
        assert!(parsed.headings.is_empty());
    }

    #[test]
    fn links_skip_code_fences_and_inline_code() {
        let source =
            "[a](./a.md)\n```\n[b](./b.md)\n```\n`[c](./c.md)`\n[d](<带 空 格.md> \"标题\")\n";
        let parsed = parse(source);
        let targets: Vec<&str> = parsed
            .links
            .iter()
            .map(|link| link.target.as_str())
            .collect();
        assert_eq!(targets, ["./a.md", "带 空 格.md"]);
    }

    #[test]
    fn fences_close_and_remain_open_at_eof() {
        let closed = parse("```rust\nfn main() {}\n```\n");
        assert_eq!(
            closed.fences,
            [Fence {
                start_line: 0,
                end_line: 2
            }]
        );

        let open = parse("~~~\ntext\n");
        assert_eq!(
            open.fences,
            [Fence {
                start_line: 0,
                end_line: 1
            }]
        );
    }

    #[test]
    fn wikilinks_are_recognized() {
        let source = "见 [[安装]] 与 [[使用手册#进阶|手册]]。
";
        let parsed = parse(source);
        assert_eq!(parsed.links.len(), 2);
        assert_eq!(parsed.links[0].target, "安装");
        assert_eq!(parsed.links[0].kind, LinkKind::Wiki);
        // 目标保留 `|别名` 原文，由后端在解析时切掉
        assert_eq!(parsed.links[1].target, "使用手册#进阶|手册");
        assert_eq!(
            parsed.links[1].target_span.text(source),
            "使用手册#进阶|手册"
        );
    }

    #[test]
    fn reference_definitions_are_collected() {
        let source = "[说明][ref]

[ref]: ./docs/安装.md
";
        let parsed = parse(source);
        assert_eq!(parsed.definitions.len(), 1);
        assert_eq!(parsed.definitions[0].label, "ref");
        assert_eq!(parsed.definitions[0].target, "./docs/安装.md");
    }

    #[test]
    fn reference_links_carry_the_label() {
        let source = "[说明][ref] 与 [空][]
";
        let parsed = parse(source);
        assert_eq!(parsed.links.len(), 1);
        assert_eq!(parsed.links[0].kind, LinkKind::Reference("ref".to_string()));
        assert_eq!(parsed.links[0].target, "ref");
    }

    #[test]
    fn full_span_covers_text_and_target() {
        let source = "见 [说明](./a.md) 与 [[b]]。
";
        let parsed = parse(source);
        assert_eq!(parsed.links[0].full_span.text(source), "[说明](./a.md)");
        assert_eq!(parsed.links[1].full_span.text(source), "[[b]]");
        let referenced = parse(
            "[文本][标签]
",
        );
        assert_eq!(
            referenced.links[0].full_span.text(
                "[文本][标签]
"
            ),
            "[文本][标签]"
        );
    }

    #[test]
    fn inline_code_links_are_ignored_for_wiki_too() {
        let parsed = parse(
            "`[[不是链接]]`
",
        );
        assert!(parsed.links.is_empty());
    }

    /// setext 标题：`===` 为一级，`---` 为二级；空行后的 `---` 不算标题。
    #[test]
    fn setext_headings_follow_paragraph_lines() {
        let source = "一级标题\n===\n\n二级标题\n---\n\n正文\n\n---\n";
        let parsed = parse(source);
        let levels: Vec<u8> = parsed.headings.iter().map(|item| item.level).collect();
        assert_eq!(levels, [1, 2]);
        assert_eq!(parsed.headings[0].text_span.text(source), "一级标题");
        assert_eq!(parsed.headings[1].text_span.text(source), "二级标题");
        assert_eq!(parsed.headings[0].slug, "一级标题");
    }

    /// 列表项与引用行不能作为 setext 标题正文。
    #[test]
    fn setext_skips_non_paragraph_lines() {
        let parsed = parse("- 列表\n---\n> 引用\n===\n");
        assert!(parsed.headings.is_empty());
    }

    /// 快捷引用只有在定义表存在时才算链接。
    #[test]
    fn shortcut_references_require_definitions() {
        let with_definition = parse("[说明]\n\n[说明]: ./a.md\n");
        assert_eq!(with_definition.links.len(), 1);
        assert_eq!(
            with_definition.links[0].kind,
            LinkKind::Reference("说明".into())
        );
        assert_eq!(
            with_definition.links[0]
                .target_span
                .text("[说明]\n\n[说明]: ./a.md\n"),
            "说明"
        );

        let without = parse("普通方括号 [不是链接]\n");
        assert!(without.links.is_empty());
    }

    /// 锚点区间只覆盖 `#` 之后的文本，wikilink 的别名不计入。
    #[test]
    fn anchor_span_covers_only_the_anchor() {
        let source = "见 [说明](./a.md#安装) 与 [[b#小节|别名]]。\n";
        let parsed = parse(source);
        assert_eq!(parser_anchor(&parsed, source, 0), Some("安装"));
        assert_eq!(parser_anchor(&parsed, source, 1), Some("小节"));
    }

    /// 取指定链接的锚点文本。
    fn parser_anchor<'a>(parsed: &Parsed, source: &'a str, index: usize) -> Option<&'a str> {
        let link = parsed.links.get(index)?;
        let span = anchor_span(link, &parsed.definitions)?;
        source.get(span.start as usize..span.end as usize)
    }

    #[test]
    fn anchor_span_points_at_target_text() {
        let source = "见 [说明](../docs/说明.md#安装)\n";
        let parsed = parse(source);
        let link = &parsed.links[0];
        assert_eq!(link.target_span.text(source), "../docs/说明.md#安装");
    }
}
