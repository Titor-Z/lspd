//! 编译期生成的 CSS 数据表。
//!
//! 生成物来自 `build.rs`：全部落在只读段，查询按名称二分，不分配内存。
//! 数据源为 vscode-css-languageservice 与 @vscode/web-custom-data（均为 MIT）。

include!(concat!(env!("OUT_DIR"), "/css_data.rs"));

/// 按名称查属性。
pub fn property(name: &str) -> Option<&'static EntryData> {
    lookup(PROPERTIES, name)
}

/// 按名称查 at 规则（名称含 `@`）。
pub fn at_rule(name: &str) -> Option<&'static EntryData> {
    lookup(AT_RULES, name)
}

/// 按名称查伪类（名称含 `:`）。
pub fn pseudo_class(name: &str) -> Option<&'static EntryData> {
    lookup(PSEUDO_CLASSES, name)
}

/// 按名称查伪元素（名称含 `::`）。
pub fn pseudo_element(name: &str) -> Option<&'static EntryData> {
    lookup(PSEUDO_ELEMENTS, name)
}

/// 按名称查命名颜色。
pub fn named_color(name: &str) -> Option<&'static ColorData> {
    NAMED_COLORS.binary_search_by(|entry| entry.name.cmp(name)).ok().map(|index| &NAMED_COLORS[index])
}

/// 属性是否合法。带厂商前缀时按去前缀后的名称判断。
pub fn is_known_property(name: &str) -> bool {
    if property(name).is_some() {
        return true;
    }
    // 自定义属性与方言变量由上层单独判定
    if name.starts_with("--") || name.starts_with('$') || name.starts_with('@') {
        return true;
    }
    // -webkit-xxx / -moz-xxx / -ms-xxx / -o-xxx 按其后的标准名判断
    strip_vendor_prefix(name).is_some_and(|rest| property(rest).is_some())
}

/// 去掉厂商前缀。
fn strip_vendor_prefix(name: &str) -> Option<&str> {
    ["-webkit-", "-moz-", "-ms-", "-o-"].iter().find_map(|prefix| name.strip_prefix(prefix))
}

/// 二分查找数据项。
fn lookup(table: &'static [EntryData], name: &str) -> Option<&'static EntryData> {
    table.binary_search_by(|entry| entry.name.cmp(name)).ok().map(|index| &table[index])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 属性表应可查询，且带值语法。
    #[test]
    fn looks_up_property() {
        let color = property("color").expect("应存在 color 属性");
        assert!(!color.description.is_empty());
        assert!(!color.syntax.is_empty());
        assert!(property("not-a-property").is_none());
    }

    /// 厂商前缀属性按其标准名判定。
    #[test]
    fn accepts_vendor_prefix() {
        assert!(is_known_property("-webkit-transform"));
        assert!(is_known_property("--custom-prop"));
        assert!(!is_known_property("colr"));
    }

    /// 命名颜色表可用于颜色提供者。
    #[test]
    fn looks_up_color() {
        let red = named_color("red").expect("应存在 red");
        assert_eq!((red.red, red.green, red.blue), (255, 0, 0));
    }
}
