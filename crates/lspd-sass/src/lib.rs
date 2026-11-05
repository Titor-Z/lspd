//! Sass 缩进语法语言服务。
//!
//! 缩进语法与 SCSS 的差异在于块由缩进界定、没有花括号与分号。
//! 本模块只承担"缩进结构 → 与 SCSS 等价的语法树"的适配（见 `parser`），
//! 补全、悬停、诊断等能力全部复用 CSS 侧实现，不重复实现一套数据驱动逻辑。

mod parser;

use lspd_core::Backend;
use lspd_css::StyleBackend;

/// 创建 Sass 后端的工厂函数：复用 CSS 的 `StyleBackend`，只把解析换成缩进语法。
pub fn backend() -> Box<dyn Backend> {
    Box::new(StyleBackend::new("sass", &["sass"], &[".sass"], parse_sass))
}

/// 缩进式 Sass 解析入口，适配成 CSS 后端的注入式解析函数。
fn parse_sass(source: &str, _language_id: &str) -> lspd_css::parser::ParseResult {
    parser::parse(source)
}
