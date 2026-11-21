//! 离线 JSON Schema：加载本地/内置 schema，提供校验、补全与悬停。
//!
//! 优先使用文档 `$schema` 指向的本地文件；未声明时按文件名匹配内置 schema。
//! 只实现常用关键字子集：`type` / `enum` / `const` / `required` / `properties` /
//! `additionalProperties` / `items` / `minimum`–`maximum` / `minLength`–`maxLength` /
//! `pattern` / `description` / `markdownDescription` / `default`，以及本地 `$ref`
//!（`#/...` JSON Pointer，支持 `$defs`/`definitions`）。
//! 不解析 `$id`/远程引用、不访问网络。

use lspd_core::text::{LineIndex, Span};
use serde_json::{Value, json};

use crate::parser::{Value as Json, decode_string};

/// LSP 诊断严重程度。
mod level {
    pub const ERROR: u8 = 1;
    pub const WARNING: u8 = 2;
}

/// 内置 schema，构建期嵌入，运行时不读盘不联网。
const TSCONFIG_SCHEMA: &str = include_str!("../schemas/tsconfig.schema.json");
const PACKAGE_SCHEMA: &str = include_str!("../schemas/package.schema.json");
const VSCODE_SETTINGS_SCHEMA: &str = include_str!("../schemas/vscode-settings.schema.json");
const VSCODE_EXTENSIONS_SCHEMA: &str = include_str!("../schemas/vscode-extensions.schema.json");
const VSCODE_TASKS_SCHEMA: &str = include_str!("../schemas/vscode-tasks.schema.json");
const VSCODE_LAUNCH_SCHEMA: &str = include_str!("../schemas/vscode-launch.schema.json");
const CARGO_SCHEMA: &str = include_str!("../../lspd-toml/schemas/cargo.schema.json");
const DOCKER_COMPOSE_SCHEMA: &str = include_str!("../schemas/docker-compose.schema.json");


/// 读取并解析 schema 文件。
fn read_schema_file(path: &std::path::Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_schema_text(&text)
}

/// 解析 schema 文本：优先 JSON，失败则尝试 YAML（支持 yaml/yml schema 文件）。
fn parse_schema_text(text: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return Some(value);
    }
    let yaml: serde_yaml::Value = serde_yaml::from_str(text).ok()?;
    serde_json::to_value(yaml).ok()
}

/// 按文件名命中内置 schema。
fn builtin_schema(document_uri: &str) -> Option<Value> {
    let name = file_name(document_uri);
    let source = match name {
        _ if is_tsconfig_name(name) => TSCONFIG_SCHEMA,
        _ if is_docker_compose_name(name) => DOCKER_COMPOSE_SCHEMA,
        "package.json" => PACKAGE_SCHEMA,
        "settings.json" => VSCODE_SETTINGS_SCHEMA,
        "extensions.json" => VSCODE_EXTENSIONS_SCHEMA,
        "tasks.json" => VSCODE_TASKS_SCHEMA,
        "launch.json" => VSCODE_LAUNCH_SCHEMA,
        "Cargo.toml" => CARGO_SCHEMA,
        _ => return None,
    };

    serde_json::from_str::<Value>(source).ok()
}

/// tsconfig/jsconfig 家族文件名判断。
fn is_tsconfig_name(name: &str) -> bool {
    (name.starts_with("tsconfig") || name.starts_with("jsconfig")) && name.ends_with(".json")
}

/// Docker Compose 文件名判断。
fn is_docker_compose_name(name: &str) -> bool {
    matches!(
        name,
        "docker-compose.yml"
            | "docker-compose.yaml"
            | "compose.yml"
            | "compose.yaml"
    ) || (name.starts_with("docker-compose.") && (name.ends_with(".yml") || name.ends_with(".yaml")))
        || (name.starts_with("compose.") && (name.ends_with(".yml") || name.ends_with(".yaml")))
}

/// 从 URI/路径里取文件名。
fn file_name(document_uri: &str) -> &str {
    let path = document_uri
        .split(['?', '#'])
        .next()
        .unwrap_or(document_uri);
    path.rsplit('/').next().unwrap_or(path)
}

/// 取顶层对象里 `$schema` 的字符串值（已解码）。
fn schema_reference(root: &Json, source: &str) -> Option<String> {
    let Json::Object { members, .. } = root else {
        return None;
    };

    members.iter().find_map(|member| {
        if member.key_name != "$schema" {
            return None;
        }
        match &member.value {
            Json::String { content, .. } => Some(decode_string(source, *content)),
            _ => None,
        }
    })
}

/// 相对路径 / `file://` / 绝对路径都归一到文件系统路径；`http(s)` 返回 `None`。
fn resolve_path(reference: &str, document_uri: &str) -> Option<std::path::PathBuf> {
    let trimmed = reference.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return None;
    }
    let candidate = trimmed.strip_prefix("file://").unwrap_or(trimmed);
    let path = std::path::Path::new(candidate);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    let document_path = document_uri.strip_prefix("file://").unwrap_or(document_uri);
    let parent = std::path::Path::new(document_path).parent()?;
    Some(parent.join(path))
}

/// 已加载的 schema，包含根、基目录和 `$id` 索引。
#[derive(Debug, Clone)]
pub struct LoadedSchema {
    /// schema 根
    pub root: Value,
    /// schema 文件所在目录；内置 schema 为 `None`
    pub base: Option<std::path::PathBuf>,
    /// 文档内 `$id`/`id` -> JSON Pointer 索引
    pub ids: std::collections::HashMap<String, String>,
}

impl LoadedSchema {
    /// 从 JSON 值创建（无文件基目录）。
    pub fn from_value(root: Value) -> Self {
        let ids = collect_ids(&root);
        Self { root, base: None, ids }
    }
    /// 从文件创建。
    pub fn from_file(root: Value, path: &std::path::Path) -> Self {
        let ids = collect_ids(&root);
        Self { root, base: path.parent().map(|p| p.to_path_buf()), ids }
    }
}

/// schema 缓存配置。
#[derive(Debug, Clone)]
pub struct SchemaConfig {
    /// 是否允许获取远程 schema（默认关闭，保持离线确定性）。
    pub allow_remote: bool,
    /// 远程 schema 磁盘缓存目录。
    pub cache_dir: std::path::PathBuf,
}

impl Default for SchemaConfig {
    fn default() -> Self {
        let allow_remote = std::env::var("LSPD_SCHEMA_REMOTE")
            .map(|v| matches!(v.as_str(), "1" | "true" | "allow" | "yes"))
            .unwrap_or(false);
        let cache_dir = std::env::var("LSPD_SCHEMA_CACHE_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| default_cache_dir());
        Self { allow_remote, cache_dir }
    }
}

/// 默认缓存目录。
fn default_cache_dir() -> std::path::PathBuf {
    if let Some(base) = std::env::var_os("XDG_CACHE_HOME") {
        return std::path::PathBuf::from(base).join("lspd").join("schemas");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return std::path::PathBuf::from(home).join(".cache").join("lspd").join("schemas");
    }
    std::env::temp_dir().join("lspd-schemas")
}

/// schema 缓存，避免重复读盘/解析；进程内单线程使用 `RefCell`。
#[derive(Default)]
pub struct SchemaStore {
    files: std::cell::RefCell<std::collections::HashMap<std::path::PathBuf, std::rc::Rc<LoadedSchema>>>,
    remote: std::cell::RefCell<std::collections::HashMap<String, std::rc::Rc<LoadedSchema>>>,
    pending_remote: std::cell::RefCell<std::collections::HashSet<String>>,
    config: SchemaConfig,
}

impl SchemaStore {
    /// 创建空缓存。
    pub fn new() -> Self { Self::default() }
    /// 使用指定配置创建缓存。
    pub fn with_config(config: SchemaConfig) -> Self { Self { config, ..Self::default() } }
    /// 是否允许远程 schema。
    pub fn allow_remote(&self) -> bool { self.config.allow_remote }

    /// 加载/缓存本地 schema 文件。
    pub fn load_file(&self, path: &std::path::Path) -> Option<std::rc::Rc<LoadedSchema>> {
        let key = normalize_path(path);
        if let Some(hit) = self.files.borrow().get(&key).cloned() { return Some(hit); }
        let root = read_schema_file(&key)?;
        let schema = std::rc::Rc::new(LoadedSchema::from_file(root, &key));
        self.files.borrow_mut().insert(key, schema.clone());
        Some(schema)
    }

    /// 加载远程 schema：内存缓存 -> 内置映射 -> 磁盘缓存 ->（允许时）同步 HTTPS 获取。
    pub fn load_remote(&self, url: &str) -> Option<std::rc::Rc<LoadedSchema>> {
        let url = url.trim_end_matches('#');
        if let Some(hit) = self.remote.borrow().get(url).cloned() { return Some(hit); }
        if let Some(builtin) = builtin_schema_by_id(url) {
            let schema = std::rc::Rc::new(LoadedSchema::from_value(builtin));
            self.remote.borrow_mut().insert(url.to_string(), schema.clone());
            return Some(schema);
        }
        let cache_path = self.remote_cache_path(url);
        if let Some(root) = read_schema_file(&cache_path) {
            let schema = std::rc::Rc::new(LoadedSchema::from_value(root));
            self.remote.borrow_mut().insert(url.to_string(), schema.clone());
            return Some(schema);
        }
        if !self.config.allow_remote || !is_allowed_remote_url(url) {
            return None;
        }
        let bytes = fetch_remote_schema(url)?;
        let root = parse_schema_text(&String::from_utf8(bytes.clone()).ok()?)?;
        let _ = std::fs::create_dir_all(&self.config.cache_dir);
        let _ = std::fs::write(&cache_path, &bytes);
        let schema = std::rc::Rc::new(LoadedSchema::from_value(root));
        self.remote.borrow_mut().insert(url.to_string(), schema.clone());
        Some(schema)
    }

