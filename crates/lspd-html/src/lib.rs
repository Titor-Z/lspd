//! HTML 语言服务。
//!
//! 结构：
//! - `parser`：单遍扫描的容错解析，产出扁平语法树与结构诊断；
//! - `ast`：扁平节点池，`u32` 索引互链，无引用计数、无逐节点分配；
//! - `data`：编译期生成的标签与属性表，运行时不解析 JSON；
//! - `features`：在语法树上实现各 LSP 能力；
//! - `backend`：把各能力接到核心层的 `Backend` 契约上。

mod ast;
mod backend;
mod data;
mod features;
mod parser;

pub use backend::backend;
