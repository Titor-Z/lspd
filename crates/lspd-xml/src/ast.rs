//! 扁平 XML 语法树。

use lspd_core::text::Span;

/// 空索引。
pub const NONE: u32 = u32::MAX;

/// 节点类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// 元素
    Element,
    /// 文本
    Text,
    /// 注释
    Comment,
    /// CDATA
    CData,
    /// 处理指令
    ProcessingInstruction,
    /// DOCTYPE
    Doctype,
}

/// 一个节点。
#[derive(Debug, Clone, Copy)]
pub struct Node {
    /// 节点类型
    pub kind: NodeKind,
    /// 节点整体区间
    pub span: Span,
    /// 名称区间；非命名节点为空区间
    pub name: Span,
    /// 父节点
    pub parent: u32,
    /// 首子
    pub first_child: u32,
    /// 末子
    pub last_child: u32,
    /// 下一兄弟
    pub next_sibling: u32,
    /// 属性起始
    pub attribute_start: u32,
    /// 属性数量
    pub attribute_len: u32,
    /// 子树结束下标（不含）
    pub subtree_end: u32,
}

impl Node {
    /// 是否为元素。
    pub fn is_element(&self) -> bool {
        self.kind == NodeKind::Element
    }
}

/// 元素属性。
#[derive(Debug, Clone, Copy)]
pub struct Attribute {
    /// 整体区间
    pub span: Span,
    /// 名称区间
    pub name: Span,
    /// 值区间（含引号）；None 表示无值
    pub value: Option<Span>,
}

/// 语法树。
#[derive(Default)]
pub struct Ast {
    /// 节点池
    pub nodes: Vec<Node>,
    /// 属性池
    pub attributes: Vec<Attribute>,
    /// 根节点
    pub roots: Vec<u32>,
}

impl Ast {
    /// 取节点。
    pub fn node(&self, index: u32) -> &Node {
        &self.nodes[index as usize]
    }

    /// 取可变节点。
    pub fn node_mut(&mut self, index: u32) -> &mut Node {
        &mut self.nodes[index as usize]
    }

    /// 节点属性。
    pub fn attributes_of(&self, node: &Node) -> &[Attribute] {
        let start = node.attribute_start as usize;
        &self.attributes[start..start + node.attribute_len as usize]
    }

    /// 子节点迭代。
    pub fn children(&self, index: u32) -> ChildIter<'_> {
        ChildIter { ast: self, next: self.node(index).first_child }
    }

    /// 所有元素。
    pub fn elements(&self) -> impl Iterator<Item = u32> + '_ {
        self.nodes.iter().enumerate().filter(|(_, n)| n.is_element()).map(|(i, _)| i as u32)
    }

    /// 包含偏移的最深元素。
    pub fn element_at(&self, offset: u32) -> Option<u32> {
        self.elements()
            .filter(|index| self.node(*index).span.contains(offset))
            .max_by_key(|index| self.node(*index).span.start)
    }
}

/// 子节点迭代器。
pub struct ChildIter<'a> {
    ast: &'a Ast,
    next: u32,
}

impl Iterator for ChildIter<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        let current = self.next;
        if current == NONE {
            return None;
        }
        self.next = self.ast.node(current).next_sibling;
        Some(current)
    }
}
