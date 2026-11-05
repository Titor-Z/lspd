//! 扁平语法树。
//!
//! 所有节点放在一个 `Vec` 里，用 `u32` 索引互相引用：
//! - 没有 `Rc`/`RefCell`，没有每节点一次堆分配；
//! - 属性集中在另一个 `Vec`，节点只记录起始下标与数量；
//! - 子节点用"首子 + 下一兄弟"链表达，避免每个节点各带一个 `Vec`；
//! - 节点按文档顺序创建，因此一个元素的子树是连续下标区间，遍历不需要分配。

use lspd_core::text::Span;

/// 空索引。用 `u32::MAX` 表示"没有"。
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
    /// 文档类型声明
    Doctype,
}

/// 一个节点。
#[derive(Debug, Clone, Copy)]
pub struct Node {
    /// 节点类型
    pub kind: NodeKind,
    /// 节点整体区间（元素含起止标签）
    pub span: Span,
    /// 元素名区间；非元素为空区间
    pub name: Span,
    /// 父节点索引
    pub parent: u32,
    /// 首个子节点
    pub first_child: u32,
    /// 末个子节点
    pub last_child: u32,
    /// 下一个兄弟节点
    pub next_sibling: u32,
    /// 属性在属性池中的起始下标
    pub attribute_start: u32,
    /// 属性个数
    pub attribute_len: u32,
    /// 子树结束下标（不含），闭区间遍历用
    pub subtree_end: u32,
}

impl Node {
    /// 是否为元素。
    pub fn is_element(&self) -> bool {
        self.kind == NodeKind::Element
    }
}

/// 元素上的一个属性。
#[derive(Debug, Clone, Copy)]
pub struct Attribute {
    /// 属性整体区间
    pub span: Span,
    /// 属性名区间
    pub name: Span,
    /// 属性值区间；`None` 表示无值属性（如 `disabled`）
    pub value: Option<Span>,
}

/// 一棵解析完成的语法树。
#[derive(Default)]
pub struct Ast {
    /// 节点池，按文档顺序
    pub nodes: Vec<Node>,
    /// 属性池
    pub attributes: Vec<Attribute>,
    /// 顶层节点
    pub roots: Vec<u32>,
}

impl Ast {
    /// 按索引取节点。
    pub fn node(&self, index: u32) -> &Node {
        &self.nodes[index as usize]
    }

    /// 按索引取可变节点，供解析期回填区间使用。
    pub fn node_mut(&mut self, index: u32) -> &mut Node {
        &mut self.nodes[index as usize]
    }

    /// 取节点的属性切片。
    pub fn attributes_of(&self, node: &Node) -> &[Attribute] {
        let start = node.attribute_start as usize;
        &self.attributes[start..start + node.attribute_len as usize]
    }

    /// 遍历某个节点的直接子节点。
    pub fn children(&self, index: u32) -> ChildIter<'_> {
        ChildIter {
            ast: self,
            next: self.node(index).first_child,
        }
    }

    /// 按文档顺序遍历所有元素。
    pub fn elements(&self) -> impl Iterator<Item = u32> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.is_element())
            .map(|(index, _)| index as u32)
    }

    /// 找到包含指定偏移的最深元素。
    pub fn element_at(&self, offset: u32) -> Option<u32> {
        self.elements()
            .filter(|index| self.node(*index).span.contains(offset))
            .max_by_key(|index| self.node(*index).span.start)
    }
}

/// 直接子节点迭代器，不分配内存。
pub struct ChildIter<'a> {
    /// 所属语法树
    ast: &'a Ast,
    /// 下一个兄弟索引
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
