//! Taffy flexbox layout for the renderer-native gameplay UI.
//!
//! The frontend Main page (`neon_menu::layout`) proved the pattern: build a
//! small Taffy tree, compute it once, and use the same rectangles for drawing
//! and for pointer hit testing so the two can never disagree. This is the
//! shared version of that for the scoreboard, kill feed and their menus.
//! Units are physical pixels; rectangles come back as `[x, y, w, h]` in
//! absolute screen space.
use taffy::prelude::{
    auto, length, percent, AlignItems, AvailableSpace, Dimension, FlexDirection, JustifyContent,
    LengthPercentage, LengthPercentageAuto, NodeId, Rect, Size, Style, TaffyTree,
};

pub(crate) type Bounds = [f32; 4];

pub(crate) fn contains(bounds: Bounds, point: [f32; 2]) -> bool {
    point[0] >= bounds[0]
        && point[1] >= bounds[1]
        && point[0] < bounds[0] + bounds[2]
        && point[1] < bounds[1] + bounds[3]
}

/// One layout pass. Nodes are cheap; build a fresh `Flex` per frame.
pub(crate) struct Flex {
    tree: TaffyTree<()>,
}

/// Container description: direction, gap, padding and alignment.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Container {
    pub direction: FlexDirection,
    pub gap: f32,
    /// left, top, right, bottom.
    pub padding: [f32; 4],
    pub align: AlignItems,
    pub justify: JustifyContent,
    pub width: Option<f32>,
    pub height: Option<f32>,
}

impl Container {
    pub(crate) fn row() -> Self {
        Self {
            direction: FlexDirection::Row,
            gap: 0.0,
            padding: [0.0; 4],
            align: AlignItems::CENTER,
            justify: JustifyContent::FLEX_START,
            width: None,
            height: None,
        }
    }

    pub(crate) fn column() -> Self {
        Self { direction: FlexDirection::Column, align: AlignItems::STRETCH, ..Self::row() }
    }

    pub(crate) fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }

    pub(crate) fn pad(mut self, x: f32, y: f32) -> Self {
        self.padding = [x, y, x, y];
        self
    }

    pub(crate) fn align(mut self, align: AlignItems) -> Self {
        self.align = align;
        self
    }

    pub(crate) fn justify(mut self, justify: JustifyContent) -> Self {
        self.justify = justify;
        self
    }

    pub(crate) fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub(crate) fn height(mut self, height: f32) -> Self {
        self.height = Some(height);
        self
    }

    fn style(self) -> Style {
        let lp = LengthPercentage::length;
        Style {
            flex_direction: self.direction,
            gap: Size { width: lp(self.gap), height: lp(self.gap) },
            padding: Rect {
                left: lp(self.padding[0]),
                top: lp(self.padding[1]),
                right: lp(self.padding[2]),
                bottom: lp(self.padding[3]),
            },
            align_items: self.align,
            justify_content: self.justify,
            size: Size {
                width: self.width.map_or(Dimension::auto(), length),
                height: self.height.map_or(Dimension::auto(), length),
            },
            flex_shrink: 0.0,
            ..Default::default()
        }
    }
}

impl Flex {
    pub(crate) fn new() -> Self {
        Self { tree: TaffyTree::new() }
    }

    /// Fixed-size box: a text run, an icon, a spacer.
    pub(crate) fn leaf(&mut self, width: f32, height: f32) -> NodeId {
        self.tree
            .new_leaf(Style {
                size: Size { width: length(width.max(0.0)), height: length(height.max(0.0)) },
                flex_shrink: 0.0,
                ..Default::default()
            })
            .expect("taffy leaf")
    }

    /// Box that soaks up the remaining main-axis space (pushes later
    /// siblings to the far edge).
    pub(crate) fn spring(&mut self) -> NodeId {
        self.tree
            .new_leaf(Style { flex_grow: 1.0, flex_shrink: 1.0, ..Default::default() })
            .expect("taffy spring")
    }

