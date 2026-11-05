//! 由 `data/html-data.json` 生成静态标签与属性表。
//!
//! 数据来源：`@vscode/web-custom-data`（经 vscode-html-languageservice 分发，MIT）。
//! 生成物是排序好的静态数组，运行时按名称二分查找，不解析任何 JSON，
//! 也不在堆上保留数据副本；字符串全部落在只读段。

use std::collections::HashMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;

fn main() {
    let manifest =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("构建脚本需要 CARGO_MANIFEST_DIR"));
    let data_path = manifest.join("data").join("html-data.json");
    println!("cargo:rerun-if-changed={}", data_path.display());

    let raw = fs::read_to_string(&data_path).expect("读取 HTML 数据失败");
    let root: Value = serde_json::from_str(&raw).expect("解析 HTML 数据失败");

    // 数据里的属性取值以 `valueSet` 引用共享集合，先拍平成名称到取值的映射
    let value_sets = collect_value_sets(&root);

    let mut tags = collect_tags(&root, &value_sets);
    tags.sort_by(|left, right| left.0.cmp(&right.0));

    let mut globals = collect_attributes(root.get("globalAttributes"), &value_sets);
    globals.sort_by(|left, right| left.0.cmp(&right.0));

    let mut output = String::with_capacity(512 * 1024);
    output.push_str("// 由 build.rs 生成，请勿手工修改。\n\n");

    output.push_str("/// 属性的允许取值。\n");
    output.push_str("pub struct ValueData {\n    /// 取值\n    pub name: &'static str,\n    /// 取值说明\n    pub description: &'static str,\n}\n\n");

    output.push_str("/// 属性。\n");
    output.push_str("pub struct AttributeData {\n    /// 属性名\n    pub name: &'static str,\n    /// 属性说明\n    pub description: &'static str,\n    /// 允许取值，空表示自由文本\n    pub values: &'static [ValueData],\n}\n\n");

    output.push_str("/// 标签。\n");
    output.push_str("pub struct TagData {\n    /// 标签名\n    pub name: &'static str,\n    /// 标签说明\n    pub description: &'static str,\n    /// 该标签专属属性\n    pub attributes: &'static [AttributeData],\n}\n\n");

    output.push_str("/// 全部标签，按名称升序。\n");
    output.push_str("pub static TAGS: &[TagData] = &[\n");
    for (name, description, attributes) in &tags {
        let _ = writeln!(
            output,
            "    TagData {{ name: {:?}, description: {:?}, attributes: {} }},",
            name,
            description,
            attributes_reference(attributes)
        );
    }
    output.push_str("];\n\n");

    output.push_str("/// 全局属性，按名称升序。\n");
    output.push_str("pub static GLOBAL_ATTRIBUTES: &[AttributeData] = &[\n");
    write_attributes(&mut output, &globals);
    output.push_str("];\n");

    let out_path =
        PathBuf::from(env::var("OUT_DIR").expect("构建脚本需要 OUT_DIR")).join("html_data.rs");
    fs::write(&out_path, output).expect("写出生成代码失败");
}

/// 取值集合表：valueSet 名称 → 取值列表。
type ValueSets = HashMap<String, Vec<(String, String)>>;

/// 收集共享的取值集合。
fn collect_value_sets(root: &Value) -> ValueSets {
    let Some(items) = root.get("valueSets").and_then(Value::as_array) else {
        return HashMap::new();
    };

    let mut sets = HashMap::with_capacity(items.len());
    for item in items {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        sets.insert(name.to_string(), collect_values(item.get("values")));
    }
    sets
}

/// 收集标签及其专属属性。
fn collect_tags(
    root: &Value,
    value_sets: &ValueSets,
) -> Vec<(String, String, Vec<AttributeEntry>)> {
    let Some(items) = root.get("tags").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut tags = Vec::with_capacity(items.len());
    for item in items {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        let mut attributes = collect_attributes(item.get("attributes"), value_sets);
        attributes.sort_by(|left, right| left.0.cmp(&right.0));
        tags.push((name.to_string(), description_of(item), attributes));
    }
    tags
}

/// 属性的中间表示。
type AttributeEntry = (String, String, Vec<(String, String)>);

/// 收集一组属性，属性取值可以是内嵌列表，也可以引用共享取值集合。
fn collect_attributes(items: Option<&Value>, value_sets: &ValueSets) -> Vec<AttributeEntry> {
    let Some(items) = items.and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut attributes = Vec::with_capacity(items.len());
    for item in items {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };

        let inline = collect_values(item.get("values"));
        let values = if !inline.is_empty() {
            inline
        } else {
            item.get("valueSet")
                .and_then(Value::as_str)
                .and_then(|set| value_sets.get(set))
                .cloned()
                .unwrap_or_default()
        };

        attributes.push((name.to_string(), description_of(item), values));
    }
    attributes
}

/// 收集属性的允许取值。
fn collect_values(items: Option<&Value>) -> Vec<(String, String)> {
    let Some(items) = items.and_then(Value::as_array) else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            Some((
                item.get("name")?.as_str()?.to_string(),
                description_of(item),
            ))
        })
        .collect()
}

/// 取出说明文本。数据里说明可能是字符串，也可能是 `{kind, value}` 结构。
fn description_of(item: &Value) -> String {
    match item.get("description") {
        Some(Value::String(text)) => text.clone(),
        Some(value) => value
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        None => String::new(),
    }
}

/// 为标签属性生成静态切片。属性挂在标签内部时直接内联展开，避免额外的中间数组。
fn attributes_reference(attributes: &[AttributeEntry]) -> String {
    if attributes.is_empty() {
        return "&[]".to_string();
    }

    let mut text = String::from("&[");
    for (name, description, values) in attributes {
        let _ = write!(
            text,
            "AttributeData {{ name: {:?}, description: {:?}, values: {} }},",
            name,
            description,
            values_reference(values)
        );
    }
    text.push(']');
    text
}

/// 为属性取值生成静态切片。
fn values_reference(values: &[(String, String)]) -> String {
    if values.is_empty() {
        return "&[]".to_string();
    }

    let mut text = String::from("&[");
    for (name, description) in values {
        let _ = write!(
            text,
            "ValueData {{ name: {:?}, description: {:?} }},",
            name, description
        );
    }
    text.push(']');
    text
}

/// 写出一组属性到顶层静态数组。
fn write_attributes(output: &mut String, attributes: &[AttributeEntry]) {
    for (name, description, values) in attributes {
        let _ = writeln!(
            output,
            "    AttributeData {{ name: {:?}, description: {:?}, values: {} }},",
            name,
            description,
            values_reference(values)
        );
    }
}
