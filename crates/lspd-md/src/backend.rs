//! Markdown 后端：文档大纲、折叠区间与链接跳转。
//!
//! 缓存只保留当前文档的解析结果，按 URI 与版本失效，关闭文档即释放。
//! 跳转目标未打开时按需从磁盘读取并解析，读完立即丢弃，不建立常驻的工作区索引。

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use lspd_core::docs::{Document, DocumentStore, Position};
use lspd_core::text::{LineIndex, Span};
use lspd_core::{Backend, Outgoing, ResponseError};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::parser::{self, Heading, Link, Parsed};

/// LSP SymbolKind：标题按命名空间，与主流 Markdown 客户端一致。
const KIND_NAMESPACE: u8 = 3;

/// LSP DiagnosticSeverity：Error。
mod level {
    pub const ERROR: u8 = 1;
}

/// 创建 Markdown 后端的工厂函数。
pub fn backend() -> Box<dyn Backend> {
    Box::new(MarkdownBackend::default())
}

/// Markdown 语言后端。
#[derive(Default)]
pub struct MarkdownBackend {
    /// 单文档解析缓存
    cache: Option<ParseCache>,
    /// 工作区标题索引；initialize 时建立，变更时不清零（成本换简单）
    index: Option<WorkspaceIndex>,
}

/// 一份文档的解析缓存。
struct ParseCache {
    uri: String,
    version: i32,
    parsed: Parsed,
}

/// 工作区索引：文件名 → 文档内标题列表（slug + 原文）。
/// 只存标题与文件名，不存正文与 AST，一千篇笔记也就是几百 KB。
#[derive(Default)]
struct WorkspaceIndex {
    /// 文件路径 → 标题（slug → 原文）
    headings: Vec<(PathBuf, Vec<(String, String)>)>,
}

impl WorkspaceIndex {
    /// 扫描工作区内的 Markdown 文件并建立标题索引。
    fn build(root: &Path) -> Self {
        let mut headings = Vec::new();
        walk_markdown(root, &mut headings);
        Self { headings }
    }

    /// 找与缺失文件名最接近的候选（编辑距离 ≤ 3，同时比对带与不带扩展名的形式）。
    ///
    /// `exclude` 用于排除当前文档：从 `a.md` 里写断链时，把 `a.md` 本身列为候选没有意义。
    fn similar_files(&self, name: &str, exclude: Option<&Path>) -> Vec<String> {
        let mut candidates: Vec<(usize, String)> = self
            .headings
            .iter()
            .filter(|(path, _)| exclude != Some(path.as_path()))
            .filter_map(|(path, _)| {
                let file = path.file_name().and_then(|file| file.to_str())?;
                let stem = path.file_stem().and_then(|stem| stem.to_str())?;
                let distance = [close_distance(name, file), close_distance(name, stem)]
                    .into_iter()
                    .flatten()
                    .min()?;
                Some((distance, file.to_string()))
            })
            .collect();
        candidates.sort();
        candidates.truncate(3);
        candidates.into_iter().map(|(_, file)| file).collect()
    }

    /// 按文件名主干（无扩展名）找 wikilink 目标。
    fn find_by_stem(&self, stem: &str) -> Option<PathBuf> {
        self.headings
            .iter()
            .find(|(path, _)| path.file_stem().and_then(|name| name.to_str()) == Some(stem))
            .map(|(path, _)| path.clone())
    }

    /// 取某文件内与缺失锚点最接近的标题（编辑距离 ≤ 3）。
    fn similar_headings(&self, path: &Path, slug: &str) -> Vec<String> {
        let Some((_, titles)) = self.headings.iter().find(|(file, _)| file == path) else {
            return Vec::new();
        };
        let mut candidates: Vec<(usize, String)> = titles
            .iter()
            .filter_map(|(title_slug, text)| {
                close_distance(slug, title_slug).map(|distance| (distance, text.clone()))
            })
            .collect();
        candidates.sort();
        candidates.truncate(3);
        candidates.into_iter().map(|(_, text)| text).collect()
    }
}

