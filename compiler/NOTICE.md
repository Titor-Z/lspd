# 本目录是 lspd 的 TypeScript 语言服务来源

## 来历

- 上游：`oxc-project/tsrs`（微软 TypeScript 7 的 Rust 移植），版本 **v0.22.0**
- 许可：Apache-2.0（见同目录 `LICENSE` 与 `NOTICE`）
- 上游 TS 基线：`7.1.0-beta`（commit `3e9236361fa97a9e6c685f4833ac457ae2d8c50a`）
- 获取时间：2026-11-06

## 本 fork 的修改（按 Apache-2.0 §4(b) 标注）

1. **仅保留 LSP 用途**。删除与语言服务无关的内容：
   - crate：`tsrs_lint`（lint）、`tsrs_testrunner`（测试运行器）、`tsrs_bench`（基准）、
     `tsrs_wasm`（wasm 绑定）、`tsrs_contentmappertest`（测试辅助）、
     `tsrs_cli`（`tsrs` 命令行分发器：check/bench/api 入口）
   - 目录：`testdata/`（一致性测试语料）、`npm/`、`notes/`、`tools/`、`playground/`、
     `docs/`、`tasks/`、`bench/`、`assets/`、`licenses/`、CI 配置
2. `crates/tsrs_execute/Cargo.toml`、`crates/tsrs_project/Cargo.toml`：
   移除指向已删 crate 的 dev-dependencies 条目。
3. 新增本文件 `NOTICE.md`。
4. `Cargo.toml`：`members` 去掉 `tasks/*`（对应目录已删）。
5. `rust-toolchain.toml`：`components = ["clippy", "rustfmt"]` 改为 `profile = "default"`，
   避免 rustup 重复安装 `cargo-fmt` 而失败。
6. 新增可选的批量打开合并（默认关闭，`LSPD_TS_BATCH_DIDOPEN=1` 开启）：
   `tsrs_project/src/session.rs` 中 `did_open_file` 在批量模式下把变更摘要累积到
   `pending_batch`，由防抖窗口结束时统一触发一次快照更新，避免连续打开多个文件时
   逐个全量重建 program；`tsrs_lsp/src/server.rs` 新增 `batch_did_open_window()` 读环境变量。
   `FileChangeSummary.opened` 由单个 URI 改为集合，配套调整 `overlayfs.rs`
   （去掉"一次只能处理一个 open"的断言）、`projectcollectionbuilder.rs`、
   `snapshotfs.rs`、`snapshot.rs`，使一次变更批次可以包含多个打开的文件。

## 与其它 fork 的关系

本目录服务于 **lspd**（通用 TS/JS 语言服务器，面向 node / 浏览器 / bun / deno）。
它与任何其他项目的 tsrs fork 完全独立，互不共享、互不影响。

## 升级方式

上游发版后，把新 tag 的 crate 源码与元数据合入本目录，再按上面的裁剪清单重做一遍。
本目录不保留 `.git`，不跟踪上游历史。

## 上游原始说明

见 `README.md`（上游文档，未修改）。
