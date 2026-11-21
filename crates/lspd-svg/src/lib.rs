//! SVG 语言服务。
//!
//! SVG 是 XML 方言：复用 `lspd-xml` 的容错解析、大纲、折叠、格式化，
//! 在 hover/completion 上叠加 SVG 静态词表（build.rs 生成）。

mod data;

use lspd_core::Backend;
use lspd_xml::{XmlHooks, backend_with_hooks};

/// 创建 SVG 后端。
pub fn backend() -> Box<dyn Backend> {
    backend_with_hooks("svg", &["svg", "xml"], &[".svg"], SvgHooks)
}

/// SVG 词表钩子。
struct SvgHooks;

impl XmlHooks for SvgHooks {
    fn tag_doc(&self, tag: &str) -> Option<String> {
        data::tag(tag).map(|t| format!("### `<{}>`\n\n{}", t.name, t.description))
    }

    fn attribute_doc(&self, tag: &str, attribute: &str) -> Option<String> {
        data::attribute(tag, attribute).map(|a| format!("### `{}`\n\n{}", a.name, a.description))
    }

    fn complete_tags(&self, prefix: &str) -> Vec<String> {
        data::TAGS.iter().map(|t| t.name.to_string()).filter(|n| prefix.is_empty() || n.starts_with(prefix)).collect()
    }

    fn complete_attributes(&self, tag: &str, prefix: &str) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(t) = data::tag(tag) {
            for a in t.attributes { out.push(a.name.to_string()); }
        }
        for a in data::GLOBAL_ATTRIBUTES { out.push(a.name.to_string()); }
        out.retain(|n| prefix.is_empty() || n.starts_with(prefix));
        out.sort();
        out.dedup();
        out
    }
}