/// 递归收集工作区里的 Markdown 文件标题。符号链接与权限错误一律跳过。
fn walk_markdown(dir: &Path, out: &mut Vec<(PathBuf, Vec<(String, String)>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };

        if kind.is_dir() {
            // 跳过版本控制与依赖目录，避免索引膨胀
            if path.file_name().and_then(|name| name.to_str()) != Some(".git") {
                walk_markdown(&path, out);
            }
            continue;
        }

        if !kind.is_file() || !is_markdown(&path) {
            continue;
        }

        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let headings = parser::parse(&text)
            .headings
            .iter()
            .map(|heading| {
                (
                    heading.slug.clone(),
                    heading.text_span.text(&text).to_string(),
                )
            })
            .collect();
        out.push((path, headings));
    }
}

/// 是否为 Markdown 文件（按扩展名）。
fn is_markdown(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("md" | "markdown")
    )
}

/// 候选门槛：编辑距离不超过 3，且不超过较长一方长度的一半（短词至少允许 1）。
///
/// 只看绝对距离会让短名字乱命中：`不存在` 与 `安装` 的距离恰好是 3，
/// 于是任意三个汉字的断链都会建议一个两字标题的文件。按长度比例收紧后，
/// 这类无关候选被排除，拼写错误（`安裝` / `安装`）仍然命中。
fn close_distance(name: &str, candidate: &str) -> Option<usize> {
    let distance = edit_distance(name, candidate);
    let longest = name.chars().count().max(candidate.chars().count());
    (distance <= 3 && distance <= (longest / 2).max(1)).then_some(distance)
}

/// Levenshtein 编辑距离，用于「猜你想去」（按字符，而非字节）。
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];

    for (i, left) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, right) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(left != right);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[b.len()]
}

impl MarkdownBackend {
    /// 确保当前文档已解析，必要时重做。
    fn ensure(&mut self, document: &Document) {
        if self
            .cache
            .as_ref()
            .is_some_and(|cache| cache.uri == document.uri() && cache.version == document.version())
        {
            return;
        }

        self.cache = Some(ParseCache {
            uri: document.uri().to_string(),
            version: document.version(),
            parsed: parser::parse(document.text()),
        });
    }

    /// 取覆盖指定偏移的链接（文本与目标都算）。
    fn link_at(&self, offset: u32) -> Option<&Link> {
        let cache = self.cache.as_ref()?;
        cache
            .parsed
            .links
            .iter()
            .find(|link| link.full_span.start <= offset && offset <= link.full_span.end)
    }

    /// 发布诊断：断链（文件不存在 / 锚点不存在）并给出「猜你想去」。
    fn publish(&mut self, document: &Document, out: &mut Vec<Outgoing>) {
        self.ensure(document);
        let cache = self.cache.as_ref().expect("刚写入缓存");
        let lines = LineIndex::new(document.text());
        let current = uri_path(document.uri());
        let mut items: Vec<Value> = Vec::new();

        for link in &cache.parsed.links {
            let target = resolve_target(link, &cache.parsed.definitions);
            let resolved = resolve_link(document, &target, &link.kind, self.index.as_ref());

            match resolved.filter(|item| item.path.exists()) {
                None => {
                    let display = match &link.kind {
                        parser::LinkKind::Wiki => format!("[[{}]]", target),
                        parser::LinkKind::Inline => target.to_string(),
                        parser::LinkKind::Reference(label) => format!("[{}]", label),
                    };
                    let mut message = format!("链接目标不存在：{}", display);
                    if let Some(index) = self.index.as_ref() {
                        let name = Path::new(target.as_ref())
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or(target.as_ref());
                        let candidates = index.similar_files(name, current.as_deref());
                        if !candidates.is_empty() {
                            message.push_str(&format!("，是否想找：{}", candidates.join("、")));
                        }
                    }
                    items.push(link_diagnostic(&lines, link, &message));
                }
                Some(Resolved {
                    path: _,
                    anchor: None,
                }) => {}
                Some(Resolved {
                    path,
                    anchor: Some(anchor),
                }) => {
                    let wanted = parser::slugify(&anchor);
                    let exists = headings_of(&path).iter().any(|item| item.slug == wanted);
                    if !exists {
                        let mut message = format!("锚点 `#{}` 不存在", anchor);
                        if let Some(index) = self.index.as_ref() {
                            let candidates = index.similar_headings(&path, &wanted);
                            if !candidates.is_empty() {
                                message.push_str(&format!("，是否想找：{}", candidates.join("、")));
                            }
                        }
                        items.push(link_diagnostic(&lines, link, &message));
                    }
                }
            }
        }

        out.push(Outgoing::notify(
            "textDocument/publishDiagnostics",
            json!({ "uri": document.uri(), "version": document.version(), "diagnostics": items }),
        ));
    }
}

