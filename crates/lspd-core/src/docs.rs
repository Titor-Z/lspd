//! 文档存储：正文只保留一份，负责增量编辑与行列换算。
//!
//! 位置编码固定为 UTF-16：LSP 规定客户端必须先支持 UTF-16，
//! 因此这里不做协商，直接按 UTF-16 代码单元换算，避免额外分支。

use rustc_hash::FxHashMap;
use serde::Deserialize;

/// 文档中的位置。`character` 以 UTF-16 代码单元计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Position {
    /// 行号，从 0 开始
    pub line: u32,
    /// 行内偏移，以 UTF-16 代码单元计，从 0 开始
    pub character: u32,
}

/// 文本区间，含头不含尾。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Range {
    /// 起点
    pub start: Position,
    /// 终点
    pub end: Position,
}

/// 客户端打开文档时给出的初始内容。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentItem {
    /// 文档 URI
    pub uri: String,
    /// 语言标识，用于路由到对应后端
    pub language_id: String,
    /// 版本号，客户端每次修改递增
    pub version: i32,
    /// 全文
    pub text: String,
}

/// 单次内容变更。`range` 为空表示整篇替换。
#[derive(Debug, Clone, Deserialize)]
pub struct ContentChange {
    /// 变更区间
    #[serde(default)]
    pub range: Option<Range>,
    /// 新的文本片段
    pub text: String,
}

/// 一份已打开的文档。
pub struct Document {
    /// 文档 URI
    uri: String,
    /// 语言标识
    language_id: String,
    /// 版本号
    version: i32,
    /// 全文，唯一副本
    text: String,
    /// 每一行起始处的字节偏移，用于位置换算
    line_starts: Vec<u32>,
}

impl Document {
    /// 用客户端给出的初始内容创建文档。
    pub fn new(item: TextDocumentItem) -> Self {
        let mut document = Self {
            uri: item.uri,
            language_id: item.language_id,
            version: item.version,
            text: item.text,
            line_starts: Vec::new(),
        };
        document.reindex();
        document
    }

    /// 文档 URI。
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// 语言标识。
    pub fn language_id(&self) -> &str {
        &self.language_id
    }

    /// 当前版本号。
    pub fn version(&self) -> i32 {
        self.version
    }

    /// 全文。
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 行数。
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// 指定行的文本，不含行尾换行符。
    pub fn line_text(&self, line: u32) -> &str {
        let index = line as usize;
        let Some(&start) = self.line_starts.get(index) else {
            return "";
        };
        let end = self.line_end(index);
        &self.text[start as usize..end]
    }

    /// 应用一次内容变更并重建行索引。
    pub fn apply_change(&mut self, change: ContentChange) {
        match change.range {
            None => self.text = change.text,
            Some(range) => {
                let start = self.offset_at(range.start);
                let end = self.offset_at(range.end).max(start);
                if end <= self.text.len() {
                    self.text.replace_range(start..end, &change.text);
                }
            }
        }
        self.reindex();
    }

    /// 把行列位置换算成字节偏移。越界位置会被夹到行尾。
    pub fn offset_at(&self, position: Position) -> usize {
        let index = position.line as usize;
        let Some(&start) = self.line_starts.get(index) else {
            return self.text.len();
        };

        let line_start = start as usize;
        let line_end = self.line_end(index);
        let mut offset = line_start;
        let mut remaining = position.character;

        // 逐字符前进，按 UTF-16 代码单元扣减，保证代理对与中文都能正确落点
        for character in self.text[line_start..line_end].chars() {
            if remaining == 0 {
                break;
            }
            let units = character.len_utf16() as u32;
            if remaining < units {
                break;
            }
            remaining -= units;
            offset += character.len_utf8();
        }

        offset
    }

    /// 把字节偏移换算成行列位置。
    pub fn position_at(&self, offset: usize) -> Position {
        let offset = offset.min(self.text.len());
        let line = match self.line_starts.binary_search(&(offset as u32)) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        let line_start = self.line_starts[line] as usize;
        let character = self.text[line_start..offset]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>();
        Position {
            line: line as u32,
            character: character as u32,
        }
    }

