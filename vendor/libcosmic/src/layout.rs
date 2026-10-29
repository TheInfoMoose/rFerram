//! Taffy (`0.10.1`) Layout Engine & Cursor Hit-Testing for `libcosmic` (`cosmic`)
//!
//! Converts declarative [`Element`] and [`WidgetNode`] trees (including `Row`, `Column`, `Grid`,
//! `Container`, `Scrollable`, and the modal/drawer overlay classes `rferram-add-desktop-modal`,
//! `rferram-settings-drawer`, and `rferram-help-drawer`) into positioned [`LayoutBox`] records
//! for a given viewport `(width, height)`, and resolves cursor click coordinates to interactive
//! widget IDs or cloned `Message` (`M`) values from `Button` and `Toggler` nodes.

use taffy::prelude::{
    AlignItems, AvailableSpace, Display, FlexDirection, NodeId, Position, Rect as TaffyRect, Size,
    Style, TaffyTree, auto, fr, length, percent,
};
use taffy::{Overflow, Point};

use crate::{Element, WidgetNode};

/// Container class name for the "Add Desktop" modal overlay.
pub const CLASS_ADD_DESKTOP_MODAL: &str = "rferram-add-desktop-modal";

/// Container class name for the Settings context drawer overlay.
pub const CLASS_SETTINGS_DRAWER: &str = "rferram-settings-drawer";

/// Container class name for the Help / Diagnostics context drawer overlay.
pub const CLASS_HELP_DRAWER: &str = "rferram-help-drawer";

/// Axis-aligned 2D bounding box in logical viewport coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    /// Creates a new [`Rect`] from position and dimensions.
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Returns `true` if `(px, py)` lies within `[x, x + width]` and `[y, y + height]`.
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= (self.x + self.width) && py >= self.y && py <= (self.y + self.height)
    }

    /// Returns `true` if `other` lies completely within `self`.
    pub fn contains_rect(&self, other: Self) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && (other.x + other.width) <= (self.x + self.width)
            && (other.y + other.height) <= (self.y + self.height)
    }

    /// Returns the clamped intersection of `self` and `other` (with `width = 0, height = 0` when disjoint).
    pub fn intersect_clamped(&self, other: Self) -> Self {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.width).min(other.x + other.width);
        let y1 = (self.y + self.height).min(other.y + other.height);
        Self::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }
}

/// Strongly-typed identifier assigned to each laid-out widget node in a [`LayoutEngine`] pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WidgetId(pub usize);

impl From<usize> for WidgetId {
    fn from(val: usize) -> Self {
        Self(val)
    }
}

impl From<WidgetId> for usize {
    fn from(val: WidgetId) -> Self {
        val.0
    }
}

/// Positioned bounding box record produced by laying out a [`WidgetNode`] tree for a viewport.
#[derive(Debug, Clone)]
pub struct LayoutBox<M> {
    /// Deterministic pre-order widget identifier within the laid-out tree.
    pub id: usize,
    /// Absolute horizontal offset in viewport logical pixels.
    pub x: f32,
    /// Absolute vertical offset in viewport logical pixels.
    pub y: f32,
    /// Computed width in logical pixels.
    pub width: f32,
    /// Computed height in logical pixels.
    pub height: f32,
    /// Stacking order (`0` for normal flow, `> 0` for modal/drawer overlays).
    pub z_index: u32,
    /// Optional container CSS/theme class when this box represents or is tagged by a container.
    pub class: Option<String>,
    /// Cloned [`WidgetNode`] associated with this layout box.
    pub node: WidgetNode<M>,
}

impl<M> LayoutBox<M> {
    /// Returns the strongly-typed [`WidgetId`] for this layout box.
    pub const fn widget_id(&self) -> WidgetId {
        WidgetId(self.id)
    }

    /// Returns the axis-aligned [`Rect`] bounds of this layout box.
    pub const fn bounds(&self) -> Rect {
        Rect::new(self.x, self.y, self.width, self.height)
    }

    /// Returns `true` if the point `(px, py)` falls inside this layout box.
    pub fn contains(&self, px: f32, py: f32) -> bool {
        self.bounds().contains(px, py)
    }

    /// Returns `true` if this node is an interactive control (`Button`, `Toggler`, `TextInput`, `Dropdown`).
    pub const fn is_interactive(&self) -> bool {
        matches!(
            self.node,
            WidgetNode::Button { .. }
                | WidgetNode::Toggler { .. }
                | WidgetNode::TextInput { .. }
                | WidgetNode::Dropdown { .. }
        )
    }

    /// Returns `true` if this node is an overlay container (`rferram-add-desktop-modal`,
    /// `rferram-settings-drawer`, or `rferram-help-drawer`).
    pub fn is_overlay_container(&self) -> bool {
        self.class
            .as_deref()
            .is_some_and(is_overlay_container_class)
    }
}

impl<M: Clone> LayoutBox<M> {
    /// Extracts the cloned message `M` if this node is an actionable `Button` (`on_press`)
    /// or `Toggler` (`on_toggle`).
    pub fn message(&self) -> Option<M> {
        match &self.node {
            WidgetNode::Button { on_press, .. } => on_press.clone(),
            WidgetNode::Toggler { on_toggle, .. } => on_toggle.clone(),
            _ => None,
        }
    }
}

/// Result of a cursor hit-test query against a slice of [`LayoutBox`] records.
#[derive(Debug, Clone)]
pub struct HitTestResult<M> {
    /// Identifier of the hit widget.
    pub widget_id: usize,
    /// Bounding rectangle of the hit widget.
    pub bounds: Rect,
    /// Stacking layer of the hit widget.
    pub z_index: u32,
    /// Container class name if applicable.
    pub class: Option<String>,
    /// Cloned interaction message (`on_press` for `Button`, `on_toggle` for `Toggler`).
    pub message: Option<M>,
    /// Cloned [`WidgetNode`] of the hit widget.
    pub node: WidgetNode<M>,
}