impl Backend for MarkdownBackend {
    fn name(&self) -> &'static str {
        "markdown"
    }

    fn language_ids(&self) -> &'static [&'static str] {
        &["markdown"]
    }

    fn file_extensions(&self) -> &'static [&'static str] {
        &[".md", ".markdown"]
    }

    fn capabilities(&self) -> Value {
        json!({
            "documentSymbolProvider": true,
            "foldingRangeProvider": true,
            "definitionProvider": true,
            "completionProvider": { "triggerCharacters": ["[", "#"] },
        })
    }

    fn did_open(&mut self, document: &Document, out: &mut Vec<Outgoing>) {
        self.publish(document, out);
    }

    fn did_change(&mut self, document: &Document, out: &mut Vec<Outgoing>) {
        self.publish(document, out);
    }

    fn did_close(&mut self, uri: &str, _out: &mut Vec<Outgoing>) {
        if self.cache.as_ref().is_some_and(|cache| cache.uri == uri) {
            self.cache = None;
        }
    }

    fn request(
        &mut self,
        method: &str,
        params: Option<&RawValue>,
        documents: &DocumentStore,
        _out: &mut Vec<Outgoing>,
    ) -> Option<Result<Value, ResponseError>> {
        match method {
            "textDocument/documentSymbol" | "textDocument/foldingRange" => {
                let params: DocumentParams = serde_json::from_str(params?.get()).ok()?;
                let document = documents.get(&params.text_document.uri)?;
                self.ensure(document);
                let cache = self.cache.as_ref().expect("刚写入缓存");
                let lines = LineIndex::new(document.text());

                let result = match method {
                    "textDocument/documentSymbol" => {
                        document_symbols(document, &cache.parsed, &lines)
                    }
                    _ => folding_ranges(document, &cache.parsed),
                };
                Some(Ok(result))
            }
            "textDocument/definition" => {
                let params: PositionParams = serde_json::from_str(params?.get()).ok()?;
                let document = documents.get(&params.text_document.uri)?;
                self.ensure(document);
                let offset = document.offset_at(params.position) as u32;
                let cache = self.cache.as_ref().expect("刚写入缓存");
                let resolved = self.link_at(offset).and_then(|link| {
                    let target = resolve_target(link, &cache.parsed.definitions);
                    resolve_link(document, &target, &link.kind, self.index.as_ref())
                });
                let resolved = resolved.filter(|item| item.path.exists());

                Some(Ok(resolved
                    .and_then(|resolved| {
                        let uri = path_uri(&resolved.path)?;
                        let text = match documents.get(&uri) {
                            Some(open) => Cow::Owned(open.text().to_string()),
                            None => Cow::Owned(std::fs::read_to_string(&resolved.path).ok()?),
                        };
                        let Some(anchor) = resolved.anchor else {
                            return Some(location(&uri, &LineIndex::new(&text), Span::default()));
                        };
                        let parsed = parser::parse(&text);
                        let wanted = parser::slugify(&anchor);
                        let heading = parsed
                            .headings
                            .iter()
                            .find(|item| item.slug == anchor || item.slug == wanted)?;
                        Some(location(&uri, &LineIndex::new(&text), heading.text_span))
                    })
                    .unwrap_or(Value::Null)))
            }
            "textDocument/completion" => {
                let params: PositionParams = serde_json::from_str(params?.get()).ok()?;
                let document = documents.get(&params.text_document.uri)?;
                self.ensure(document);
                let offset = document.offset_at(params.position) as u32;
                let cache = self.cache.as_ref().expect("刚写入缓存");

                let items = completion(document, offset, &cache.parsed, self.index.as_ref());
                Some(Ok(json!({ "isIncomplete": false, "items": items })))
            }
            _ => Some(Err(ResponseError {
                code: lspd_core::message::error_code::METHOD_NOT_FOUND,
                message: format!("markdown 后端不支持 {method}"),
                data: None,
            })),
        }
    }

    fn did_initialize(&mut self, root_uri: &str, _out: &mut Vec<Outgoing>) {
        if let Some(path) = uri_path(root_uri) {
            self.index = Some(WorkspaceIndex::build(&path));
        }
    }

    fn shutdown(&mut self) {
        self.cache = None;
        self.index = None;
    }
}

