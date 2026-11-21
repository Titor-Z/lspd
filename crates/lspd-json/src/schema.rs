//! 离线 JSON Schema：加载本地/内置 schema，提供校验、补全与悬停。
//!
//! 优先使用文档 `$schema` 指向的本地文件；未声明时按文件名匹配内置 schema。
//! 只实现常用关键字子集：`type` / `enum` / `const` / `required` / `properties` /
//! `additionalProperties` / `items` / `minimum`–`maximum` / `minLength`–`maxLength` /
//! `pattern` / `description` / `markdownDescription` / `default`。
//! 不解析 `$ref`/`$id`、不访问网络。

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

/// 从根节点 `$schema` 或文件名映射加载 schema。
///
/// 优先级：文档显式 `$schema`（本地/file://）> 内置文件名映射 > 无。
pub fn load(source: &str, root: &Json, document_uri: &str) -> Option<Value> {
    if let Some(reference) = schema_reference(root, source) {
        if let Some(path) = resolve_path(&reference, document_uri) {
            if let Some(schema) = read_schema_file(&path) {
                return Some(schema);
            }
        }

        // 显式声明了 $schema 但无法离线加载：不再回退内置，避免误导
        return None;
    }

    builtin_schema(document_uri)
}

/// 读取并解析 schema 文件。
fn read_schema_file(path: &std::path::Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Value>(&text).ok()
}

/// 按文件名命中内置 schema。
fn builtin_schema(document_uri: &str) -> Option<Value> {
    let name = file_name(document_uri);
    let source = match name {
        _ if is_tsconfig_name(name) => TSCONFIG_SCHEMA,
        "package.json" => PACKAGE_SCHEMA,
        "settings.json" => VSCODE_SETTINGS_SCHEMA,
        "extensions.json" => VSCODE_EXTENSIONS_SCHEMA,
        "tasks.json" => VSCODE_TASKS_SCHEMA,
        "launch.json" => VSCODE_LAUNCH_SCHEMA,
        _ => return None,
    };

    serde_json::from_str::<Value>(source).ok()
}

/// tsconfig/jsconfig 家族文件名判断。
fn is_tsconfig_name(name: &str) -> bool {
    (name.starts_with("tsconfig") || name.starts_with("jsconfig")) && name.ends_with(".json")
}

/// 从 URI/路径里取文件名。
fn file_name(document_uri: &str) -> &str {
    let path = document_uri.split(['?', '#']).next().unwrap_or(document_uri);
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

/// 依据 schema 校验整篇文档，产出 LSP 诊断。
pub fn validate(source: &str, root: &Json, schema: &Value, lines: &LineIndex<'_>) -> Vec<Value> {
    let mut diagnostics = Vec::new();
    check(source, root, schema, lines, &mut diagnostics);
    diagnostics
}

/// LSP CompletionItemKind：对象键按 Property，枚举/字面量按 Value。
const ITEM_PROPERTY: u8 = 10;
const ITEM_VALUE: u8 = 12;

/// 依据 schema 给出补全项：对象体里补键名，冒号后补枚举/布尔值。
/// `schema` 为 `None` 时不做 schema 补全（返回空）。
pub fn completion(source: &str, root: &Json, schema: Option<&Value>, offset: u32) -> Vec<Value> {
    let Some(root_schema) = schema else {
        return Vec::new();
    };

    let located = locate(root, root_schema, offset);
    let line_start = start_of_line(source, offset);

    match located.node {
        Json::Object { members, .. } => {
            // 当前行上、光标落在某个成员的值区间内（缺失值时为其插入点）→ 值补全
            let value_member = members.iter().find(|member| {
                let value = member.value.span();
                member.key.start >= line_start && offset > member.key.end && offset <= value.end
            });

            if let Some(member) = value_member {
                let sub = located
                    .schema
                    .get("properties")
                    .and_then(|props| props.get(&member.key_name))
                    .unwrap_or(&Value::Null);
                return value_items(sub);
            }

            // 键位置：补尚未出现的属性名
            key_items(located.schema, members, located.node.span() == root.span())
        }
        Json::Array { .. } => {
            let sub = located.schema.get("items").unwrap_or(&Value::Null);
            value_items(sub)
        }
        _ => value_items(located.schema),
    }
}

/// 依据 schema 给出悬停信息：键给属性说明，值给类型/默认值/枚举说明。
pub fn hover(root: &Json, schema: Option<&Value>, offset: u32) -> Option<String> {
    let schema = schema?;
    let located = locate(root, schema, offset);

    match located.kind {
        HitKind::Key => hover_for_key(located.key_name?, located.schema),
        HitKind::Value => hover_for_value(located.key_name, located.schema),
    }
}

/// 生成键悬停内容。
fn hover_for_key(key: &str, schema: &Value) -> Option<String> {
    if !schema.is_object() || schema.as_object().is_some_and(|item| item.is_empty()) {
        return None;
    }

    let mut lines = vec![format!("`{key}`")];
    push_schema_details(&mut lines, schema);
    Some(lines.join("\n\n"))
}

/// 生成值悬停内容。
fn hover_for_value(key: Option<&str>, schema: &Value) -> Option<String> {
    if !schema.is_object() || schema.as_object().is_some_and(|item| item.is_empty()) {
        return None;
    }

    let mut lines = Vec::new();
    if let Some(key) = key {
        lines.push(format!("`{key}`"));
    }
    push_schema_details(&mut lines, schema);

    (lines.len() > (usize::from(key.is_some()))).then(|| lines.join("\n\n"))
}

/// 把 schema 的说明性字段追加到悬停文本。
fn push_schema_details(lines: &mut Vec<String>, schema: &Value) {
    let description = schema
        .get("markdownDescription")
        .or_else(|| schema.get("description"))
        .and_then(Value::as_str);
    if let Some(description) = description {
        lines.push(description.to_string());
    }

    if let Some(types) = schema.get("type") {
        let text = describe_type(types);
        if !text.is_empty() {
            lines.push(format!("类型：`{text}`"));
        }
    }

    if let Some(default) = schema.get("default") {
        lines.push(format!("默认值：`{}`", compact_json(default)));
    }

    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        let values: Vec<String> = allowed.iter().map(compact_json).collect();
        if !values.is_empty() {
            lines.push(format!("可选值：{}", values.join(", ")));
        }
    }
}

