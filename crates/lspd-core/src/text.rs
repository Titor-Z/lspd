//! 源码区间与行列换算。
//!
//! 语言后端统一用字节区间表示位置，只在生成 LSP 结构时换算成行列，
//! 因此这份转换是所有后端共用的公共能力。

/// 源码中的半开字节区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    /// 起始字节偏移
    pub start: u32,
    /// 结束字节偏移（不含）
    pub end: u32,
}

impl Span {
    /// 构造区间。
    pub fn new(start: usize, end: usize) -> Self {
        Self { start: start as u32, end: end as u32 }
    }

    /// 取区间对应的文本；越界时返回空串，避免解析期的越界访问扩散到上层。
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        source.get(self.start as usize..self.end as usize).unwrap_or("")
    }

    /// 是否包含指定字节位置。
    pub fn contains(&self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }

    /// 是否为空区间。
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// 行索引：把字节偏移与行列互相换算。
pub struct LineIndex<'a> {
    /// 原文
    source: &'a str,
    /// 每行起始字节偏移
    starts: Vec<u32>,
}

impl<'a> LineIndex<'a> {
    /// 建立行索引。只在需要输出 LSP 结构时构建。
    pub fn new(source: &'a str) -> Self {
        let mut starts = Vec::with_capacity(source.len() / 32 + 1);
        starts.push(0);
        for (offset, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(offset as u32 + 1);
            }
        }
        Self { source, starts }
    }

    /// 行数。
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// 某一行的起始字节偏移。
    pub fn line_start(&self, line: u32) -> u32 {
        self.starts.get(line as usize).copied().unwrap_or(self.source.len() as u32)
    }

    /// 字节偏移换算为行列（UTF-16 代码单元）。
    pub fn position(&self, offset: u32) -> (u32, u32) {
        let offset = offset.min(self.source.len() as u32);
        let line = match self.starts.binary_search(&offset) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        let start = self.starts[line] as usize;
        let character = self.source[start..offset as usize].chars().map(char::len_utf16).sum::<usize>();
        (line as u32, character as u32)
    }

    /// 行列（UTF-16 代码单元）换算为字节偏移，超出列宽时夹到本行末尾。
    pub fn offset(&self, line: u32, character: u32) -> u32 {
        let start = self.line_start(line) as usize;
        let end = self
            .starts
            .get(line as usize + 1)
            .map_or(self.source.len(), |next| *next as usize);
        let text = self.source[start..end].trim_end_matches(['\n', '\r']);

        let mut units = 0u32;
        for (index, value) in text.char_indices() {
            if units >= character {
                return (start + index) as u32;
            }
            units += value.len_utf16() as u32;
        }
        (start + text.len()) as u32
    }
}
