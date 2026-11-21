//! Markdown 后端：大纲、折叠、跳转、反查引用、重命名、悬停、补全与诊断。
//!
//! 缓存只保留当前文档的解析结果，按 URI 与版本失效，关闭文档即释放。
//! 跨文件信息来自工作区索引（`index`）：保存、打开、关闭与文件监听都会增量更新它。

use std::path::{Path, PathBuf};

use lspd_core::docs::{Document, DocumentStore, Position, Range};
use lspd_core::message::Id;
use lspd_core::text::{LineIndex, Span};
use lspd_core::{Backend, Outgoing, ResponseError};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

use crate::index::{self, WorkspaceIndex};
use crate::parser::{self, Heading, Link, Parsed};

/// LSP SymbolKind：标题按命名空间，与主流 Markdown 客户端一致。
const KIND_NAMESPACE: u8 = 3;

/// LSP DiagnosticSeverity：Error。
mod level {
    pub const ERROR: u8 = 1;
}

/// LSP CompletionItemKind：文件按 File，标题按 Class。
mod item_kind {
    pub const FILE: u8 = 17;
    pub const CLASS: u8 = 7;
}

/// 动态注册文件监听时使用的固定请求标识。
const WATCH_REGISTRATION: &str = "lspd-md-watch";

/// 创建 Markdown 后端的工厂函数。
pub fn backend() -> Box<dyn Backend> {
    Box::new(MarkdownBackend::default())
}

/// Markdown 语言后端。
#[derive(Default)]
pub struct MarkdownBackend {
    /// 单文档解析缓存
    cache: Option<ParseCache>,
    /// 工作区标题与链接索引
    index: Option<WorkspaceIndex>,
    /// 已打开文档的 URI，用于避免文件监听覆盖内存中的最新内容
    open: Vec<String>,
}

/// 一份文档的解析缓存。
struct ParseCache {
    uri: String,
    version: i32,
    parsed: Parsed,
}

/// 断链问题的描述。
struct BrokenLink {
    /// 面向用户的说明（不含候选）
    message: String,
    /// 候选：显示文本与替换文本
    candidates: Vec<Candidate>,
    /// 替换区间：文件候选替换整个目标，锚点候选只替换锚点
    replace: Span,
}

/// 一条候选。
struct Candidate {
    /// 展示用文本
    label: String,
    /// 写回文档的文本
    replacement: String,
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

    /// 把当前文档的最新内容同步进工作区索引。
    fn sync_index(&mut self, document: &Document) {
        let Some(path) = index::uri_path(document.uri()) else {
            return;
        };
        let Some(index) = self.index.as_mut() else {
            return;
        };

        if path.starts_with(index.root()) {
            index.update(&path, document.text());
        }
    }

