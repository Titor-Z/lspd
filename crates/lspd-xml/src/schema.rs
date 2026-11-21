//! XML SchemaProvider 抽象：为 hover/completion 提供词表与文档。
//!
//! 这是最小子集接口，后续可由内置静态词表、本地 XSD 子集或 DTD 子集实现。
//! 设计上借鉴 JSON schema 的"定位 -> 返回信息"流程，但模型面向 XML 元素/属性。

/// 标签/属性信息。
#[derive(Debug, Clone, Default)]
pub struct NodeDoc {
    /// Markdown 文档
    pub markdown: String,
}

/// Schema 提供者。
pub trait SchemaProvider: Send + Sync {
    /// 标签文档。
    fn tag_doc(&self, _namespace: Option<&str>, _tag: &str) -> Option<NodeDoc> { None }
    /// 属性文档。
    fn attribute_doc(&self, _namespace: Option<&str>, _tag: &str, _attribute: &str) -> Option<NodeDoc> { None }
    /// 标签补全候选（不含前缀过滤由调用方处理也可处理）。
    fn complete_tags(&self, _namespace: Option<&str>, _prefix: &str) -> Vec<String> { Vec::new() }
    /// 属性补全候选。
    fn complete_attributes(&self, _namespace: Option<&str>, _tag: &str, _prefix: &str) -> Vec<String> { Vec::new() }
}

/// 空实现。
pub struct NoSchema;
impl SchemaProvider for NoSchema {}
