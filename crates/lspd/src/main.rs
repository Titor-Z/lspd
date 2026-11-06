//! `lspd` 可执行入口。
//!
//! 形态：同一个可执行文件，通过子命令决定本次进程装载哪些语言后端。
//! 编辑器按 languageId 分别启动 `lspd ts` 与 `lspd web`，跨语言分流由编辑器承担，
//! 因此本程序不需要在一个 stdio 上多路复用多种语言。

// 可选堆：实测 mimalloc 在 web 栈省约 1 MB，但在 TS 栈多约 3 MB（program 常驻，purge 无从回收）。
// 因此默认用系统 allocator，需要时由 feature 开启。
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc_safe::MiMalloc = mimalloc_safe::MiMalloc;

use std::process::ExitCode;

use lspd_core::Backend;

/// 帮助文本。列出子命令与选项，示例只保留编辑器真实会用的两种启动方式。
const USAGE: &str = "\
lspd — 低内存多语言语言服务器（LSP over stdio）

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

示例
  lspd ts
  lspd web
";

/// 语言子命令。取值与构建侧 feature 一一对应，决定本次进程装载哪些后端。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Subcommand {
    /// TypeScript 与 JavaScript
    Ts,
    /// 网页技术栈聚合：HTML + CSS + Sass + JSON
    Web,
    /// HTML
    Html,
    /// CSS / SCSS / Less
    Css,
    /// Sass 缩进语法
    Sass,
    /// JSON
    Json,
}

impl Subcommand {
    /// 解析子命令文本。未知名称返回 `None`，由调用方转为命令行错误。
    fn parse(text: &str) -> Option<Self> {
        match text {
            "ts" => Some(Self::Ts),
            "web" => Some(Self::Web),
            "html" => Some(Self::Html),
            "css" => Some(Self::Css),
            "sass" => Some(Self::Sass),
            "json" => Some(Self::Json),
            _ => None,
        }
    }

    /// 该子命令所需的编译期 feature。缺失时启动即报错，避免"静默不服务"。
    fn required_features(self) -> &'static [&'static str] {
        match self {
            Self::Ts => &["ts"],
            Self::Web => &["html", "css", "sass", "json"],
            Self::Html => &["html"],
            Self::Css => &["css"],
            Self::Sass => &["sass"],
            Self::Json => &["json"],
        }
    }
}

/// 命令行解析结果。三种终态互斥，避免用布尔标志堆叠出非法组合。
enum Cli {
    /// 打印帮助后正常退出
    Help,
    /// 打印版本后正常退出
    Version,
    /// 启动语言服务器，参数为子命令；`None` 表示本次构建包含的全部语言
    Serve(Option<Subcommand>),
}

/// 解析命令行参数。
///
/// 只接受一个位置参数（子命令）与若干开关；重复子命令、未知参数均报错，
/// 错误信息交由 `main` 打印用法后返回退出码 2。
fn parse_args<I: Iterator<Item = String>>(args: I) -> Result<Cli, String> {
    let mut subcommand: Option<Subcommand> = None;

    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Cli::Help),
            "--version" => return Ok(Cli::Version),
            // LSP 的通用传输就是 stdio，该开关为兼容客户端配置模板而接受
            "--stdio" => {}
            _ if arg.starts_with('-') => return Err(format!("未知选项 {arg}")),
            _ => {
                let Some(parsed) = Subcommand::parse(&arg) else {
                    return Err(format!("未知子命令 {arg}"));
                };
                if subcommand.replace(parsed).is_some() {
                    return Err("只能指定一个子命令".to_string());
                }
            }
        }
    }

    Ok(Cli::Serve(subcommand))
}

/// 本次构建实际启用的语言 feature。
///
/// 用于在启动阶段校验子命令与构建配置是否匹配，也是构建矩阵的运行时自证。
fn built_features() -> Vec<&'static str> {
    // `cfg!` 在编译期求值，因此无需在数组元素上挂属性即可逐个 feature 开关
    [
        cfg!(feature = "ts").then_some("ts"),
        cfg!(feature = "html").then_some("html"),
        cfg!(feature = "css").then_some("css"),
        cfg!(feature = "sass").then_some("sass"),
        cfg!(feature = "json").then_some("json"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// 按编译期 feature 组装后端列表。
///
/// 注册顺序决定“同一方法多个后端都能处理”时的优先级，
/// 具体语言能力由各语言 crate 的工厂函数提供。
fn backends() -> Vec<Box<dyn Backend>> {
    let mut list: Vec<Box<dyn Backend>> = Vec::new();

    #[cfg(feature = "html")]
    list.push(lspd_html::backend());

    #[cfg(feature = "css")]
    list.push(lspd_css::backend());

    #[cfg(feature = "sass")]
    list.push(lspd_sass::backend());

    #[cfg(feature = "json")]
    list.push(lspd_json::backend());

    list.shrink_to_fit();
    list
}

/// 启动语言服务器。
///
/// 先校验子命令与构建配置是否匹配：不匹配直接失败，避免“静默不服务”；
/// 匹配则把控制权交给核心层的事件循环。
fn serve(subcommand: Option<Subcommand>) -> ExitCode {
    let enabled = built_features();
    let required: Vec<&str> =
        subcommand.map_or_else(|| enabled.clone(), |sub| sub.required_features().to_vec());

    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|name| !enabled.contains(name))
        .collect();
    if !missing.is_empty() {
        eprintln!("lspd: 当前构建缺少 feature：{}", missing.join(", "));
        return ExitCode::from(2);
    }

    // TypeScript 有独立的持续驻留 program，必须单独进程、且不经过 core 的协议层：
    // tsrs_lsp 自己独占 stdin/stdout。
    #[cfg(feature = "ts")]
    if subcommand == Some(Subcommand::Ts) {
        return ExitCode::from(lspd_tsbridge::run() as u8);
    }

    ExitCode::from(lspd_core::run(backends()) as u8)
}

fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1)) {
        Ok(Cli::Help) => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Cli::Version) => {
            println!("lspd {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Cli::Serve(subcommand)) => serve(subcommand),
        Err(message) => {
            eprintln!("lspd: {message}");
            eprintln!();
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