    /// 取覆盖指定偏移的链接（文本与目标都算）。
    fn link_at<'a>(&self, parsed: &'a Parsed, offset: u32) -> Option<&'a Link> {
        parsed
            .links
            .iter()
            .find(|link| link.full_span.start <= offset && offset <= link.full_span.end)
    }

    /// 某个文件的标题锚点列表：优先索引，未索引时读盘。
    fn heading_slugs(&self, path: &Path) -> Vec<String> {
        if let Some(index) = self.index.as_ref()
            && let Some(file) = index.file(path)
        {
            return file.headings.iter().map(|item| item.slug.clone()).collect();
        }

        headings_of(path)
            .iter()
            .map(|heading| heading.slug.clone())
            .collect()
    }

    /// 描述一条链接的问题；正常时返回 `None`。
    fn link_problem(&self, document: &Document, link: &Link, parsed: &Parsed) -> Option<BrokenLink> {
        let resolved = resolve_document_link(document, link, parsed, self.index.as_ref())?;
        let display = display_target(link, parsed);
        let current = index::uri_path(document.uri());

        if !resolved.path.exists() {
            let display_path = display_target_path(link, parsed);
            let name = Path::new(&display_path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let candidates = self
                .index
                .as_ref()
                .map(|index| index.similar_files(name, current.as_deref()))
                .unwrap_or_default()
                .into_iter()
                .map(|file| Candidate {
                    label: file.clone(),
                    replacement: file,
                })
                .collect();

            return Some(BrokenLink {
                message: format!("链接目标不存在：{display}"),
                candidates,
                replace: link.target_span,
            });
        }

        let anchor = resolved.anchor?;
        let wanted = parser::slugify(&anchor);
        if self.heading_slugs(&resolved.path).iter().any(|slug| *slug == wanted) {
            return None;
        }

        let candidates = self
            .index
            .as_ref()
            .map(|index| index.similar_headings(&resolved.path, &wanted))
            .unwrap_or_default()
            .into_iter()
            .map(|text| Candidate {
                replacement: parser::slugify(&text),
                label: text,
            })
            .collect();

        Some(BrokenLink {
            message: format!("锚点 `#{anchor}` 不存在"),
            candidates,
            replace: parser::anchor_span(link, &parsed.definitions).unwrap_or(link.target_span),
        })
    }

    /// 发布诊断：断链（文件不存在 / 锚点不存在）并给出「猜你想去」。
    fn publish(&mut self, document: &Document, out: &mut Vec<Outgoing>) {
        self.ensure(document);
        let cache = self.cache.as_ref().expect("刚写入缓存");
        let lines = LineIndex::new(document.text());
        let mut items: Vec<Value> = Vec::new();

        for link in &cache.parsed.links {
            let Some(problem) = self.link_problem(document, link, &cache.parsed) else {
                continue;
            };
            let message = with_candidates(problem.message, &problem.candidates);
            items.push(link_diagnostic(&lines, link, &message));
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
            "referencesProvider": true,
            "renameProvider": true,
            "hoverProvider": true,
            "documentLinkProvider": { "resolveProvider": false },
            "completionProvider": { "triggerCharacters": ["[", "#", "(", "/"] },
            "codeActionProvider": { "codeActionKinds": ["quickfix"] },
            "selectionRangeProvider": true,
            "workspaceSymbolProvider": true,
        })
    }

    fn did_initialize(&mut self, root_uri: &str, _out: &mut Vec<Outgoing>) {
        let Some(path) = index::uri_path(root_uri) else {
            return;
        };
        self.index = Some(WorkspaceIndex::build(&path));
    }

    fn initialized(&mut self, out: &mut Vec<Outgoing>) {
        if self.index.is_none() {
            return;
        }

        // 动态注册文件监听：新建、删除、外部修改都要反映到索引里。
        // 握手完成后再发，避免在 initialize 应答之前向客户端发请求。
        // 客户端不支持动态注册时会回一个错误应答，核心层静默丢弃即可。
        out.push(Outgoing::request(
            Id::String(WATCH_REGISTRATION.to_string()),
            "client/registerCapability",
            json!({
                "registrations": [{
                    "id": WATCH_REGISTRATION,
                    "method": "workspace/didChangeWatchedFiles",
                    "registerOptions": {
                        "watchers": [{ "globPattern": "**/*.{md,markdown}" }]
                    }
                }]
            }),
        ));
    }

    fn did_open(&mut self, document: &Document, out: &mut Vec<Outgoing>) {
        if !self.open.iter().any(|uri| uri == document.uri()) {
            self.open.push(document.uri().to_string());
        }
        self.sync_index(document);
        self.publish(document, out);
    }

    fn did_change(&mut self, document: &Document, out: &mut Vec<Outgoing>) {
        self.sync_index(document);
        self.publish(document, out);
    }

    fn did_save(&mut self, document: &Document, _out: &mut Vec<Outgoing>) {
        self.sync_index(document);
    }

    fn did_close(&mut self, uri: &str, out: &mut Vec<Outgoing>) {
        self.open.retain(|item| item != uri);

        // 关闭后以磁盘内容为准，撤掉未保存时的内存版本
        if let Some(path) = index::uri_path(uri)
            && let Some(index) = self.index.as_mut()
        {
            index.reindex(&path);
        }

        if self.cache.as_ref().is_some_and(|cache| cache.uri == uri) {
            self.cache = None;
        }

        out.push(Outgoing::notify(
            "textDocument/publishDiagnostics",
            json!({ "uri": uri, "diagnostics": [] }),
        ));
    }

    fn did_change_watched_files(&mut self, params: Option<&RawValue>, _out: &mut Vec<Outgoing>) {
        let Some(params) = params.and_then(|raw| serde_json::from_str::<WatchedFilesParams>(raw.get()).ok()) else {
            return;
        };
        let Some(index) = self.index.as_mut() else {
            return;
        };

        for change in params.changes {
            let Some(path) = index::uri_path(&change.uri) else {
                continue;
            };
            if !index::is_markdown(&path) || self.open.iter().any(|uri| *uri == change.uri) {
                continue;
            }

            match change.kind {
                FILE_DELETED => index.remove(&path),
                _ => index.reindex(&path),
            }
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
                let document = markdown_document(documents, &params.text_document.uri)?;
                self.ensure(document);
                let cache = self.cache.as_ref().expect("刚写入缓存");
                let lines = LineIndex::new(document.text());

                let result = match method {
                    "textDocument/documentSymbol" => document_symbols(document, &cache.parsed, &lines),
                    _ => folding_ranges(document, &cache.parsed),
                };
                Some(Ok(result))
            }
            "textDocument/definition" => {
                let params: PositionParams = serde_json::from_str(params?.get()).ok()?;
                let document = markdown_document(documents, &params.text_document.uri)?;
                self.ensure(document);
                let offset = document.offset_at(params.position) as u32;
                let cache = self.cache.as_ref().expect("刚写入缓存");

                let resolved = self
                    .link_at(&cache.parsed, offset)
                    .and_then(|link| resolve_document_link(document, link, &cache.parsed, self.index.as_ref()))
                    .filter(|item| item.path.exists());

                Some(Ok(resolved.and_then(|item| definition_location(documents, item)).unwrap_or(Value::Null)))
            }
            "textDocument/references" => {
                let params: PositionParams = serde_json::from_str(params?.get()).ok()?;
                let document = markdown_document(documents, &params.text_document.uri)?;
                self.ensure(document);
                let offset = document.offset_at(params.position) as u32;
                let cache = self.cache.as_ref().expect("刚写入缓存");
                let Some(index) = self.index.as_ref() else {
                    return Some(Ok(Value::Array(Vec::new())));
                };
                let base = index::uri_path(document.uri())?;

                let found = match self.link_at(&cache.parsed, offset) {
                    Some(link) => resolve_document_link(document, link, &cache.parsed, Some(index))
                        .map(|resolved| index.references_to(&resolved.path, resolved.anchor.as_deref()))
                        .unwrap_or_default(),
                    None => {
                        let heading = heading_on_line(&cache.parsed, document.position_at(offset as usize).line)?;
                        index.references_to(&base, Some(&heading.slug))
                    }
                };

                Some(Ok(locations(documents, found)))
            }
            "textDocument/rename" => {
                let params: RenameParams = serde_json::from_str(params?.get()).ok()?;
                let document = markdown_document(documents, &params.text_document.uri)?;
                self.ensure(document);
                let offset = document.offset_at(params.position) as u32;
                let cache = self.cache.as_ref().expect("刚写入缓存");
                let lines = LineIndex::new(document.text());
                let Some(heading) = heading_on_line(&cache.parsed, document.position_at(offset as usize).line) else {
                    return Some(Ok(Value::Null));
                };

                let Some(index) = self.index.as_ref() else {
                    return Some(Ok(Value::Null));
                };
                let base = index::uri_path(document.uri())?;
                let mut changes: Map<String, Value> = Map::new();

                push_edit(&mut changes, document.uri(), range(&lines, heading.text_span), &params.new_name);

                for (path, span) in index.references_to(&base, Some(&heading.slug)) {
                    let Some(uri) = index::path_uri(&path) else {
                        continue;
                    };
                    let Some(source) = file_text(documents, &path) else {
                        continue;
                    };
                    // 只替换锚点文本：链接里的锚点对应标题 slug，随新标题一起变化
                    push_edit(
                        &mut changes,
                        &uri,
                        range(&LineIndex::new(&source), span),
                        &parser::slugify(&params.new_name),
                    );
                }

                Some(Ok(json!({ "changes": Value::Object(changes) })))
            }
            "textDocument/hover" => {
                let params: PositionParams = serde_json::from_str(params?.get()).ok()?;
                let document = markdown_document(documents, &params.text_document.uri)?;
                self.ensure(document);
                let offset = document.offset_at(params.position) as u32;
                let cache = self.cache.as_ref().expect("刚写入缓存");
                let lines = LineIndex::new(document.text());

                Some(Ok(hover(&cache.parsed, document, offset, self.index.as_ref(), &lines).unwrap_or(Value::Null)))
            }
            "textDocument/documentLink" => {
                let params: DocumentParams = serde_json::from_str(params?.get()).ok()?;
                let document = markdown_document(documents, &params.text_document.uri)?;
                self.ensure(document);
                let cache = self.cache.as_ref().expect("刚写入缓存");
                let lines = LineIndex::new(document.text());

                Some(Ok(document_links(document, &cache.parsed, self.index.as_ref(), &lines)))
            }
            "textDocument/codeAction" => {
                let params: CodeActionParams = serde_json::from_str(params?.get()).ok()?;
                let document = markdown_document(documents, &params.text_document.uri)?;
                self.ensure(document);
                let cache = self.cache.as_ref().expect("刚写入缓存");
                let lines = LineIndex::new(document.text());
                let start = document.offset_at(params.range.start) as u32;
                let end = document.offset_at(params.range.end) as u32;

                Some(Ok(code_actions(document, &cache.parsed, self, &lines, start, end)))
            }
            "textDocument/selectionRange" => {
                let params: SelectionRangeParams = serde_json::from_str(params?.get()).ok()?;
                let document = markdown_document(documents, &params.text_document.uri)?;
                self.ensure(document);
                let cache = self.cache.as_ref().expect("刚写入缓存");
                let lines = LineIndex::new(document.text());

                let items: Vec<Value> = params
                    .positions
                    .iter()
                    .map(|position| {
                        selection_chain(document, &cache.parsed, &lines, document.offset_at(*position) as u32)
                    })
                    .collect();
                Some(Ok(Value::Array(items)))
            }
            "textDocument/completion" => {
                let params: PositionParams = serde_json::from_str(params?.get()).ok()?;
                let document = markdown_document(documents, &params.text_document.uri)?;
                self.ensure(document);
                let cache = self.cache.as_ref().expect("刚写入缓存");
                let offset = document.offset_at(params.position) as u32;
                let lines = LineIndex::new(document.text());

                let items = completion(document, offset, &cache.parsed, self.index.as_ref(), &lines);
                Some(Ok(json!({ "isIncomplete": false, "items": items })))
            }
            "workspace/symbol" => {
                let params: WorkspaceSymbolParams = serde_json::from_str(params?.get()).ok()?;
                Some(Ok(workspace_symbols(self.index.as_ref(), &params.query)))
            }
            _ => None,
        }
    }

    fn shutdown(&mut self) {
        self.cache = None;
        self.index = None;
        self.open.clear();
    }
}