    /// 已缓存的远程 schema（不触发网络）。
    pub fn peek_remote(&self, url: &str) -> Option<std::rc::Rc<LoadedSchema>> {
        let url = url.trim_end_matches('#');
        if let Some(hit) = self.remote.borrow().get(url).cloned() { return Some(hit); }
        if let Some(builtin) = builtin_schema_by_id(url) {
            let schema = std::rc::Rc::new(LoadedSchema::from_value(builtin));
            self.remote.borrow_mut().insert(url.to_string(), schema.clone());
            return Some(schema);
        }
        let cache_path = self.remote_cache_path(url);
        let root = read_schema_file(&cache_path)?;
        let schema = std::rc::Rc::new(LoadedSchema::from_value(root));
        self.remote.borrow_mut().insert(url.to_string(), schema.clone());
        Some(schema)
    }

    /// 调度后台获取远程 schema；不阻塞。成功后写入磁盘缓存，后续请求可命中。
    pub fn schedule_remote_fetch(&self, url: String) {
        let url = url.trim_end_matches('#').to_string();
        if self.peek_remote(&url).is_some() { return; }
        if !self.config.allow_remote || !is_allowed_remote_url(&url) { return; }
        {
            let mut pending = self.pending_remote.borrow_mut();
            if pending.contains(&url) { return; }
            pending.insert(url.clone());
        }
        let cache_dir = self.config.cache_dir.clone();
        std::thread::spawn(move || {
            let _ = fetch_remote_schema_to_cache(&url, &cache_dir);
        });
    }

    /// 远程 URL 对应的磁盘缓存路径。
    fn remote_cache_path(&self, url: &str) -> std::path::PathBuf {
        let mut hash: u64 = 0xcbf29ce484222325;
        for b in url.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        self.config.cache_dir.join(format!("{hash:016x}.json"))
    }
}

/// 远程 schema URL 安全检查：仅允许 https，拒绝本地/私网地址。
fn is_allowed_remote_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false; };
    let host = rest.split(['/', ':', '?', '#']).next().unwrap_or("");
    let host = host.to_ascii_lowercase();
    if host.is_empty() || host == "localhost" || host.ends_with(".local") { return false; }
    if host.parse::<std::net::IpAddr>().is_ok_and(|ip| is_private_ip(&ip)) { return false; }
    true
}

/// 判断是否为私网/本地 IP。
fn is_private_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || o[0] == 0 || (o[0] == 100 && (64..128).contains(&o[1]))
        }
        std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unspecified(),
    }
}

/// 通过系统 curl/wget 获取远程 schema（纯 Rust 代码，无 C 依赖）。
fn fetch_remote_schema(url: &str) -> Option<Vec<u8>> {
    const MAX_BYTES: usize = 1024 * 1024;
    let attempts: Vec<(&str, Vec<String>)> = vec![
        ("curl", vec!["-fsSL".into(), "--max-time".into(), "5".into(), "--max-filesize".into(), MAX_BYTES.to_string(), url.into()]),
        ("wget", vec!["-qO-".into(), "--timeout=5".into(), url.into()]),
    ];
    for (cmd, args) in attempts {
        let Ok(output) = std::process::Command::new(cmd).args(&args).output() else { continue; };
        if !output.status.success() { continue; }
        if output.stdout.len() > MAX_BYTES { return None; }
        return Some(output.stdout);
    }
    None
}

/// 拉取远程 schema 并写入磁盘缓存。
fn fetch_remote_schema_to_cache(url: &str, cache_dir: &std::path::Path) -> bool {
    let Some(bytes) = fetch_remote_schema(url) else { return false; };
    let Ok(text) = String::from_utf8(bytes.clone()) else { return false; };
    if parse_schema_text(&text).is_none() { return false; }
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in url.trim_end_matches('#').as_bytes() { hash ^= *b as u64; hash = hash.wrapping_mul(0x100000001b3); }
    let path = cache_dir.join(format!("{hash:016x}.json"));
    let _ = std::fs::create_dir_all(cache_dir);
    std::fs::write(path, bytes).is_ok()
}

/// 路径归一化（不做 symlink resolve，避免越权；只清理 `.`/`..`）。
fn normalize_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => { out.pop(); }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() { path.to_path_buf() } else { out }
}

/// 收集 schema 文档内 `$id`/`id` 到 JSON Pointer 的映射。
fn collect_ids(root: &Value) -> std::collections::HashMap<String, String> {
    let mut ids = std::collections::HashMap::new();
    collect_ids_at(root, "", &mut ids);
    ids
}
fn collect_ids_at(value: &Value, pointer: &str, ids: &mut std::collections::HashMap<String, String>) {
    let Value::Object(map) = value else { return; };
    let id = map.get("$id").or_else(|| map.get("id")).and_then(Value::as_str);
    if let Some(id) = id {
        let raw = id.to_string();
        let normalized = raw.trim_end_matches('#').to_string();
        if !normalized.is_empty() {
            ids.insert(normalized.clone(), pointer.to_string());
        }
        // 支持 `#name` 形式的本地锚点
        if let Some(anchor) = raw.strip_prefix('#') {
            if !anchor.is_empty() {
                ids.insert(anchor.to_string(), pointer.to_string());
                ids.insert(raw.clone(), pointer.to_string());
            }
        } else if !raw.is_empty() {
            ids.insert(raw.clone(), pointer.to_string());
        }
    }
    for (k, v) in map {
        let child = if pointer.is_empty() { format!("/{k}") } else { format!("{pointer}/{k}") };
        collect_ids_at(v, &child, ids);
    }
    if let Some(Value::Array(items)) = map.get("prefixItems").or(map.get("items")) {
        let base = if pointer.is_empty() { "/prefixItems".to_string() } else { format!("{pointer}/prefixItems") };
        for (i, item) in items.iter().enumerate() { collect_ids_at(item, &format!("{base}/{i}"), ids); }
    }
}

/// 兼容旧 API：只返回 schema 根值。
pub fn load(source: &str, root: &Json, document_uri: &str) -> Option<Value> {
    load_schema(source, root, document_uri).map(|s| s.root)
}

/// 从根节点 `$schema` 或文件名映射加载 schema，并保留基目录。
pub fn load_schema(source: &str, root: &Json, document_uri: &str) -> Option<LoadedSchema> {
    let store = SchemaStore::new();
    load_schema_with_store(source, root, document_uri, &store)
}

/// 使用外部缓存加载 schema。
pub fn load_schema_with_store(
    source: &str,
    root: &Json,
    document_uri: &str,
    store: &SchemaStore,
) -> Option<LoadedSchema> {
    if let Some(reference) = schema_reference(root, source) {
        let trimmed = reference.trim();
        if let Some(path) = resolve_path(trimmed, document_uri) {
            return store.load_file(&path).map(|s| (*s).clone());
        }
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            return store.load_remote(trimmed).map(|s| (*s).clone());
        }
        return None;
    }
    builtin_schema(document_uri).map(LoadedSchema::from_value)
}

/// 依据 schema 校验整篇文档，产出 LSP 诊断。
pub fn validate(source: &str, root: &Json, schema: &Value, lines: &LineIndex<'_>) -> Vec<Value> {
    let loaded = LoadedSchema::from_value(schema.clone());
    let store = SchemaStore::new();
    validate_with_store(source, root, &loaded, &store, lines)
}

/// 使用已加载 schema 校验（支持本地文件 `$ref`）。
pub fn validate_loaded(
    source: &str,
    root: &Json,
    schema: &LoadedSchema,
    lines: &LineIndex<'_>,
) -> Vec<Value> {
    let store = SchemaStore::new();
    validate_with_store(source, root, schema, &store, lines)
}

/// 使用缓存校验。
pub fn validate_with_store(
    source: &str,
    root: &Json,
    schema: &LoadedSchema,
    store: &SchemaStore,
    lines: &LineIndex<'_>,
) -> Vec<Value> {
    let mut diagnostics = Vec::new();
    let ctx = Ctx::new(schema, store);
    check(source, root, &schema.root, &ctx, lines, &mut diagnostics);
    diagnostics
}

/// LSP CompletionItemKind：对象键按 Property，枚举/字面量按 Value。
const ITEM_PROPERTY: u8 = 10;
const ITEM_VALUE: u8 = 12;

/// 依据 schema 给出补全项：对象体里补键名，冒号后补枚举/布尔值。
pub fn completion(source: &str, root: &Json, schema: Option<&Value>, offset: u32) -> Vec<Value> {
    let Some(schema) = schema else { return Vec::new(); };
    let loaded = LoadedSchema::from_value(schema.clone());
    let store = SchemaStore::new();
    completion_with_store(source, root, &loaded, &store, offset)
}

