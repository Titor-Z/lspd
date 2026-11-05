//! JSON 语言服务。
//!
//! 校验范围限定为离线可完成的子集：语法诊断，以及 `$schema` 或客户端配置提供的
//! schema 中的常用关键字（type / enum / required / properties / items / pattern / 范围约束）。
//! 不访问网络，不实现完整 draft 全集。
//!
//! 结构：
//! - `parser`：容错 JSON 解析，产出值树与语法诊断；
//! - `backend`：接入核心层的 `Backend` 契约（M5.1：诊断、大纲、折叠；M5.2：schema 补全与校验）。

mod backend;
pub mod parser;

pub use backend::backend;