/// Returns `true` if `class` is one of the modal or context drawer overlay classes.
pub fn is_overlay_container_class(class: &str) -> bool {
    matches!(
        class,
        CLASS_ADD_DESKTOP_MODAL | CLASS_SETTINGS_DRAWER | CLASS_HELP_DRAWER
    )
}

/// Returns the stacking `z_index` assigned to a container class (`0` for standard layout flow).
pub fn overlay_z_index_for_class(class: &str) -> u32 {
    match class {
        CLASS_SETTINGS_DRAWER => 20,
        CLASS_HELP_DRAWER => 25,
        CLASS_ADD_DESKTOP_MODAL => 30,
        _ => 0,
    }
}

/// Taffy-backed UI layout engine that computes [`LayoutBox`] coordinates and performs cursor hit-testing.
#[derive(Debug, Clone, Default)]
pub struct LayoutEngine {
    pub viewport_width: f32,
    pub viewport_height: f32,
}

struct NodeMeta<M> {
    id: usize,
    taffy_id: NodeId,
    z_index: u32,
    class: Option<String>,
    node: WidgetNode<M>,
    children: Vec<NodeMeta<M>>,
}

impl LayoutEngine {
    /// Creates a new [`LayoutEngine`] configured for the given viewport dimensions.
    pub fn new(viewport_width: f32, viewport_height: f32) -> Self {
        Self {
            viewport_width: viewport_width.max(1.0),
            viewport_height: viewport_height.max(1.0),
        }
    }

    /// Computes positioned [`LayoutBox`] records for an [`Element`] tree within the configured viewport.
    pub fn compute_layout<M: Clone>(&self, element: &Element<'_, M>) -> Vec<LayoutBox<M>> {
        self.compute_node_layout(&element.node)
    }

    /// Computes positioned [`LayoutBox`] records for a [`WidgetNode`] tree within the configured viewport.
    pub fn compute_node_layout<M: Clone>(&self, root: &WidgetNode<M>) -> Vec<LayoutBox<M>> {
        let mut taffy: TaffyTree<()> = TaffyTree::new();
        let mut next_id = 0usize;

        let Some(root_meta) = self.build_taffy_subtree(&mut taffy, root, &mut next_id, 0, true)
        else {
            return Vec::new();
        };

        let available = Size {
            width: AvailableSpace::Definite(self.viewport_width),
            height: AvailableSpace::Definite(self.viewport_height),
        };

        if taffy.compute_layout(root_meta.taffy_id, available).is_err() {
            return Vec::new();
        }

        let mut boxes = Vec::with_capacity(next_id);
        self.collect_layout_boxes(&taffy, &root_meta, 0.0, 0.0, &mut boxes);
        boxes
    }

