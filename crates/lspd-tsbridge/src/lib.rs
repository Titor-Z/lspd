//! TypeScript / JavaScript 语言服务。
//!
//! 实现来自 `compiler/`（源自 oxc-project/tsrs，Apache-2.0）的 `tsrs_lsp`。
//! 该实现的 LSP 层已完备，本模块只做进程装配：把标准输入输出交给它的服务器，
//! 不重复实现协议层，也不与其争夺 stdin/stdout。
//!
//! 与 HTML/CSS 分离为不同进程的原因：TypeScript 的 program 跨文件常驻且无法按需释放，
//! 独立进程边界使它可被单独重启与回收，且不影响其余语言服务。
