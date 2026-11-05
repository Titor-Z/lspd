//! CSS 方言。
//!
//! 三种方言共用同一套词法、语法与数据表，差异集中在注释写法与变量前缀，
//! 因此以参数而非三份实现来表达。

/// 支持的 CSS 方言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// 标准 CSS
    Css,
    /// SCSS：支持 `//` 行注释与 `$变量`
    Scss,
    /// Less：支持 `//` 行注释与 `@变量`
    Less,
}

impl Dialect {
    /// 由语言标识创建；不认识的标识按标准 CSS 处理。
    pub fn from_language_id(language_id: &str) -> Self {
        match language_id {
            "scss" => Self::Scss,
            "less" => Self::Less,
            _ => Self::Css,
        }
    }

    /// 由文件扩展名创建。
    pub fn from_extension(extension: &str) -> Self {
        match extension {
            ".scss" => Self::Scss,
            ".less" => Self::Less,
            _ => Self::Css,
        }
    }

    /// 该方言对应的 LSP 语言标识。
    pub fn language_id(self) -> &'static str {
        match self {
            Self::Css => "css",
            Self::Scss => "scss",
            Self::Less => "less",
        }
    }

    /// 是否支持 `//` 行注释。
    pub fn has_line_comments(self) -> bool {
        !matches!(self, Self::Css)
    }

    /// 变量前缀字符。
    pub fn variable_prefix(self) -> char {
        match self {
            Self::Css => '-',
            Self::Scss => '$',
            Self::Less => '@',
        }
    }
}