    fn build_taffy_subtree<M: Clone>(
        &self,
        taffy: &mut TaffyTree<()>,
        node: &WidgetNode<M>,
        next_id: &mut usize,
        inherited_z: u32,
        is_root: bool,
    ) -> Option<NodeMeta<M>> {
        let my_id = *next_id;
        *next_id += 1;

        match node {
            WidgetNode::Text { content, size } => {
                let font_px = f32::from(*size).max(10.0);
                let char_count = content.chars().count().max(1) as f32;
                let est_width = (char_count * font_px * 0.56).clamp(24.0, self.viewport_width);
                let est_height = (font_px * 1.4).max(18.0);
                let style = Style {
                    size: Size {
                        width: length(est_width),
                        height: length(est_height),
                    },
                    flex_shrink: 0.0,
                    ..Default::default()
                };
                let taffy_id = taffy.new_leaf(style).ok()?;
                Some(NodeMeta {
                    id: my_id,
                    taffy_id,
                    z_index: inherited_z,
                    class: None,
                    node: node.clone(),
                    children: Vec::new(),
                })
            }
            WidgetNode::Button { label, .. } => {
                let char_count = label.chars().count().max(1) as f32;
                let est_width = (char_count * 8.5 + 24.0).max(40.0);
                let style = Style {
                    size: Size {
                        width: length(est_width),
                        height: length(34.0_f32),
                    },
                    flex_shrink: 0.0,
                    ..Default::default()
                };
                let taffy_id = taffy.new_leaf(style).ok()?;
                Some(NodeMeta {
                    id: my_id,
                    taffy_id,
                    z_index: inherited_z,
                    class: None,
                    node: node.clone(),
                    children: Vec::new(),
                })
            }
            WidgetNode::TextInput {
                placeholder, value, ..
            } => {
                let sample_len = value
                    .chars()
                    .count()
                    .max(placeholder.chars().count())
                    .max(12);
                let est_width = (sample_len as f32 * 8.0 + 24.0).clamp(180.0, 420.0);
                let style = Style {
                    size: Size {
                        width: length(est_width),
                        height: length(34.0_f32),
                    },
                    flex_shrink: 0.0,
                    ..Default::default()
                };
                let taffy_id = taffy.new_leaf(style).ok()?;
                Some(NodeMeta {
                    id: my_id,
                    taffy_id,
                    z_index: inherited_z,
                    class: None,
                    node: node.clone(),
                    children: Vec::new(),
                })
            }
            WidgetNode::Toggler { label, .. } => {
                let char_count = label.chars().count().max(1) as f32;
                let est_width = (char_count * 8.0 + 48.0).max(72.0);
                let style = Style {
                    size: Size {
                        width: length(est_width),
                        height: length(28.0_f32),
                    },
                    flex_shrink: 0.0,
                    ..Default::default()
                };
                let taffy_id = taffy.new_leaf(style).ok()?;
                Some(NodeMeta {
                    id: my_id,
                    taffy_id,
                    z_index: inherited_z,
                    class: None,
                    node: node.clone(),
                    children: Vec::new(),
                })
            }
            WidgetNode::Dropdown { options, .. } => {
                let max_chars = options
                    .iter()
                    .map(|s| s.chars().count())
                    .max()
                    .unwrap_or(10)
                    .max(8);
                let est_width = (max_chars as f32 * 8.0 + 36.0).clamp(160.0, 320.0);
                let style = Style {
                    size: Size {
                        width: length(est_width),
                        height: length(34.0_f32),
                    },
                    flex_shrink: 0.0,
                    ..Default::default()
                };
                let taffy_id = taffy.new_leaf(style).ok()?;
                Some(NodeMeta {
                    id: my_id,
                    taffy_id,
                    z_index: inherited_z,
                    class: None,
                    node: node.clone(),
                    children: Vec::new(),
                })
            }
            WidgetNode::Icon { size, .. } => {
                let side = f32::from(*size).max(16.0);
                let style = Style {
                    size: Size {
                        width: length(side),
                        height: length(side),
                    },
                    flex_shrink: 0.0,
                    ..Default::default()
                };
                let taffy_id = taffy.new_leaf(style).ok()?;
                Some(NodeMeta {
                    id: my_id,
                    taffy_id,
                    z_index: inherited_z,
                    class: None,
                    node: node.clone(),
                    children: Vec::new(),
                })
            }
            WidgetNode::Divider => {
                let style = Style {
                    size: Size {
                        width: percent(1.0_f32),
                        height: length(1.0_f32),
                    },
                    min_size: Size {
                        width: length(16.0_f32),
                        height: length(1.0_f32),
                    },
                    flex_shrink: 0.0,
                    ..Default::default()
                };
                let taffy_id = taffy.new_leaf(style).ok()?;
                Some(NodeMeta {
                    id: my_id,
                    taffy_id,
                    z_index: inherited_z,
                    class: None,
                    node: node.clone(),
                    children: Vec::new(),
                })
            }
            WidgetNode::Container { class, child } => {
                let class_z = overlay_z_index_for_class(class);
                let effective_z = inherited_z.max(class_z);
                let child_meta =
                    self.build_taffy_subtree(taffy, child, next_id, effective_z, false)?;
                let mut style = self.style_for_container_class(class, is_root);
                if !is_overlay_container_class(class) && node_contains_scrollable(child) {
                    style.flex_grow = 1.0;
                    style.min_size.height = length(0.0_f32);
                }
                let taffy_id = taffy
                    .new_with_children(style, &[child_meta.taffy_id])
                    .ok()?;
                Some(NodeMeta {
                    id: my_id,
                    taffy_id,
                    z_index: effective_z,
                    class: Some(class.clone()),
                    node: node.clone(),
                    children: vec![child_meta],
                })
            }
            WidgetNode::Scrollable { child } => {
                let child_meta =
                    self.build_taffy_subtree(taffy, child, next_id, inherited_z, false)?;
                // Prevent Taffy from compressing the content child inside a Scrollable container
                // when its content height exceeds the Scrollable viewport height.
                if let Ok(mut child_style) = taffy.style(child_meta.taffy_id).cloned() {
                    child_style.flex_shrink = 0.0;
                    let _ = taffy.set_style(child_meta.taffy_id, child_style);
                }
                let style = Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Column,
                    align_self: Some(taffy::prelude::AlignSelf::Stretch),
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    min_size: Size {
                        width: length(0.0_f32),
                        height: length(0.0_f32),
                    },
                    overflow: Point {
                        x: Overflow::Hidden,
                        y: Overflow::Scroll,
                    },
                    size: if is_root {
                        Size {
                            width: length(self.viewport_width),
                            height: length(self.viewport_height),
                        }
                    } else {
                        Size::auto()
                    },
                    ..Default::default()
                };
                let taffy_id = taffy
                    .new_with_children(style, &[child_meta.taffy_id])
                    .ok()?;
                Some(NodeMeta {
                    id: my_id,
                    taffy_id,
                    z_index: inherited_z,
                    class: None,
                    node: node.clone(),
                    children: vec![child_meta],
                })
            }
            WidgetNode::Row { spacing, children } => {
                let mut child_metas = Vec::with_capacity(children.len());
                let mut child_ids = Vec::with_capacity(children.len());
                for c in children {
                    if let Some(cm) =
                        self.build_taffy_subtree(taffy, c, next_id, inherited_z, false)
                    {
                        child_ids.push(cm.taffy_id);
                        child_metas.push(cm);
                    }
                }
                let gap_px = f32::from(*spacing);
                let has_scrollable = children.iter().any(node_contains_scrollable);
                let style = Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    align_items: Some(AlignItems::flex_start_or_center(children)),
                    flex_grow: if has_scrollable { 1.0 } else { 0.0 },
                    min_size: if has_scrollable {
                        Size {
                            width: length(0.0_f32),
                            height: length(0.0_f32),
                        }
                    } else {
                        Size::auto()
                    },
                    gap: Size {
                        width: length(gap_px),
                        height: length(0.0_f32),
                    },
                    size: if is_root {
                        Size {
                            width: length(self.viewport_width),
                            height: length(self.viewport_height),
                        }
                    } else {
                        Size::auto()
                    },
                    ..Default::default()
                };
                let taffy_id = taffy.new_with_children(style, &child_ids).ok()?;
                Some(NodeMeta {
                    id: my_id,
                    taffy_id,
                    z_index: inherited_z,
                    class: None,
                    node: node.clone(),
                    children: child_metas,
                })
            }
            WidgetNode::Column { spacing, children } => {
                let mut child_metas = Vec::with_capacity(children.len());
                let mut child_ids = Vec::with_capacity(children.len());
                for c in children {
                    if let Some(cm) =
                        self.build_taffy_subtree(taffy, c, next_id, inherited_z, false)
                    {
                        child_ids.push(cm.taffy_id);
                        child_metas.push(cm);
                    }
                }
                let gap_px = f32::from(*spacing);
                let has_scrollable = children.iter().any(node_contains_scrollable);
                let style = Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Column,
                    flex_grow: if has_scrollable { 1.0 } else { 0.0 },
                    min_size: if has_scrollable {
                        Size {
                            width: length(0.0_f32),
                            height: length(0.0_f32),
                        }
                    } else {
                        Size::auto()
                    },
                    gap: Size {
                        width: length(0.0_f32),
                        height: length(gap_px),
                    },
                    size: if is_root {
                        Size {
                            width: length(self.viewport_width),
                            height: length(self.viewport_height),
                        }
                    } else {
                        Size::auto()
                    },
                    ..Default::default()
                };
                let taffy_id = taffy.new_with_children(style, &child_ids).ok()?;
                Some(NodeMeta {
                    id: my_id,
                    taffy_id,
                    z_index: inherited_z,
                    class: None,
                    node: node.clone(),
                    children: child_metas,
                })
            }
            WidgetNode::Grid {
                columns,
                spacing,
                children,
            } => {
                let mut child_metas = Vec::with_capacity(children.len());
                let mut child_ids = Vec::with_capacity(children.len());
                for c in children {
                    if let Some(cm) =
                        self.build_taffy_subtree(taffy, c, next_id, inherited_z, false)
                    {
                        child_ids.push(cm.taffy_id);
                        child_metas.push(cm);
                    }
                }
                let col_count = (*columns).max(1);
                let gap_px = f32::from(*spacing);
                let style = Style {
                    display: Display::Grid,
                    grid_template_columns: vec![fr(1.0_f32); col_count],
                    gap: Size {
                        width: length(gap_px),
                        height: length(gap_px),
                    },
                    size: if is_root {
                        Size {
                            width: length(self.viewport_width),
                            height: auto(),
                        }
                    } else {
                        Size {
                            width: percent(1.0_f32),
                            height: auto(),
                        }
                    },
                    ..Default::default()
                };
                let taffy_id = taffy.new_with_children(style, &child_ids).ok()?;
                Some(NodeMeta {
                    id: my_id,
                    taffy_id,
                    z_index: inherited_z,
                    class: None,
                    node: node.clone(),
                    children: child_metas,
                })
            }
        }
    }

    fn style_for_container_class(&self, class: &str, is_root: bool) -> Style {
        let pad = |p: f32| TaffyRect {
            left: length(p),
            right: length(p),
            top: length(p),
            bottom: length(p),
        };

        match class {
            CLASS_ADD_DESKTOP_MODAL => {
                let modal_w = 560.0_f32.min((self.viewport_width - 32.0).max(240.0));
                let left_offset = ((self.viewport_width - modal_w) * 0.5).max(0.0);
                let top_offset = (self.viewport_height * 0.1).max(24.0);
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Column,
                    position: Position::Absolute,
                    inset: TaffyRect {
                        left: length(left_offset),
                        top: length(top_offset),
                        right: auto(),
                        bottom: auto(),
                    },
                    size: Size {
                        width: length(modal_w),
                        height: auto(),
                    },
                    padding: pad(16.0),
                    ..Default::default()
                }
            }
            CLASS_SETTINGS_DRAWER | CLASS_HELP_DRAWER => {
                let drawer_w = 340.0_f32.min((self.viewport_width - 24.0).max(200.0));
                let left_offset = (self.viewport_width - drawer_w).max(0.0);
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Column,
                    position: Position::Absolute,
                    inset: TaffyRect {
                        left: length(left_offset),
                        top: length(0.0_f32),
                        right: length(0.0_f32),
                        bottom: length(0.0_f32),
                    },
                    size: Size {
                        width: length(drawer_w),
                        height: length(self.viewport_height),
                    },
                    padding: pad(16.0),
                    ..Default::default()
                }
            }
            "rferram-root-shell" => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                size: Size {
                    width: length(self.viewport_width),
                    height: length(self.viewport_height),
                },
                padding: pad(12.0),
                ..Default::default()
            },
            "rferram-header" => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                flex_shrink: 0.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                min_size: Size {
                    width: auto(),
                    height: length(48.0_f32),
                },
                padding: pad(8.0),
                ..Default::default()
            },
            "rferram-sidebar" => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                size: Size {
                    width: length(200.0_f32),
                    height: auto(),
                },
                flex_shrink: 0.0,
                padding: pad(8.0),
                ..Default::default()
            },
            "rferram-group-section" => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                padding: pad(8.0),
                ..Default::default()
            },
            "rferram-connection-card" => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                min_size: Size {
                    width: length(220.0_f32),
                    height: length(120.0_f32),
                },
                padding: pad(12.0),
                ..Default::default()
            },
            _ => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                size: if is_root {
                    Size {
                        width: length(self.viewport_width),
                        height: length(self.viewport_height),
                    }
                } else {
                    Size::auto()
                },
                padding: pad(4.0),
                ..Default::default()
            },
        }
    }

    fn collect_layout_boxes<M: Clone>(
        &self,
        taffy: &TaffyTree<()>,
        meta: &NodeMeta<M>,
        parent_x: f32,
        parent_y: f32,
        out: &mut Vec<LayoutBox<M>>,
    ) {
        let Ok(layout) = taffy.layout(meta.taffy_id) else {
            return;
        };

        let is_overlay_root = meta
            .class
            .as_deref()
            .is_some_and(is_overlay_container_class);
        let abs_x = if is_overlay_root {
            layout.location.x
        } else {
            parent_x + layout.location.x
        };
        let abs_y = if is_overlay_root {
            layout.location.y
        } else {
            parent_y + layout.location.y
        };
        let width = layout.size.width;
        let height = if matches!(meta.node, WidgetNode::Scrollable { .. }) {
            let max_viewport_h = (self.viewport_height - abs_y).max(1.0);
            layout.size.height.min(max_viewport_h)
        } else {
            layout.size.height
        };

        out.push(LayoutBox {
            id: meta.id,
            x: abs_x,
            y: abs_y,
            width,
            height,
            z_index: meta.z_index,
            class: meta.class.clone(),
            node: meta.node.clone(),
        });

        for child in &meta.children {
            self.collect_layout_boxes(taffy, child, abs_x, abs_y, out);
        }
    }
}

