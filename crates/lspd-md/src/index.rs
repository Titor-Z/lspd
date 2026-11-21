//! 工作区索引：文件名、标题与链接。
//!
//! 只常驻「解析后仍然有用」的部分：标题的锚点与原文、链接解析出的目标文件与锚点。
//! 正文与语法树不进索引——打开中的文档由 `backend` 的单文档缓存负责。
//!
//! 索引支持增量更新：保存、打开、关闭与文件监听事件都只重解析受影响的那一个文件，
//! 避免在大 vault 上每次改动都全量重扫。

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

use lspd_core::text::Span;

use crate::parser::{self, Link, LinkDefinition, LinkKind};

/// 索引中的一个标题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedHeading {
    /// GitHub 风格锚点
    pub slug: String,
    /// 标题原文
    pub text: String,
    /// 所在行号（0 起）
    pub line: u32,
}

/// 索引中的一条链接。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedLink {
    /// 目标原文在源文件中的字节区间（诊断与引用定位都用它）
    pub span: Span,
    /// 锚点原文在源文件中的字节区间；无锚点为 `None`
    ///
    /// 与 `span` 不同：`[去](./b.md#小节)` 的 `span` 覆盖 `./b.md#小节`，
    /// 而锚点区间只覆盖 `小节`，重命名标题时只改这一段。
    pub anchor_span: Option<Span>,
    /// 解析后的目标文件
    pub path: PathBuf,
    /// 锚点原文；无锚点为 `None`
    pub anchor: Option<String>,
    /// 链接的有效目标（wikilink 已去掉 `|别名`）
    pub target: String,
}

/// 索引中的一个文件。
#[derive(Debug, Clone, Default)]
pub struct IndexedFile {
    /// 文件绝对路径
    pub path: PathBuf,
    /// 标题
    pub headings: Vec<IndexedHeading>,
    /// 链接
    pub links: Vec<IndexedLink>,
}

/// 工作区索引。
#[derive(Default)]
pub struct WorkspaceIndex {
    /// 工作区根，用于计算相对路径
    root: PathBuf,
    /// 已索引文件
    files: Vec<IndexedFile>,
}

impl WorkspaceIndex {
    /// 扫描工作区并建立索引。
    pub fn build(root: &Path) -> Self {
        let mut index = Self {
            root: normalize(root.to_path_buf()),
            files: Vec::new(),
        };
        let mut paths = Vec::new();
        walk_markdown(&index.root, &mut paths);

        for path in paths {
            index.reindex(&path);
        }

        index
    }

    /// 重新读取磁盘内容并替换该文件的条目；读不到视为已删除。
    pub fn reindex(&mut self, path: &Path) {
        let path = normalize(path.to_path_buf());

        match std::fs::read_to_string(&path) {
            Ok(text) => self.update(&path, &text),
            Err(_) => self.remove(&path),
        }
    }

    /// 用给定正文更新一个文件。链接解析依赖全量文件名表，因此先落条目再解析链接。
    pub fn update(&mut self, path: &Path, text: &str) {
        let path = normalize(path.to_path_buf());
        let parsed = parser::parse(text);

        let file = IndexedFile {
            path: path.clone(),
            headings: parsed
                .headings
                .iter()
                .map(|heading| IndexedHeading {
                    slug: heading.slug.clone(),
                    text: heading.text_span.text(text).to_string(),
                    line: heading.line,
                })
                .collect(),
            links: Vec::new(),
        };

        match self.files.iter_mut().find(|item| item.path == path) {
            Some(slot) => *slot = file,
            None => self.files.push(file),
        }

        let links: Vec<IndexedLink> = parsed
            .links
            .iter()
            .filter_map(|link| self.index_link(&path, link, &parsed.definitions))
            .collect();

        if let Some(slot) = self.files.iter_mut().find(|item| item.path == path) {
            slot.links = links;
        }
    }

    /// 把一条链接解析成索引条目；目标无法解析时丢弃。
    fn index_link(
        &self,
        base: &Path,
        link: &Link,
        definitions: &[LinkDefinition],
    ) -> Option<IndexedLink> {
        let target = effective_target(link, definitions);
        let resolved = resolve_target(base, &target, &link.kind, Some(self))?;

        Some(IndexedLink {
            span: link.target_span,
            anchor_span: parser::anchor_span(link, definitions),
            path: resolved.path,
            anchor: resolved.anchor,
            target: target.to_string(),
        })
    }

    /// 删除一个文件的条目。
    pub fn remove(&mut self, path: &Path) {
        let path = normalize(path.to_path_buf());
        self.files.retain(|item| item.path != path);
    }

    /// 工作区根。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 全部已索引文件。
    pub fn files(&self) -> &[IndexedFile] {
        &self.files
    }

