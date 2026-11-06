# Changelog

v0.1.0 (2026-11-06)

## 概述

`lspd` —— 低内存多语言语言服务器，单可执行文件，LSP over stdio。支持 TypeScript/JavaScript（`lspd ts`）与 Web 技术栈（HTML/CSS/SCSS/Sass/JSON，`lspd web`）。按子命令或 feature 组合装载，OpenIDEA、Neovim、Helix、VSCode 共用同一二进制。

## 功能

### TypeScript / JavaScript

- **实现方式**：移植自上游 `oxc-project/tsrs` v0.22.0（TypeScript 7.1.0-beta），纯 Rust 实现，零 C 依赖。
- **能力覆盖**：初始化协商（29 项）、补全（类型感知、导入建议）、悬停（类型信息）、跳转定义、查找引用、重命名、诊断（语法 + 语义，拉取模式）、大纲、折叠范围、链接编辑等。
- **工作态内存**：在真实规模项目（162 文件 / 4.2 MB）下常驻约 16.7 MB（含 program、AST、类型缓存），是 vscode 系同类服务器的约 1/3–1/4。
- **请求延迟**：hover P50 约 0.2 ms，completion P50 约 11 ms，diagnostic P50 约 0.3 ms；与官方 tsgo 同量级，显著优于 Node 系方案。

### HTML

- **解析器**：容错递归下降，扁平 AST（单遍扫描、零引用计数、零逐节点分配），隐式闭合规则完整。
- **数据表**：编译期从 `@vscode/web-custom-data` 生成静态标签/属性表（valueSet 展开、二分查找，运行时零解析）。
- **能力**：补全（标签/属性/属性值）、悬停、结构 + 数据驱动诊断、文档大纲、折叠范围、标签联动改名、id/class 跳转与引用。
- **工作态内存**：打开 16 个 HTML 文档（1.16 MB）后约 8.9 MB，是 vscode-html-language-server 的约 1/16。
- **请求延迟**：补全往返 P50 0.44 ms，P95 1.08 ms。

### CSS / SCSS / Less

- **解析器**：容错解析器，支持嵌套规则、行注释；dialect 参数化。
- **数据表**：编译期生成属性/At 规则/伪类/伪元素/命名颜色表。
- **能力**：补全（数据驱动）、悬停、颜色选择与转换（documentColor/colorPresentation）、自定义属性与变量定义/引用/重命名、大纲、折叠、选择范围（基于 AST 嵌套链）、诊断。
- **工作态内存**：与 HTML 单进程共享，打开 16 个 CSS 文档后峰值约 8.9 MB。

### Sass（缩进语法）

- **适配层**：按行首缩进定块，换行为语句边界；直接复用 CSS AST，字节偏移不转码。
- **能力**：复用 CSS 所有特性，支持 `$` 变量定义/引用。

### JSON

- **解析器**：容错递归下降，产出值树与逐节点区间，坏输入可恢复。
- **Schema 驱动**：从本地 `$schema` 路径读取（离线运行），支持 type/enum/const/required/properties/items/pattern/数值与长度范围校验。
- **补全**：对象键补全、枚举值补全、布尔值/数字填充、pattern 校验（使用 `regex-lite` 无回溯匹配）。
- **诊断**：语法错误、schema 约束违反。

## 内存与性能

在同一机器、同一 fixture、同一测量工具下对比：

| 服务器 | 场景 | 启动 | 空闲 RSS | 打开后 RSS |
|---|---|---|---|---|
| **lspd ts** | 50 文件 / 23 KB | 1.3 ms | 3.7 MB | 3.8 MB |
| 官方 tsgo | 50 文件 / 23 KB | 6.8 ms | 9.6 MB | 9.9 MB |
| typescript-language-server | 50 文件 / 23 KB | 191.3 ms | 51.3 MB | 57.5 MB |
| vtsls | 50 文件 / 23 KB | 311.4 ms | 60.2 MB | 64.9 MB |
| **lspd web** | 16 HTML+16 CSS | 9.5 ms | 2.7 MB | 8.9 MB |
| vscode-html-language-server | 16 HTML | 1092.9 ms | 137.6 MB | 133.4 MB |
| vscode-css-language-server | 16 CSS | 508.0 ms | 72.1 MB | 115.1 MB |

vscode 方案要同时覆盖 HTML 与 CSS，需两个进程（空闲合计 209.7 MB），是 lspd web 的 77.7 倍。

## 架构

- **多 crate 空间**：`lspd-core`（协议与文档）、`lspd-tsbridge`（TS 桥接）、`lspd`（主入口）。Web 栈后端（HTML/CSS/Sass/JSON）作为插件按需加载。
- **编译器来源**：TS 侧来自上游 `oxc-project/tsrs`（Apache-2.0），与 `~/mio/compiler` 完全独立，零耦合。
- **构建策略**：保持纯 Rust 工具链，不得引入任何需要 `cc`/`cmake` 编译的依赖；内存优化以实测数据为准（系统 allocator vs mimalloc 对比，后者被剔除）。
- **协议标准**：遵循 LSP 3.17，启用 pull 模式诊断（`textDocument/diagnostic`），而非仅 push 模式。

## 发布说明

- **包名**：`lspd`
- **目标平台**：x86_64-apple-darwin / aarch64-apple-darwin / x86_64-unknown-linux-gnu
- **分发格式**：单个 tar.gz 压缩包（含校验和），解压后可直接运行。
- **许可证**：自有代码（Apache-2.0 待确认）+ `compiler/` 保留 Apache-2.0 许可。
- **已知问题**：tsgo 在 162 文件 / 4.2 MB 上无法完成握手（属实测现象，非 lspd 问题）。

## 贡献与文档

- **测试脚本**：`scripts/bench.py`（启动与内存）、`scripts/verify_ts.py`（能力自检）、`scripts/measure_latency.py`（请求延迟）。
- **讨论记录**：见 `docs/Taolun.md`。
- **详细报告**：见 `docs/Bench.md`（私有目录，未随仓库发布）。