/// Returns `true` if `node` or any normal-flow descendant is a `WidgetNode::Scrollable`.
fn node_contains_scrollable<M>(node: &WidgetNode<M>) -> bool {
    match node {
        WidgetNode::Scrollable { .. } => true,
        WidgetNode::Container { class, child } => {
            !is_overlay_container_class(class) && node_contains_scrollable(child)
        }
        WidgetNode::Row { children, .. }
        | WidgetNode::Column { children, .. }
        | WidgetNode::Grid { children, .. } => children.iter().any(node_contains_scrollable),
        _ => false,
    }
}

/// Returns the total number of [`WidgetNode`] entries in `node`'s subtree (including `node` itself).
pub fn widget_subtree_count<M>(node: &WidgetNode<M>) -> usize {
    match node {
        WidgetNode::Text { .. }
        | WidgetNode::Button { .. }
        | WidgetNode::TextInput { .. }
        | WidgetNode::Toggler { .. }
        | WidgetNode::Dropdown { .. }
        | WidgetNode::Icon { .. }
        | WidgetNode::Divider => 1,
        WidgetNode::Container { child, .. } | WidgetNode::Scrollable { child } => {
            1 + widget_subtree_count(child)
        }
        WidgetNode::Row { children, .. }
        | WidgetNode::Column { children, .. }
        | WidgetNode::Grid { children, .. } => {
            1 + children.iter().map(widget_subtree_count).sum::<usize>()
        }
    }
}