    /// 取某个文件的条目。
    pub fn file(&self, path: &Path) -> Option<&IndexedFile> {
        let path = normalize(path.to_path_buf());
        self.files.iter().find(|item| item.path == path)
    }

    /// 某个文件的标题；未索引时为空。
    pub fn headings_of(&self, path: &Path) -> &[IndexedHeading] {
        self.file(path).map_or(&[], |file| file.headings.as_slice())
    }

    /// 反查指向某个文件（可选某个锚点）的链接。
    ///
    /// 指定锚点时返回锚点自身的区间，便于重命名只替换锚点文本；
    /// 锚点区间未知（例如引用式链接缺定义）时退回整个目标区间。
    pub fn references_to(&self, path: &Path, anchor: Option<&str>) -> Vec<(PathBuf, Span)> {
        let path = normalize(path.to_path_buf());
        let wanted = anchor.map(parser::slugify);
        let mut found = Vec::new();

        for file in &self.files {
            for link in &file.links {
                if link.path != path {
                    continue;
                }

                if let Some(wanted) = &wanted {
                    let matches = link
                        .anchor
                        .as_deref()
                        .is_some_and(|anchor| &parser::slugify(anchor) == wanted);
                    if !matches {
                        continue;
                    }
                }

                let span = match &wanted {
                    Some(_) => link.anchor_span.unwrap_or(link.span),
                    None => link.span,
                };
                found.push((file.path.clone(), span));
            }
        }

        found
    }

    /// 找与缺失文件名最接近的候选（编辑距离 ≤ 3，同时比对带与不带扩展名的形式）。
    ///
    /// `exclude` 用于排除当前文档：从 `a.md` 里写断链时，把 `a.md` 本身列为候选没有意义。
    pub fn similar_files(&self, name: &str, exclude: Option<&Path>) -> Vec<String> {
        let mut candidates: Vec<(usize, String)> = self
            .files
            .iter()
            .filter(|item| exclude != Some(item.path.as_path()))
            .filter_map(|item| {
                let file = item.path.file_name().and_then(|file| file.to_str())?;
                let stem = item.path.file_stem().and_then(|stem| stem.to_str())?;
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
    pub fn find_by_stem(&self, stem: &str) -> Option<PathBuf> {
        self.files
            .iter()
            .find(|item| item.path.file_stem().and_then(|name| name.to_str()) == Some(stem))
            .map(|item| item.path.clone())
    }

    /// 取某文件内与缺失锚点最接近的标题（编辑距离门槛同文件名候选）。
    pub fn similar_headings(&self, path: &Path, slug: &str) -> Vec<String> {
        let mut candidates: Vec<(usize, String)> = self
            .headings_of(path)
            .iter()
            .filter_map(|heading| {
                close_distance(slug, &heading.slug).map(|distance| (distance, heading.text.clone()))
            })
            .collect();
        candidates.sort();
        candidates.truncate(3);
        candidates.into_iter().map(|(_, text)| text).collect()
    }
}

/// 链接解析结果：目标文件与可选锚点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// 目标文件
    pub path: PathBuf,
    /// 锚点原文
    pub anchor: Option<String>,
}

/// 取链接的最终目标。wikilink 去掉 `|别名`；引用式查定义表，未定义则为空。
pub fn effective_target<'a>(link: &'a Link, definitions: &'a [LinkDefinition]) -> Cow<'a, str> {
    match &link.kind {
        LinkKind::Inline => Cow::Borrowed(link.target.as_str()),
        LinkKind::Wiki => Cow::Owned(
            link.target
                .split('|')
                .next()
                .unwrap_or("")
                .trim()
                .to_string(),
        ),
        LinkKind::Reference(label) => match definitions.iter().find(|item| &item.label == label) {
            Some(definition) => Cow::Borrowed(&definition.target),
            None => Cow::Borrowed(""),
        },
    }
}

/// 把链接目标解析成「文件 + 锚点」。
///
/// 行内链接按相对路径解析；`[[wikilink]]` 的目标若无扩展名，
/// 按文件名主干在工作区索引里匹配。同文档跳转（无文件部分）指向当前文件。
pub fn resolve_target(
    base: &Path,
    target: &str,
    kind: &LinkKind,
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
        return Some(Resolved {
            path: normalize(base.to_path_buf()),
            anchor,
        });
    }

    if matches!(kind, LinkKind::Wiki)
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
        normalize(base.parent()?.join(path))
    };

    Some(Resolved { path, anchor })
}

/// 是否为 Markdown 文件（按扩展名）。
pub fn is_markdown(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("md" | "markdown")
    )
}

/// 递归收集工作区里的 Markdown 文件。符号链接与权限错误一律跳过。
fn walk_markdown(dir: &Path, out: &mut Vec<PathBuf>) {
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

        if kind.is_file() && is_markdown(&path) {
            out.push(path);
        }
    }
}

