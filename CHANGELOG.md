# Changelog

> 仅保留最新版本的发布内容，供 GitHub Actions 生成 Release 使用。以产品经理角度书写，去掉语气词。

## 未发布

- 建立项目骨架：`lspd` 单一可执行文件，按子命令与 feature 组合装载 TypeScript、HTML、CSS、Sass、JSON 语言服务。
- 语言模块按 crate 拆分：`lspd-core`（协议与文档）、`lspd-html`、`lspd-css`、`lspd-sass`、`lspd-json`、`lspd-tsbridge`。
- 协议为标准 LSP over stdio，OpenIDEA、Neovim、Helix、VSCode 共用同一二进制。
- HTML 语言服务：补全（标签、属性、属性值）、悬停、结构诊断、文档大纲、折叠、标签联动改名、id/class 的跳转与引用；
  打开 16 个 HTML 文档（1.16 MB）时常驻内存 4.8 MB，是同类 Node 实现的约 1/27。
