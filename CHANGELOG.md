# Changelog

v0.2.0 (2026-11-08)

## 概述

TypeScript 侧的内存优化版本：打开文件不再逐个重建 program，改为在防抖窗口内合并，
连续打开一批文件只重建一次。发布产物同步补齐 Windows 平台，并改为直接提供可执行文件。

## 新增

### TypeScript：批量合并 didOpen

一次打开多个文件时，此前每收到一个 `didOpen` 就重建一次 program，内存平台随之抬高。
现在 `didOpen` 并入 300ms 防抖窗口，窗口内提交的文件只触发一次重建。

- `LSPD_TS_BATCH_DIDOPEN=0` 关闭批量合并，回到逐次重建
- `LSPD_TS_BATCH_DELAY_MS` 调整窗口长度（默认 300）

实测（40 个 `.js`，间隔 100ms）：默认开启时 program 创建 1 次，关闭时 40 次。

### 发布产物

- 增加 Windows x64（`x86_64-pc-windows-msvc`）。
- Release 直接提供编译好的可执行文件，不再打 tar.gz，也不含解压步骤；
  同名 `.sha256` 为校验和。
- 产物文件名不带版本号，下载地址可固定为 `releases/latest/download/<平台>`，
  官网安装命令因此不随版本号变化。

### 测量工具

`scripts/region_stats.py` 统计 program region 的创建与释放计数；`scripts/bench.py`
支持逐文件阶梯采样，用于定位内存平台出现的位置。

## 修正

- 官网安装命令与页脚版本号随本次发布同步。

v0.1.1 (2026-11-07)

## 概述

文档与官网版本。语言服务代码与 v0.1.0 相同，本次发布的是使用说明与项目官网。

## 新增

### README

首次提供完整文档：安装、平台产物、与官方实现的实测对照、命令行说明，
以及编辑器集成指南。集成部分先给通用用法——stdio 传输、
`ts` 与 `web` 必须分别启动进程的原因、语言到子命令的映射表，
再说明换到任何编辑器都可套用的三条自查条件。随后逐条给出
OpenIDEA、Neovim、Helix、VSCode、Vim 的具体配置与冲突提示。

### 官网

`site/` 目录下的静态站点，包豪斯风格：三原色只做图形与标记，正文黑白；
只用矩形、圆、三角；不用渐变与投影。内容为实测数据、能力清单、
五款编辑器的接入配置与安装命令。

站点标记的三个约束：无框架依赖、无网络字体、无第三方脚本。
页脚标注的体积由构建脚本按实际产物 gzip 后回填，不是手写数字。

构建产物：JS 1.1 KB、CSS 2.5 KB（gzip）。

### 发布流程

新增 GitHub Actions 工作流：推送 `v*` 标签时按三个目标平台构建并发布，
`site/**` 变化时构建官网并部署到 GitHub Pages。

## 修正

- 页脚体积标注改为构建时计算，此前是占位符
- 窄屏下表格与代码块各自横向滚动，不再撑破页面
- 移除依赖 JavaScript 才可见的滚动显现效果，内容不执行脚本也能完整阅读

---

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
