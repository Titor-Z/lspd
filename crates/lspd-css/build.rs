//! 由 `data/css-data.json` 与 `data/colors.json` 生成静态表。
//!
//! 数据来源：vscode-css-languageservice（MIT）与 @vscode/web-custom-data。
//! 生成物为按名称升序的静态数组，运行时二分查找，不解析 JSON，字符串落在只读段。

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("构建脚本需要 CARGO_MANIFEST_DIR"));

    let data_path = manifest.join("data").join("css-data.json");
    let colors_path = manifest.join("data").join("colors.json");
    println!("cargo:rerun-if-changed={}", data_path.display());
    println!("cargo:rerun-if-changed={}", colors_path.display());

    let root: Value = serde_json::from_str(&fs::read_to_string(&data_path).expect("读取 CSS 数据失败"))
        .expect("解析 CSS 数据失败");
    let colors: Value = serde_json::from_str(&fs::read_to_string(&colors_path).expect("读取颜色数据失败"))
        .expect("解析颜色数据失败");

    let mut output = String::with_capacity(512 * 1024);
    output.push_str("// 由 build.rs 生成，请勿手工修改。\n\n");

    output.push_str("/// 一条 CSS 数据项。\n");
    output.push_str("pub struct EntryData {\n    /// 名称\n    pub name: &'static str,\n    /// 说明\n    pub description: &'static str,\n    /// 值语法，仅属性有\n    pub syntax: &'static str,\n    /// 所属 at 规则，仅部分属性有\n    pub at_rule: &'static str,\n}\n\n");

    output.push_str("/// 命名颜色。\n");
    output.push_str("pub struct ColorData {\n    /// 颜色名\n    pub name: &'static str,\n    /// 红色分量\n    pub red: u8,\n    /// 绿色分量\n    pub green: u8,\n    /// 蓝色分量\n    pub blue: u8,\n}\n\n");

    write_entries(&mut output, "PROPERTIES", "全部属性，按名称升序。", root.get("properties"), true);
    write_entries(&mut output, "AT_RULES", "全部 at 规则（名称含 `@`），按名称升序。", root.get("atDirectives"), false);
    write_entries(&mut output, "PSEUDO_CLASSES", "全部伪类（名称含 `:`），按名称升序。", root.get("pseudoClasses"), false);
    write_entries(&mut output, "PSEUDO_ELEMENTS", "全部伪元素（名称含 `::`），按名称升序。", root.get("pseudoElements"), false);

    output.push_str("/// 命名颜色，按名称升序。\n");
    output.push_str("pub static NAMED_COLORS: &[ColorData] = &[\n");
    let mut items: Vec<(String, u8, u8, u8)> = Vec::new();
    if let Some(list) = colors.as_array() {
        for item in list {
            let Some(name) = item.get(0).and_then(Value::as_str) else {
                continue;
            };
            let channel = |index: usize| item.get(index).and_then(Value::as_u64).unwrap_or(0) as u8;
            items.push((name.to_string(), channel(1), channel(2), channel(3)));
        }
    }
    items.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, red, green, blue) in &items {
        let _ = writeln!(
            output,
            "    ColorData {{ name: {:?}, red: {}, green: {}, blue: {} }},",
            name, red, green, blue
        );
    }
    output.push_str("];\n");

    let out_path = PathBuf::from(env::var("OUT_DIR").expect("构建脚本需要 OUT_DIR")).join("css_data.rs");
    fs::write(&out_path, output).expect("写出生成代码失败");
}

/// 写出一组数据项，输出前按名称排序。
fn write_entries(output: &mut String, name: &str, doc: &str, source: Option<&Value>, with_syntax: bool) {
    let mut items: Vec<(String, String, String, String)> = Vec::new();

    if let Some(list) = source.and_then(Value::as_array) {
        for item in list {
            let Some(item_name) = item.get("name").and_then(Value::as_str) else {
                continue;
            };
            let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("").to_string();
            let syntax = if with_syntax { text("syntax") } else { String::new() };
            items.push((item_name.to_string(), text("description"), syntax, text("atRule")));
        }
    }
    items.sort_by(|left, right| left.0.cmp(&right.0));

    let _ = writeln!(output, "/// {doc}");
    let _ = writeln!(output, "pub static {name}: &[EntryData] = &[");
    for (item_name, description, syntax, at_rule) in &items {
        let _ = writeln!(
            output,
            "    EntryData {{ name: {:?}, description: {:?}, syntax: {:?}, at_rule: {:?} }},",
            item_name, description, syntax, at_rule
        );
    }
    output.push_str("];\n\n");
}
