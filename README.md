# lspd

低内存多语言语言服务器。单可执行文件，LSP over stdio，按子命令装载语言后端。

支持 TypeScript / JavaScript，以及 HTML、CSS、SCSS、Less、Sass、JSON。

```
lspd web   # HTML / CSS / SCSS / Less / Sass / JSON
lspd ts    # TypeScript / JavaScript
```

## 安装

从 [Releases](https://github.com/Titor-Z/lspd/releases) 下载对应平台的压缩包，解压后得到单个 `lspd` 可执行文件。

| 平台 | 文件名 |
|---|---|
| macOS Apple Silicon | `lspd-vX.Y.Z-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `lspd-vX.Y.Z-x86_64-apple-darwin.tar.gz` |
| Linux x86_64 | `lspd-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` |

```bash
tar -xzf lspd-v0.1.0-x86_64-apple-darwin.tar.gz
shasum -a 256 -c lspd-v0.1.0-x86_64-apple-darwin.tar.gz.sha256
sudo mv lspd /usr/local/bin/
lspd --version
```

不需要运行时，不需要 Node、Python 或任何外部依赖。

## 为什么是它

同一台机器、同一份测试工程、同一套测量工具下的对照（详见各项目实测脚本）：

| 服务器 | 启动 | 空闲内存 | 打开工程后 |
|---|---|---|---|
| **lspd ts** | **1.3 ms** | **3.7 MB** | 16.7 MB |
| tsgo（官方 TypeScript 7） | 6.8 ms | 9.6 MB | — |
| typescript-language-server | 191 ms | 51.3 MB | 57.5 MB |
| vtsls | 311 ms | 60.2 MB | 64.9 MB |
| **lspd web** | **9.5 ms** | **2.7 MB** | 8.9 MB |
| vscode-html-language-server | 1093 ms | 137.6 MB | 133.4 MB |
| vscode-css-language-server | 508 ms | 72.1 MB | 115.1 MB |

TypeScript 侧移植自 [oxc-project/tsrs](https://github.com/oxc-project/tsrs)（TypeScript 7 的 Rust 实现），
因此在同样的输入下产出的诊断与官方实现一致。

## 命令行

```
用法
  lspd [子命令] [选项]

子命令
  （缺省）     本次构建包含的全部语言
  ts           TypeScript / JavaScript
  web          HTML + CSS + SCSS + Less + Sass + JSON
  html         HTML
  css          CSS / SCSS / Less
  sass         Sass（缩进语法）
  json         JSON

选项
  --stdio      使用标准输入输出，LSP 通用传输方式（默认，可省略）
  --version    打印版本号
  -h, --help   打印本帮助
```

---

# 在编辑器里使用

## 通用用法

`lspd` 是标准 LSP 服务器，通过 **stdio** 通信：编辑器启动它，然后用 stdin / stdout 收发
`Content-Length` 分帧的 JSON-RPC 消息。它不自带 UI，所有能力都由编辑器呈现。

### 一条重要约束

**`lspd ts` 与 `lspd web` 是两个独立进程，不会互相代理。**

这不是限制，而是设计：TypeScript 的语法树与类型信息跨文件常驻、无法按需释放，
Web 后端只跟随已打开文档，两者生命周期完全不同，放在一个进程里会让前者的内存成本
绑架后者。所以：

- 编辑 HTML / CSS 时启动一个 `lspd web`
- 编辑 TypeScript / JavaScript 时启动另一个 `lspd ts`
- 同一语言组内的所有文件复用同一个进程，不要每个文件启一个

### 语言与子命令对应关系

配置任何编辑器时，用的都是这张表：

| languageId | 典型扩展名 | 子命令 |
|---|---|---|
| `html` | `.html` `.htm` | `lspd web` |
| `css` | `.css` | `lspd web` |
| `scss` | `.scss` | `lspd web` |
| `less` | `.less` | `lspd web` |
| `sass` | `.sass` | `lspd web` |
| `json` / `jsonc` | `.json` `.jsonc` | `lspd web` |
| `typescript` | `.ts` | `lspd ts` |
| `typescriptreact` | `.tsx` | `lspd ts` |
| `javascript` | `.js` `.mjs` `.cjs` | `lspd ts` |
| `javascriptreact` | `.jsx` | `lspd ts` |

### 换到任何编辑器都能用的三条自查

下面所有编辑器的配置，本质都是同一件事。如果你的编辑器不在名单里，对照这三条判断能不能接：

1. **能填命令与参数吗** —— 需要能指定可执行文件 `lspd` 加上参数 `web` 或 `ts`（有的编辑器分两个字段，有的只收一整行命令）。
2. **能按语言绑定不同服务器吗** —— 需要让 HTML 走 `lspd web`、TS 走 `lspd ts`，而不是所有语言共用一个。
3. **传输是 stdio 吗** —— `lspd` 只支持 stdio，不支持 TCP 或 socket。

三条都满足，按上面的表填参数即可。只满足前两条而第三条是 socket，则无法使用。

### 用绝对路径

编辑器从图形界面启动时，拿到的 `$PATH` 往往和终端里不同。**建议一律填 `lspd` 的绝对路径**，
例如 `/usr/local/bin/lspd`，避免「终端里能跑、编辑器里报找不到」。

```bash
which lspd
```

---

## OpenIDEA

OpenIDEA **原生支持自定义语言服务器**，不需要装插件。

1. 打开 `Settings` → `Languages & Frameworks` → `Language Servers`
2. 点左上角的 `+`
3. 在「New Language Server」里填三个字段，建**两条**：

**条目一（Web）**

| 字段 | 值 |
|---|---|
| Command | `/usr/local/bin/lspd web` |
| Name | `lspd web` |
| Languages | HTML、CSS、SCSS、Less、Sass、JSON |

**条目二（TypeScript）**

| 字段 | 值 |
|---|---|
| Command | `/usr/local/bin/lspd ts` |
| Name | `lspd ts` |
| Languages | TypeScript、TypeScript React、JavaScript、JavaScript React |

要点：

- **Command 是一整行命令**，可执行文件和参数写在一起，中间用空格分隔。
- 右上角有 `Global` 与 `This project` 两个范围。`Global` 对所有工程生效，
  `This project` 只对当前工程生效；建议先用 `This project` 试通，再切 `Global`。
- 列表里服务器前面的标签表示状态：`ON PATH` 是已在 `$PATH` 中找到可执行文件；
  如果自定义条目没有这个标签，说明路径不对，换成绝对路径重填。
- OpenIDEA 内置了同类语言服务器（例如 CSS Language Server、TypeScript 相关项）。
  同一个语言只能由一个服务器处理，否则会出现两份补全互相打架。
  在列表里选中内置项，取消勾选「Use xxx for … files」，或者直接关掉它。

---

## Neovim

Neovim **≥ 0.8** 自带 LSP 客户端，不需要装 lspconfig，用 `vim.lsp.start` 即可。

在 `init.lua` 里加：

```lua
-- lspd 可执行文件的绝对路径
local lspd = "/usr/local/bin/lspd"

-- 按文件类型自动启动对应的语言服务器
local servers = {
  web = {
    name = "lspd-web",
    cmd = { lspd, "web" },
    filetypes = { "html", "css", "scss", "less", "sass", "json", "jsonc" },
  },
  ts = {
    name = "lspd-ts",
    cmd = { lspd, "ts" },
    filetypes = { "typescript", "typescriptreact", "javascript", "javascriptreact" },
  },
}

-- 把 filetype 反查成对应的服务器配置
local by_filetype = {}
for _, server in pairs(servers) do
  for _, filetype in ipairs(server.filetypes) do
    by_filetype[filetype] = server
  end
end

vim.api.nvim_create_autocmd("FileType", {
  callback = function(args)
    local server = by_filetype[vim.bo[args.buf].filetype]
    if not server then
      return
    end
    -- root_dir 取工程根：优先 .git，其次当前文件所在目录
    local root = vim.fs.dirname(vim.fs.find({ ".git" }, { upward = true })[1])
    vim.lsp.start({
      name = server.name,
      cmd = server.cmd,
      root_dir = root or vim.fn.getcwd(),
    }, { bufnr = args.buf })
  end,
})
```

验证：打开一个 `.ts` 文件，执行 `:LspInfo`（或 Neovim 0.11+ 的 `:checkhealth lsp`），
应能看到 `lspd-ts` 已挂载到该缓冲区。

### 用 lspconfig（可选）

如果已经在用 `nvim-lspconfig`，把它注册成自定义服务器也可以：

```lua
local configs = require("lspconfig.configs")
local lspconfig = require("lspconfig")

if not configs.lspd_ts then
  configs.lspd_ts = {
    default_config = {
      cmd = { "/usr/local/bin/lspd", "ts" },
      filetypes = { "typescript", "typescriptreact", "javascript", "javascriptreact" },
      root_dir = lspconfig.util.root_pattern(".git", "package.json", "tsconfig.json"),
    },
  }
end

lspconfig.lspd_ts.setup({})
```

`web` 组同理，把 `cmd` 换成 `{ "/usr/local/bin/lspd", "web" }`、`filetypes` 换成
`{ "html", "css", "scss", "less", "sass", "json" }` 即可。

---

## Helix

Helix **≥ 23.05** 支持在 `languages.toml` 里自定义语言服务器。

编辑 `~/.config/helix/languages.toml`：

```toml
[language-server.lspd-web]
command = "lspd"
args = ["web"]

[language-server.lspd-ts]
command = "lspd"
args = ["ts"]

[[language]]
name = "html"
language-servers = ["lspd-web"]

[[language]]
name = "css"
language-servers = ["lspd-web"]

[[language]]
name = "scss"
language-servers = ["lspd-web"]

[[language]]
name = "json"
language-servers = ["lspd-web"]

[[language]]
name = "typescript"
language-servers = ["lspd-ts"]

[[language]]
name = "tsx"
language-servers = ["lspd-ts"]

[[language]]
name = "javascript"
language-servers = ["lspd-ts"]

[[language]]
name = "jsx"
language-servers = ["lspd-ts"]
```

验证：`hx --health typescript`，在 Language Server 一栏应看到 `lspd-ts` 为 `✓`。

如果 `lspd` 不在 Helix 的 `$PATH` 里，把 `command` 改成绝对路径。

---

## VSCode

**VSCode 没有「填一行命令就能用」的内置入口**，这一点和 OpenIDEA 不同。它只认扩展。
两条可行路径：

### 路径一：社区通用 LSP 客户端

安装一个能自定义语言服务器的客户端扩展（例如 LSP 相关的桥接扩展），
在它的设置里把命令填成 `lspd web` / `lspd ts`，并按上面的语言表绑定。
具体设置项名称取决于所装扩展，以其文档为准。

### 路径二：写一个薄扩展（最可控）

新建扩展目录，`package.json` 声明语言与激活事件：

```json
{
  "name": "lspd-client",
  "version": "0.1.0",
  "engines": { "vscode": "^1.84.0" },
  "activationEvents": ["onLanguage:html", "onLanguage:typescript"],
  "main": "./out/extension.js",
  "dependencies": { "vscode-languageclient": "^9.0.1" },
  "contributes": {
    "languages": [
      { "id": "html", "extensions": [".html", ".htm"] },
      { "id": "typescript", "extensions": [".ts"] }
    ]
  }
}
```

`src/extension.ts` 的关键几行：

```ts
import { LanguageClient, ServerOptions, TransportKind } from "vscode-languageclient/node";

const lspd = "/usr/local/bin/lspd";

const webClient = new LanguageClient(
  "lspd-web",
  "lspd (Web)",
  { command: lspd, args: ["web"], transport: TransportKind.stdio } as ServerOptions,
  { documentSelector: ["html", "css", "scss", "less", "sass", "json"] }
);

const tsClient = new LanguageClient(
  "lspd-ts",
  "lspd (TypeScript)",
  { command: lspd, args: ["ts"], transport: TransportKind.stdio } as ServerOptions,
  { documentSelector: ["typescript", "typescriptreact", "javascript", "javascriptreact"] }
);

export async function activate() {
  await webClient.start();
  await tsClient.start();
}
```

`transport` 必须是 `TransportKind.stdio`。

### 必须处理的内置服务冲突

VSCode 自带 TypeScript、HTML、CSS 语言服务。两组服务同时运行会对同一文件给出两份结果，
表现为补全重复、诊断重复或互相覆盖。启动扩展前需要把内置服务关掉：

```json
{
  "html.validate.scripts": false,
  "css.validate": false,
  "typescript.validate.enable": false,
  "javascript.validate.enable": false
}
```

或者在扩展里用 `vscode.commands.executeCommand("typescript.restartTsServer")` 之外的方式禁用，
具体取决于你希望保留哪些内置能力。**如果只是试用，建议先只接 `lspd ts`，把内置 TS 校验关掉**，确认效果后再扩到 Web。

---

## Vim

Vim **本身没有内置 LSP 客户端**，必须装插件。常用两个：`vim-lsp` 与 `coc.nvim`。

### vim-lsp

```vim
if executable('lspd')
  augroup lspd
    autocmd!
    autocmd User lsp_setup call lsp#register_server({
      \ 'name': 'lspd-web',
      \ 'cmd': { server_info -> ['/usr/local/bin/lspd', 'web'] },
      \ 'allowlist': ['html', 'css', 'scss', 'less', 'sass', 'json'],
      \ })
    autocmd User lsp_setup call lsp#register_server({
      \ 'name': 'lspd-ts',
      \ 'cmd': { server_info -> ['/usr/local/bin/lspd', 'ts'] },
      \ 'allowlist': ['typescript', 'typescriptreact', 'javascript', 'javascriptreact'],
      \ })
  augroup END
