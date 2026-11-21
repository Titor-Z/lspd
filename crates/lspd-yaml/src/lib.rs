//! YAML 语言服务。
//!
//! 解析使用 `saphyr-parser`（纯 Rust），值树转换后复用 `lspd-json` 的 schema
//! 子集做补全、悬停与校验。第一版重点支持 docker-compose 配置。

mod backend;
mod parser;

pub use backend::backend;