    /// 该行的结束字节偏移，不含 `\n` 与 `\r`。
    fn line_end(&self, index: usize) -> usize {
        let next_start = self
            .line_starts
            .get(index + 1)
            .copied()
            .unwrap_or(self.text.len() as u32);
        let mut end = next_start as usize;
        if end > 0 && self.text.as_bytes().get(end - 1) == Some(&b'\n') {
            end -= 1;
        }
        if end > 0 && self.text.as_bytes().get(end - 1) == Some(&b'\r') {
            end -= 1;
        }
        end
    }

    /// 重建行起始索引。
    fn reindex(&mut self) {
        self.line_starts.clear();
        self.line_starts.push(0);
        for (offset, byte) in self.text.bytes().enumerate() {
            if byte == b'\n' {
                self.line_starts.push(offset as u32 + 1);
            }
        }
    }
}

/// 所有已打开文档的集合。键为 URI，与 LSP 的消息粒度一致。
#[derive(Default)]
pub struct DocumentStore {
    /// URI 到文档的映射
    documents: FxHashMap<String, Document>,
}

impl DocumentStore {
    /// 打开或替换一份文档，返回其引用。
    pub fn open(&mut self, item: TextDocumentItem) -> &Document {
        let uri = item.uri.clone();
        self.documents.insert(uri.clone(), Document::new(item));
        // 上一步刚插入，必定存在
        self.documents.get(&uri).expect("文档刚插入")
    }

    /// 按 URI 取文档。
    pub fn get(&self, uri: &str) -> Option<&Document> {
        self.documents.get(uri)
    }

    /// 按 URI 取可变文档。
    pub fn get_mut(&mut self, uri: &str) -> Option<&mut Document> {
        self.documents.get_mut(uri)
    }

    /// 关闭并释放一份文档。
    pub fn close(&mut self, uri: &str) -> Option<Document> {
        self.documents.remove(uri)
    }

    /// 已打开文档数量。
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    /// 是否没有任何已打开文档。
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    /// 遍历所有文档。
    pub fn iter(&self) -> impl Iterator<Item = &Document> {
        self.documents.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一份用于换算测试的文档，含中文与代理对（emoji）。
    fn sample() -> Document {
        Document::new(TextDocumentItem {
            uri: "file:///a.html".to_string(),
            language_id: "html".to_string(),
            version: 1,
            text: "第一行\nsecond line\n😀 尾巴".to_string(),
        })
    }

    /// 行数按换行符切分，最后一行没有换行也算一行。
    #[test]
    fn counts_lines() {
        assert_eq!(sample().line_count(), 3);
        assert_eq!(sample().line_text(1), "second line");
    }

    /// UTF-16 代码单元到字节偏移：中文 1 单元 3 字节，emoji 2 单元 4 字节。
    #[test]
    fn converts_utf16_positions() {
        let document = sample();
        assert_eq!(
            document.offset_at(Position {
                line: 0,
                character: 2
            }),
            6
        );
        assert_eq!(
            document.offset_at(Position {
                line: 2,
                character: 2
            }),
            "第一行\nsecond line\n".len() + 4
        );
        assert_eq!(
            document.position_at("第一行\n".len()),
            Position {
                line: 1,
                character: 0
            }
        );
    }

    /// 超出实际列宽的位置应夹到行尾，而不是越界到下一行。
    #[test]
    fn clamps_position_beyond_line_end() {
        let document = sample();
        let offset = document.offset_at(Position {
            line: 1,
            character: 999,
        });
        assert_eq!(offset, "第一行\nsecond line".len());
    }

    /// 增量编辑只替换给定区间，其余内容保持不变。
    #[test]
    fn applies_incremental_change() {
        let mut document = sample();
        document.apply_change(ContentChange {
            range: Some(Range {
                start: Position {
                    line: 1,
                    character: 0,
                },
                end: Position {
                    line: 1,
                    character: 6,
                },
            }),
            text: "another".to_string(),
        });
        assert_eq!(document.line_text(1), "another line");
    }

    /// 整篇替换后行索引必须重建。
    #[test]
    fn replaces_whole_text() {
        let mut document = sample();
        document.apply_change(ContentChange {
            range: None,
            text: "only".to_string(),
        });
        assert_eq!(document.line_count(), 1);
        assert_eq!(document.text(), "only");
    }
}