endif
```

注意 `allowlist` 因 `vim-lsp` 版本不同可能叫 `whitelist`，以你安装的版本为准。

### coc.nvim

在 `coc-settings.json` 的 `languageserver` 字段里加：

```json
{
  "languageserver": {
    "lspd-web": {
      "command": "/usr/local/bin/lspd",
      "args": ["web"],
      "filetypes": ["html", "css", "scss", "less", "sass", "json"]
    },
    "lspd-ts": {
      "command": "/usr/local/bin/lspd",
      "args": ["ts"],
      "filetypes": ["typescript", "typescriptreact", "javascript", "javascriptreact"]
    }
  }
}
```

Vim 的 LSP 生态比 Neovim 落后一代（异步能力、诊断拉取支持都不如 Neovim），
**能换 Neovim 就换**。

---

## 排障

### 先确认二进制本身没问题

```bash
/usr/local/bin/lspd --version     # 应打印 lspd 0.1.0
/usr/local/bin/lspd web --help    # 应打印帮助
```

### 三类典型症状与原因

| 症状 | 原因 |
|---|---|
| 服务器进程启动后立刻退出 | 命令路径错，或参数写错（例如漏了 `web` / `ts`） |
| 补全、悬停没有任何反应 | 该语言没绑定到这个服务器，或绑定了但服务器没启动成功 |
| 补全正常但诊断一直不出现 | 客户端不支持拉取式诊断（见下） |

### 诊断是拉取式的

本服务器按 LSP 3.17 的 **pull 模式**提供诊断（`textDocument/diagnostic`），
不主动推送 `publishDiagnostics`。客户端必须支持拉取式诊断才能显示错误：

- Neovim ≥ 0.10：支持
- Helix：较新版本支持，旧版仅推送式，具体以 `hx --version` 对应文档为准
- OpenIDEA：按其版本支持情况为准
- VSCode：需要客户端显式实现 `textDocument/diagnostic`，普通扩展默认不拉取

如果你的编辑器只支持推送式诊断，补全与跳转可用，但不会显示错误标记。

### 看日志

| 编辑器 | 位置 |
|---|---|
| OpenIDEA | `Language Servers` 列表里的状态标签，以及 IDE 的日志面板 |
| Neovim | `:LspLog`，或 `~/.local/state/nvim/lsp.log` |
| Helix | `hx --health <language>` |
| VSCode | `输出` 面板，下拉选对应的语言服务器通道 |

## 许可

本仓库自有代码与 `compiler/` 目录分别适用不同许可，`compiler/` 源自上游
[oxc-project/tsrs](https://github.com/oxc-project/tsrs)，保留了其 `LICENSE` 与 `NOTICE`，
修改清单见 `compiler/NOTICE.md`。