/// 取链接的最终目标。wikilink 去掉 `|别名`；引用式查定义表，未定义则为空。
fn resolve_target<'a>(
    link: &'a Link,
    definitions: &'a [parser::LinkDefinition],
) -> std::borrow::Cow<'a, str> {
    match &link.kind {
        parser::LinkKind::Inline => Cow::Borrowed(link.target.as_str()),
        parser::LinkKind::Wiki => Cow::Owned(
            link.target
                .split('|')
                .next()
                .unwrap_or("")
                .trim()
                .to_string(),
        ),
        parser::LinkKind::Reference(label) => {
            match definitions.iter().find(|item| &item.label == label) {
                Some(definition) => Cow::Borrowed(&definition.target),
                None => Cow::Borrowed(""),
            }
        }
    }
}

/// 链接解析结果：目标文件与可选锚点。
struct Resolved {
    path: PathBuf,
    anchor: Option<String>,
}

/// 把链接目标解析成「文件 + 锚点」。
///
/// 行内链接按相对路径解析；`[[wikilink]]` 的目标若无扩展名，
/// 按文件名（去掉扩展名）在工作区索引里匹配。同文档跳转（无文件部分）指向当前文档。
fn resolve_link(
    document: &Document,
    target: &str,
    kind: &parser::LinkKind,
    index: Option<&WorkspaceIndex>,
) -> Option<Resolved> {
    if target.is_empty() {
        return None;
    }

    let (path_part, anchor) = match target.split_once('#') {
        Some((path, anchor)) => (path, Some(anchor.to_string())),
        None => (target, None),
    };

    if path_part.is_empty() {
        let path = uri_path(document.uri())?;
        return Some(Resolved { path, anchor });
    }

    // wikilink 无扩展名时按文件名主干匹配工作区索引
    if matches!(kind, parser::LinkKind::Wiki)
        && Path::new(path_part).extension().is_none()
        && let Some(index) = index
        && let Some(path) = index.find_by_stem(path_part)
    {
        return Some(Resolved { path, anchor });
    }

    let decoded = percent_decode(path_part)?;
    let path = Path::new(&decoded);
    let path = if path.is_absolute() {
        normalize(path.to_path_buf())
    } else {
        let base = uri_path(document.uri())?;
        normalize(base.parent()?.join(path))
    };
    Some(Resolved { path, anchor })
}

/// 读一份文件的标题（用于锚点校验），读不到时为空。
fn headings_of(path: &Path) -> Vec<Heading> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| parser::parse(&text).headings)
        .unwrap_or_default()
}

/// 生成一条链接诊断。
fn link_diagnostic(lines: &LineIndex<'_>, link: &Link, message: &str) -> Value {
    json!({
        "range": range(lines, link.target_span),
        "severity": level::ERROR,
        "source": "lspd",
        "message": message,
    })
}

/// LSP CompletionItemKind：文件按 File，标题按 Class。
mod kind {
    pub const FILE: u8 = 17;
    pub const CLASS: u8 = 7;
}