/// 使用已加载 schema 给出补全项。
pub fn completion_loaded(source: &str, root: &Json, schema: &LoadedSchema, offset: u32) -> Vec<Value> {
    let store = SchemaStore::new();
    completion_with_store(source, root, schema, &store, offset)
}

/// 使用缓存给出补全项。
pub fn completion_with_store(source: &str, root: &Json, schema: &LoadedSchema, store: &SchemaStore, offset: u32) -> Vec<Value> {
    let ctx = Ctx::new(schema, store);
    let located = locate(root, &schema.root, &ctx, offset);
    let line_start = start_of_line(source, offset);
    match located.node {
        Json::Object { members, .. } => {
            let value_member = members.iter().find(|member| {
                let value = member.value.span();
                member.key.start >= line_start && offset > member.key.end && offset <= value.end
            });
            if let Some(member) = value_member {
                let sub = property_schema(located.schema, &ctx, &member.key_name);
                return value_items(sub, &ctx);
            }
            key_items(located.schema, &ctx, members, located.node.span() == root.span())
        }
        Json::Array { .. } => {
            let sub = resolve_ref(located.schema.get("items").unwrap_or(&Value::Null), &ctx);
            value_items(sub, &ctx)
        }
        _ => value_items(located.schema, &ctx),
    }
}

/// 依据 schema 给出悬停信息：键给属性说明，值给类型/默认值/枚举说明。
pub fn hover(root: &Json, schema: Option<&Value>, offset: u32) -> Option<String> {
    let schema = schema?;
    let loaded = LoadedSchema::from_value(schema.clone());
    let store = SchemaStore::new();
    hover_with_store(root, &loaded, &store, offset)
}

/// 使用已加载 schema 给出悬停信息。
pub fn hover_loaded(root: &Json, schema: &LoadedSchema, offset: u32) -> Option<String> {
    let store = SchemaStore::new();
    hover_with_store(root, schema, &store, offset)
}

/// 使用缓存给出悬停信息。
pub fn hover_with_store(root: &Json, schema: &LoadedSchema, store: &SchemaStore, offset: u32) -> Option<String> {
    let ctx = Ctx::new(schema, store);
    let located = locate(root, &schema.root, &ctx, offset);
    match located.kind {
        HitKind::Key => hover_for_key(located.key_name?, located.schema),
        HitKind::Value => hover_for_value(located.key_name, located.schema),
    }
}

/// 生成键悬停内容。
fn hover_for_key(key: &str, schema: &Value) -> Option<String> {
    if !schema.is_object() || schema.as_object().is_some_and(|item| item.is_empty()) { return None; }
    let mut lines = vec![format!("`{key}`")];
    push_schema_details(&mut lines, schema);
    Some(lines.join("\n\n"))
}

/// 生成值悬停内容。
fn hover_for_value(key: Option<&str>, schema: &Value) -> Option<String> {
    if !schema.is_object() || schema.as_object().is_some_and(|item| item.is_empty()) { return None; }
    let mut lines = Vec::new();
    if let Some(key) = key { lines.push(format!("`{key}`")); }
    push_schema_details(&mut lines, schema);
    (lines.len() > usize::from(key.is_some())).then(|| lines.join("\n\n"))
}

/// 把 schema 的说明性字段追加到悬停文本。
fn push_schema_details(lines: &mut Vec<String>, schema: &Value) {
    let description = schema.get("markdownDescription").or_else(|| schema.get("description")).and_then(Value::as_str);
    if let Some(description) = description { lines.push(description.to_string()); }
    if let Some(types) = schema.get("type") {
        let text = describe_type(types);
        if !text.is_empty() { lines.push(format!("类型：`{text}`")); }
    }
    if let Some(default) = schema.get("default") { lines.push(format!("默认值：`{}`", compact_json(default))); }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        let values: Vec<String> = allowed.iter().map(compact_json).collect();
        if !values.is_empty() { lines.push(format!("可选值：{}", values.join(", "))); }
    }
    if let Some(format) = schema.get("format").and_then(Value::as_str) { lines.push(format!("格式：`{format}`")); }
    if schema.get("deprecated").and_then(Value::as_bool) == Some(true) { lines.push("**已废弃**".to_string()); }
    if schema.get("readOnly").and_then(Value::as_bool) == Some(true) { lines.push("只读".to_string()); }
}

/// 把 JSON 值压成简短文本，供 hover 展示。
fn compact_json(value: &Value) -> String {
    match value {
        Value::String(text) => format!("\"{text}\""),
        _ => value.to_string(),
    }
}

/// schema 解析上下文：根 schema、缓存与外部文档。
struct Ctx<'a> {
    schema: &'a LoadedSchema,
    store: &'a SchemaStore,
    /// 外部 schema 保活，保证返回的 `&Value` 在 ctx 生命周期内有效。
    external: std::cell::RefCell<Vec<std::rc::Rc<LoadedSchema>>>,
    external_index: std::cell::RefCell<std::collections::HashMap<std::path::PathBuf, usize>>,
    remote_index: std::cell::RefCell<std::collections::HashMap<String, usize>>,
}

impl<'a> Ctx<'a> {
    fn new(schema: &'a LoadedSchema, store: &'a SchemaStore) -> Self {
        Self {
            schema,
            store,
            external: std::cell::RefCell::new(Vec::new()),
            external_index: std::cell::RefCell::new(std::collections::HashMap::new()),
            remote_index: std::cell::RefCell::new(std::collections::HashMap::new()),
        }
    }
    fn root(&self) -> &'a Value { &self.schema.root }
    /// 加载外部本地 schema，返回生命周期绑定到 ctx 的引用。
    fn load_external(&'a self, path: &std::path::Path) -> Option<&'a LoadedSchema> {
        let key = normalize_path(path);
        if let Some(&idx) = self.external_index.borrow().get(&key) {
            let ext = self.external.borrow();
            let rc = ext.get(idx)?;
            return Some(unsafe { &*((&**rc) as *const LoadedSchema) });
        }
        let loaded = self.store.load_file(&key)?;
        let mut ext = self.external.borrow_mut();
        let idx = ext.len();
        ext.push(loaded);
        self.external_index.borrow_mut().insert(key, idx);
        let rc = ext.get(idx)?;
        Some(unsafe { &*((&**rc) as *const LoadedSchema) })
    }
    /// 加载外部远程 schema，返回生命周期绑定到 ctx 的引用。
    fn load_remote_ref(&'a self, url: &str) -> Option<&'a LoadedSchema> {
        let url = url.trim_end_matches('#');
        if let Some(&idx) = self.remote_index.borrow().get(url) {
            let ext = self.external.borrow();
            let rc = ext.get(idx)?;
            return Some(unsafe { &*((&**rc) as *const LoadedSchema) });
        }
        let loaded = self.store.load_remote(url)?;
        let mut ext = self.external.borrow_mut();
        let idx = ext.len();
        ext.push(loaded);
        self.remote_index.borrow_mut().insert(url.to_string(), idx);
        let rc = ext.get(idx)?;
        Some(unsafe { &*((&**rc) as *const LoadedSchema) })
    }
}

/// 解析 `$ref`，返回目标 schema；无法解析或超过深度时返回原值。
fn resolve_ref<'a>(schema: &'a Value, ctx: &'a Ctx<'a>) -> &'a Value {
    resolve_ref_depth(schema, ctx, 0)
}

/// 带深度限制的 `$ref` 解析，避免循环引用。
fn resolve_ref_depth<'a>(schema: &'a Value, ctx: &'a Ctx<'a>, depth: u8) -> &'a Value {
    if depth > 16 { return schema; }
    let Some(reference) = schema.get("$ref").and_then(Value::as_str) else { return schema; };
    let Some(target) = resolve_ref_target(ctx, reference) else { return schema; };
    resolve_ref_depth(target, ctx, depth + 1)
}

/// 解析 `$ref` 目标：支持 `#/...`、`$id`、相对/绝对本地文件（含 fragment）。
fn resolve_ref_target<'a>(ctx: &'a Ctx<'a>, reference: &str) -> Option<&'a Value> {
    let (file, fragment) = reference.split_once('#').unwrap_or((reference, ""));
    if file.is_empty() {
        return resolve_pointer_or_id(ctx.root(), &ctx.schema.ids, fragment);
    }

    if file.starts_with("http://") || file.starts_with("https://") {
        let loaded = ctx.load_remote_ref(file)?;
        return resolve_pointer_or_id(&loaded.root, &loaded.ids, fragment);
    }

    let id_key = if fragment.is_empty() { file.to_string() } else { format!("{file}#{fragment}") };
    if let Some(pointer) = ctx.schema.ids.get(&id_key).or_else(|| ctx.schema.ids.get(file)) {
        let target = resolve_pointer(ctx.root(), pointer.trim_start_matches('#')).or(Some(ctx.root()))?;
        return if fragment.is_empty() || ctx.schema.ids.contains_key(&id_key) {
            Some(target)
        } else {
            resolve_pointer_or_id(target, &collect_ids(target), fragment)
        };
    }

    let base = ctx.schema.base.as_deref()?;
    let candidate = file.strip_prefix("file://").unwrap_or(file);
    let path = std::path::Path::new(candidate);
    let path = if path.is_absolute() { path.to_path_buf() } else { base.join(path) };
    let path = normalize_path(&path);
    if !is_allowed_ref_path(base, &path) { return None; }
    let loaded = ctx.load_external(&path)?;
    resolve_pointer_or_id(&loaded.root, &loaded.ids, fragment)
}

