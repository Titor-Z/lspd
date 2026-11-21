//! Dockerfile 语言服务。
//!
//! Dockerfile 是行指令语法，不适合 JSON Schema；本 crate 提供内置指令词表驱动的
//! hover、completion 与基础诊断，保持离线、纯 Rust、低内存。

mod backend;
mod parser;

pub use backend::backend;