/// Computes the active [`Scrollable`](WidgetNode::Scrollable) clipping rectangle (`Option<Rect>`)
/// for each entry in `boxes`.
pub fn compute_clip_rects<M>(boxes: &[LayoutBox<M>]) -> Vec<Option<Rect>> {
    let mut clips: Vec<Option<Rect>> = vec![None; boxes.len()];
    for (idx, lbox) in boxes.iter().enumerate() {
        if matches!(lbox.node, WidgetNode::Scrollable { .. }) {
            let own_bounds = lbox.bounds();
            let effective_clip = clips[idx].map_or(own_bounds, |parent_clip| {
                parent_clip.intersect_clamped(own_bounds)
            });
            let subtree_len = widget_subtree_count(&lbox.node);
            let end = (idx + subtree_len).min(boxes.len());
            for (desc_idx, desc_box) in boxes.iter().enumerate().take(end).skip(idx + 1) {
                if desc_box.z_index == lbox.z_index {
                    clips[desc_idx] = Some(clips[desc_idx].map_or(effective_clip, |existing| {
                        existing.intersect_clamped(effective_clip)
                    }));
                }
            }
        }
    }
    clips
}

trait AlignItemsExt {
    fn flex_start_or_center<M>(children: &[WidgetNode<M>]) -> AlignItems;
}

impl AlignItemsExt for AlignItems {
    fn flex_start_or_center<M>(children: &[WidgetNode<M>]) -> AlignItems {
        let has_structural_panel = children.iter().any(|c| {
            matches!(
                c,
                WidgetNode::Container { .. }
                    | WidgetNode::Scrollable { .. }
                    | WidgetNode::Column { .. }
                    | WidgetNode::Grid { .. }
            )
        });
        if has_structural_panel {
            AlignItems::FlexStart
        } else {
            AlignItems::Center
        }
    }
}

/// Computes positioned [`LayoutBox`] records for an [`Element`] tree at `(viewport_width, viewport_height)`.
pub fn compute_layout<M: Clone>(
    element: &Element<'_, M>,
    viewport_width: f32,
    viewport_height: f32,
) -> Vec<LayoutBox<M>> {
    LayoutEngine::new(viewport_width, viewport_height).compute_layout(element)
}

/// Computes positioned [`LayoutBox`] records for a [`WidgetNode`] tree at `(viewport_width, viewport_height)`.
pub fn compute_node_layout<M: Clone>(
    node: &WidgetNode<M>,
    viewport_width: f32,
    viewport_height: f32,
) -> Vec<LayoutBox<M>> {
    LayoutEngine::new(viewport_width, viewport_height).compute_node_layout(node)
}

fn box_visible_at_cursor<M>(lbox: &LayoutBox<M>, clip: Option<Rect>, cx: f32, cy: f32) -> bool {
    lbox.contains(cx, cy)
        && clip.is_none_or(|c| c.width > 0.0 && c.height > 0.0 && c.contains(cx, cy))
}

/// Performs cursor hit-testing at `(cursor_x, cursor_y)` across a slice of [`LayoutBox`] records.
///
/// Prioritizes:
/// 1. Highest `z_index` layer containing the cursor (so modal and drawer overlays occlude background nodes).
/// 2. Interactive controls (`Button`, `Toggler`, `TextInput`, `Dropdown`) over structural containers.
/// 3. Deepest / last-rendered node in tree order (respecting `Scrollable` clip bounds).
pub fn hit_test<M: Clone>(
    boxes: &[LayoutBox<M>],
    cursor_x: f32,
    cursor_y: f32,
) -> Option<HitTestResult<M>> {
    let clips = compute_clip_rects(boxes);

    let top_z_at_cursor = boxes
        .iter()
        .enumerate()
        .filter(|(idx, b)| box_visible_at_cursor(b, clips[*idx], cursor_x, cursor_y))
        .map(|(_, b)| b.z_index)
        .max()?;

    // First look for the deepest interactive widget in the topmost z_index layer under the cursor
    let target_box = boxes
        .iter()
        .enumerate()
        .rev()
        .find(|(idx, b)| {
            b.z_index == top_z_at_cursor
                && b.is_interactive()
                && box_visible_at_cursor(b, clips[*idx], cursor_x, cursor_y)
        })
        .or_else(|| {
            // Fall back to the deepest non-interactive node in the topmost z_index layer
            boxes.iter().enumerate().rev().find(|(idx, b)| {
                b.z_index == top_z_at_cursor
                    && box_visible_at_cursor(b, clips[*idx], cursor_x, cursor_y)
            })
        })
        .map(|(_, b)| b)?;

    Some(HitTestResult {
        widget_id: target_box.id,
        bounds: target_box.bounds(),
        z_index: target_box.z_index,
        class: target_box.class.clone(),
        message: target_box.message(),
        node: target_box.node.clone(),
    })
}

