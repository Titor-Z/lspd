//! XML 方言钩子：兼容旧接口，内部转发到 `SchemaProvider`。

use crate::schema::{NodeDoc, SchemaProvider};

/// 方言钩子。默认实现不提供额外数据。
pub trait XmlHooks: Send + Sync {
    /// 标签 hover 文档（Markdown）。
    fn tag_doc(&self, _tag: &str) -> Option<String> { None }
    /// 属性 hover 文档（Markdown）。
    fn attribute_doc(&self, _tag: &str, _attribute: &str) -> Option<String> { None }
    /// 标签补全候选。
    fn complete_tags(&self, _prefix: &str) -> Vec<String> { Vec::new() }
    /// 属性补全候选。
    fn complete_attributes(&self, _tag: &str, _prefix: &str) -> Vec<String> { Vec::new() }
}

/// 无额外数据的默认钩子。
pub struct NoHooks;
impl XmlHooks for NoHooks {}

/// 把旧 `XmlHooks` 适配为 `SchemaProvider`。
pub struct HooksAdapter<H: XmlHooks>(pub H);

impl<H: XmlHooks> SchemaProvider for HooksAdapter<H> {
    fn tag_doc(&self, _namespace: Option<&str>, tag: &str) -> Option<NodeDoc> {
        self.0.tag_doc(tag).map(|markdown| NodeDoc { markdown })
    }

    fn attribute_doc(&self, _namespace: Option<&str>, tag: &str, attribute: &str) -> Option<NodeDoc> {
        self.0.attribute_doc(tag, attribute).map(|markdown| NodeDoc { markdown })
    }

    fn complete_tags(&self, _namespace: Option<&str>, prefix: &str) -> Vec<String> {
        self.0.complete_tags(prefix)
    }

    fn complete_attributes(&self, _namespace: Option<&str>, tag: &str, prefix: &str) -> Vec<String> {
        self.0.complete_attributes(tag, prefix)
    }
}