/// 把 JSON 值压成简短文本，供 hover 展示。
fn compact_json(value: &Value) -> String {
    match value {
        Value::String(text) => format!("\"{text}\""),
        _ => value.to_string(),
    }
}

/// 命中的位置：键、值或容器自身。
#[derive(Debug, Clone, Copy)]
enum HitKind {
    /// 光标落在对象键上
    Key,
    /// 光标落在值上
    Value,
}

/// 下钻结果：节点、对应 schema、以及命中的是键还是值。
struct Located<'s> {
    node: &'s Json,
    schema: &'s Value,
    kind: HitKind,
    /// 若命中对象成员，给出成员键名（hover 时显示属性名）
    key_name: Option<&'s str>,
}

/// 下钻到包含 `offset` 的最深节点，并同步带上其子 schema。
fn locate<'s>(node: &'s Json, schema: &'s Value, offset: u32) -> Located<'s> {
    match node {
        Json::Object { members, .. } => {
            for member in members {
                if member.key.contains(offset) {
                    let sub = schema
                        .get("properties")
                        .and_then(|props| props.get(&member.key_name))
                        .unwrap_or(&Value::Null);
                    return Located { node: &member.value, schema: sub, kind: HitKind::Key, key_name: Some(&member.key_name) };
                }

                if member.value.span().contains(offset) {
                    let sub = schema
                        .get("properties")
                        .and_then(|props| props.get(&member.key_name))
                        .unwrap_or(&Value::Null);
                    if matches!(member.value, Json::Object { .. } | Json::Array { .. }) {
                        return locate(&member.value, sub, offset);
                    }
                    return Located { node: &member.value, schema: sub, kind: HitKind::Value, key_name: Some(&member.key_name) };
                }
            }
            Located { node, schema, kind: HitKind::Value, key_name: None }
        }
        Json::Array { items, .. } => {
            let sub = schema.get("items").unwrap_or(&Value::Null);
            for item in items {
                if item.span().contains(offset) {
                    if matches!(item, Json::Object { .. } | Json::Array { .. }) {
                        return locate(item, sub, offset);
                    }
                    return Located { node: item, schema: sub, kind: HitKind::Value, key_name: None };
                }
            }
            Located { node, schema, kind: HitKind::Value, key_name: None }
        }
        _ => Located { node, schema, kind: HitKind::Value, key_name: None },
    }
}