/// 限制本地 `$ref` 只能读取 schema 基目录下的文件，避免任意文件读取。
fn is_allowed_ref_path(base: &std::path::Path, target: &std::path::Path) -> bool {
    let Ok(base) = base.canonicalize() else { return target.starts_with(base); };
    let Ok(target) = target.canonicalize() else { return false; };
    target.starts_with(base)
}

/// 按 fragment 或 `$id` 解析。
fn resolve_pointer_or_id<'a>(root: &'a Value, ids: &std::collections::HashMap<String, String>, fragment: &str) -> Option<&'a Value> {
    if fragment.is_empty() { return Some(root); }
    if !fragment.starts_with('/') {
        if let Some(pointer) = ids.get(fragment).or_else(|| ids.get(&format!("#{fragment}"))) {
            return resolve_pointer(root, pointer.trim_start_matches('#')).or(Some(root));
        }
    }
    resolve_pointer(root, fragment)
}

/// 按 JSON Pointer fragment 解析（空 fragment 返回根）。
fn resolve_pointer<'a>(root: &'a Value, fragment: &str) -> Option<&'a Value> {
    if fragment.is_empty() { return Some(root); }
    let fragment = fragment.strip_prefix('/')?;
    let mut current = root;
    for raw in fragment.split('/') {
        let key = raw.replace("~1", "/").replace("~0", "~");
        current = match current {
            Value::Object(map) => map.get(&key)?,
            Value::Array(items) => items.get(key.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

/// 通过 `$schema`/`$id` URL 映射到内置 schema（离线）。
fn builtin_schema_by_id(id: &str) -> Option<Value> {
    let id = id.trim_end_matches('#');
    let source = if id.contains("json-schema.org/draft-07/schema") {
        // draft-07 元 schema 子集：允许任意对象，不做强校验
        return Some(json!({"type":"object"}));
    } else if id.contains("schemastore.org") && id.contains("tsconfig") { TSCONFIG_SCHEMA }
    else if id.contains("schemastore.org") && id.contains("package") { PACKAGE_SCHEMA }
    else { return None; };
    serde_json::from_str(source).ok()
}

/// 命中的位置：键、值或容器自身。
#[derive(Debug, Clone, Copy)]
enum HitKind { Key, Value }

/// 下钻结果。
struct Located<'s> {
    node: &'s Json,
    schema: &'s Value,
    kind: HitKind,
    key_name: Option<&'s str>,
}

/// 取对象成员对应的子 schema。
fn property_schema<'s>(schema: &'s Value, ctx: &'s Ctx<'s>, key: &str) -> &'s Value {
    let schema = resolve_ref(schema, ctx);
    if let Some(sub) = schema.get("properties").and_then(|p| p.get(key)) {
        return resolve_ref(sub, ctx);
    }
    if let Some(props) = schema.get("patternProperties").and_then(Value::as_object) {
        for (pattern, sub) in props {
            if regex_lite::Regex::new(pattern).ok().is_some_and(|re| re.is_match(key)) {
                return resolve_ref(sub, ctx);
            }
        }
    }
    match schema.get("additionalProperties") {
        Some(sub) if sub.is_object() => resolve_ref(sub, ctx),
        _ => &Value::Null,
    }
}

/// 下钻到包含 `offset` 的最深节点，并同步带上其子 schema。
fn locate<'s>(node: &'s Json, schema: &'s Value, ctx: &'s Ctx<'s>, offset: u32) -> Located<'s> {
    let schema = resolve_ref(schema, ctx);
    match node {
        Json::Object { members, .. } => {
            for member in members {
                let sub = property_schema(schema, ctx, &member.key_name);
                if member.key.contains(offset) {
                    return Located { node: &member.value, schema: sub, kind: HitKind::Key, key_name: Some(&member.key_name) };
                }
                if member.value.span().contains(offset) {
                    if matches!(member.value, Json::Object { .. } | Json::Array { .. }) {
                        return locate(&member.value, sub, ctx, offset);
                    }
                    return Located { node: &member.value, schema: sub, kind: HitKind::Value, key_name: Some(&member.key_name) };
                }
            }
            Located { node, schema, kind: HitKind::Value, key_name: None }
        }
        Json::Array { items, .. } => {
            let sub = item_schema(schema, ctx, 0);
            for (i, item) in items.iter().enumerate() {
                let sub = item_schema(schema, ctx, i);
                if item.span().contains(offset) {
                    if matches!(item, Json::Object { .. } | Json::Array { .. }) {
                        return locate(item, sub, ctx, offset);
                    }
                    return Located { node: item, schema: sub, kind: HitKind::Value, key_name: None };
                }
            }
            let _ = sub;
            Located { node, schema, kind: HitKind::Value, key_name: None }
        }
        _ => Located { node, schema, kind: HitKind::Value, key_name: None },
    }
}

/// 取数组第 `index` 项对应的 schema。
fn item_schema<'s>(schema: &'s Value, ctx: &'s Ctx<'s>, index: usize) -> &'s Value {
    let schema = resolve_ref(schema, ctx);
    if let Some(Value::Array(tuple)) = schema.get("prefixItems").or_else(|| schema.get("items").filter(|v| v.is_array())) {
        if let Some(sub) = tuple.get(index) { return resolve_ref(sub, ctx); }
        return match schema.get("additionalItems").or_else(|| schema.get("items").filter(|v| v.is_object())) {
            Some(sub) if sub.is_object() => resolve_ref(sub, ctx),
            _ => &Value::Null,
        };
    }
    schema.get("items").map(|s| resolve_ref(s, ctx)).unwrap_or(&Value::Null)
}

/// 对象键补全：schema 的 properties 中未出现的键；根对象额外给 `$schema`。
fn key_items(
    schema: &Value,
    ctx: &Ctx<'_>,
    members: &[crate::parser::Member],
    is_root: bool,
) -> Vec<Value> {
    let mut items = Vec::new();
    let present: Vec<&str> = members.iter().map(|m| m.key_name.as_str()).collect();
    let schema = resolve_ref(schema, ctx);
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (key, sub) in properties {
            if present.contains(&key.as_str()) { continue; }
            let sub = resolve_ref(sub, ctx);
            let mut item = json!({
                "label": key,
                "kind": ITEM_PROPERTY,
                "insertText": format!("\"{key}\": "),
            });
            let desc = sub.get("markdownDescription").or_else(|| sub.get("description")).and_then(Value::as_str);
            if let Some(desc) = desc { item["documentation"] = json!({"kind":"markdown","value":desc}); }
            if sub.get("deprecated").and_then(Value::as_bool) == Some(true) { item["deprecated"] = json!(true); }
            items.push(item);
        }
    }
    // anyOf/oneOf 分支里的 properties 也作为候选补全
    for key in ["anyOf", "oneOf"] {
        if let Some(variants) = schema.get(key).and_then(Value::as_array) {
            for variant in variants {
                let variant = resolve_ref(variant, ctx);
                if let Some(properties) = variant.get("properties").and_then(Value::as_object) {
                    for (k, sub) in properties {
                        if present.contains(&k.as_str()) || items.iter().any(|i| i["label"].as_str()==Some(k)) { continue; }
                        let sub = resolve_ref(sub, ctx);
                        let mut item = json!({"label": k, "kind": ITEM_PROPERTY, "insertText": format!("\"{k}\": ")});
                        let desc = sub.get("markdownDescription").or_else(|| sub.get("description")).and_then(Value::as_str);
                        if let Some(desc) = desc { item["documentation"] = json!({"kind":"markdown","value":desc}); }
                        items.push(item);
                    }
                }
            }
        }
    }
    if is_root && !present.contains(&"$schema") {
        items.push(json!({ "label": "$schema", "kind": ITEM_PROPERTY, "insertText": "\"$schema\": " }));
    }
    items
}

/// 值补全：枚举值、布尔字面量、默认值。
fn value_items(schema: &Value, _ctx: &Ctx<'_>) -> Vec<Value> {
    let mut items = Vec::new();
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        for option in allowed {
            let (label, insert) = match option {
                Value::String(text) => (text.clone(), format!("\"{text}\"")),
                other => (other.to_string(), other.to_string()),
            };
            items.push(json!({ "label": label, "kind": ITEM_VALUE, "insertText": insert }));
        }
    }
    if let Some(constant) = schema.get("const") {
        let (label, insert) = match constant {
            Value::String(text) => (text.clone(), format!("\"{text}\"")),
            other => (other.to_string(), other.to_string()),
        };
        if !items.iter().any(|i| i["label"].as_str() == Some(&label)) {
            items.push(json!({ "label": label, "kind": ITEM_VALUE, "insertText": insert }));
        }
    }
    if items.is_empty() {
        let types: Vec<&str> = match schema.get("type") {
            Some(Value::String(s)) => vec![s.as_str()],
            Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if types.contains(&"boolean") {
            items.push(json!({ "label": "true", "kind": ITEM_VALUE, "insertText": "true" }));
            items.push(json!({ "label": "false", "kind": ITEM_VALUE, "insertText": "false" }));
        }
        if types.contains(&"null") {
            items.push(json!({ "label": "null", "kind": ITEM_VALUE, "insertText": "null" }));
        }
    }
    items
}

