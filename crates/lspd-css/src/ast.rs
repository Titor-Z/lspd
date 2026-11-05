//! 扁平样式表语法树。
//!
//! 与 HTML 侧同一套做法：节点集中在一个 `Vec`，用 `u32` 索引互链，
//! 子节点用"首子 + 下一兄弟"链，节点按文档顺序创建所以子树是连续区间。

use lspd_core::text::Span;

/// 空索引。
pub const NONE: u32 = u32::MAX;

/// 节点类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// 规则：`选择器 { ... }`
    Rule,
    /// at 规则：`@media ... { ... }` 或 `@import ...;`
    AtRule,
    /// 声明：`属性: 值;`
    Declaration,
}

/// 一个节点。
#[derive(Debug, Clone, Copy)]
pub struct Node {
    /// 节点类型
    pub kind: NodeKind,
    /// 节点整体区间
    pub span: Span,
    /// 规则为选择器、at 规则为 `@名称`、声明为属性名
    pub name: Span,
    /// at 规则为前奏、声明为值；规则为空
    pub value: Span,
    /// 父节点索引
    pub parent: u32,
    /// 首个子节点
    pub first_child: u32,
    /// 末个子节点
    pub last_child: u32,
    /// 下一个兄弟节点
    pub next_sibling: u32,
    /// 子树结束下标（不含）
    pub subtree_end: u32,
}

/// 一棵解析完成的样式表。
#[derive(Default)]
pub struct Ast {
    /// 节点池，按文档顺序
    pub nodes: Vec<Node>,
    /// 顶层节点
    pub roots: Vec<u32>,
}

impl Ast {
    /// 按索引取节点。
    pub fn node(&self, index: u32) -> &Node {
        &self.nodes[index as usize]
    }

    /// 按索引取可变节点。
    pub fn node_mut(&mut self, index: u32) -> &mut Node {
        &mut self.nodes[index as usize]
    }

    /// 遍历某个节点的直接子节点。
    pub fn children(&self, index: u32) -> ChildIter<'_> {
        ChildIter { ast: self, next: self.node(index).first_child }
    }

    /// 按文档顺序遍历指定类型的节点。
    pub fn nodes_of(&self, kind: NodeKind) -> impl Iterator<Item = u32> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter(move |(_, node)| node.kind == kind)
            .map(|(index, _)| index as u32)
    }
}

/// 直接子节点迭代器。
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