/// 候选门槛：编辑距离不超过 3，且不超过较长一方长度的一半（短词至少允许 1）。
///
/// 只看绝对距离会让短名字乱命中：`不存在` 与 `安装` 的距离恰好是 3，
/// 于是任意三个汉字的断链都会建议一个两字标题的文件。按长度比例收紧后，
/// 这类无关候选被排除，拼写错误（`安裝` / `安装`）仍然命中。
pub fn close_distance(name: &str, candidate: &str) -> Option<usize> {
    let distance = edit_distance(name, candidate);
    let longest = name.chars().count().max(candidate.chars().count());
    (distance <= 3 && distance <= (longest / 2).max(1)).then_some(distance)
}

/// Levenshtein 编辑距离，用于「猜你想去」（按字符，而非字节）。
pub fn edit_distance(a: &str, b: &str) -> usize {
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

/// 归一路径：去掉 `.` 并把 `..` 折叠到上级，避免 URI 中出现 `./`。
/// 不用 `canonicalize`，它会把符号链接解开（macOS 上 `/tmp` 会变成 `/private/tmp`），
/// 从而与客户端给出的 URI 对不上。
pub fn normalize(path: PathBuf) -> PathBuf {
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
pub fn uri_path(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?;
    percent_decode(raw).map(PathBuf::from)
}

/// 本地路径转换为 `file://` URI。只转义 URI 路径中必须转义的字符。
pub fn path_uri(path: &Path) -> Option<String> {
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
pub fn percent_decode(text: &str) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一个临时工作区并写入给定文件。
    fn workspace(tag: &str, files: &[(&str, &str)]) -> (PathBuf, WorkspaceIndex) {
        let root = std::env::temp_dir().join(format!("lspd-md-index-{tag}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        for (name, text) in files {
            let path = root.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, text).unwrap();
        }

        let index = WorkspaceIndex::build(&root);
        (root, index)
    }

    #[test]
    fn indexes_headings_and_resolved_links() {
        let (root, index) = workspace(
            "basic",
            &[
                (
                    "a.md",
                    "# A\n\n见 [安装](./docs/安装.md#快速开始) 与 [[b]]。\n",
                ),
                ("b.md", "# B\n"),
                ("docs/安装.md", "# 安装\n\n## 快速开始\n"),
            ],
        );

        let file = index.file(&root.join("a.md")).expect("a.md 已索引");
        assert_eq!(file.headings[0].slug, "a");
        assert_eq!(file.links.len(), 2);
        assert_eq!(file.links[0].path, normalize(root.join("docs/安装.md")));
        assert_eq!(file.links[0].anchor.as_deref(), Some("快速开始"));
        assert_eq!(file.links[1].path, normalize(root.join("b.md")));
    }

    #[test]
    fn references_find_links_to_file_and_anchor() {
        let (root, index) = workspace(
            "refs",
            &[
                ("a.md", "[一](./b.md#小节)\n[二](./b.md)\n[三](#本地)\n"),
                ("b.md", "# B\n\n## 小节\n"),
            ],
        );

        let all = index.references_to(&root.join("b.md"), None);
        assert_eq!(all.len(), 2);
        let anchored = index.references_to(&root.join("b.md"), Some("小节"));
        assert_eq!(anchored.len(), 1);
        assert_eq!(anchored[0].0, normalize(root.join("a.md")));
    }

    #[test]
    fn update_replaces_only_target_file() {
        let (root, index) = workspace("update", &[("a.md", "# 旧\n"), ("b.md", "# B\n")]);
        let mut index = index;
        index.update(&root.join("a.md"), "# 新\n\n[[b]]\n");

        let file = index.file(&root.join("a.md")).unwrap();
        assert_eq!(file.headings[0].slug, "新");
        assert_eq!(file.links.len(), 1);
        assert!(index.file(&root.join("b.md")).is_some());
    }

    #[test]
    fn remove_drops_the_file() {
        let (root, mut index) = workspace("remove", &[("a.md", "# A\n")]);
        index.remove(&root.join("a.md"));
        assert!(index.file(&root.join("a.md")).is_none());
    }

    #[test]
    fn similar_files_skips_current_document() {
        let (root, index) = workspace("similar", &[("a.md", "# A\n")]);
        let current = root.join("a.md");

        assert_eq!(index.similar_files("a.md", None), ["a.md"]);
        assert!(index.similar_files("a.md", Some(&current)).is_empty());
        assert!(index.similar_files("不存在", None).is_empty());
    }

    #[test]
    fn close_distance_scales_with_length() {
        assert_eq!(close_distance("安裝", "安装"), Some(1));
        assert_eq!(close_distance("不存在", "安装"), None);
        assert_eq!(close_distance("tsconfg", "tsconfig"), Some(1));
        assert_eq!(close_distance("完全不同", "别的文件.md"), None);
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
}