/// 行首字节偏移。
fn start_of_line(source: &str, offset: u32) -> u32 {
    let bytes = source.as_bytes();
    let mut index = (offset as usize).min(bytes.len());
    while index > 0 && bytes[index - 1] != b'\n' {
        index -= 1;
    }
    index as u32
}

/// 递归校验一个节点。
fn check(
    source: &str,
    node: &Json,
    schema: &Value,
    ctx: &Ctx<'_>,
    lines: &LineIndex<'_>,
    out: &mut Vec<Value>,
) {
    let schema = resolve_ref(schema, ctx);
    if !schema.is_object() { return; }

    // 组合 schema
    if let Some(subs) = schema.get("allOf").and_then(Value::as_array) {
        for sub in subs { check(source, node, sub, ctx, lines, out); }
    }
    if let Some(subs) = schema.get("anyOf").and_then(Value::as_array) {
        if !subs.is_empty() && !subs.iter().any(|sub| matches_schema(source, node, sub, ctx)) {
            out.push(diagnostic(lines, node.span(), level::ERROR, "取值不匹配 anyOf 中的任一 schema"));
        }
    }
    if let Some(subs) = schema.get("oneOf").and_then(Value::as_array) {
        let count = subs.iter().filter(|sub| matches_schema(source, node, sub, ctx)).count();
        if count == 0 { out.push(diagnostic(lines, node.span(), level::ERROR, "取值不匹配 oneOf 中的任何 schema")); }
        else if count > 1 { out.push(diagnostic(lines, node.span(), level::ERROR, "取值匹配 oneOf 中多个 schema")); }
    }
    if let Some(sub) = schema.get("not") {
        if matches_schema(source, node, sub, ctx) {
            out.push(diagnostic(lines, node.span(), level::ERROR, "取值不应匹配 not schema"));
        }
    }
    if let Some(cond) = schema.get("if") {
        if matches_schema(source, node, cond, ctx) {
            if let Some(then_s) = schema.get("then") { check(source, node, then_s, ctx, lines, out); }
        } else if let Some(else_s) = schema.get("else") {
            check(source, node, else_s, ctx, lines, out);
        }
    }

    if let Some(expected) = schema.get("type") {
        if !type_matches(source, node, expected) {
            out.push(diagnostic(lines, node.span(), level::ERROR, format!("类型应为 {}", describe_type(expected))));
            return;
        }
    }

    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.iter().any(|option| equals(source, node, option)) {
            out.push(diagnostic(lines, node.span(), level::ERROR, "取值不在允许集合内"));
        }
    }
    if let Some(constant) = schema.get("const") {
        if !equals(source, node, constant) {
            out.push(diagnostic(lines, node.span(), level::ERROR, "取值与 `const` 不符"));
        }
    }

    match node {
        Json::Object { span, members } => check_object(source, *span, members, schema, ctx, lines, out),
        Json::Array { span, items } => check_array(source, *span, items, schema, ctx, lines, out),
        Json::Number { span } => check_number(source, *span, schema, lines, out),
        Json::String { content, .. } => {
            let text = decode_string(source, *content);
            check_length(text.chars().count() as u64, schema, node.span(), lines, out);
            check_pattern(&text, schema, node.span(), lines, out);
            check_format(&text, schema, node.span(), lines, out);
        }
        _ => {}
    }
}

/// 校验对象节点。
fn check_object(
    source: &str,
    span: Span,
    members: &[crate::parser::Member],
    schema: &Value,
    ctx: &Ctx<'_>,
    lines: &LineIndex<'_>,
    out: &mut Vec<Value>,
) {
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !members.iter().any(|m| m.key_name == name) {
                let target = members.first().map_or(span, |m| m.key);
                out.push(diagnostic(lines, target, level::ERROR, format!("缺少必需属性 `{name}`")));
            }
        }
    }
    // dependentRequired
    if let Some(deps) = schema.get("dependentRequired").and_then(Value::as_object) {
        for (key, req) in deps {
            if members.iter().any(|m| m.key_name == *key) {
                if let Some(reqs) = req.as_array() {
                    for name in reqs.iter().filter_map(Value::as_str) {
                        if !members.iter().any(|m| m.key_name == name) {
                            let target = members.iter().find(|m| m.key_name == *key).map_or(span, |m| m.key);
                            out.push(diagnostic(lines, target, level::ERROR, format!("属性 `{key}` 存在时缺少 `{name}`")));
                        }
                    }
                }
            }
        }
    }
    // dependentSchemas
    if let Some(deps) = schema.get("dependentSchemas").and_then(Value::as_object) {
        for (key, sub) in deps {
            if members.iter().any(|m| m.key_name == *key) {
                let obj = Json::Object { span, members: members.to_vec() };
                check(source, &obj, sub, ctx, lines, out);
            }
        }
    }
    if let Some(max) = schema.get("maxProperties").and_then(Value::as_u64) {
        if (members.len() as u64) > max {
            out.push(diagnostic(lines, span, level::WARNING, format!("属性数量应不超过 {max}")));
        }
    }
    if let Some(min) = schema.get("minProperties").and_then(Value::as_u64) {
        if (members.len() as u64) < min {
            out.push(diagnostic(lines, span, level::WARNING, format!("属性数量应不少于 {min}")));
        }
    }

    let properties = schema.get("properties").and_then(Value::as_object);
    let pattern_props = schema.get("patternProperties").and_then(Value::as_object);
    let additional = schema.get("additionalProperties");

    for member in members {
        let mut matched = false;
        if let Some(sub) = properties.and_then(|p| p.get(&member.key_name)) {
            check(source, &member.value, sub, ctx, lines, out);
            matched = true;
        }
        if let Some(pp) = pattern_props {
            for (pattern, sub) in pp {
                if regex_lite::Regex::new(pattern).ok().is_some_and(|re| re.is_match(&member.key_name)) {
                    check(source, &member.value, sub, ctx, lines, out);
                    matched = true;
                }
            }
        }
        if !matched {
            match additional {
                Some(Value::Bool(false)) => out.push(diagnostic(lines, member.key, level::WARNING, format!("不应存在的属性 `{}`", member.key_name))),
                Some(sub) if sub.is_object() => check(source, &member.value, sub, ctx, lines, out),
                _ => {}
            }
        }
    }
}

/// 校验数组节点。
fn check_array(
    source: &str,
    span: Span,
    items: &[Json],
    schema: &Value,
    ctx: &Ctx<'_>,
    lines: &LineIndex<'_>,
    out: &mut Vec<Value>,
) {
    if let Some(min) = schema.get("minItems").and_then(Value::as_u64) {
        if (items.len() as u64) < min { out.push(diagnostic(lines, span, level::WARNING, format!("数组元素应不少于 {min} 个"))); }
    }
    if let Some(max) = schema.get("maxItems").and_then(Value::as_u64) {
        if (items.len() as u64) > max { out.push(diagnostic(lines, span, level::WARNING, format!("数组元素应不超过 {max} 个"))); }
    }
    if schema.get("uniqueItems").and_then(Value::as_bool) == Some(true) {
        for i in 0..items.len() {
            for j in (i + 1)..items.len() {
                if json_equal(source, &items[i], &items[j]) {
                    out.push(diagnostic(lines, items[j].span(), level::WARNING, "数组元素不应重复"));
                    break;
                }
            }
        }
    }
    if let Some(contains) = schema.get("contains") {
        let min = schema.get("minContains").and_then(Value::as_u64).unwrap_or(1);
        let max = schema.get("maxContains").and_then(Value::as_u64).unwrap_or(u64::MAX);
        let count = items.iter().filter(|item| matches_schema(source, item, contains, ctx)).count() as u64;
        if count < min { out.push(diagnostic(lines, span, level::WARNING, format!("数组应至少包含 {min} 个匹配 contains 的元素"))); }
        if count > max { out.push(diagnostic(lines, span, level::WARNING, format!("数组匹配 contains 的元素不应超过 {max} 个"))); }
    }

    // prefixItems / tuple items
    let tuple = schema.get("prefixItems").or_else(|| schema.get("items").filter(|v| v.is_array())).and_then(Value::as_array);
    if let Some(tuple) = tuple {
        for (i, item) in items.iter().enumerate() {
            if let Some(sub) = tuple.get(i) { check(source, item, sub, ctx, lines, out); }
            else {
                match schema.get("items").filter(|v| v.is_object()).or_else(|| schema.get("additionalItems")) {
                    Some(Value::Bool(false)) => out.push(diagnostic(lines, item.span(), level::WARNING, "不允许额外数组元素")),
                    Some(sub) if sub.is_object() => check(source, item, sub, ctx, lines, out),
                    _ => {}
                }
            }
        }
        return;
    }
    if let Some(sub) = schema.get("items").filter(|v| v.is_object()) {
        for item in items { check(source, item, sub, ctx, lines, out); }
    }
}

