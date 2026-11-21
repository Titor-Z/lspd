//! Markdown 语言服务。
//!
//! 结构：
//! - `parser`：行扫描解析，产出标题（含 GitHub 锚点）、行内链接与代码围栏；
//! - `index`：工作区索引，记录文件名、标题与链接解析结果，支持增量更新；
//! - `backend`：接入核心层的 `Backend` 契约（大纲、折叠、跳转、反查、重命名等）。

mod backend;
pub mod index;
pub mod parser;

pub use backend::backend;