/// 只接受 Markdown 文档，避免把其他语言的请求误当 Markdown 处理。
fn markdown_document<'a>(documents: &'a DocumentStore, uri: &str) -> Option<&'a Document> {
    let document = documents.get(uri)?;
    let is_markdown = document.language_id() == "markdown"
        || index::uri_path(uri).is_some_and(|path| index::is_markdown(&path));
    is_markdown.then_some(document)
}

/// 链接的展示形式：保留用户写法。
fn display_target(link: &Link, parsed: &Parsed) -> String {
    match &link.kind {
        parser::LinkKind::Wiki => format!("[[{}]]", link.target),
        parser::LinkKind::Inline => index::effective_target(link, &parsed.definitions).to_string(),
        parser::LinkKind::Reference(label) => format!("[{label}]"),
    }
}

/// 链接目标里可用于比较文件名的部分。
fn display_target_path(link: &Link, parsed: &Parsed) -> String {
    index::effective_target(link, &parsed.definitions).to_string()
}

/// 在消息末尾追加候选。
fn with_candidates(message: String, candidates: &[Candidate]) -> String {
    if candidates.is_empty() {
        return message;
    }

    let labels: Vec<&str> = candidates.iter().map(|item| item.label.as_str()).collect();
    format!("{message}，是否想找：{}", labels.join("、"))
}