/// 轻量级 schema 匹配检查（用于 anyOf/oneOf/not/if/contains）。
fn matches_schema(source: &str, node: &Json, schema: &Value, ctx: &Ctx<'_>) -> bool {
    let schema = resolve_ref(schema, ctx);
    if !schema.is_object() { return true; }
    if let Some(subs) = schema.get("allOf").and_then(Value::as_array) {
        if !subs.iter().all(|s| matches_schema(source, node, s, ctx)) { return false; }
    }
    if let Some(subs) = schema.get("anyOf").and_then(Value::as_array) {
        if !subs.is_empty() && !subs.iter().any(|s| matches_schema(source, node, s, ctx)) { return false; }
    }
    if let Some(subs) = schema.get("oneOf").and_then(Value::as_array) {
        if subs.iter().filter(|s| matches_schema(source, node, s, ctx)).count() != 1 { return false; }
    }
    if let Some(sub) = schema.get("not") { if matches_schema(source, node, sub, ctx) { return false; } }
    if let Some(expected) = schema.get("type") { if !type_matches(source, node, expected) { return false; } }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.iter().any(|o| equals(source, node, o)) { return false; }
    }
    if let Some(c) = schema.get("const") { if !equals(source, node, c) { return false; } }
    match node {
        Json::Object { members, .. } => {
            if let Some(req) = schema.get("required").and_then(Value::as_array) {
                for name in req.iter().filter_map(Value::as_str) {
                    if !members.iter().any(|m| m.key_name == name) { return false; }
                }
            }
            let props = schema.get("properties").and_then(Value::as_object);
            let add = schema.get("additionalProperties");
            for m in members {
                let mut matched = false;
                if let Some(sub) = props.and_then(|p| p.get(&m.key_name)) { if !matches_schema(source, &m.value, sub, ctx) { return false; } matched = true; }
                if !matched {
                    match add {
                        Some(Value::Bool(false)) => return false,
                        Some(sub) if sub.is_object() => { if !matches_schema(source, &m.value, sub, ctx) { return false; } }
                        _ => {}
                    }
                }
            }
        }
        Json::Array { items, .. } => {
            if let Some(min) = schema.get("minItems").and_then(Value::as_u64) { if (items.len() as u64) < min { return false; } }
            if let Some(max) = schema.get("maxItems").and_then(Value::as_u64) { if (items.len() as u64) > max { return false; } }
            if let Some(sub) = schema.get("items").filter(|v| v.is_object()) {
                if !items.iter().all(|i| matches_schema(source, i, sub, ctx)) { return false; }
            }
        }
        Json::String { content, .. } => {
            let text = decode_string(source, *content);
            if let Some(min) = schema.get("minLength").and_then(Value::as_u64) { if (text.chars().count() as u64) < min { return false; } }
            if let Some(max) = schema.get("maxLength").and_then(Value::as_u64) { if (text.chars().count() as u64) > max { return false; } }
            if let Some(pat) = schema.get("pattern").and_then(Value::as_str) {
                if regex_lite::Regex::new(pat).ok().is_none_or(|re| !re.is_match(&text)) { return false; }
            }
        }
        Json::Number { span } => {
            let Ok(n) = span.text(source).parse::<f64>() else { return false; };
            if let Some(v) = schema.get("minimum").and_then(Value::as_f64) { if n < v { return false; } }
            if let Some(v) = schema.get("maximum").and_then(Value::as_f64) { if n > v { return false; } }
        }
        _ => {}
    }
    true
}

/// 简单 JSON 值相等（用于 uniqueItems）。
fn json_equal(source: &str, a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Null{..}, Json::Null{..}) => true,
        (Json::Bool{value:a,..}, Json::Bool{value:b,..}) => a==b,
        (Json::Number{span:a}, Json::Number{span:b}) => a.text(source)==b.text(source),
        (Json::String{content:a,..}, Json::String{content:b,..}) => decode_string(source,*a)==decode_string(source,*b),
        (Json::Array{items:a,..}, Json::Array{items:b,..}) => a.len()==b.len() && a.iter().zip(b).all(|(x,y)| json_equal(source,x,y)),
        (Json::Object{members:a,..}, Json::Object{members:b,..}) => a.len()==b.len() && a.iter().all(|ma| b.iter().any(|mb| ma.key_name==mb.key_name && json_equal(source,&ma.value,&mb.value))),
        _ => false,
    }
}

/// `format` 校验：已知 format 给出 warning；未知 format 保留信息但不误报。
fn check_format(text: &str, schema: &Value, span: Span, lines: &LineIndex<'_>, out: &mut Vec<Value>) {
    let Some(format) = schema.get("format").and_then(Value::as_str) else { return; };
    let ok = match format {
        "email" => is_email(text),
        "uri" => is_uri(text),
        "uri-reference" => is_uri_reference(text),
        "iri" => is_uri(text),
        "iri-reference" => is_uri_reference(text),
        "date-time" => is_date_time(text),
        "date" => is_date(text),
        "time" => is_time(text),
        "ipv4" => is_ipv4(text),
        "ipv6" => is_ipv6(text),
        "uuid" => is_uuid(text),
        "hostname" => is_hostname(text),
        "json-pointer" => text.is_empty() || text.starts_with('/'),
        "relative-json-pointer" => is_relative_json_pointer(text),
        "regex" => regex_lite::Regex::new(text).is_ok(),
        _ => return,
    };
    if !ok {
        out.push(diagnostic(lines, span, level::WARNING, format!("取值不符合 format `{format}`")));
    }
}

fn is_email(text: &str) -> bool {
    let Some((local, domain)) = text.rsplit_once('@') else { return false; };
    !local.is_empty() && !domain.is_empty() && !domain.contains('@') && domain.contains('.')
}
fn is_uri(text: &str) -> bool {
    let Some((scheme, rest)) = text.split_once(':') else { return false; };
    !scheme.is_empty() && scheme.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+'|b'-'|b'.')) && !rest.is_empty()
}
fn is_uri_reference(text: &str) -> bool { is_uri(text) || text.starts_with('/') || text.starts_with('#') || !text.is_empty() }
fn is_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(i,c)| matches!(i,4|7) || c.is_ascii_digit())
}
fn is_time(text: &str) -> bool {
    let re = regex_lite::Regex::new(r"^\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$");
    re.ok().is_some_and(|r| r.is_match(text))
}
fn is_date_time(text: &str) -> bool {
    let Some((d, t)) = text.split_once('T').or_else(|| text.split_once('t')) else { return false; };
    is_date(d) && is_time(t)
}
fn is_ipv4(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.parse::<u8>().is_ok() && (p.len() == 1 || !p.starts_with('0')))
}
fn is_ipv6(text: &str) -> bool { text.contains(':') && text.bytes().all(|b| b.is_ascii_hexdigit() || matches!(b, b':'|b'.')) }
fn is_uuid(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 36 && [8,13,18,23].iter().all(|&i| b[i]==b'-') && b.iter().enumerate().all(|(i,c)| matches!(i,8|13|18|23) || c.is_ascii_hexdigit())
}
fn is_hostname(text: &str) -> bool {
    !text.is_empty() && text.len() <= 253 && text.split('.').all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|b| b.is_ascii_alphanumeric() || b==b'-'))
}
fn is_relative_json_pointer(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else { return false; };
    if !first.is_ascii_digit() { return false; }
    let mut rest = String::new();
    let mut seen_nonzero = first != '0';
    rest.push(first);
    for c in chars {
        if c.is_ascii_digit() && !seen_nonzero { return false; }
        if c.is_ascii_digit() { seen_nonzero = true; rest.push(c); continue; }
        return c == '#' || (c == '/' );
    }
    true
}

/// `pattern` 校验：ECMA-262 正则，锚定语义（未匹配即告警）。
fn check_pattern(
    text: &str,
    schema: &Value,
    span: Span,
    lines: &LineIndex<'_>,
    out: &mut Vec<Value>,
) {
    let Some(pattern) = schema.get("pattern").and_then(Value::as_str) else {
        return;
    };

    match regex_lite::Regex::new(pattern) {
        Ok(re) => {
            if !re.is_match(text) {
                out.push(diagnostic(
                    lines,
                    span,
                    level::WARNING,
                    format!("取值不匹配模式 `{pattern}`"),
                ));
            }
        }
        // schema 本身的 pattern 写错了：不阻塞编辑，只提示一次
        Err(error) => out.push(diagnostic(
            lines,
            span,
            level::WARNING,
            format!("schema 的 `pattern` 无效：{error}"),
        )),
    }
}

