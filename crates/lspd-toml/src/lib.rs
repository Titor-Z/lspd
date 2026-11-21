//! TOML 语言服务。
//!
//! 解析使用 `toml_edit`（保留 span，纯 Rust），值树转换后复用 `lspd-json` 的 schema
//! 子集做补全、悬停与校验。这样 TOML/JSON 配置文件共享同一套 schema 维护方式。

mod backend;
mod parser;

pub use backend::backend;