/// Resolves a click at `(cursor_x, cursor_y)` directly to the cloned `Message` (`M`) of a hit
/// `Button` (`on_press`) or `Toggler` (`on_toggle`) node, if any.
pub fn hit_test_message<M: Clone>(
    boxes: &[LayoutBox<M>],
    cursor_x: f32,
    cursor_y: f32,
) -> Option<M> {
    hit_test(boxes, cursor_x, cursor_y).and_then(|hit| hit.message)
}

/// Resolves a click at `(cursor_x, cursor_y)` to the interactive widget ID (`usize`) when an
/// interactive widget (`Button`, `Toggler`, `TextInput`, `Dropdown`) is hit.
pub fn hit_test_widget_id<M: Clone>(
    boxes: &[LayoutBox<M>],
    cursor_x: f32,
    cursor_y: f32,
) -> Option<usize> {
    let clips = compute_clip_rects(boxes);

    let top_z_at_cursor = boxes
        .iter()
        .enumerate()
        .filter(|(idx, b)| box_visible_at_cursor(b, clips[*idx], cursor_x, cursor_y))
        .map(|(_, b)| b.z_index)
        .max()?;

    boxes
        .iter()
        .enumerate()
        .rev()
        .find(|(idx, b)| {
            b.z_index == top_z_at_cursor
                && b.is_interactive()
                && box_visible_at_cursor(b, clips[*idx], cursor_x, cursor_y)
        })
        .map(|(_, b)| b.id)
}

/// Complete positioned layout scene for a viewport, ready for hit-testing and software/vector rasterization.
#[derive(Debug, Clone)]
pub struct LayoutScene<M> {
    /// Logical viewport width in pixels.
    pub viewport_width: f32,
    /// Logical viewport height in pixels.
    pub viewport_height: f32,
    /// Positioned widget bounding boxes in pre-order traversal.
    pub boxes: Vec<LayoutBox<M>>,
    /// Optional focused widget ID (e.g., active `TextInput` showing focus border and caret).
    pub focused_widget_id: Option<usize>,
    /// Optional hovered widget ID for interactive highlight feedback.
    pub hovered_widget_id: Option<usize>,
    /// Optional caret character index within the focused `TextInput` (`None` places caret at end of text).
    pub caret_position: Option<usize>,
    /// Vertical scroll offset in logical pixels applied to overflowing `Scrollable` containers.
    pub scroll_offset_y: f32,
}

impl<M: Clone> LayoutScene<M> {
    /// Creates a new [`LayoutScene`] from precomputed [`LayoutBox`] records and viewport dimensions.
    pub fn new(viewport_width: f32, viewport_height: f32, boxes: Vec<LayoutBox<M>>) -> Self {
        Self {
            viewport_width: viewport_width.max(1.0),
            viewport_height: viewport_height.max(1.0),
            boxes,
            focused_widget_id: None,
            hovered_widget_id: None,
            caret_position: None,
            scroll_offset_y: 0.0,
        }
    }

    /// Computes a [`LayoutScene`] directly from a declarative [`Element`] tree for `(viewport_width, viewport_height)`.
    pub fn from_element(
        element: &Element<'_, M>,
        viewport_width: f32,
        viewport_height: f32,
    ) -> Self {
        let engine = LayoutEngine::new(viewport_width, viewport_height);
        let boxes = engine.compute_layout(element);
        Self::new(engine.viewport_width, engine.viewport_height, boxes)
    }

    /// Computes a [`LayoutScene`] directly from a [`WidgetNode`] tree for `(viewport_width, viewport_height)`.
    pub fn from_node(node: &WidgetNode<M>, viewport_width: f32, viewport_height: f32) -> Self {
        let engine = LayoutEngine::new(viewport_width, viewport_height);
        let boxes = engine.compute_node_layout(node);
        Self::new(engine.viewport_width, engine.viewport_height, boxes)
    }

    /// Sets the focused widget ID (`Some(id)` or `None`) for focus border and caret rendering.
    pub fn with_focused_widget(mut self, widget_id: Option<usize>) -> Self {
        self.focused_widget_id = widget_id;
        self
    }

    /// Sets the hovered widget ID (`Some(id)` or `None`) for hover highlight rendering.
    pub fn with_hovered_widget(mut self, widget_id: Option<usize>) -> Self {
        self.hovered_widget_id = widget_id;
        self
    }

    /// Sets the explicit caret character offset within the focused `TextInput`.
    pub fn with_caret_position(mut self, caret_position: Option<usize>) -> Self {
        self.caret_position = caret_position;
        self
    }

    /// Returns the maximum vertical scroll overflow in logical pixels across `Scrollable` containers in this scene.
    pub fn max_scroll_offset(&self) -> f32 {
        let mut max_overflow = 0.0_f32;
        for (idx, lbox) in self.boxes.iter().enumerate() {
            if matches!(lbox.node, WidgetNode::Scrollable { .. }) {
                let bounds = lbox.bounds();
                let subtree_len = widget_subtree_count(&lbox.node);
                let end = (idx + subtree_len).min(self.boxes.len());
                let content_bottom = self.boxes[(idx + 1)..end]
                    .iter()
                    .filter(|b| b.z_index == lbox.z_index)
                    .map(|b| b.y + b.height)
                    .fold(bounds.y + bounds.height, f32::max);
                let overflow = (content_bottom - (bounds.y + bounds.height)).max(0.0);
                max_overflow = max_overflow.max(overflow);
            }
        }
        max_overflow
    }

