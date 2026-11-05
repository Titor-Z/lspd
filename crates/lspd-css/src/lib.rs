//! CSS 语言服务。
//!
//! 结构：
//! - `parser`：单遍扫描的容错解析，产出扁平语法树与结构诊断；
//! - `dialect`：css / scss / less 的差异参数化，避免三份实现；
//! - `data`：编译期生成的属性、at 规则、伪类、颜色表；
//! - `features`：各 LSP 能力实现；
//! - `backend`：接入核心层的 `Backend` 契约。

mod ast;
mod backend;
mod data;
mod dialect;
mod features;
mod parser;

pub use backend::backend;
pub use dialect::Dialect;
