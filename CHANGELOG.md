# Changelog

v0.3.0 (2026-11-21)

## 概述

新增 XML / SVG 语言服务，并补齐编辑器接入文档中的 ttt 配置说明。XML 与 SVG 复用同一套容错解析与 SchemaProvider 架构，SVG 在此基础上提供静态词表驱动的提示。

## 新增

### XML 语言服务

- `lspd xml`：容错单遍扫描解析器，扁平 AST（字节区间），支持元素、属性、文本、注释、CDATA、PI、DOCTYPE。
- 能力：语法诊断、documentSymbol、foldingRange、hover、completion、documentFormatting。
- 命名空间语义：文档内 `xmlns` / `xmlns:prefix` 作用域解析、QName 拆分、元素/属性命名空间解析；不抓取外部资源。
- SchemaProvider 抽象：按 namespace/tag/attribute/prefix 提供 hover/completion 数据；后续可接入本地 XSD 子集。

### SVG 语言服务

- `lspd svg`：复用 XML 解析、诊断、结构能力与格式化。
- 静态 SVG 词表：构建期从 `data/svg-data.json` 生成标签/属性表，运行时二分查找，不解析 JSON。
- hover/completion 叠加 SVG 标签/属性文档与候选。

### 格式化

- XML/SVG 格式化为保守策略：调整缩进与标签换行，归一化标签内空白。
- `xml:space="preserve"` 范围保持原样；混合内容中的文本节点不强制 trim/独立成行；注释/CDATA/PI/DOCTYPE 保留。

### 编辑器接入文档

- README 新增 ttt 手动接入：通过 `~/.config/ttt/settings.json` 的 `lsp.servers` 配置 `lspd-ts` / `lspd-web` / `lspd-md`。
- README 与官网同步补齐 Markdown、TOML、XML、SVG、JSONC 的命令与语言映射示例。
- 暂不提供 ttt 插件：当前已确认手动 LSP server 配置可用，插件注册 LSP server 的 API 尚未确认。
