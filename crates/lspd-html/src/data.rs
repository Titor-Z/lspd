//! 编译期生成的标签与属性表。
//!
//! 生成物来自 `build.rs`，全部落在只读段；查询用二分查找，不分配内存。
//! 数据源为 `@vscode/web-custom-data`（MIT），说明文本保持英文原文。

include!(concat!(env!("OUT_DIR"), "/html_data.rs"));

/// 按名称查标签。表按名称升序，直接二分。
pub fn tag(name: &str) -> Option<&'static TagData> {
    TAGS.binary_search_by(|entry| entry.name.cmp(name))
        .ok()
        .map(|index| &TAGS[index])
}

/// 按名称查全局属性。
pub fn global_attribute(name: &str) -> Option<&'static AttributeData> {
    GLOBAL_ATTRIBUTES
        .binary_search_by(|entry| entry.name.cmp(name))
        .ok()
        .map(|index| &GLOBAL_ATTRIBUTES[index])
}

/// 给定标签下的属性数据：先查标签专属，再查全局。
pub fn attribute(tag_name: &str, attribute_name: &str) -> Option<&'static AttributeData> {
    tag(tag_name)
        .and_then(|tag| {
            tag.attributes
                .binary_search_by(|entry| entry.name.cmp(attribute_name))
                .ok()
                .map(|index| &tag.attributes[index])
        })
        .or_else(|| global_attribute(attribute_name))
}

/// 标签是否在数据表中。
pub fn is_known_tag(name: &str) -> bool {
    tag(name).is_some()
}

/// 属性是否在该标签上合法。
pub fn is_known_attribute(tag_name: &str, attribute_name: &str) -> bool {
    attribute(tag_name, attribute_name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 数据表应能按名称查到标签。
    #[test]
    fn looks_up_tag() {
        assert!(is_known_tag("div"));
        assert!(tag("a").is_some_and(|item| !item.description.is_empty()));
        assert!(!is_known_tag("not-a-real-tag"));
    }

    /// 属性查找应覆盖标签专属与全局两类。
    #[test]
    fn looks_up_attribute() {
        assert!(is_known_attribute("a", "href"));
        assert!(is_known_attribute("div", "class"));
        assert!(!is_known_attribute("div", "href"));
    }
}