/// 数值范围校验。
fn check_number(
    source: &str,
    span: Span,
    schema: &Value,
    lines: &LineIndex<'_>,
    out: &mut Vec<Value>,
) {
    let Ok(number) = span.text(source).parse::<f64>() else {
        return;
    };

    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64) {
        if number < minimum {
            out.push(diagnostic(
                lines,
                span,
                level::WARNING,
                format!("数值应不小于 {minimum}"),
            ));
        }
    }
    if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64) {
        if number > maximum {
            out.push(diagnostic(
                lines,
                span,
                level::WARNING,
                format!("数值应不大于 {maximum}"),
            ));
        }
    }
    if let Some(minimum) = schema.get("exclusiveMinimum").and_then(Value::as_f64) {
        if number <= minimum {
            out.push(diagnostic(
                lines,
                span,
                level::WARNING,
                format!("数值应大于 {minimum}"),
            ));
        }
    }
    if let Some(maximum) = schema.get("exclusiveMaximum").and_then(Value::as_f64) {
        if number >= maximum {
            out.push(diagnostic(
                lines,
                span,
                level::WARNING,
                format!("数值应小于 {maximum}"),
            ));
        }
    }
}

/// 字符串长度校验。
fn check_length(
    length: u64,
    schema: &Value,
    span: Span,
    lines: &LineIndex<'_>,
    out: &mut Vec<Value>,
) {
    if let Some(min) = schema.get("minLength").and_then(Value::as_u64) {
        if length < min {
            out.push(diagnostic(
                lines,
                span,
                level::WARNING,
                format!("长度应不少于 {min}"),
            ));
        }
    }
    if let Some(max) = schema.get("maxLength").and_then(Value::as_u64) {
        if length > max {
            out.push(diagnostic(
                lines,
                span,
                level::WARNING,
                format!("长度应不超过 {max}"),
            ));
        }
    }
}

/// 节点类型是否匹配 `type`（字符串或字符串数组）。
fn type_matches(source: &str, node: &Json, expected: &Value) -> bool {
    let candidates: Vec<&str> = match expected {
        Value::String(single) => vec![single.as_str()],
        Value::Array(list) => list.iter().filter_map(Value::as_str).collect(),
        _ => return true,
    };

    candidates.iter().any(|candidate| match *candidate {
        "object" => matches!(node, Json::Object { .. }),
        "array" => matches!(node, Json::Array { .. }),
        "string" => matches!(node, Json::String { .. }),
        "boolean" => matches!(node, Json::Bool { .. }),
        "null" => matches!(node, Json::Null { .. }),
        "number" => matches!(node, Json::Number { .. }),
        "integer" => matches!(node, Json::Number { span } if is_integer(span.text(source))),
        _ => true,
    })
}

/// 值与 schema 常量是否相等（按节点类型比较）。
fn equals(source: &str, node: &Json, option: &Value) -> bool {
    match (node, option) {
        (Json::String { content, .. }, Value::String(text)) => {
            decode_string(source, *content) == *text
        }
        (Json::Bool { value, .. }, Value::Bool(flag)) => value == flag,
        (Json::Null { .. }, Value::Null) => true,
        (Json::Number { span }, Value::Number(number)) => span
            .text(source)
            .parse::<f64>()
            .ok()
            .zip(number.as_f64())
            .is_some_and(|(left, right)| left == right),
        _ => false,
    }
}

/// 构造一条 LSP 诊断。
fn diagnostic(
    lines: &LineIndex<'_>,
    span: Span,
    severity: u8,
    message: impl Into<String>,
) -> Value {
    let (start_line, start_char) = lines.position(span.start);
    let (end_line, end_char) = lines.position(span.end);
    json!({
        "range": {
            "start": { "line": start_line, "character": start_char },
            "end": { "line": end_line, "character": end_char },
        },
        "severity": severity,
        "source": "lspd(schema)",
        "message": message.into(),
    })
}

/// 把 `type` 值描述为可读文本。
fn describe_type(expected: &Value) -> String {
    match expected {
        Value::String(single) => single.clone(),
        Value::Array(list) => list
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" | "),
        _ => String::new(),
    }
}