    /// Applies a clamped vertical scroll offset (`>= 0.0`) to descendants of overflowing `Scrollable` containers.
    pub fn with_scroll_offset(mut self, scroll_offset_y: f32) -> Self {
        let max_scroll = self.max_scroll_offset();
        let clamped = scroll_offset_y.clamp(0.0, max_scroll);
        self.scroll_offset_y = clamped;
        if clamped <= 0.0 {
            return self;
        }

        // Apply scroll translation once to top-level overflowing Scrollable subtrees
        let mut skip_until = 0usize;
        let len = self.boxes.len();
        for idx in 0..len {
            if idx < skip_until {
                continue;
            }
            if matches!(self.boxes[idx].node, WidgetNode::Scrollable { .. }) {
                let scroll_z = self.boxes[idx].z_index;
                let bounds = self.boxes[idx].bounds();
                let subtree_len = widget_subtree_count(&self.boxes[idx].node);
                let end = (idx + subtree_len).min(len);
                let content_bottom = self.boxes[(idx + 1)..end]
                    .iter()
                    .filter(|b| b.z_index == scroll_z)
                    .map(|b| b.y + b.height)
                    .fold(bounds.y + bounds.height, f32::max);
                let overflow = (content_bottom - (bounds.y + bounds.height)).max(0.0);
                if overflow > 0.0 {
                    let delta = clamped.min(overflow);
                    for desc in &mut self.boxes[(idx + 1)..end] {
                        if desc.z_index == scroll_z {
                            desc.y -= delta;
                        }
                    }
                    skip_until = end;
                }
            }
        }
        self
    }

    /// Returns the integer pixel width of the scene (`>= 1`).
    pub fn width(&self) -> u32 {
        self.viewport_width.round().max(1.0) as u32
    }

    /// Returns the integer pixel height of the scene (`>= 1`).
    pub fn height(&self) -> u32 {
        self.viewport_height.round().max(1.0) as u32
    }

    /// Performs cursor hit-testing at `(cursor_x, cursor_y)` against this scene.
    pub fn hit_test(&self, cursor_x: f32, cursor_y: f32) -> Option<HitTestResult<M>> {
        hit_test(&self.boxes, cursor_x, cursor_y)
    }

    /// Resolves a click at `(cursor_x, cursor_y)` directly to a cloned `Message` (`M`).
    pub fn hit_test_message(&self, cursor_x: f32, cursor_y: f32) -> Option<M> {
        hit_test_message(&self.boxes, cursor_x, cursor_y)
    }

    /// Resolves a click at `(cursor_x, cursor_y)` to the interactive widget ID (`usize`).
    pub fn hit_test_widget_id(&self, cursor_x: f32, cursor_y: f32) -> Option<usize> {
        hit_test_widget_id(&self.boxes, cursor_x, cursor_y)
    }
}

impl LayoutEngine {
    /// Computes a complete [`LayoutScene`] for an [`Element`] tree within the configured viewport.
    pub fn compute_scene<M: Clone>(&self, element: &Element<'_, M>) -> LayoutScene<M> {
        LayoutScene::new(
            self.viewport_width,
            self.viewport_height,
            self.compute_layout(element),
        )
    }
}