/// 生成补全项：`[[` 后给工作区内的文件名与标题，`#` 后给当前文档的标题。
fn completion(
    document: &Document,
    offset: u32,
    parsed: &Parsed,
    index: Option<&WorkspaceIndex>,
) -> Value {
    let source = document.text();
    // 取光标前最后一个 `[[` 之后的内容，并在 `]]` 处截断
    let prefix = source[..offset as usize].rsplit("[[").next().unwrap_or("");
    let prefix = prefix.split("]]").next().unwrap_or(prefix);
    let (file_part, heading_part) = match prefix.split_once('#') {
        Some((file, heading)) => (file, Some(heading)),
        None => (prefix, None),
    };

    let mut items = Vec::new();

    if let Some(index) = index {
        for (path, headings) in &index.headings {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if name.contains(file_part) {
                items.push(json!({
                    "label": name,
                    "kind": kind::FILE,
                    "insertText": name,
                    "detail": path.to_string_lossy(),
                }));
            }
            if let Some(wanted) = heading_part {
                for (slug, text) in headings {
                    if text.contains(wanted) {
                        items.push(json!({
                            "label": format!("{}#{}", name, text),
                            "kind": kind::CLASS,
                            "insertText": format!("{}#{}", name, slug),
                            "detail": path.to_string_lossy(),
                        }));
                    }
                }
            }
        }
    }

    // `#` 后的同文档标题补全
    if heading_part.is_some() || file_part.is_empty() {
        for heading in &parsed.headings {
            if heading.slug.contains(heading_part.unwrap_or("")) {
                items.push(json!({
                    "label": heading.text_span.text(source),
                    "kind": kind::CLASS,
                    "insertText": format!("#{}", heading.slug),
                    "detail": "当前文档",
                }));
            }
        }
    }

    Value::Array(items)
}

/// 生成文档大纲：标题按级别嵌套。
fn document_symbols(document: &Document, parsed: &Parsed, lines: &LineIndex<'_>) -> Value {
    let mut symbols = Vec::new();
    let mut cursor = 0usize;

    while cursor < parsed.headings.len() {
        let position = cursor;
        symbols.push(symbol(document, parsed, position, &mut cursor, lines));
    }

    Value::Array(symbols)
}

/// 构造一个标题符号，游标前进到其后第一个同级或更高级标题。
fn symbol(
    document: &Document,
    parsed: &Parsed,
    position: usize,
    cursor: &mut usize,
    lines: &LineIndex<'_>,
) -> Value {
    let current = &parsed.headings[position];
    let end_line = trim_blank(
        document,
        current.line,
        section_end(&parsed.headings, position, parsed.line_count),
    );

    *cursor = position + 1;
    let mut children = Vec::new();
    while *cursor < parsed.headings.len() && parsed.headings[*cursor].level > current.level {
        let child = *cursor;
        children.push(symbol(document, parsed, child, cursor, lines));
    }

    json!({
        "name": current.text_span.text(document.text()),
        "kind": KIND_NAMESPACE,
        "range": range(lines, line_span(document, lines, current.line, end_line)),
        "selectionRange": range(lines, current.text_span),
        "children": children,
    })
}

/// 生成折叠区间：标题分区按区域、代码围栏按注释。
fn folding_ranges(document: &Document, parsed: &Parsed) -> Value {
    let mut ranges = Vec::new();

    for (position, heading) in parsed.headings.iter().enumerate() {
        let end = trim_blank(
            document,
            heading.line,
            section_end(&parsed.headings, position, parsed.line_count),
        );
        if end > heading.line {
            ranges.push(json!({ "startLine": heading.line, "endLine": end, "kind": "region" }));
        }
    }

    for fence in &parsed.fences {
        if fence.end_line > fence.start_line {
            ranges.push(json!({ "startLine": fence.start_line, "endLine": fence.end_line, "kind": "comment" }));
        }
    }

    Value::Array(ranges)
}

