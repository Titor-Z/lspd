//! 编译期生成的 SVG 标签与属性表。

include!(concat!(env!("OUT_DIR"), "/svg_data.rs"));

/// 按名称查标签。
pub fn tag(name: &str) -> Option<&'static TagData> {
    TAGS.binary_search_by(|e| e.name.cmp(name)).ok().map(|i| &TAGS[i])
}

/// 按名称查全局属性。
pub fn global_attribute(name: &str) -> Option<&'static AttributeData> {
    GLOBAL_ATTRIBUTES.binary_search_by(|e| e.name.cmp(name)).ok().map(|i| &GLOBAL_ATTRIBUTES[i])
}

/// 查标签专属或全局属性。
pub fn attribute(tag_name: &str, attribute_name: &str) -> Option<&'static AttributeData> {
    tag(tag_name)
        .and_then(|t| t.attributes.binary_search_by(|e| e.name.cmp(attribute_name)).ok().map(|i| &t.attributes[i]))
        .or_else(|| global_attribute(attribute_name))
}
