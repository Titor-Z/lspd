//! CSS 语言服务。
//!
//! 结构：
//! - `parser`：单遍扫描的容错解析，产出扁平语法树与结构诊断；
//! - `dialect`：css / scss / less 的差异参数化，避免三份实现；
//! - `data`：编译期生成的属性、at 规则、伪类、颜色表；
//! - `features`：各 LSP 能力实现；
//! - `backend`：接入核心层的 `Backend` 契约。
//!
//! `ast` / `parser` / `data` 对外可见，供缩进式 Sass（`lspd-sass`）复用同一套语法树、
//! 诊断类型与属性表，只自带一个把缩进结构解析成 `Ast` 的适配层。

pub mod ast;
mod backend;
pub mod data;
mod dialect;
mod features;
pub mod parser;

pub use backend::{ParseFn, StyleBackend, backend};
pub use dialect::Dialect;