    /// Fixed-height box that fills its parent's width.
    pub(crate) fn band(&mut self, height: f32) -> NodeId {
        self.tree
            .new_leaf(Style {
                size: Size { width: percent(1.0), height: length(height.max(0.0)) },
                flex_shrink: 0.0,
                ..Default::default()
            })
            .expect("taffy band")
    }

    pub(crate) fn node(&mut self, container: Container, children: &[NodeId]) -> NodeId {
        self.tree.new_with_children(container.style(), children).expect("taffy node")
    }

    /// Absolutely placed child at `(x, y)` inside its parent.
    pub(crate) fn absolute(&mut self, container: Container, x: f32, y: f32, children: &[NodeId]) -> NodeId {
        let mut style = container.style();
        style.position = taffy::Position::Absolute;
        style.inset = Rect {
            left: LengthPercentageAuto::length(x),
            top: LengthPercentageAuto::length(y),
            right: auto(),
            bottom: auto(),
        };
        self.tree.new_with_children(style, children).expect("taffy absolute")
    }

    /// Let `node` grow (and shrink) along its parent's main axis.
    pub(crate) fn set_grow(&mut self, node: NodeId, grow: f32) {
        if let Ok(style) = self.tree.style(node) {
            let mut style = style.clone();
            style.flex_grow = grow;
            style.flex_shrink = 1.0;
            let _ = self.tree.set_style(node, style);
        }
    }

    /// Lay `root` out inside a `width` x `height` viewport, then offset the
    /// whole tree by `origin`.
    pub(crate) fn compute(&mut self, root: NodeId, width: f32, height: f32) {
        let _ = self.tree.compute_layout(
            root,
            Size {
                width: AvailableSpace::Definite(width.max(1.0)),
                height: AvailableSpace::Definite(height.max(1.0)),
            },
        );
    }

    /// Absolute `[x, y, w, h]` of a computed node.
    pub(crate) fn bounds(&self, node: NodeId) -> Bounds {
        let Ok(layout) = self.tree.layout(node) else { return [0.0; 4] };
        let mut x = layout.location.x;
        let mut y = layout.location.y;
        let mut parent = self.tree.parent(node);
        while let Some(id) = parent {
            if let Ok(parent_layout) = self.tree.layout(id) {
                x += parent_layout.location.x;
                y += parent_layout.location.y;
            }
            parent = self.tree.parent(id);
        }
        [x, y, layout.size.width, layout.size.height]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_with_spring_pushes_last_child_to_the_edge() {
        let mut flex = Flex::new();
        let a = flex.leaf(40.0, 10.0);
        let spring = flex.spring();
        let b = flex.leaf(20.0, 10.0);
        let row = flex.node(Container::row().width(200.0).pad(5.0, 2.0), &[a, spring, b]);
        let root = flex.node(Container::column().width(300.0).height(100.0).pad(10.0, 10.0), &[row]);
        flex.compute(root, 300.0, 100.0);
        assert_eq!(flex.bounds(a), [15.0, 12.0, 40.0, 10.0]);
        assert_eq!(flex.bounds(b), [10.0 + 200.0 - 5.0 - 20.0, 12.0, 20.0, 10.0]);
    }

    #[test]
    fn column_centers_children_on_cross_axis() {
        let mut flex = Flex::new();
        let crown = flex.leaf(10.0, 4.0);
        let name = flex.leaf(30.0, 12.0);
        let stack = flex.node(Container::column().align(AlignItems::CENTER).gap(2.0), &[crown, name]);
        flex.compute(stack, 100.0, 100.0);
        let [cx, _, cw, _] = flex.bounds(crown);
        let [nx, ny, nw, _] = flex.bounds(name);
        assert_eq!(cx + cw * 0.5, nx + nw * 0.5);
        assert_eq!(ny, 6.0);
    }

    #[test]
    fn contains_is_half_open() {
        assert!(contains([0.0, 0.0, 10.0, 10.0], [0.0, 9.99]));
        assert!(!contains([0.0, 0.0, 10.0, 10.0], [10.0, 5.0]));
    }
}
