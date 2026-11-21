//! Markdown 语言服务。
//!
//! 结构：
//! - `parser`：行扫描解析，产出标题（含 GitHub 锚点）、行内链接与代码围栏；
//! - `backend`：接入核心层的 `Backend` 契约（大纲、折叠、链接跳转）。

mod backend;
pub mod parser;

pub use backend::backend;