/// 标题分区的结束行：下一个同级或更高级标题的前一行。
fn section_end(headings: &[Heading], position: usize, line_count: u32) -> u32 {
    let level = headings[position].level;
    headings[position + 1..]
        .iter()
        .find(|item| item.level <= level)
        .map_or_else(
            || line_count.saturating_sub(1),
            |item| item.line.saturating_sub(1),
        )
}

/// 去掉分区末尾的空行，让折叠与大纲的末行停在有内容的行上。
fn trim_blank(document: &Document, start: u32, end: u32) -> u32 {
    let mut end = end;
    while end > start && document.line_text(end).trim().is_empty() {
        end -= 1;
    }
    end
}

/// 若干整行的字节区间。
fn line_span(document: &Document, lines: &LineIndex<'_>, start_line: u32, end_line: u32) -> Span {
    let start = lines.line_start(start_line);
    let end = lines
        .line_start(end_line + 1)
        .min(document.text().len() as u32);
    Span::new(start as usize, end as usize)
}

/// 字节区间转为 LSP `Range`。
fn range(lines: &LineIndex<'_>, span: Span) -> Value {
    let (start_line, start_char) = lines.position(span.start);
    let (end_line, end_char) = lines.position(span.end);
    json!({
        "start": { "line": start_line, "character": start_char },
        "end": { "line": end_line, "character": end_char },
    })
}

/// 生成 LSP `Location`。
fn location(uri: &str, lines: &LineIndex<'_>, span: Span) -> Value {
    json!({ "uri": uri, "range": range(lines, span) })
}

/// 归一路径：去掉 `.` 并把 `..` 折叠到上级，避免 URI 中出现 `./`。
/// 不用 `canonicalize`，它会把符号链接解开（macOS 上 `/tmp` 会变成 `/private/tmp`），
/// 从而与客户端给出的 URI 对不上。
fn normalize(path: PathBuf) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                _ => out.push(".."),
            },
            other => out.push(other),
        }
    }

    out
}

/// `file://` URI 转换为本地路径。
fn uri_path(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?;
    percent_decode(raw).map(PathBuf::from)
}

/// 本地路径转换为 `file://` URI。只转义 URI 路径中必须转义的字符。
fn path_uri(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    let mut uri = String::with_capacity(text.len() + 7);
    uri.push_str("file://");

    for ch in text.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' | ':' => uri.push(ch),
            _ => {
                let mut buffer = [0u8; 4];
                for byte in ch.encode_utf8(&mut buffer).bytes() {
                    uri.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }

    Some(uri)
}

/// 解码百分号转义；非法转义原样保留。
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 3 <= bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                decoded.push(byte);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }

    String::from_utf8(decoded).ok()
}

/// 只带文档标识的参数。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentParams {
    text_document: TextIdentifier,
}

/// 带位置的请求参数。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PositionParams {
    text_document: TextIdentifier,
    position: Position,
}

