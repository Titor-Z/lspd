//! 结构诊断与数据驱动诊断。

use serde_json::Value;

use crate::data;
use crate::features::{View, diagnostic};
use crate::parser::{Diagnostic, Severity};

/// 单次发布的诊断上限，避免文件大面积报错时刷屏。
const MAX_DIAGNOSTICS: usize = 200;

/// LSP 的严重程度取值。
mod level {
    /// 错误
    pub const ERROR: u8 = 1;
    /// 警告
    pub const WARNING: u8 = 2;
}

/// 汇总一份文档的全部诊断。
pub fn collect(view: &View<'_>, structural: &[Diagnostic]) -> Vec<Value> {
    let mut diagnostics: Vec<Value> = Vec::new();

    for item in structural {
        let severity = match item.severity {
            Severity::Error => level::ERROR,
            Severity::Warning => level::WARNING,
        };
        diagnostics.push(diagnostic(view, item.span, severity, item.message.clone()));
        if diagnostics.len() >= MAX_DIAGNOSTICS {
            return diagnostics;
        }
    }

    for index in view.ast.elements() {
        let node = view.ast.node(index);
        let tag_name = view.text(node.name).to_ascii_lowercase();

        if is_unknown_tag(&tag_name) {
            diagnostics.push(diagnostic(
                view,
                node.name,
                level::WARNING,
                format!("未知标签 <{tag_name}>"),
            ));
            if diagnostics.len() >= MAX_DIAGNOSTICS {
                return diagnostics;
            }
        }

        for attribute in view.ast.attributes_of(node) {
            let attribute_name = view.text(attribute.name).to_ascii_lowercase();
            if is_custom_name(&attribute_name)
                || data::is_known_attribute(&tag_name, &attribute_name)
            {
                continue;
            }
            // 未知属性只在已知标签上报，避免自定义元素连带刷屏
            if data::is_known_tag(&tag_name) {
                diagnostics.push(diagnostic(
                    view,
                    attribute.name,
                    level::WARNING,
                    format!("<{tag_name}> 不支持属性 `{attribute_name}`"),
                ));
                if diagnostics.len() >= MAX_DIAGNOSTICS {
                    return diagnostics;
                }
            }
        }
    }

    diagnostics
}

/// 未知标签判定：数据表里没有，且不是自定义元素（含连字符）。
fn is_unknown_tag(name: &str) -> bool {
    !data::is_known_tag(name) && !name.contains('-')
}

/// 自定义属性判定：含连字符（`data-*`、`aria-*`、`x-*`）或含命名空间前缀。
fn is_custom_name(name: &str) -> bool {
    name.contains('-') || name.contains(':') || name.contains('@') || name.starts_with("v-")
}