/// 对象键补全：schema 的 properties 中未出现的键；根对象额外给 `$schema`。
fn key_items(schema: &Value, members: &[crate::parser::Member], is_root: bool) -> Vec<Value> {
    let mut items = Vec::new();
    let present: Vec<&str> = members.iter().map(|member| member.key_name.as_str()).collect();

    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for key in properties.keys() {
            if present.contains(&key.as_str()) {
                continue;
            }
            items.push(json!({
                "label": key,
                "kind": ITEM_PROPERTY,
                "insertText": format!("\"{key}\": "),
            }));
        }
    }

    if is_root && !present.contains(&"$schema") {
        items.push(json!({ "label": "$schema", "kind": ITEM_PROPERTY, "insertText": "\"$schema\": " }));
    }

    items
}

/// 值补全：枚举值与布尔字面量。
fn value_items(schema: &Value) -> Vec<Value> {
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

    if items.is_empty() {
        let types: Vec<&str> = match schema.get("type") {
            Some(Value::String(single)) => vec![single.as_str()],
            Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if types.contains(&"boolean") {
            items.push(json!({ "label": "true", "kind": ITEM_VALUE, "insertText": "true" }));
            items.push(json!({ "label": "false", "kind": ITEM_VALUE, "insertText": "false" }));
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
fn check(source: &str, node: &Json, schema: &Value, lines: &LineIndex<'_>, out: &mut Vec<Value>) {
    if !schema.is_object() {
        return;
    }

    if let Some(expected) = schema.get("type") {
        if !type_matches(source, node, expected) {
            out.push(diagnostic(lines, node.span(), level::ERROR, format!("类型应为 {}", describe_type(expected))));
            return; // 类型不符时不再深入，避免连带噪声
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
        Json::Object { span, members } => {
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for name in required.iter().filter_map(Value::as_str) {
                    if !members.iter().any(|member| member.key_name == name) {
                        let target = members.first().map_or(*span, |member| member.key);
                        out.push(diagnostic(lines, target, level::ERROR, format!("缺少必需属性 `{name}`")));
                    }
                }
            }

            let properties = schema.get("properties");
            let additional = schema.get("additionalProperties");

            for member in members {
                if let Some(sub) = properties.and_then(|props| props.get(&member.key_name)) {
                    check(source, &member.value, sub, lines, out);
                } else if additional.and_then(Value::as_bool) == Some(false) {
                    out.push(diagnostic(lines, member.key, level::WARNING, format!("不应存在的属性 `{}`", member.key_name)));
                }
            }
        }
        Json::Array { items, .. } => match schema.get("items") {
            Some(Value::Array(tuple)) => {
                for (index, item) in items.iter().enumerate() {
                    if let Some(sub) = tuple.get(index) {
                        check(source, item, sub, lines, out);
                    }
                }
            }
            Some(sub) if sub.is_object() => {
                for item in items {
                    check(source, item, sub, lines, out);
                }
            }
            _ => {}
        },
        Json::Number { span } => check_number(source, *span, schema, lines, out),
        Json::String { content, .. } => {
            let text = decode_string(source, *content);
            check_length(text.chars().count() as u64, schema, node.span(), lines, out);
            check_pattern(&text, schema, node.span(), lines, out);
        }
        _ => {}
    }
}

/// `pattern` 校验：ECMA-262 正则，锚定语义（未匹配即告警）。
fn check_pattern(text: &str, schema: &Value, span: Span, lines: &LineIndex<'_>, out: &mut Vec<Value>) {
    let Some(pattern) = schema.get("pattern").and_then(Value::as_str) else {
        return;
    };

    match regex_lite::Regex::new(pattern) {
        Ok(re) => {
            if !re.is_match(text) {
                out.push(diagnostic(lines, span, level::WARNING, format!("取值不匹配模式 `{pattern}`")));
            }
        }
        // schema 本身的 pattern 写错了：不阻塞编辑，只提示一次
        Err(error) => out.push(diagnostic(lines, span, level::WARNING, format!("schema 的 `pattern` 无效：{error}"))),
    }
}

/// 数值范围校验。
fn check_number(source: &str, span: Span, schema: &Value, lines: &LineIndex<'_>, out: &mut Vec<Value>) {
    let Ok(number) = span.text(source).parse::<f64>() else {
        return;
    };

    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64) {
        if number < minimum {
            out.push(diagnostic(lines, span, level::WARNING, format!("数值应不小于 {minimum}")));
        }
    }
    if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64) {
        if number > maximum {
            out.push(diagnostic(lines, span, level::WARNING, format!("数值应不大于 {maximum}")));
        }
    }
    if let Some(minimum) = schema.get("exclusiveMinimum").and_then(Value::as_f64) {
        if number <= minimum {
            out.push(diagnostic(lines, span, level::WARNING, format!("数值应大于 {minimum}")));
        }
    }
    if let Some(maximum) = schema.get("exclusiveMaximum").and_then(Value::as_f64) {
        if number >= maximum {
            out.push(diagnostic(lines, span, level::WARNING, format!("数值应小于 {maximum}")));
        }
    }
}

/// 字符串长度校验。
fn check_length(length: u64, schema: &Value, span: Span, lines: &LineIndex<'_>, out: &mut Vec<Value>) {
    if let Some(min) = schema.get("minLength").and_then(Value::as_u64) {
        if length < min {
            out.push(diagnostic(lines, span, level::WARNING, format!("长度应不少于 {min}")));
        }
    }
    if let Some(max) = schema.get("maxLength").and_then(Value::as_u64) {
        if length > max {
            out.push(diagnostic(lines, span, level::WARNING, format!("长度应不超过 {max}")));
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
        (Json::String { content, .. }, Value::String(text)) => decode_string(source, *content) == *text,
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
fn diagnostic(lines: &LineIndex<'_>, span: Span, severity: u8, message: impl Into<String>) -> Value {
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
        Value::Array(list) => list.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" | "),
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
        assert!(found.iter().any(|m| m.contains("类型应为 string")), "{found:?}");
    }

    /// required 缺失要报错。
    #[test]
    fn required_missing() {
        let schema = r#"{ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }"#;
        assert!(messages(schema, r#"{ "other": 1 }"#).iter().any(|m| m.contains("缺少必需属性 `name`")));
        assert!(messages(schema, r#"{ "name": "ok" }"#).is_empty());
    }

    /// enum 与 const 命中则无诊断。
    #[test]
    fn enum_and_const() {
        assert!(messages(r#"{ "enum": ["a", "b"] }"#, "\"c\"").iter().any(|m| m.contains("允许集合")));
        assert!(messages(r#"{ "enum": ["a", "b"] }"#, "\"a\"").is_empty());
        assert!(messages(r#"{ "const": 5 }"#, "5").is_empty());
    }

    /// 数值范围与字符串长度。
    #[test]
    fn ranges_and_lengths() {
        assert!(messages(r#"{ "type": "number", "minimum": 10 }"#, "3").iter().any(|m| m.contains("不小于")));
        assert!(messages(r#"{ "type": "string", "maxLength": 2 }"#, "\"abc\"").iter().any(|m| m.contains("不超过")));
    }

    /// properties 递归到嵌套值。
    #[test]
    fn nested_properties() {
        let schema = r#"{ "type": "object", "properties": { "port": { "type": "integer" } } }"#;
        assert!(messages(schema, r#"{ "port": 1.5 }"#).iter().any(|m| m.contains("类型应为 integer")));
        assert!(messages(schema, r#"{ "port": 8080 }"#).is_empty());
    }

    /// additionalProperties: false 拒绝未知键。
    #[test]
    fn additional_properties_false() {
        let schema = r#"{ "type": "object", "properties": { "a": {} }, "additionalProperties": false }"#;
        assert!(messages(schema, r#"{ "a": 1, "b": 2 }"#).iter().any(|m| m.contains("不应存在的属性 `b`")));
    }

    /// items 对数组元素递归。
    #[test]
    fn array_items() {
        let schema = r#"{ "type": "array", "items": { "type": "integer" } }"#;
        assert!(messages(schema, "[1, \"x\"]").iter().any(|m| m.contains("类型应为 integer")));
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
        assert!(invalid.iter().any(|m| m.contains("`pattern` 无效")), "{invalid:?}");
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
        assert!(labels.contains(&"name".to_string()) && labels.contains(&"age".to_string()), "{labels:?}");
        assert!(labels.contains(&"$schema".to_string()));

        // 已有的键不再重复给
        let partial = completion_labels(schema, "{ \"name\": \"x\", }", 16);
        assert!(!partial.contains(&"name".to_string()) && partial.contains(&"age".to_string()), "{partial:?}");
    }

    /// 冒号后的值位置：按属性子 schema 给枚举值。
    #[test]
    fn completes_enum_values() {
        let schema = r#"{ "type": "object", "properties": { "env": { "enum": ["dev", "prod"] } } }"#;
        let offset = (r#"{ "env": "#).len() as u32;
        let labels = completion_labels(schema, r#"{ "env": "#, offset);
        assert!(labels.contains(&"dev".to_string()) && labels.contains(&"prod".to_string()), "{labels:?}");
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
        ] {
            let schema = builtin_schema(uri).unwrap_or_else(|| panic!("{uri} 应命中内置 schema"));
            assert!(schema["properties"].get(key).is_some(), "{uri} 缺少 {key}");
        }
    }
}