/// 解析文档内某条链接的目标。
fn resolve_document_link(
    document: &Document,
    link: &Link,
    parsed: &Parsed,
    index: Option<&WorkspaceIndex>,
) -> Option<index::Resolved> {
    let base = index::uri_path(document.uri())?;
    let target = index::effective_target(link, &parsed.definitions);
    index::resolve_target(&base, &target, &link.kind, index)
}

/// 跳转结果：目标文件里的标题或文件开头。
fn definition_location(documents: &DocumentStore, resolved: index::Resolved) -> Option<Value> {
    let uri = index::path_uri(&resolved.path)?;
    let text = file_text(documents, &resolved.path)?;
    let lines = LineIndex::new(&text);
    let Some(anchor) = resolved.anchor else {
        return Some(location(&uri, &lines, Span::default()));
    };

    let wanted = parser::slugify(&anchor);
    let heading = parser::parse(&text)
        .headings
        .into_iter()
        .find(|item| item.slug == anchor || item.slug == wanted)?;
    Some(location(&uri, &lines, heading.text_span))
}

/// 读一份文件的正文：已打开取内存版本，否则读盘。
fn file_text(documents: &DocumentStore, path: &Path) -> Option<String> {
    let uri = index::path_uri(path)?;
    match documents.get(&uri) {
        Some(document) => Some(document.text().to_string()),
        None => std::fs::read_to_string(path).ok(),
    }
}

/// 读一份文件的标题（用于锚点校验），读不到时为空。
fn headings_of(path: &Path) -> Vec<Heading> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| parser::parse(&text).headings)
        .unwrap_or_default()
}

