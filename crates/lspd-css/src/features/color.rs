//! 颜色提供者：识别声明值里的十六进制与命名颜色，并给出多种表示法。

use serde_json::{json, Value};

use crate::ast::NodeKind;
use crate::data;
use crate::features::View;
use lspd_core::text::Span;

/// 带透明度的颜色分量，取值 0.0~1.0。
pub struct Rgba {
    /// 红
    pub red: f64,
    /// 绿
    pub green: f64,
    /// 蓝
    pub blue: f64,
    /// 透明度
    pub alpha: f64,
}

/// 收集文档中出现的颜色。只扫描声明值，避免把类名中的颜色单词误判为颜色。
pub fn document_colors(view: &View<'_>) -> Value {
    let mut colors = Vec::new();

    for index in view.ast.nodes_of(NodeKind::Declaration) {
        let value = view.ast.node(index).value;
        scan_value(view, value, &mut colors);
    }

    Value::Array(colors)
}

/// 扫描一段值，产出颜色与区间。
fn scan_value(view: &View<'_>, span: Span, colors: &mut Vec<Value>) {
    let text = view.text(span);
    let bytes = text.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'#' => {
                let (length, color) = read_hex(&bytes[index + 1..]);
                if let Some(color) = color {
                    let start = span.start + index as u32;
                    colors.push(json!({
                        "range": view.range(Span { start, end: start + 1 + length as u32 }),
                        "color": color_json(&color),
                    }));
                }
                index += 1 + length;
            }
            byte if is_ident_start(byte) => {
                let start = index;
                while index < bytes.len() && is_ident_byte(bytes[index]) {
                    index += 1;
                }
                let word = &text[start..index];
                if let Some(named) = data::named_color(&word.to_ascii_lowercase()) {
                    let offset = span.start + start as u32;
                    let color = Rgba {
                        red: named.red as f64 / 255.0,
                        green: named.green as f64 / 255.0,
                        blue: named.blue as f64 / 255.0,
                        alpha: 1.0,
                    };
                    colors.push(json!({
                        "range": view.range(Span { start: offset, end: offset + word.len() as u32 }),
                        "color": color_json(&color),
                    }));
                }
            }
            _ => index += 1,
        }
    }
}

/// 读取十六进制颜色，返回消耗的字符数与解析结果。
fn read_hex(text: &[u8]) -> (usize, Option<Rgba>) {
    let mut length = 0;
    while length < text.len() && text[length].is_ascii_hexdigit() {
        length += 1;
    }

    let parse = |pair: &[u8]| u8::from_str_radix(std::str::from_utf8(pair).unwrap_or("0"), 16).unwrap_or(0);
    let unit = |byte: u8| byte as f64 / 255.0;

    let color = match length {
        3 | 4 => {
            let expand = |byte: u8| byte * 17;
            let red = expand(parse(&text[0..1]));
            let green = expand(parse(&text[1..2]));
            let blue = expand(parse(&text[2..3]));
            let alpha = if length == 4 { unit(expand(parse(&text[3..4]))) } else { 1.0 };
            Some(Rgba { red: unit(red), green: unit(green), blue: unit(blue), alpha })
        }
        6 | 8 => {
            let red = parse(&text[0..2]);
            let green = parse(&text[2..4]);
            let blue = parse(&text[4..6]);
            let alpha = if length == 8 { unit(parse(&text[6..8])) } else { 1.0 };
            Some(Rgba { red: unit(red), green: unit(green), blue: unit(blue), alpha })
        }
        _ => None,
    };

    (length, color)
}

/// 颜色对象转 JSON。
fn color_json(color: &Rgba) -> Value {
    json!({
        "red": color.red,
        "green": color.green,
        "blue": color.blue,
        "alpha": color.alpha,
    })
}

/// 给出同一颜色的多种写法，供编辑器选择。
pub fn presentations(view: &View<'_>, span: Span, color: &Rgba) -> Value {
    let red = to_channel(color.red);
    let green = to_channel(color.green);
    let blue = to_channel(color.blue);
    let opacity = color.alpha.clamp(0.0, 1.0);

    let hex = if (opacity - 1.0).abs() < f64::EPSILON {
        format!("#{red:02x}{green:02x}{blue:02x}")
    } else {
        format!("#{red:02x}{green:02x}{blue:02x}{:02x}", to_channel(opacity))
    };
    let rgb = if (opacity - 1.0).abs() < f64::EPSILON {
        format!("rgb({red}, {green}, {blue})")
    } else {
        format!("rgba({red}, {green}, {blue}, {opacity:.2})")
    };
    let hsl = to_hsl(red, green, blue, opacity);

    let range = view.range(span);
    json!([
        { "label": hex, "textEdit": { "range": range, "newText": hex } },
        { "label": rgb, "textEdit": { "range": range, "newText": rgb } },
        { "label": hsl, "textEdit": { "range": range, "newText": hsl } },
    ])
}

/// 0~1 的分量转 0~255。
fn to_channel(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// 转成 `hsl()` 表示。
fn to_hsl(red: u8, green: u8, blue: u8, alpha: f64) -> String {
    let (r, g, b) = (red as f64 / 255.0, green as f64 / 255.0, blue as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;

    let delta = max - min;
    let saturation = if delta.abs() < f64::EPSILON {
        0.0
    } else {
        delta / (1.0 - (2.0 * lightness - 1.0).abs())
    };

    let hue = if delta.abs() < f64::EPSILON {
        0.0
    } else if (max - r).abs() < f64::EPSILON {
        60.0 * (((g - b) / delta) % 6.0)
    } else if (max - g).abs() < f64::EPSILON {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };

    let hue = if hue < 0.0 { hue + 360.0 } else { hue };
    let saturation = saturation * 100.0;
    let lightness = lightness * 100.0;

    if (alpha - 1.0).abs() < f64::EPSILON {
        format!("hsl({hue:.0}, {saturation:.0}%, {lightness:.0}%)")
    } else {
        format!("hsla({hue:.0}, {saturation:.0}%, {lightness:.0}%, {alpha:.2})")
    }
}

/// 标识符首字符。
fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte >= 0x80
}

/// 标识符字符。
fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') || byte >= 0x80
}
