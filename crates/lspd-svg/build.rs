//! 由 `data/svg-data.json` 生成静态标签与属性表。

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let data_path = manifest.join("data").join("svg-data.json");
    println!("cargo:rerun-if-changed={}", data_path.display());

    let raw = fs::read_to_string(&data_path).expect("读取 SVG 数据失败");
    let root: Value = serde_json::from_str(&raw).expect("解析 SVG 数据失败");

    let mut tags = collect_tags(&root);
    tags.sort_by(|a, b| a.0.cmp(&b.0));
    let mut globals = collect_attributes(root.get("globalAttributes"));
    globals.sort_by(|a, b| a.0.cmp(&b.0));

    let mut output = String::with_capacity(64 * 1024);
    output.push_str("// 由 build.rs 生成，请勿手工修改。\n\n");
    output.push_str("/// 属性。\npub struct AttributeData {\n    pub name: &'static str,\n    pub description: &'static str,\n}\n\n");
    output.push_str("/// 标签。\npub struct TagData {\n    pub name: &'static str,\n    pub description: &'static str,\n    pub attributes: &'static [AttributeData],\n}\n\n");
    output.push_str("pub static TAGS: &[TagData] = &[\n");
    for (name, description, attributes) in &tags {
        let _ = writeln!(output, "    TagData {{ name: {:?}, description: {:?}, attributes: {} }},", name, description, attrs_ref(attributes));
    }
    output.push_str("];\n\npub static GLOBAL_ATTRIBUTES: &[AttributeData] = &[\n");
    for (name, description) in &globals {
        let _ = writeln!(output, "    AttributeData {{ name: {:?}, description: {:?} }},", name, description);
    }
    output.push_str("];\n");

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("svg_data.rs");
    fs::write(out, output).expect("写出生成代码失败");
}

type Attr = (String, String);

fn collect_tags(root: &Value) -> Vec<(String, String, Vec<Attr>)> {
    let Some(items) = root.get("tags").and_then(Value::as_array) else { return Vec::new() };
    items.iter().filter_map(|item| {
        let name = item.get("name")?.as_str()?.to_string();
        let mut attrs = collect_attributes(item.get("attributes"));
        attrs.sort_by(|a, b| a.0.cmp(&b.0));
        Some((name, desc(item), attrs))
    }).collect()
}

fn collect_attributes(items: Option<&Value>) -> Vec<Attr> {
    let Some(items) = items.and_then(Value::as_array) else { return Vec::new() };
    items.iter().filter_map(|item| Some((item.get("name")?.as_str()?.to_string(), desc(item)))).collect()
}

fn desc(item: &Value) -> String {
    item.get("description").and_then(Value::as_str).unwrap_or("").to_string()
}

fn attrs_ref(attrs: &[Attr]) -> String {
    if attrs.is_empty() { return "&[]".to_string(); }
    let mut s = String::from("&[");
    for (name, description) in attrs {
        let _ = write!(s, "AttributeData {{ name: {:?}, description: {:?} }},", name, description);
    }
    s.push(']');
    s
}
