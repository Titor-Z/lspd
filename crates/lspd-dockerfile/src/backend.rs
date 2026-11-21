//! Dockerfile 后端：指令 hover/completion 与基础诊断。

use lspd_core::docs::{Document, DocumentStore, Position};
use lspd_core::text::{LineIndex, Span};
use lspd_core::{Backend, Outgoing, ResponseError};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::parser::{self, ParseResult};

mod level {
    pub const ERROR: u8 = 1;
    pub const WARNING: u8 = 2;
}

/// 指令元数据。
struct InstructionSpec {
    name: &'static str,
    detail: &'static str,
    doc: &'static str,
}

/// 内置 Dockerfile 指令词表。
const SPECS: &[InstructionSpec] = &[
    InstructionSpec { name: "ADD", detail: "添加文件/目录/URL", doc: "将源文件、目录或远程 URL 复制到镜像。支持自动解压本地 tar。通常优先考虑 `COPY`。" },
    InstructionSpec { name: "ARG", detail: "构建参数", doc: "定义构建时变量，可通过 `docker build --build-arg` 传入。" },
    InstructionSpec { name: "CMD", detail: "默认命令", doc: "指定容器启动时的默认命令或参数。一个 Dockerfile 只有最后一个 `CMD` 生效。" },
    InstructionSpec { name: "COPY", detail: "复制文件", doc: "从构建上下文复制文件或目录到镜像。比 `ADD` 语义更明确。" },
    InstructionSpec { name: "ENTRYPOINT", detail: "入口点", doc: "配置容器作为可执行程序运行时的入口命令。" },
    InstructionSpec { name: "ENV", detail: "环境变量", doc: "设置环境变量，构建期和运行期均可见。" },
    InstructionSpec { name: "EXPOSE", detail: "声明端口", doc: "声明容器运行时监听的端口。它是文档/元数据，不会自动发布端口。" },
    InstructionSpec { name: "FROM", detail: "基础镜像", doc: "设置构建的基础镜像。多阶段构建中可出现多次。" },
    InstructionSpec { name: "HEALTHCHECK", detail: "健康检查", doc: "定义容器健康检查命令，如 `HEALTHCHECK CMD curl -f http://localhost/ || exit 1`。" },
    InstructionSpec { name: "LABEL", detail: "元数据标签", doc: "为镜像添加键值对元数据。" },
    InstructionSpec { name: "MAINTAINER", detail: "维护者（已废弃）", doc: "旧的维护者字段，已废弃，建议使用 `LABEL org.opencontainers.image.authors=...`。" },
    InstructionSpec { name: "ONBUILD", detail: "下游构建触发器", doc: "为以此镜像为基础的下游构建添加触发指令。" },
    InstructionSpec { name: "RUN", detail: "执行命令", doc: "在构建过程中执行命令并提交结果层。建议合并相关命令减少层数。" },
    InstructionSpec { name: "SHELL", detail: "默认 shell", doc: "覆盖 shell 形式命令使用的默认 shell。" },
    InstructionSpec { name: "STOPSIGNAL", detail: "停止信号", doc: "设置容器退出时发送的系统调用信号。" },
    InstructionSpec { name: "USER", detail: "运行用户", doc: "设置后续 `RUN`、`CMD`、`ENTRYPOINT` 使用的用户/组。" },
    InstructionSpec { name: "VOLUME", detail: "挂载点", doc: "创建挂载点并标记为外部挂载卷。" },
    InstructionSpec { name: "WORKDIR", detail: "工作目录", doc: "设置后续指令的工作目录；目录不存在时会创建。" },
];

/// 创建 Dockerfile 后端。
pub fn backend() -> Box<dyn Backend> {
    Box::new(DockerfileBackend::default())
}

/// Dockerfile 后端。
#[derive(Default)]
pub struct DockerfileBackend {
    cache: Option<Cache>,
}

struct Cache {
    uri: String,
    version: i32,
    parsed: ParseResult,
}

impl DockerfileBackend {
    fn ensure(&mut self, document: &Document) {
        let fresh = self
            .cache
            .as_ref()
            .is_some_and(|c| c.uri == document.uri() && c.version == document.version());
        if fresh {
            return;
        }
        self.cache = Some(Cache {
            uri: document.uri().to_string(),
            version: document.version(),
            parsed: parser::parse(document.text()),
        });
    }

    fn publish(&mut self, document: &Document, out: &mut Vec<Outgoing>) {
        self.ensure(document);
        let cache = self.cache.as_ref().expect("cache");
        let lines = LineIndex::new(document.text());
        let diagnostics = diagnostics(document.text(), &cache.parsed, &lines);
        out.push(Outgoing::notify(
            "textDocument/publishDiagnostics",
            json!({
                "uri": document.uri(),
                "version": document.version(),
                "diagnostics": diagnostics,
            }),
        ));
    }
}

