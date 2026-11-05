//! JSON 语言服务。
//!
//! 校验范围限定为离线可完成的子集：语法诊断，以及 `$schema` 或客户端配置提供的
//! schema 中的常用关键字（type / enum / required / properties / items / pattern / 范围约束）。
//! 不访问网络，不实现完整 draft 全集。