/// 文档标识。
#[derive(Deserialize)]
struct TextIdentifier {
    uri: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一份临时文档与工作区，返回后端与索引。
    fn setup(tag: &str, source: &str) -> (MarkdownBackend, WorkspaceIndex, Document) {
        let root = std::env::temp_dir().join(format!("lspd-md-test-{tag}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(
            root.join("docs/安装.md"),
            "# 安装手册\n\n## 快速开始\n\n跑起来。\n",
        )
        .unwrap();
        let document = Document::new(lspd_core::docs::TextDocumentItem {
            uri: "file:///tmp/lspd-md-test/a.md".to_string(),
            language_id: "markdown".to_string(),
            version: 1,
            text: source.to_string(),
        });
        let index = WorkspaceIndex::build(&root);
        (MarkdownBackend::default(), index, document)
    }

    #[test]
    fn normalize_collapses_dots_without_canonicalize() {
        assert_eq!(
            normalize(PathBuf::from("/a/./b/../c")),
            PathBuf::from("/a/c")
        );
        assert_eq!(normalize(PathBuf::from("../a")), PathBuf::from("../a"));
    }

    #[test]
    fn edit_distance_counts_edits() {
        assert_eq!(edit_distance("安装", "安装"), 0);
        assert_eq!(edit_distance("安装", "安裝"), 1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
    }

    /// 候选门槛按长度比例收紧：无关短词不命中，拼写错误仍命中。
    #[test]
    fn close_distance_scales_with_length() {
        assert_eq!(close_distance("安裝", "安装"), Some(1));
        assert_eq!(close_distance("不存在", "安装"), None);
        assert_eq!(close_distance("tsconfg", "tsconfig"), Some(1));
        assert_eq!(close_distance("完全不同", "别的文件.md"), None);
    }

    #[test]
    fn wiki_link_resolves_by_stem() {
        let (_, index, document) = setup("wiki_stem", "见 [[安装]]。\n");
        let parsed = parser::parse(document.text());
        let link = &parsed.links[0];
        let resolved = resolve_link(
            &document,
            &resolve_target(link, &[]),
            &link.kind,
            Some(&index),
        );
        let resolved = resolved.expect("按文件名主干命中");
        assert_eq!(resolved.path.file_name().unwrap(), "安装.md");
        assert_eq!(resolved.anchor, None);
    }

    #[test]
    fn wiki_link_with_anchor_resolves() {
        let (_, index, document) = setup("wiki_anchor", "见 [[安装#快速开始]]。\n");
        let parsed = parser::parse(document.text());
        let link = &parsed.links[0];
        let resolved = resolve_link(
            &document,
            &resolve_target(link, &[]),
            &link.kind,
            Some(&index),
        );
        let resolved = resolved.expect("wikilink 带锚点可解析");
        assert_eq!(resolved.anchor.as_deref(), Some("快速开始"));
    }

    #[test]
    fn missing_wiki_link_reports_candidate() {
        let (_, index, document) = setup("wiki_missing", "见 [[安裝]]。\n");
        let parsed = parser::parse(document.text());
        let link = &parsed.links[0];
        // resolve_link 只负责解析，存在性由诊断与 definition 检查
        let resolved = resolve_link(
            &document,
            &resolve_target(link, &[]),
            &link.kind,
            Some(&index),
        );
        let resolved = resolved.expect("按相对路径解析");
        assert!(!resolved.path.exists());
        let candidates = index.similar_files("安裝", None);
        assert_eq!(candidates, ["安装.md"]);
    }

    /// 候选不应把当前文档自己列出来。
    #[test]
    fn similar_files_skips_current_document() {
        let root = std::env::temp_dir().join("lspd-md-test-similar-self");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.md"), "# 标题\n").unwrap();

        let index = WorkspaceIndex::build(&root);
        let current = root.join("a.md");

        assert_eq!(index.similar_files("a.md", None), ["a.md"]);
        assert!(index.similar_files("a.md", Some(&current)).is_empty());
        assert!(index.similar_files("不存在", None).is_empty());
    }

    #[test]
    fn inline_link_resolves_relative_path() {
        let (_, index, document) = setup("inline", "见 [说明](./docs/安装.md#快速开始)。\n");
        let parsed = parser::parse(document.text());
        let link = &parsed.links[0];
        let resolved = resolve_link(
            &document,
            &resolve_target(link, &[]),
            &link.kind,
            Some(&index),
        );
        let resolved = resolved.expect("相对路径可解析");
        assert_eq!(resolved.anchor.as_deref(), Some("快速开始"));
        assert!(resolved.path.ends_with("docs/安装.md"));
    }

    #[test]
    fn completion_lists_files_and_headings() {
        let (_, index, document) = setup("completion", "见 [[安装#\n");
        let parsed = parser::parse(document.text());
        let items = completion(&document, 13, &parsed, Some(&index));
        let labels: Vec<String> = items
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["label"].as_str().unwrap().to_string())
            .collect();
        assert!(labels.contains(&"安装.md".to_string()));
        assert!(labels.contains(&"安装.md#快速开始".to_string()));
        assert!(labels.contains(&"安装.md#安装手册".to_string()));
    }
}