impl Backend for DockerfileBackend {
    fn name(&self) -> &'static str { "dockerfile" }
    fn language_ids(&self) -> &'static [&'static str] { &["dockerfile"] }
    fn file_extensions(&self) -> &'static [&'static str] { &[".dockerfile"] }
    fn file_names(&self) -> &'static [&'static str] { &["Dockerfile", "Containerfile"] }

    fn capabilities(&self) -> Value {
        json!({
            "completionProvider": { "triggerCharacters": [" ", "\n"] },
            "hoverProvider": true,
        })
    }

    fn did_open(&mut self, document: &Document, out: &mut Vec<Outgoing>) { self.publish(document, out); }
    fn did_change(&mut self, document: &Document, out: &mut Vec<Outgoing>) { self.publish(document, out); }
    fn did_close(&mut self, uri: &str, _out: &mut Vec<Outgoing>) {
        if self.cache.as_ref().is_some_and(|c| c.uri == uri) { self.cache = None; }
    }

    fn request(&mut self, method: &str, params: Option<&RawValue>, documents: &DocumentStore, _out: &mut Vec<Outgoing>) -> Option<Result<Value, ResponseError>> {
        match method {
            "textDocument/hover" => {
                let params: PositionParams = serde_json::from_str(params?.get()).ok()?;
                let document = documents.get(&params.text_document.uri)?;
                self.ensure(document);
                let cache = self.cache.as_ref().expect("cache");
                let offset = document.offset_at(params.position) as u32;
                let hover = parser::instruction_at(&cache.parsed, offset)
                    .and_then(|inst| spec(&inst.name))
                    .map(|spec| json!({
                        "contents": { "kind": "markdown", "value": format!("**{}** — {}\n\n{}", spec.name, spec.detail, spec.doc) }
                    }))
                    .unwrap_or(Value::Null);
                Some(Ok(hover))
            }
            "textDocument/completion" => {
                let params: PositionParams = serde_json::from_str(params?.get()).ok()?;
                let document = documents.get(&params.text_document.uri)?;
                let prefix = line_prefix(document.text(), document.offset_at(params.position));
                if !is_instruction_position(&prefix) {
                    return Some(Ok(json!({ "isIncomplete": false, "items": [] })));
                }
                let items: Vec<Value> = SPECS.iter().map(|s| json!({
                    "label": s.name,
                    "kind": 14,
                    "detail": s.detail,
                    "documentation": { "kind": "markdown", "value": s.doc },
                    "insertText": s.name,
                })).collect();
                Some(Ok(json!({ "isIncomplete": false, "items": items })))
            }
            _ => None,
        }
    }

    fn shutdown(&mut self) { self.cache = None; }
}

/// 查找指令元数据。
fn spec(name: &str) -> Option<&'static InstructionSpec> {
    SPECS.iter().find(|s| s.name == name)
}

/// 生成基础诊断。
fn diagnostics(source: &str, parsed: &ParseResult, lines: &LineIndex<'_>) -> Vec<Value> {
    let mut out = Vec::new();
    let mut has_from = false;
    for inst in &parsed.instructions {
        if inst.name == "FROM" { has_from = true; }
        if spec(&inst.name).is_none() {
            out.push(diag(lines, inst.name_span, level::WARNING, format!("未知 Dockerfile 指令 `{}`", inst.name)));
        }
        if inst.name == "MAINTAINER" {
            out.push(diag(lines, inst.name_span, level::WARNING, "MAINTAINER 已废弃，建议使用 LABEL org.opencontainers.image.authors".to_string()));
        }
    }
    if !parsed.instructions.is_empty() && !has_from {
        // 没有 FROM：提示，但 ARG 可在 FROM 前，这里只做温和警告
        let span = parsed.instructions[0].name_span;
        if parsed.instructions.iter().all(|i| i.name != "ARG") {
            out.push(diag(lines, span, level::ERROR, "Dockerfile 缺少 FROM 指令".to_string()));
        }
    }
    let _ = source;
    out
}

fn diag(lines: &LineIndex<'_>, span: Span, severity: u8, message: String) -> Value {
    let (sl, sc) = lines.position(span.start);
    let (el, ec) = lines.position(span.end);
    json!({"range":{"start":{"line":sl,"character":sc},"end":{"line":el,"character":ec}},"severity":severity,"source":"lspd","message":message})
}

/// 光标前的行前缀。
fn line_prefix(source: &str, offset: usize) -> String {
    let bytes = source.as_bytes();
    let mut start = offset.min(bytes.len());
    while start > 0 && bytes[start - 1] != b'\n' {
        start -= 1;
    }
    source[start..offset.min(source.len())].to_string()
}

/// 是否处于指令位置（行首空白后，或仅空白）。
fn is_instruction_position(prefix: &str) -> bool {
    let trimmed = prefix.trim_start();
    !prefix.contains('#')
        && (trimmed.is_empty() || trimmed.bytes().all(|b| b.is_ascii_alphabetic()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PositionParams { text_document: TextIdentifier, position: Position }
#[derive(Deserialize)]
struct TextIdentifier { uri: String }