/// 若干「文件 + 区间」转成 LSP `Location` 数组。
fn locations(documents: &DocumentStore, found: Vec<(PathBuf, Span)>) -> Value {
    let items: Vec<Value> = found
        .into_iter()
        .filter_map(|(path, span)| {
            let uri = index::path_uri(&path)?;
            let text = file_text(documents, &path)?;
            Some(location(&uri, &LineIndex::new(&text), span))
        })
        .collect();
    Value::Array(items)
}

/// 取光标所在行的标题（标题行任意位置都算）。
fn heading_on_line(parsed: &Parsed, line: u32) -> Option<&Heading> {
    parsed.headings.iter().find(|heading| heading.line == line)
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

/// 生成补全项。
///
/// 三种上下文：
/// - `[[` 之后：工作区文件名与标题；
/// - `](` 之后：行内链接目标，按相对路径给文件，`#` 之后给锚点；
/// - 其他位置：不给补全。
fn completion(
    document: &Document,
    offset: u32,
    parsed: &Parsed,
    index: Option<&WorkspaceIndex>,
    lines: &LineIndex<'_>,
) -> Value {
    let source = document.text();
    let before = &source[..offset as usize];

    if let Some(items) = inline_link_items(document, before, parsed, index, lines) {
        return Value::Array(items);
    }

    // 只有光标确实处在 `[[...` 之内才给 wikilink 补全。
    // 否则输入 `#`（写标题）或 `[`（写普通文字）都会弹出整份工作区列表。
    let Some(open) = before.rfind("[[") else {
        return Value::Array(Vec::new());
    };
    let prefix = &before[open + 2..];
    if prefix.contains("]]") {
        return Value::Array(Vec::new());
    }

    let (file_part, heading_part) = match prefix.split_once('#') {
        Some((file, heading)) => (file, Some(heading)),
        None => (prefix, None),
    };

    let mut items = Vec::new();

    if let Some(index) = index {
        for file in index.files() {
            let name = file.path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
            if name.contains(file_part) {
                items.push(json!({
                    "label": name,
                    "kind": item_kind::FILE,
                    "insertText": name,
                    "detail": file.path.to_string_lossy(),
                }));
            }
            if let Some(wanted) = heading_part {
                for heading in &file.headings {
                    if heading.text.contains(wanted) {
                        items.push(json!({
                            "label": format!("{}#{}", name, heading.text),
                            "kind": item_kind::CLASS,
                            "insertText": format!("{}#{}", name, heading.slug),
                            "detail": file.path.to_string_lossy(),
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
                    "kind": item_kind::CLASS,
                    "insertText": format!("#{}", heading.slug),
                    "detail": "当前文档",
                }));
            }
        }
    }

    Value::Array(items)
}

/// 行内链接目标补全：`](` 之后且尚未闭合。
fn inline_link_items(
    document: &Document,
    before: &str,
    parsed: &Parsed,
    index: Option<&WorkspaceIndex>,
    lines: &LineIndex<'_>,
) -> Option<Vec<Value>> {
    let open = before.rfind("](")?;
    let typed = &before[open + 2..];
    if typed.contains(')') || typed.contains('\n') {
        return None;
    }

    let typed_start = (open + 2) as u32;
    let edit_range = range(lines, Span::new(typed_start as usize, typed_start as usize + typed.len()));
    let mut items = Vec::new();

    // `#锚点`：当前文档标题
    if let Some(anchor) = typed.strip_prefix('#') {
        for heading in &parsed.headings {
            let text = heading.text_span.text(document.text());
            if text.contains(anchor) || heading.slug.contains(anchor) {
                items.push(json!({
                    "label": text,
                    "kind": item_kind::CLASS,
                    "textEdit": { "range": edit_range, "newText": format!("#{}", heading.slug) },
                    "detail": "当前文档",
                }));
            }
        }
        return Some(items);
    }

    let Some(index) = index else {
        return Some(items);
    };
    let current = index::uri_path(document.uri());

    // `文件#锚点`：给该文件的标题
    if let Some((file_part, anchor)) = typed.split_once('#') {
        let Some(path) = resolve_relative(index.root(), &index::uri_path(document.uri())?, file_part) else {
            return Some(items);
        };
        for heading in index.headings_of(&path) {
            if heading.text.contains(anchor) || heading.slug.contains(anchor) {
                items.push(json!({
                    "label": heading.text,
                    "kind": item_kind::CLASS,
                    "textEdit": { "range": edit_range, "newText": format!("{file_part}#{}", heading.slug) },
                }));
            }
        }
        return Some(items);
    }

    // 文件路径：按相对当前目录的形式给出
    for file in index.files() {
        if Some(&file.path) == current.as_ref() {
            continue;
        }

        let relative = relative_path(index.root(), &file.path);
        let name = file.path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        // 用户可能已经输入 `./` 或 `../`，过滤时忽略这段前缀
        let needle = typed.trim_start_matches("./");
        if !relative.contains(needle) && !name.contains(needle) {
            continue;
        }

        items.push(json!({
            "label": relative,
            "kind": item_kind::FILE,
            "textEdit": { "range": edit_range, "newText": relative },
            "detail": file.path.to_string_lossy(),
        }));
    }

    Some(items)
}

/// 工作区根到文件的相对路径，始终以 `./` 或 `../` 或子目录形式给出。
fn relative_path(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let text = relative.to_string_lossy().replace('\\', "/");

    // 指向工作区外时保留 `../`；工作区内统一加 `./`，与常见写法一致
    match text.starts_with("..") {
        true => text,
        false => format!("./{text}"),
    }
}

/// 把相对路径解析成绝对路径。
fn resolve_relative(root: &Path, base: &Path, relative: &str) -> Option<PathBuf> {
    let path = Path::new(relative);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.parent()?.join(path)
    };
    let normalized = index::normalize(joined);
    normalized.starts_with(root).then_some(normalized)
}

/// 悬停：链接给出目标与存在性，标题给出引用数。
fn hover(
    parsed: &Parsed,
    document: &Document,
    offset: u32,
    index: Option<&WorkspaceIndex>,
    lines: &LineIndex<'_>,
) -> Option<Value> {
    if let Some(link) = parsed
        .links
        .iter()
        .find(|link| link.full_span.start <= offset && offset <= link.full_span.end)
    {
        let target = index::effective_target(link, &parsed.definitions);
        let mut text = format!("`{target}`");

        if let Some(base) = index::uri_path(document.uri())
            && let Some(resolved) = index::resolve_target(&base, &target, &link.kind, index)
        {
            if resolved.path.exists() {
                text.push_str(" · 存在");
            } else {
                text.push_str(" · 不存在");
            }
        }

        return Some(json!({
            "contents": { "kind": "markdown", "value": text },
            "range": range(lines, link.target_span),
        }));
    }

    let heading = heading_on_line(parsed, document.position_at(offset as usize).line)?;
    let text = format!("# {} · {}", heading.text_span.text(document.text()), heading.slug);
    Some(json!({
        "contents": { "kind": "markdown", "value": text },
        "range": range(lines, heading.text_span),
    }))
}

/// 文档链接：把可跳转的目标交给客户端，让链接可点击。
fn document_links(
    document: &Document,
    parsed: &Parsed,
    index: Option<&WorkspaceIndex>,
    lines: &LineIndex<'_>,
) -> Value {
    let Some(base) = index::uri_path(document.uri()) else {
        return Value::Array(Vec::new());
    };

    let items: Vec<Value> = parsed
        .links
        .iter()
        .filter_map(|link| {
            let target = index::effective_target(link, &parsed.definitions);
            let resolved = index::resolve_target(&base, &target, &link.kind, index)?;
            if !resolved.path.exists() {
                return None;
            }

            let uri = index::path_uri(&resolved.path)?;
            let target = match resolved.anchor {
                Some(anchor) => format!("{uri}#{anchor}"),
                None => uri,
            };
            Some(json!({ "range": range(lines, link.target_span), "target": target }))
        })
        .collect();

    Value::Array(items)
}

/// 快速修复：为范围内每条断链给出候选替换。
fn code_actions(
    document: &Document,
    parsed: &Parsed,
    backend: &MarkdownBackend,
    lines: &LineIndex<'_>,
    start: u32,
    end: u32,
) -> Value {
    let mut actions = Vec::new();

    for link in &parsed.links {
        if link.target_span.end < start || link.target_span.start > end {
            continue;
        }

        let Some(problem) = backend.link_problem(document, link, parsed) else {
            continue;
        };
        let message = with_candidates(problem.message, &problem.candidates);
        let diagnostic = link_diagnostic(lines, link, &message);

        for candidate in problem.candidates {
            actions.push(json!({
                "title": format!("改为 `{}`", candidate.replacement),
                "kind": "quickfix",
                "diagnostics": [diagnostic],
                "edit": {
                    "changes": {
                        document.uri(): [{ "range": range(lines, problem.replace), "newText": candidate.replacement }]
                    }
                }
            }));
        }
    }

    Value::Array(actions)
}

/// 选择范围：行 → 段落 → 标题分区 → 整篇。
fn selection_chain(document: &Document, parsed: &Parsed, lines: &LineIndex<'_>, offset: u32) -> Value {
    let line = document.position_at(offset as usize).line;
    let mut spans = vec![line_span(document, lines, line, line)];

    let (paragraph_start, paragraph_end) = paragraph_bounds(document, line);
    spans.push(line_span(document, lines, paragraph_start, paragraph_end));

    if let Some(position) = parsed.headings.iter().rposition(|heading| heading.line <= line) {
        let end = section_end(&parsed.headings, position, parsed.line_count);
        spans.push(line_span(document, lines, parsed.headings[position].line, end));
    }

    spans.push(Span::new(0, document.text().len()));

    let mut chain: Option<Value> = None;
    let mut previous: Option<Span> = None;
    for span in spans {
        if previous == Some(span) {
            continue;
        }
        chain = Some(match chain {
            Some(parent) => json!({ "range": range(lines, span), "parent": parent }),
            None => json!({ "range": range(lines, span) }),
        });
        previous = Some(span);
    }

    chain.unwrap_or(Value::Null)
}

/// 段落边界：上下扩展到空行为止。
fn paragraph_bounds(document: &Document, line: u32) -> (u32, u32) {
    let mut start = line;
    while start > 0 && !document.line_text(start - 1).trim().is_empty() {
        start -= 1;
    }

    let mut end = line;
    let last = document.line_count().saturating_sub(1) as u32;
    while end < last && !document.line_text(end + 1).trim().is_empty() {
        end += 1;
    }

    (start, end)
}

/// 工作区符号：索引里的全部标题。
fn workspace_symbols(index: Option<&WorkspaceIndex>, query: &str) -> Value {
    let Some(index) = index else {
        return Value::Array(Vec::new());
    };

    let query = query.to_lowercase();
    let mut items = Vec::new();

    for file in index.files() {
        let Some(uri) = index::path_uri(&file.path) else {
            continue;
        };
        let Some(text) = std::fs::read_to_string(&file.path).ok() else {
            continue;
        };
        let lines = LineIndex::new(&text);
        let container = file.path.file_name().and_then(|name| name.to_str()).unwrap_or_default();

        for heading in &file.headings {
            if !query.is_empty() && !heading.text.to_lowercase().contains(&query) {
                continue;
            }

            let span = line_span_from(&lines, &text, heading.line);
            items.push(json!({
                "name": heading.text,
                "kind": KIND_NAMESPACE,
                "location": location(&uri, &lines, span),
                "containerName": container,
            }));

            if items.len() >= 500 {
                return Value::Array(items);
            }
        }
    }

    Value::Array(items)
}

/// 整行区间；用于只记录行号的标题。
fn line_span_from(lines: &LineIndex<'_>, text: &str, line: u32) -> Span {
    let start = lines.line_start(line) as usize;
    let end = lines.line_start(line + 1).min(text.len() as u32) as usize;
    Span::new(start, end)
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

/// 追加一条编辑到 `changes` 里。
fn push_edit(changes: &mut Map<String, Value>, uri: &str, edit_range: Value, new_text: &str) {
    let entry = changes
        .entry(uri.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(items) = entry.as_array_mut() {
        items.push(json!({ "range": edit_range, "newText": new_text }));
    }
}

/// 文件监听事件类型：删除。
const FILE_DELETED: u8 = 3;

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

/// 重命名参数。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameParams {
    text_document: TextIdentifier,
    position: Position,
    new_name: String,
}

/// 快速修复参数。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeActionParams {
    text_document: TextIdentifier,
    range: Range,
}

/// 选择范围参数。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectionRangeParams {
    text_document: TextIdentifier,
    positions: Vec<Position>,
}

/// 工作区符号参数。
#[derive(Deserialize)]
struct WorkspaceSymbolParams {
    #[serde(default)]
    query: String,
}

/// 文件监听参数。
#[derive(Deserialize)]
struct WatchedFilesParams {
    changes: Vec<FileEvent>,
}

/// 单条文件变化。
#[derive(Deserialize)]
struct FileEvent {
    uri: String,
    #[serde(rename = "type")]
    kind: u8,
}

/// 文档标识。
#[derive(Deserialize)]
struct TextIdentifier {
    uri: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use lspd_core::docs::TextDocumentItem;

    /// 建一个临时工作区，返回根目录。
    fn workspace(tag: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("lspd-md-backend-{tag}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        for (name, text) in files {
            let path = root.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, text).unwrap();
        }

        root
    }

    /// 造一份已打开的文档。
    fn document(root: &Path, name: &str, text: &str) -> Document {
        Document::new(TextDocumentItem {
            uri: index::path_uri(&root.join(name)).unwrap(),
            language_id: "markdown".to_string(),
            version: 1,
            text: text.to_string(),
        })
    }

    /// 造一份带索引的后端。
    fn backend(root: &Path) -> MarkdownBackend {
        MarkdownBackend {
            index: Some(WorkspaceIndex::build(root)),
            ..MarkdownBackend::default()
        }
    }

    /// 把文档塞进后端缓存，模拟 didOpen 后的状态。
    fn open(backend: &mut MarkdownBackend, document: &Document) {
        backend.ensure(document);
    }

    #[test]
    fn diagnostics_report_missing_file_with_candidate() {
        let root = workspace("diag", &[("a.md", "# A\n\n见 [[安裝]]。\n"), ("安装.md", "# 安装\n")]);
        let document = document(&root, "a.md", "# A\n\n见 [[安裝]]。\n");
        let mut backend = backend(&root);
        open(&mut backend, &document);

        let mut out = Vec::new();
        backend.publish(&document, &mut out);
        let Outgoing::Notification { params, .. } = &out[0] else {
            panic!("应为诊断通知");
        };
        let diagnostics = params["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0]["message"].as_str().unwrap().contains("安装.md"));
    }

    #[test]
    fn references_find_links_to_heading() {
        let root = workspace(
            "refs",
            &[("a.md", "# A\n\n[去](./b.md#小节)\n"), ("b.md", "# B\n\n## 小节\n")],
        );
        let index = WorkspaceIndex::build(&root);
        let found = index.references_to(&root.join("b.md"), Some("小节"));
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn rename_updates_heading_and_links() {
        let root = workspace(
            "rename",
            &[("a.md", "# 旧标题\n"), ("b.md", "[去](./a.md#旧标题)\n")],
        );
        let mut backend = backend(&root);
        let document = document(&root, "a.md", "# 旧标题\n");
        open(&mut backend, &document);
        let cache = backend.cache.as_ref().unwrap();

        let heading = heading_on_line(&cache.parsed, 0).expect("标题行");
        assert_eq!(heading.slug, "旧标题");

        let index = backend.index.as_ref().unwrap();
        let base = index::uri_path(document.uri()).unwrap();
        let refs = index.references_to(&base, Some("旧标题"));
        assert_eq!(refs.len(), 1, "跨文件引用应被找到");

        // 引用区间应正好覆盖锚点文本，便于重命名时只替换锚点
        let source = std::fs::read_to_string(root.join("b.md")).unwrap();
        let (_, span) = &refs[0];
        assert_eq!(&source[span.start as usize..span.end as usize], "旧标题");
    }

    #[test]
    fn inline_completion_lists_relative_files() {
        let root = workspace(
            "inline-completion",
            &[("a.md", "# A\n\n[去](./\n"), ("docs/b.md", "# B\n")],
        );
        // 光标停在 `./` 之后、换行之前
        let source = "# A\n\n[去](./";
        let document = document(&root, "a.md", source);
        let mut backend = backend(&root);
        open(&mut backend, &document);
        let cache = backend.cache.as_ref().unwrap();
        let lines = LineIndex::new(source);
        let offset = source.len() as u32;

        let items = completion(&document, offset, &cache.parsed, backend.index.as_ref(), &lines);
        let labels: Vec<&str> = items
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["label"].as_str())
            .collect();
        assert!(labels.contains(&"./docs/b.md"), "候选：{labels:?}");
    }

    #[test]
    fn selection_chain_nests_line_paragraph_section_document() {
        let source = "# 标题\n\n第一行\n第二行\n\n## 小节\n\n内容\n";
        let document = Document::new(TextDocumentItem {
            uri: "file:///tmp/lspd-md-selection/a.md".to_string(),
            language_id: "markdown".to_string(),
            version: 1,
            text: source.to_string(),
        });
        let parsed = parser::parse(source);
        let lines = LineIndex::new(source);
        let offset = source.find("第二行").unwrap() as u32;

        let chain = selection_chain(&document, &parsed, &lines, offset);
        assert!(chain["range"].is_object());
        assert!(chain["parent"].is_object());
        assert!(chain["parent"]["parent"]["range"].is_object());
    }

    #[test]
    fn workspace_symbols_filter_by_query() {
        let root = workspace("symbols", &[("a.md", "# Alpha\n\n## Beta\n")]);
        let index = WorkspaceIndex::build(&root);

        let all = workspace_symbols(Some(&index), "");
        assert_eq!(all.as_array().unwrap().len(), 2);

        let beta = workspace_symbols(Some(&index), "beta");
        assert_eq!(beta.as_array().unwrap().len(), 1);
    }
}