/// 整数判定：不含小数点与指数。
fn is_integer(text: &str) -> bool {
    !text.contains('.') && !text.contains('e') && !text.contains('E')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use lspd_core::text::LineIndex;

    /// 用内联 schema 校验文档，返回诊断信息列表。
    fn messages(schema: &str, document: &str) -> Vec<String> {
        let schema_value: Value = serde_json::from_str(schema).unwrap();
        let parsed = parse(document);
        let lines = LineIndex::new(document);
        validate(document, &parsed.value, &schema_value, &lines)
            .into_iter()
            .map(|item| item["message"].as_str().unwrap().to_string())
            .collect()
    }

    /// 类型不符要报错，且不再深入产生连带噪声。
    #[test]
    fn type_mismatch() {
        let found = messages(r#"{ "type": "string" }"#, "123");
        assert!(
            found.iter().any(|m| m.contains("类型应为 string")),
            "{found:?}"
        );
    }

    /// required 缺失要报错。
    #[test]
    fn required_missing() {
        let schema = r#"{ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }"#;
        assert!(
            messages(schema, r#"{ "other": 1 }"#)
                .iter()
                .any(|m| m.contains("缺少必需属性 `name`"))
        );
        let msgs = messages(schema, r#"{ "name": "ok" }"#);
        assert!(msgs.is_empty(), "{msgs:?}");
    }

    /// enum 与 const 命中则无诊断。
    #[test]
    fn enum_and_const() {
        assert!(
            messages(r#"{ "enum": ["a", "b"] }"#, "\"c\"")
                .iter()
                .any(|m| m.contains("允许集合"))
        );
        assert!(messages(r#"{ "enum": ["a", "b"] }"#, "\"a\"").is_empty());
        assert!(messages(r#"{ "const": 5 }"#, "5").is_empty());
    }

    /// 数值范围与字符串长度。
    #[test]
    fn ranges_and_lengths() {
        assert!(
            messages(r#"{ "type": "number", "minimum": 10 }"#, "3")
                .iter()
                .any(|m| m.contains("不小于"))
        );
        assert!(
            messages(r#"{ "type": "string", "maxLength": 2 }"#, "\"abc\"")
                .iter()
                .any(|m| m.contains("不超过"))
        );
    }

    /// properties 递归到嵌套值。
    #[test]
    fn nested_properties() {
        let schema = r#"{ "type": "object", "properties": { "port": { "type": "integer" } } }"#;
        assert!(
            messages(schema, r#"{ "port": 1.5 }"#)
                .iter()
                .any(|m| m.contains("类型应为 integer"))
        );
        assert!(messages(schema, r#"{ "port": 8080 }"#).is_empty());
    }

    /// additionalProperties: false 拒绝未知键。
    #[test]
    fn additional_properties_false() {
        let schema =
            r#"{ "type": "object", "properties": { "a": {} }, "additionalProperties": false }"#;
        assert!(
            messages(schema, r#"{ "a": 1, "b": 2 }"#)
                .iter()
                .any(|m| m.contains("不应存在的属性 `b`"))
        );
    }

    /// items 对数组元素递归。
    #[test]
    fn array_items() {
        let schema = r#"{ "type": "array", "items": { "type": "integer" } }"#;
        assert!(
            messages(schema, "[1, \"x\"]")
                .iter()
                .any(|m| m.contains("类型应为 integer"))
        );
    }

    /// `http(s)` 的 `$schema` 被忽略，不读取。
    #[test]
    fn ignores_remote_schema() {
        assert!(resolve_path("https://example.com/s.json", "file:///tmp/a.json").is_none());
        assert!(resolve_path("./s.json", "file:///tmp/a.json").is_some());
    }

    /// `pattern` 匹配与不匹配、以及 schema 自身 pattern 非法的提示。
    #[test]
    fn pattern_validation() {
        let found = messages(r#"{ "type": "string", "pattern": "^[a-z]+$" }"#, r#""ABC""#);
        assert!(found.iter().any(|m| m.contains("不匹配模式")), "{found:?}");
        assert!(messages(r#"{ "type": "string", "pattern": "^[a-z]+$" }"#, r#""abc""#).is_empty());

        // pattern 写错：提示 schema 问题而非静默放行
        let invalid = messages(r#"{ "type": "string", "pattern": "[" }"#, r#""x""#);
        assert!(
            invalid.iter().any(|m| m.contains("`pattern` 无效")),
            "{invalid:?}"
        );
    }

    /// 取补全项标签。
    fn completion_labels(schema: &str, document: &str, offset: u32) -> Vec<String> {
        let schema_value: Value = serde_json::from_str(schema).unwrap();
        let parsed = parse(document);
        completion(document, &parsed.value, Some(&schema_value), offset)
            .into_iter()
            .map(|item| item["label"].as_str().unwrap().to_string())
            .collect()
    }

    /// 对象体里的键位置：补未出现的属性名，根上额外给 `$schema`。
    #[test]
    fn completes_keys() {
        let schema = r#"{ "type": "object", "properties": { "name": {}, "age": {} } }"#;
        let labels = completion_labels(schema, "{ }", 2);
        assert!(
            labels.contains(&"name".to_string()) && labels.contains(&"age".to_string()),
            "{labels:?}"
        );
        assert!(labels.contains(&"$schema".to_string()));

        // 已有的键不再重复给
        let partial = completion_labels(schema, "{ \"name\": \"x\", }", 16);
        assert!(
            !partial.contains(&"name".to_string()) && partial.contains(&"age".to_string()),
            "{partial:?}"
        );
    }

    /// 冒号后的值位置：按属性子 schema 给枚举值。
    #[test]
    fn completes_enum_values() {
        let schema =
            r#"{ "type": "object", "properties": { "env": { "enum": ["dev", "prod"] } } }"#;
        let offset = (r#"{ "env": "#).len() as u32;
        let labels = completion_labels(schema, r#"{ "env": "#, offset);
        assert!(
            labels.contains(&"dev".to_string()) && labels.contains(&"prod".to_string()),
            "{labels:?}"
        );
    }

    /// 键悬停应返回描述、类型与枚举信息。
    #[test]
    fn hover_on_key_shows_description() {
        let schema = r#"{ "type": "object", "properties": { "target": { "type": "string", "description": "输出目标", "enum": ["ES5", "ESNext"] } } }"#;
        let document = r#"{ "target": "ES5" }"#;
        let offset = document.find("target").unwrap() as u32 + 1;
        let parsed = parse(document);
        let schema_value: Value = serde_json::from_str(schema).unwrap();
        let text = hover(&parsed.value, Some(&schema_value), offset).expect("应有 hover");
        assert!(text.contains("输出目标"), "{text}");
        assert!(text.contains("string"), "{text}");
        assert!(text.contains("ESNext"), "{text}");
    }

    /// 值悬停也应给出属性说明。
    #[test]
    fn hover_on_value_shows_description() {
        let schema = r#"{ "type": "object", "properties": { "strict": { "type": "boolean", "description": "严格模式" } } }"#;
        let document = r#"{ "strict": true }"#;
        let offset = document.find("true").unwrap() as u32;
        let parsed = parse(document);
        let schema_value: Value = serde_json::from_str(schema).unwrap();
        let text = hover(&parsed.value, Some(&schema_value), offset).expect("应有 hover");
        assert!(text.contains("严格模式"), "{text}");
        assert!(text.contains("boolean"), "{text}");
    }

    /// `$ref` 与 `$defs` 应支持本地 JSON Pointer。
    #[test]
    fn resolves_local_ref() {
        let schema = r##"{
            "$defs": { "port": { "type": "integer", "minimum": 1, "maximum": 65535 } },
            "type": "object",
            "properties": { "port": { "$ref": "#/$defs/port" } }
        }"##;
        assert!(messages(schema, r#"{ "port": 80 }"#).is_empty());
        assert!(messages(schema, r#"{ "port": 70000 }"#).iter().any(|m| m.contains("不大于")));
        let parsed = parse(r#"{ "port": 80 }"#);
        let schema_value: Value = serde_json::from_str(schema).unwrap();
        let hover = hover(&parsed.value, Some(&schema_value), 3).unwrap_or_default();
        assert!(hover.contains("integer") || hover.contains("port"));
    }

    /// allOf/anyOf/oneOf/not/if-then-else 组合关键字。
    #[test]
    fn composition_keywords() {
        let allof = r#"{ "allOf": [ { "type": "object", "properties": { "a": { "type": "string" } }, "required": ["a"] }, { "properties": { "b": { "type": "number" } } } ] }"#;
        assert!(messages(allof, r#"{ "a": "x", "b": 1 }"#).is_empty());
        assert!(messages(allof, r#"{ "b": 1 }"#).iter().any(|m| m.contains("缺少必需属性")));

        let anyof = r#"{ "anyOf": [ { "type": "string" }, { "type": "number" } ] }"#;
        assert!(messages(anyof, r#""ok""#).is_empty());
        assert!(messages(anyof, "true").iter().any(|m| m.contains("anyOf")));

        let oneof = r#"{ "oneOf": [ { "type": "string" }, { "type": "number" } ] }"#;
        assert!(messages(oneof, "1").is_empty());
        let nots = r#"{ "not": { "type": "string" } }"#;
        assert!(messages(nots, r#""x""#).iter().any(|m| m.contains("not")));

        let ifte = r#"{ "if": { "properties": { "kind": { "const": "a" } } }, "then": { "required": ["a"] }, "else": { "required": ["b"] } }"#;
        assert!(messages(ifte, r#"{ "kind": "a", "a": 1 }"#).is_empty());
        assert!(messages(ifte, r#"{ "kind": "b" }"#).iter().any(|m| m.contains("缺少必需属性 `b`")));
    }

    /// uniqueItems / contains / minItems / maxItems。
    #[test]
    fn array_keywords() {
        assert!(messages(r#"{ "type":"array", "uniqueItems": true }"#, "[1,1]").iter().any(|m| m.contains("重复")));
        assert!(messages(r#"{ "type":"array", "contains": { "type":"number" }, "minContains": 2 }"#, "[1,\"a\"]").iter().any(|m| m.contains("contains")));
        assert!(messages(r#"{ "type":"array", "minItems":2 }"#, "[1]").iter().any(|m| m.contains("不少于")));
    }

    /// patternProperties / dependentRequired / format / deprecated。
    #[test]
    fn object_and_format_keywords() {
        let pp = r#"{ "type":"object", "patternProperties": { "^x-": { "type":"string" } }, "additionalProperties": false }"#;
        assert!(messages(pp, r#"{ "x-a": "ok" }"#).is_empty());
        assert!(messages(pp, r#"{ "x-a": 1 }"#).iter().any(|m| m.contains("类型应为")));
        let dr = r#"{ "type":"object", "dependentRequired": { "a": ["b"] } }"#;
        assert!(messages(dr, r#"{ "a": 1 }"#).iter().any(|m| m.contains("缺少 `b`")));
        assert!(messages(r#"{ "type":"string", "format":"email" }"#, r#""not-mail""#).iter().any(|m| m.contains("format")));
    }


    /// 本地文件 `$ref` 应可解析（含 fragment），且受基目录限制。
    #[test]
    fn resolves_local_file_ref() {
        let dir = std::env::temp_dir().join(format!("lspd-schema-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let defs = dir.join("defs.json");
        std::fs::write(&defs, r##"{ "$defs": { "port": { "type": "integer", "minimum": 1 } } }"##).unwrap();
        let schema_path = dir.join("main.json");
        std::fs::write(&schema_path, r##"{ "type":"object", "properties": { "port": { "$ref": "defs.json#/$defs/port" } } }"##).unwrap();
        let doc = dir.join("doc.json");
        let source = format!("{{ \"$schema\": \"{}\", \"port\": 0 }}", schema_path.to_str().unwrap());
        let parsed = parse(&source);
        let schema = load_schema(&source, &parsed.value, &format!("file://{}", doc.to_str().unwrap())).expect("schema");
        let lines = LineIndex::new(&source);
        let store = SchemaStore::new();
        let diags = validate_with_store(&source, &parsed.value, &schema, &store, &lines);
        assert!(diags.iter().any(|d| d["message"].as_str().unwrap_or("").contains("不小于")), "{diags:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `$id` 应可作为引用目标。
    #[test]
    fn resolves_id_ref() {
        let schema = r##"{
          "$id": "https://example.com/root.json",
          "$defs": { "name": { "$id": "#name", "type":"string", "minLength": 2 } },
          "type":"object",
          "properties": { "name": { "$ref": "#name" } }
        }"##;
        let msgs = messages(schema, r#"{ "name": "ok" }"#);
        assert!(msgs.is_empty(), "{msgs:?}");
        assert!(messages(schema, r#"{ "name": "x" }"#).iter().any(|m| m.contains("不少于")));
    }

    /// format 应对已知格式给出 warning。
    #[test]
    fn format_validation() {
        assert!(messages(r#"{ "type":"string", "format":"email" }"#, r#""a@b.com""#).is_empty());
        assert!(messages(r#"{ "type":"string", "format":"uuid" }"#, r#""not-uuid""#).iter().any(|m| m.contains("format")));
        assert!(messages(r#"{ "type":"string", "format":"ipv4" }"#, r#""1.2.3.4""#).is_empty());
        assert!(messages(r#"{ "type":"string", "format":"unknown-format" }"#, r#""x""#).is_empty());
    }

    /// 默认应禁用远程 schema 获取。
    #[test]
    fn remote_schema_disabled_by_default() {
        let store = SchemaStore::with_config(SchemaConfig {
            allow_remote: false,
            cache_dir: std::env::temp_dir().join("lspd-schema-cache-test"),
        });
        assert!(store.load_remote("https://json.schemastore.org/tsconfig.json").is_some()); // 内置映射命中
        assert!(store.load_remote("https://example.invalid/nope.json").is_none());
    }

    /// 内置 schema 应覆盖常见配置文件。
    #[test]
    fn builtin_schemas_are_available() {
        for (uri, key) in [
            ("file:///p/tsconfig.json", "compilerOptions"),
            ("file:///p/jsconfig.json", "compilerOptions"),
            ("file:///p/package.json", "name"),
            ("file:///p/.vscode/settings.json", "editor.formatOnSave"),
            ("file:///p/.vscode/extensions.json", "recommendations"),
            ("file:///p/.vscode/tasks.json", "tasks"),
            ("file:///p/.vscode/launch.json", "configurations"),
            ("file:///p/Cargo.toml", "package"),
        ] {
            let schema = builtin_schema(uri).unwrap_or_else(|| panic!("{uri} 应命中内置 schema"));
            assert!(schema["properties"].get(key).is_some(), "{uri} 缺少 {key}");
        }
    }
}
