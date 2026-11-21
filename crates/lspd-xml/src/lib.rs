//! XML 语言服务。
//!
//! 结构：
//! - `parser`：单遍扫描的容错解析，产出扁平语法树与结构诊断；
//! - `ast`：扁平节点池，`u32` 索引互链；
//! - `features`：各 LSP 能力实现；
//! - `backend`：接入核心层的 `Backend` 契约。

pub mod ast;
mod backend;
mod features;
mod hooks;
pub mod ns;
pub mod parser;
pub mod schema;

pub use backend::{backend, backend_with, backend_with_hooks, backend_with_schema};
pub use hooks::{NoHooks, XmlHooks};
pub use schema::{NodeDoc, NoSchema, SchemaProvider};
