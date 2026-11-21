# Changelog

Unreleased (post-v0.3.0)

## 概述

补齐 YAML / Dockerfile 语言服务，并增强 JSON Schema 能力：支持本地 `$ref`/`$id`/组合关键字等常用子集，新增可控的远程 schema 加载（默认离线）。

## 新增

### YAML / Docker Compose

- `lspd yaml` / `lspd yml`：YAML 语法诊断、documentSymbol、foldingRange。
- schema 驱动：hover/completion/校验复用 `lspd-json` schema 子集；内置 Docker Compose schema（`docker-compose*.yml/yaml`、`compose*.yml/yaml`）。
- 第一版边界：单文档、常见标量/映射/序列；锚点/别名降级；多文档仅解析首文档并给诊断。

### Dockerfile

- `lspd dockerfile` / `lspd docker`：指令词表 hover/completion，基础诊断（未知指令、`MAINTAINER` 废弃、缺少 `FROM`）。
- 文件名路由：支持 `Dockerfile`、`Containerfile` 及 `.dockerfile` 扩展名；`lspd-core` 新增 `Backend::file_names()`。

### JSON Schema

- 引擎补全：本地 `$ref`（JSON Pointer、`$defs`/`definitions`）、`$id`/锚点、`allOf/anyOf/oneOf/not/if/then/else`、`patternProperties`、`dependentRequired/dependentSchemas`、`min/maxProperties`、`prefixItems/additionalItems`、`uniqueItems`、`contains/minContains/maxContains`、已知 `format` warning。
- `LoadedSchema + SchemaStore`：schema 文件缓存、外部 schema 保活、本地 `$ref` 基目录限制；本地 schema 文件支持 JSON/YAML。
- 远程 schema（可控）：默认离线；`LSPD_SCHEMA_REMOTE=1` 开启后仅允许 `https://`，拒绝 localhost/私网 IP，5s 超时、1MB 上限；磁盘缓存 `LSPD_SCHEMA_CACHE_DIR` 或 XDG cache；使用系统 `curl/wget` 拉取，Rust 代码无 C 依赖。`$schema` 未命中时后台拉取并写入缓存，后续打开/编辑命中；`$ref` 走同一策略。

## 修复

- `lspd-core`：`Document::line_end` 处理空最后行 EOF 偏移，避免补全越界 panic。

## 验证

- `cargo test --workspace` 全绿（3 ignored 不变）。