/// Computes a positioned [`LayoutScene`] for an [`Element`] tree at `(viewport_width, viewport_height)`.
pub fn compute_scene<M: Clone>(
    element: &Element<'_, M>,
    viewport_width: f32,
    viewport_height: f32,
) -> LayoutScene<M> {
    LayoutScene::from_element(element, viewport_width, viewport_height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::{
        button, column, container, divider, dropdown, grid, icon, row, scrollable, text,
        text_input, toggler,
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestMsg {
        OpenModal,
        ToggleReconnect(bool),
        SaveModal,
        CancelModal,
        SelectDrawerItem,
        ConnectCard(usize),
    }

    #[test]
    fn test_row_column_grid_and_scrollable_layout() {
        let view: Element<'_, TestMsg> = container(
            "rferram-root-shell",
            column(
                12,
                vec![
                    container(
                        "rferram-header",
                        row(
                            10,
                            vec![
                                text("Connections").size(18).into(),
                                button("+").on_press(TestMsg::OpenModal).into(),
                                text_input("Search connections...", "").into(),
                            ],
                        ),
                    ),
                    row(
                        16,
                        vec![
                            container(
                                "rferram-sidebar",
                                column(8, vec![button("Favorites").into(), divider()]),
                            ),
                            scrollable(container(
                                "rferram-group-section",
                                grid(
                                    3,
                                    16,
                                    vec![
                                        container(
                                            "rferram-connection-card",
                                            column(
                                                6,
                                                vec![
                                                    text("Host A").into(),
                                                    button("Connect")
                                                        .on_press(TestMsg::ConnectCard(1))
                                                        .into(),
                                                ],
                                            ),
                                        ),
                                        container(
                                            "rferram-connection-card",
                                            column(
                                                6,
                                                vec![
                                                    text("Host B").into(),
                                                    button("Connect")
                                                        .on_press(TestMsg::ConnectCard(2))
                                                        .into(),
                                                ],
                                            ),
                                        ),
                                    ],
                                ),
                            )),
                        ],
                    ),
                ],
            ),
        );

        let boxes = view.layout(1440.0, 900.0);
        assert!(!boxes.is_empty());

        // Root shell occupies the 1440x900 viewport
        let root_box = &boxes[0];
        assert_eq!(root_box.class.as_deref(), Some("rferram-root-shell"));
        assert_eq!(root_box.x, 0.0);
        assert_eq!(root_box.y, 0.0);
        assert_eq!(root_box.width, 1440.0);
        assert_eq!(root_box.height, 900.0);

        // Two connection cards in a 3-column grid must be positioned horizontally side by side
        let card_boxes: Vec<&LayoutBox<TestMsg>> = boxes
            .iter()
            .filter(|b| b.class.as_deref() == Some("rferram-connection-card"))
            .collect();
        assert_eq!(card_boxes.len(), 2);
        assert!(card_boxes[1].x > card_boxes[0].x + card_boxes[0].width);
        assert_eq!(card_boxes[0].y, card_boxes[1].y);
    }

    #[test]
    fn test_button_and_toggler_cursor_hit_testing() {
        let tree: Element<'_, TestMsg> = container(
            "rferram-root-shell",
            column(
                10,
                vec![
                    button("+").on_press(TestMsg::OpenModal).into(),
                    toggler(
                        "Reconnect if the session is dropped",
                        true,
                        TestMsg::ToggleReconnect(false),
                    ),
                    dropdown(vec!["None".to_string(), "WireGuard".to_string()], Some(0)),
                    icon("folder-remote-symbolic", 24),
                ],
            ),
        );

        let boxes = compute_layout(&tree, 1024.0, 768.0);

        let plus_btn = boxes
            .iter()
            .find(|b| matches!(&b.node, WidgetNode::Button { label, .. } if label == "+"))
            .expect("missing '+' button layout box");
        let click_x = plus_btn.x + plus_btn.width * 0.5;
        let click_y = plus_btn.y + plus_btn.height * 0.5;

        assert_eq!(
            hit_test_message(&boxes, click_x, click_y),
            Some(TestMsg::OpenModal)
        );
        assert_eq!(
            hit_test_widget_id(&boxes, click_x, click_y),
            Some(plus_btn.id)
        );

        let tog_box = boxes
            .iter()
            .find(|b| matches!(&b.node, WidgetNode::Toggler { .. }))
            .expect("missing toggler layout box");
        let tog_x = tog_box.x + tog_box.width * 0.5;
        let tog_y = tog_box.y + tog_box.height * 0.5;

        assert_eq!(
            hit_test_message(&boxes, tog_x, tog_y),
            Some(TestMsg::ToggleReconnect(false))
        );
        assert_eq!(hit_test_widget_id(&boxes, tog_x, tog_y), Some(tog_box.id));
    }

    #[test]
    fn test_modal_and_drawer_overlays_positioning_and_hit_occlusion() {
        let tree: Element<'_, TestMsg> = container(
            "rferram-root-shell",
            column(
                12,
                vec![
                    container(
                        "rferram-header",
                        row(12, vec![button("+").on_press(TestMsg::OpenModal).into()]),
                    ),
                    container(
                        CLASS_SETTINGS_DRAWER,
                        column(
                            8,
                            vec![
                                text("Settings: General").into(),
                                button("VPN").on_press(TestMsg::SelectDrawerItem).into(),
                            ],
                        ),
                    ),
                    container(
                        CLASS_HELP_DRAWER,
                        column(8, vec![text("Help: Documentation").into()]),
                    ),
                    container(
                        CLASS_ADD_DESKTOP_MODAL,
                        column(
                            10,
                            vec![
                                text("Add Desktop").size(18).into(),
                                button("Cancel").on_press(TestMsg::CancelModal).into(),
                                button("Add").on_press(TestMsg::SaveModal).into(),
                            ],
                        ),
                    ),
                ],
            ),
        );

        let boxes = compute_layout(&tree, 1440.0, 900.0);

        let modal_box = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_ADD_DESKTOP_MODAL))
            .expect("missing modal layout box");
        assert!(modal_box.z_index > 0);
        // Centered horizontally in 1440px viewport
        assert!((modal_box.x - (1440.0 - modal_box.width) * 0.5).abs() < 1.0);

        let settings_drawer = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_SETTINGS_DRAWER))
            .expect("missing settings drawer layout box");
        assert!(settings_drawer.z_index > 0);
        // Right-docked in 1440px viewport
        assert!((settings_drawer.x + settings_drawer.width - 1440.0).abs() < 1.0);

        let help_drawer = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_HELP_DRAWER))
            .expect("missing help drawer layout box");
        assert!(help_drawer.z_index > settings_drawer.z_index);

        // Clicking the "Add" button inside the modal resolves to TestMsg::SaveModal
        let add_btn = boxes
            .iter()
            .find(|b| matches!(&b.node, WidgetNode::Button { label, .. } if label == "Add"))
            .expect("missing 'Add' button in modal");
        let add_cx = add_btn.x + add_btn.width * 0.5;
        let add_cy = add_btn.y + add_btn.height * 0.5;
        assert_eq!(
            hit_test_message(&boxes, add_cx, add_cy),
            Some(TestMsg::SaveModal)
        );
    }

    #[test]
    fn test_scrollable_overflow_constraints_and_hit_test_clipping() {
        let mut items: Vec<Element<'_, TestMsg>> = Vec::new();
        for idx in 0..30 {
            items.push(
                button(format!("Item {idx}"))
                    .on_press(TestMsg::ConnectCard(idx))
                    .into(),
            );
        }

        let tree: Element<'_, TestMsg> = container(
            "rferram-root-shell",
            column(
                10,
                vec![
                    scrollable(scrollable(column(8, items))),
                    button("Footer Save").on_press(TestMsg::SaveModal).into(),
                ],
            ),
        );

        let boxes = compute_layout(&tree, 800.0, 400.0);
        let footer_btn = boxes
            .iter()
            .find(|b| matches!(&b.node, WidgetNode::Button { label, .. } if label == "Footer Save"))
            .expect("missing footer button");

        // Footer button must remain inside the 400px viewport despite 30 items inside nested Scrollables
        assert!(
            footer_btn.y + footer_btn.height <= 400.0,
            "footer button pushed off-screen: y={}, h={}",
            footer_btn.y,
            footer_btn.height
        );

        // Clicking at the footer button's center must hit Footer Save, NOT an overflowed item from the Scrollable
        let fx = footer_btn.x + footer_btn.width * 0.5;
        let fy = footer_btn.y + footer_btn.height * 0.5;
        assert_eq!(hit_test_message(&boxes, fx, fy), Some(TestMsg::SaveModal));
    }
}
