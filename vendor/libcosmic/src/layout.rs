//! Taffy (`0.10.1`) Layout Engine & Cursor Hit-Testing for `libcosmic` (`cosmic`)
//!
//! Converts declarative [`Element`] and [`WidgetNode`] trees (including `Row`, `Column`, `Grid`,
//! `Container`, `Scrollable`, and the modal/drawer overlay classes `rferram-add-desktop-modal`,
//! `rferram-settings-drawer`, and `rferram-help-drawer`) into positioned [`LayoutBox`] records
//! for a given viewport `(width, height)`, and resolves cursor click coordinates to interactive
//! widget IDs or cloned `Message` (`M`) values from `Button` and `Toggler` nodes.

use taffy::prelude::{
    AlignItems, AlignSelf, AvailableSpace, Display, FlexDirection, JustifyContent, NodeId,
    Position, Rect as TaffyRect, Size, Style, TaffyTree, auto, fr, length, percent,
};
use taffy::{Overflow, Point};

use crate::{Element, WidgetNode};

/// Container class name for the root application shell.
pub const CLASS_ROOT_SHELL: &str = "rferram-root-shell";

/// Container class name for the outer header wrapper.
pub const CLASS_HEADER: &str = "rferram-header";

/// Container class name for the upper window tab bar (`Connections` tab, `+`, Settings, Help).
pub const CLASS_TOP_TABBAR: &str = "rferram-top-tabbar";

/// Container class name for the second-tier action toolbar (`+`, segmented view/sort pills, search pill).
pub const CLASS_TOOLBAR: &str = "rferram-toolbar";

/// Container class name for a capsule-grouped segmented icon button pill (`Grid | List`, `Sort | Filter`).
pub const CLASS_SEGMENTED_PILL: &str = "rferram-segmented-pill";

/// Container class name for the rounded capsule search bar in the toolbar.
pub const CLASS_SEARCH_PILL: &str = "rferram-search-pill";

/// Container class name for the left navigation sidebar.
pub const CLASS_SIDEBAR: &str = "rferram-sidebar";

/// Container class name for a connection group section.
pub const CLASS_GROUP_SECTION: &str = "rferram-group-section";

/// Container class name for a raised group tab badge (`Group 1`).
pub const CLASS_GROUP_BADGE: &str = "rferram-group-badge";

/// Container class name for a portrait connection card with upper preview sheen and double border.
pub const CLASS_CONNECTION_CARD: &str = "rferram-connection-card";

/// Container class name for the floating card context menu popup (`Connect`, `Configure`, `Duplicate`, `Export`, `Delete`).
pub const CLASS_CONTEXT_MENU: &str = "rferram-context-menu";

/// Container class name for the floating `+` quick-action menu popup (`Add`, `Import`, `Export`).
pub const CLASS_ADD_MENU: &str = "rferram-add-menu";

/// Container class name for a horizontal `Label : Control` form row inside modals/drawers.
pub const CLASS_FORM_ROW: &str = "rferram-form-row";

/// Container class name for the segmented modal tab bar (`General | Display | Devices | Audio | Folders`).
pub const CLASS_MODAL_TABS: &str = "rferram-modal-tabs";

/// Container class name for the bordered inner tab content frame inside the "Add Desktop" modal.
pub const CLASS_MODAL_TAB_FRAME: &str = "rferram-modal-tab-frame";

/// Container class name for the bottom-right action footer (`Cancel | Add`) inside the modal.
pub const CLASS_MODAL_FOOTER: &str = "rferram-modal-footer";

/// Container class name for the left category navigation pane inside a split context drawer.
pub const CLASS_DRAWER_NAV: &str = "rferram-drawer-nav";

/// Container class name for the right detail content pane inside a split context drawer.
pub const CLASS_DRAWER_CONTENT: &str = "rferram-drawer-content";

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

/// Returns `true` if `class` is one of the modal, context drawer, or floating popup menu overlay classes.
pub fn is_overlay_container_class(class: &str) -> bool {
    matches!(
        class,
        CLASS_ADD_DESKTOP_MODAL
            | CLASS_SETTINGS_DRAWER
            | CLASS_HELP_DRAWER
            | CLASS_CONTEXT_MENU
            | CLASS_ADD_MENU
    )
}

/// Returns `true` if `class` is a floating popup menu (`rferram-context-menu` or `rferram-add-menu`).
pub fn is_popup_menu_class(class: &str) -> bool {
    matches!(class, CLASS_CONTEXT_MENU | CLASS_ADD_MENU)
}

/// Returns `true` if `class` is a compact inline pill/badge container inside a toolbar or section header row.
fn is_inline_container_class(class: &str) -> bool {
    matches!(
        class,
        CLASS_SEGMENTED_PILL | CLASS_SEARCH_PILL | CLASS_GROUP_BADGE
    )
}

/// Returns the stacking `z_index` assigned to a container class (`0` for standard layout flow).
pub fn overlay_z_index_for_class(class: &str) -> u32 {
    match class {
        CLASS_ADD_MENU => 15,
        CLASS_CONTEXT_MENU => 18,
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
        self.collect_layout_boxes(&taffy, &root_meta, 0.0, 0.0, None, &mut boxes);
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
            WidgetNode::Button {
                label,
                icon_name,
                style: btn_style,
                ..
            } => {
                let is_icon_only = icon_name
                    .as_deref()
                    .is_some_and(|ic| label.is_empty() || label == ic);
                let icon_extra = if icon_name.is_some() && !is_icon_only {
                    22.0
                } else {
                    0.0
                };
                let (est_width, est_height) = match btn_style {
                    crate::ButtonStyle::Icon => (34.0_f32, 34.0_f32),
                    crate::ButtonStyle::CardTrigger => {
                        let chars = label.chars().count();
                        if is_icon_only || chars <= 3 {
                            (28.0_f32, 28.0_f32)
                        } else {
                            ((chars as f32 * 8.0 + 20.0 + icon_extra).max(32.0), 28.0_f32)
                        }
                    }
                    crate::ButtonStyle::NavRow => {
                        if is_icon_only {
                            (120.0_f32, 34.0_f32)
                        } else {
                            let char_count = label.chars().count().max(1) as f32;
                            ((char_count * 7.0 + 20.0 + icon_extra).max(120.0), 34.0_f32)
                        }
                    }
                    crate::ButtonStyle::MenuItem => {
                        if is_icon_only {
                            (96.0_f32, 30.0_f32)
                        } else {
                            let char_count = label.chars().count().max(1) as f32;
                            ((char_count * 7.5 + 20.0 + icon_extra).max(96.0), 30.0_f32)
                        }
                    }
                    crate::ButtonStyle::Standard
                    | crate::ButtonStyle::Primary
                    | crate::ButtonStyle::Segmented => {
                        if is_icon_only {
                            (36.0_f32, 34.0_f32)
                        } else {
                            let char_count = label.chars().count().max(1) as f32;
                            ((char_count * 8.5 + 24.0 + icon_extra).max(40.0), 34.0_f32)
                        }
                    }
                };
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
                let side = if *size == 0 {
                    16.0
                } else {
                    f32::from(*size).max(1.0)
                };
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
                self.adjust_container_child_style(taffy, class, &child_meta);
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
                    align_self: Some(AlignSelf::Stretch),
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

    fn adjust_container_child_style<M>(
        &self,
        taffy: &mut TaffyTree<()>,
        class: &str,
        child_meta: &NodeMeta<M>,
    ) {
        let child = &child_meta.node;
        let child_taffy_id = child_meta.taffy_id;
        let Ok(mut child_style) = taffy.style(child_taffy_id).cloned() else {
            return;
        };

        match class {
            CLASS_FORM_ROW => {
                if matches!(child, WidgetNode::Row { .. }) {
                    child_style.flex_grow = 1.0;
                    child_style.size.width = percent(1.0_f32);
                    child_style.align_items = Some(AlignItems::Center);
                    child_style.justify_content = Some(JustifyContent::SpaceBetween);
                    let _ = taffy.set_style(child_taffy_id, child_style);

                    if child_meta.children.len() >= 2
                        && let Some(control_meta) = child_meta.children.last()
                        && matches!(
                            control_meta.node,
                            WidgetNode::TextInput { .. } | WidgetNode::Dropdown { .. }
                        )
                        && let Ok(mut ctrl_style) = taffy.style(control_meta.taffy_id).cloned()
                    {
                        ctrl_style.flex_grow = 1.0;
                        ctrl_style.flex_shrink = 1.0;
                        ctrl_style.max_size.width =
                            length(340.0_f32.min(self.viewport_width * 0.65));
                        let _ = taffy.set_style(control_meta.taffy_id, ctrl_style);
                    }
                }
            }
            CLASS_MODAL_FOOTER => {
                if matches!(child, WidgetNode::Row { .. }) {
                    child_style.flex_grow = 1.0;
                    child_style.size.width = percent(1.0_f32);
                    child_style.align_items = Some(AlignItems::Center);
                    child_style.justify_content = Some(JustifyContent::FlexEnd);
                    let _ = taffy.set_style(child_taffy_id, child_style);

                    for btn_meta in &child_meta.children {
                        if matches!(btn_meta.node, WidgetNode::Button { .. })
                            && let Ok(mut btn_style) = taffy.style(btn_meta.taffy_id).cloned()
                        {
                            btn_style.min_size.width = length(68.0_f32);
                            let _ = taffy.set_style(btn_meta.taffy_id, btn_style);
                        }
                    }
                }
            }
            CLASS_MODAL_TABS => {
                if matches!(child, WidgetNode::Row { .. }) {
                    child_style.flex_grow = 1.0;
                    child_style.size.width = percent(1.0_f32);
                    child_style.align_items = Some(AlignItems::Center);
                    child_style.justify_content = Some(JustifyContent::SpaceBetween);
                    let _ = taffy.set_style(child_taffy_id, child_style);

                    for tab_meta in &child_meta.children {
                        if matches!(tab_meta.node, WidgetNode::Button { .. })
                            && let Ok(mut tab_style) = taffy.style(tab_meta.taffy_id).cloned()
                        {
                            tab_style.flex_grow = 1.0;
                            tab_style.flex_shrink = 1.0;
                            tab_style.flex_basis = length(0.0_f32);
                            tab_style.size.width = auto();
                            tab_style.min_size.width = length(48.0_f32);
                            let _ = taffy.set_style(tab_meta.taffy_id, tab_style);
                        }
                    }
                }
            }
            CLASS_TOP_TABBAR | CLASS_TOOLBAR => {
                if matches!(child, WidgetNode::Row { .. }) {
                    child_style.flex_grow = 1.0;
                    child_style.size.width = percent(1.0_f32);
                    child_style.align_items = Some(AlignItems::Center);
                    if child_meta.children.len() == 2 {
                        child_style.justify_content = Some(JustifyContent::SpaceBetween);
                    }
                    let _ = taffy.set_style(child_taffy_id, child_style);

                    if class == CLASS_TOP_TABBAR && child_meta.children.len() > 2 {
                        // Push trailing settings/help buttons to the right edge of the top tabbar
                        let right_anchor = child_meta
                            .children
                            .iter()
                            .skip(1)
                            .find(|gm| match &gm.node {
                                WidgetNode::Button {
                                    label, icon_name, ..
                                } => {
                                    label == "Settings"
                                        || label == "?"
                                        || label.contains("preferences-system")
                                        || label.contains("help-browser")
                                        || icon_name.as_deref().is_some_and(|ic| {
                                            ic.contains("preferences-system")
                                                || ic.contains("help-browser")
                                        })
                                }
                                _ => false,
                            })
                            .or_else(|| child_meta.children.last());
                        if let Some(anchor) = right_anchor
                            && let Ok(mut s) = taffy.style(anchor.taffy_id).cloned()
                        {
                            s.margin.left = auto();
                            let _ = taffy.set_style(anchor.taffy_id, s);
                        }
                    } else if class == CLASS_TOOLBAR && child_meta.children.len() >= 2 {
                        // Push the search pill (or trailing search input) to the right edge of the toolbar
                        let search_anchor = child_meta
                            .children
                            .iter()
                            .find(|gm| {
                                gm.class.as_deref() == Some(CLASS_SEARCH_PILL)
                                    || matches!(gm.node, WidgetNode::TextInput { .. })
                            })
                            .or_else(|| child_meta.children.last());
                        if let Some(anchor) = search_anchor
                            && let Ok(mut s) = taffy.style(anchor.taffy_id).cloned()
                        {
                            s.margin.left = auto();
                            let _ = taffy.set_style(anchor.taffy_id, s);
                        }
                    }
                }
            }
            CLASS_SEGMENTED_PILL | CLASS_SEARCH_PILL | CLASS_GROUP_BADGE => {
                if matches!(child, WidgetNode::Row { .. }) {
                    child_style.align_items = Some(AlignItems::Center);
                    child_style.justify_content = Some(JustifyContent::Center);
                    let _ = taffy.set_style(child_taffy_id, child_style);
                }
            }
            CLASS_CONNECTION_CARD => {
                if matches!(child, WidgetNode::Column { .. }) {
                    child_style.flex_grow = 1.0;
                    child_style.size.width = percent(1.0_f32);
                    child_style.justify_content = Some(JustifyContent::SpaceBetween);
                    let _ = taffy.set_style(child_taffy_id, child_style);

                    // Right-align the leading card trigger row (`...` and `*` buttons) in the top-right corner
                    if let Some(first_row) = child_meta.children.first()
                        && matches!(first_row.node, WidgetNode::Row { .. })
                        && let Ok(mut row_style) = taffy.style(first_row.taffy_id).cloned()
                    {
                        row_style.size.width = percent(1.0_f32);
                        row_style.justify_content = Some(JustifyContent::FlexEnd);
                        let _ = taffy.set_style(first_row.taffy_id, row_style);
                    }
                }
            }
            CLASS_DRAWER_NAV | CLASS_SIDEBAR | CLASS_CONTEXT_MENU | CLASS_ADD_MENU => {
                if matches!(child, WidgetNode::Column { .. }) {
                    child_style.flex_grow = 1.0;
                    child_style.size.width = percent(1.0_f32);
                    child_style.align_items = Some(AlignItems::Stretch);
                    let _ = taffy.set_style(child_taffy_id, child_style);

                    for item_meta in &child_meta.children {
                        if matches!(item_meta.node, WidgetNode::Button { .. })
                            && let Ok(mut item_style) = taffy.style(item_meta.taffy_id).cloned()
                        {
                            item_style.size.width = auto();
                            item_style.align_self = Some(AlignSelf::Stretch);
                            item_style.flex_shrink = 1.0;
                            let _ = taffy.set_style(item_meta.taffy_id, item_style);
                        }
                    }
                }
            }
            CLASS_SETTINGS_DRAWER | CLASS_HELP_DRAWER => {
                if matches!(child, WidgetNode::Row { .. }) {
                    child_style.flex_grow = 1.0;
                    child_style.size.width = percent(1.0_f32);
                    child_style.size.height = percent(1.0_f32);
                    child_style.align_items = Some(AlignItems::Stretch);
                    let _ = taffy.set_style(child_taffy_id, child_style);
                }
            }
            _ => {}
        }
    }

    fn style_for_container_class(&self, class: &str, is_root: bool) -> Style {
        let pad = |p: f32| TaffyRect {
            left: length(p),
            right: length(p),
            top: length(p),
            bottom: length(p),
        };
        let pad_xy = |px: f32, py: f32| TaffyRect {
            left: length(px),
            right: length(px),
            top: length(py),
            bottom: length(py),
        };

        match class {
            CLASS_ADD_DESKTOP_MODAL => {
                let modal_w = 560.0_f32.min((self.viewport_width - 32.0).max(240.0));
                let left_offset = ((self.viewport_width - modal_w) * 0.5).max(0.0);
                let top_offset = (self.viewport_height * 0.08).max(16.0);
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
                let drawer_w = 540.0_f32.min((self.viewport_width - 24.0).max(200.0));
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
            CLASS_CONTEXT_MENU => {
                let menu_w = 156.0_f32.min((self.viewport_width - 12.0).max(96.0));
                let left_offset = if is_root {
                    0.0
                } else {
                    (self.viewport_width * 0.24)
                        .clamp(8.0, (self.viewport_width - menu_w - 8.0).max(0.0))
                };
                let top_offset = if is_root {
                    0.0
                } else {
                    176.0_f32.min((self.viewport_height - 180.0).max(0.0))
                };
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Column,
                    position: if is_root {
                        Position::Relative
                    } else {
                        Position::Absolute
                    },
                    inset: TaffyRect {
                        left: length(left_offset),
                        top: length(top_offset),
                        right: auto(),
                        bottom: auto(),
                    },
                    size: Size {
                        width: length(menu_w),
                        height: auto(),
                    },
                    min_size: Size {
                        width: length(110.0_f32.min(menu_w)),
                        height: length(44.0_f32),
                    },
                    padding: pad(6.0),
                    ..Default::default()
                }
            }
            CLASS_ADD_MENU => {
                let menu_w = 140.0_f32.min((self.viewport_width - 12.0).max(96.0));
                let left_offset = if is_root {
                    0.0
                } else {
                    (self.viewport_width * 0.28)
                        .clamp(8.0, (self.viewport_width - menu_w - 8.0).max(0.0))
                };
                let top_offset = if is_root {
                    0.0
                } else {
                    84.0_f32.min((self.viewport_height - 130.0).max(0.0))
                };
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Column,
                    position: if is_root {
                        Position::Relative
                    } else {
                        Position::Absolute
                    },
                    inset: TaffyRect {
                        left: length(left_offset),
                        top: length(top_offset),
                        right: auto(),
                        bottom: auto(),
                    },
                    size: Size {
                        width: length(menu_w),
                        height: auto(),
                    },
                    min_size: Size {
                        width: length(104.0_f32.min(menu_w)),
                        height: length(40.0_f32),
                    },
                    padding: pad(6.0),
                    ..Default::default()
                }
            }
            CLASS_ROOT_SHELL => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                size: Size {
                    width: length(self.viewport_width),
                    height: length(self.viewport_height),
                },
                padding: pad(12.0),
                ..Default::default()
            },
            CLASS_HEADER => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                justify_content: Some(JustifyContent::Center),
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
            CLASS_TOP_TABBAR => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::SpaceBetween),
                flex_shrink: 0.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                min_size: Size {
                    width: auto(),
                    height: length(36.0_f32),
                },
                padding: pad_xy(10.0, 4.0),
                ..Default::default()
            },
            CLASS_TOOLBAR => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::SpaceBetween),
                flex_shrink: 0.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                min_size: Size {
                    width: auto(),
                    height: length(42.0_f32),
                },
                padding: pad_xy(10.0, 6.0),
                ..Default::default()
            },
            CLASS_SEGMENTED_PILL => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                flex_shrink: 0.0,
                size: Size {
                    width: auto(),
                    height: auto(),
                },
                min_size: Size {
                    width: length(72.0_f32),
                    height: length(36.0_f32),
                },
                padding: pad_xy(4.0, 2.0),
                ..Default::default()
            },
            CLASS_SEARCH_PILL => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                flex_shrink: 0.0,
                size: Size {
                    width: auto(),
                    height: auto(),
                },
                min_size: Size {
                    width: length(200.0_f32.min(self.viewport_width)),
                    height: length(36.0_f32),
                },
                padding: pad_xy(10.0, 2.0),
                ..Default::default()
            },
            CLASS_SIDEBAR => Style {
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
            CLASS_GROUP_SECTION => Style {
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
            CLASS_GROUP_BADGE => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                align_self: Some(AlignSelf::FlexStart),
                flex_shrink: 0.0,
                size: Size {
                    width: auto(),
                    height: auto(),
                },
                min_size: Size {
                    width: length(112.0_f32.min(self.viewport_width)),
                    height: length(28.0_f32),
                },
                padding: pad_xy(14.0, 4.0),
                ..Default::default()
            },
            CLASS_CONNECTION_CARD => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                justify_content: Some(JustifyContent::SpaceBetween),
                min_size: Size {
                    width: length(200.0_f32.min(self.viewport_width)),
                    height: length(240.0_f32.min(self.viewport_height)),
                },
                max_size: Size {
                    width: length(220.0_f32),
                    height: auto(),
                },
                padding: pad(14.0),
                ..Default::default()
            },
            CLASS_FORM_ROW => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::SpaceBetween),
                flex_shrink: 0.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                min_size: Size {
                    width: auto(),
                    height: length(36.0_f32),
                },
                padding: pad_xy(6.0, 3.0),
                ..Default::default()
            },
            CLASS_MODAL_TABS => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                flex_shrink: 0.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                min_size: Size {
                    width: auto(),
                    height: length(36.0_f32),
                },
                padding: pad_xy(8.0, 3.0),
                ..Default::default()
            },
            CLASS_MODAL_TAB_FRAME => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                min_size: Size {
                    width: auto(),
                    height: length(180.0_f32.min(self.viewport_height)),
                },
                padding: pad(14.0),
                ..Default::default()
            },
            CLASS_MODAL_FOOTER => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::FlexEnd),
                flex_shrink: 0.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                min_size: Size {
                    width: auto(),
                    height: length(40.0_f32),
                },
                padding: pad_xy(8.0, 4.0),
                ..Default::default()
            },
            CLASS_DRAWER_NAV => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_shrink: 0.0,
                align_self: Some(AlignSelf::Stretch),
                size: Size {
                    width: length(200.0_f32.min(self.viewport_width)),
                    height: auto(),
                },
                min_size: Size {
                    width: length(160.0_f32.min(self.viewport_width)),
                    height: length(180.0_f32.min(self.viewport_height)),
                },
                padding: pad(10.0),
                ..Default::default()
            },
            CLASS_DRAWER_CONTENT => Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                align_self: Some(AlignSelf::Stretch),
                size: Size {
                    width: auto(),
                    height: auto(),
                },
                min_size: Size {
                    width: length(180.0_f32.min(self.viewport_width)),
                    height: length(180.0_f32.min(self.viewport_height)),
                },
                padding: pad(14.0),
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
        enclosing_class: Option<&str>,
        out: &mut Vec<LayoutBox<M>>,
    ) {
        let Ok(layout) = taffy.layout(meta.taffy_id) else {
            return;
        };

        let width = layout.size.width;
        let is_overlay_root = meta
            .class
            .as_deref()
            .is_some_and(is_overlay_container_class);
        let is_popup_menu = meta.class.as_deref().is_some_and(is_popup_menu_class);

        let is_nested_in_component = enclosing_class.is_some_and(|c| c != CLASS_ROOT_SHELL)
            || parent_x > 24.0
            || parent_y > 24.0;

        let (abs_x, abs_y) = if is_popup_menu {
            if meta.id == 0 {
                (0.0, 0.0)
            } else if is_nested_in_component {
                // Anchored near the parent card or toolbar control
                let max_x = (self.viewport_width - width - 8.0).max(0.0);
                let max_y = (self.viewport_height - layout.size.height - 8.0).max(0.0);
                (
                    parent_x.clamp(8.0, max_x),
                    (parent_y + 28.0).clamp(8.0, max_y),
                )
            } else {
                (layout.location.x, layout.location.y)
            }
        } else if is_overlay_root {
            (layout.location.x, layout.location.y)
        } else {
            (parent_x + layout.location.x, parent_y + layout.location.y)
        };

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

        let child_enclosing = meta.class.as_deref().or(enclosing_class);
        for child in &meta.children {
            self.collect_layout_boxes(taffy, child, abs_x, abs_y, child_enclosing, out);
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
        let has_structural_panel = children.iter().any(|c| match c {
            WidgetNode::Container { class, .. } => !is_inline_container_class(class),
            WidgetNode::Scrollable { .. } | WidgetNode::Column { .. } | WidgetNode::Grid { .. } => {
                true
            }
            _ => false,
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
                    let add_menu_z = overlay_z_index_for_class(CLASS_ADD_MENU);
                    let ctx_menu_z = overlay_z_index_for_class(CLASS_CONTEXT_MENU);
                    for desc in &mut self.boxes[(idx + 1)..end] {
                        if desc.z_index == scroll_z
                            || desc.z_index == add_menu_z
                            || desc.z_index == ctx_menu_z
                        {
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

    #[test]
    fn test_two_tier_header_group_badge_portrait_cards_and_popup_menus_layout() {
        use crate::widget::{ButtonStyle, icon_button, menu_button};

        let tree: Element<'_, TestMsg> = container(
            CLASS_ROOT_SHELL,
            column(
                10,
                vec![
                    container(
                        CLASS_HEADER,
                        column(
                            6,
                            vec![
                                container(
                                    CLASS_TOP_TABBAR,
                                    row(
                                        8,
                                        vec![
                                            row(
                                                6,
                                                vec![
                                                    icon("com.theinfomoose.rFerram-symbolic", 20),
                                                    text("Connections").size(16).into(),
                                                ],
                                            ),
                                            row(
                                                6,
                                                vec![
                                                    icon_button("preferences-system-symbolic")
                                                        .into(),
                                                    icon_button("help-browser-symbolic").into(),
                                                ],
                                            ),
                                        ],
                                    ),
                                ),
                                container(
                                    CLASS_TOOLBAR,
                                    row(
                                        10,
                                        vec![
                                            icon_button("add-symbolic")
                                                .on_press(TestMsg::OpenModal)
                                                .into(),
                                            container(
                                                CLASS_SEGMENTED_PILL,
                                                row(
                                                    4,
                                                    vec![
                                                        icon_button("view-grid-symbolic")
                                                            .style(ButtonStyle::Segmented)
                                                            .active(true)
                                                            .into(),
                                                        icon_button("view-list-symbolic")
                                                            .style(ButtonStyle::Segmented)
                                                            .into(),
                                                    ],
                                                ),
                                            ),
                                            container(
                                                CLASS_SEARCH_PILL,
                                                row(
                                                    6,
                                                    vec![
                                                        text_input("Search connections...", "")
                                                            .into(),
                                                        icon("system-search-symbolic", 16),
                                                    ],
                                                ),
                                            ),
                                        ],
                                    ),
                                ),
                            ],
                        ),
                    ),
                    container(
                        CLASS_GROUP_SECTION,
                        column(
                            8,
                            vec![
                                container(CLASS_GROUP_BADGE, text("Group 1").size(14)),
                                divider(),
                                grid(
                                    3,
                                    16,
                                    vec![container(
                                        CLASS_CONNECTION_CARD,
                                        column(
                                            8,
                                            vec![
                                                row(
                                                    4,
                                                    vec![
                                                        button("...")
                                                            .style(ButtonStyle::CardTrigger)
                                                            .into(),
                                                        button("*")
                                                            .style(ButtonStyle::CardTrigger)
                                                            .into(),
                                                    ],
                                                ),
                                                column(
                                                    4,
                                                    vec![
                                                        text("Device Friendly Name (Local)")
                                                            .size(14)
                                                            .into(),
                                                        text("Saved Profile").size(11).into(),
                                                    ],
                                                ),
                                            ],
                                        ),
                                    )],
                                ),
                            ],
                        ),
                    ),
                    container(
                        CLASS_ADD_MENU,
                        column(
                            4,
                            vec![
                                menu_button("Add").on_press(TestMsg::OpenModal).into(),
                                menu_button("Import").into(),
                                menu_button("Export").into(),
                            ],
                        ),
                    ),
                    container(
                        CLASS_CONTEXT_MENU,
                        column(
                            4,
                            vec![
                                menu_button("Connect")
                                    .on_press(TestMsg::ConnectCard(1))
                                    .into(),
                                divider(),
                                menu_button("Configure").into(),
                                menu_button("Duplicate").into(),
                                menu_button("Export").into(),
                                menu_button("Delete").into(),
                            ],
                        ),
                    ),
                ],
            ),
        );

        let boxes = compute_layout(&tree, 1440.0, 900.0);

        let top_tabbar = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_TOP_TABBAR))
            .expect("missing top tabbar");
        let toolbar = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_TOOLBAR))
            .expect("missing toolbar");
        assert!(
            toolbar.y >= top_tabbar.y + top_tabbar.height,
            "toolbar must be stacked below top tabbar in two-tier header"
        );

        let seg_pill = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_SEGMENTED_PILL))
            .expect("missing segmented pill");
        let search_pill = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_SEARCH_PILL))
            .expect("missing search pill");
        assert!(
            (seg_pill.y - search_pill.y).abs() <= 4.0,
            "segmented pill and search pill must align horizontally in toolbar"
        );
        assert!(
            search_pill.x > toolbar.x + toolbar.width * 0.5,
            "search pill must be pushed to the right side of the toolbar: x={}",
            search_pill.x
        );

        let group_badge = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_GROUP_BADGE))
            .expect("missing group badge");
        assert!(
            group_badge.width >= 112.0 && group_badge.width < 260.0,
            "group badge must remain a compact tab pill: width={}",
            group_badge.width
        );

        let card = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_CONNECTION_CARD))
            .expect("missing connection card");
        assert!(
            card.height > card.width,
            "connection card must have portrait aspect ratio (height={} > width={})",
            card.height,
            card.width
        );

        // Card trigger buttons ("..." and "*") must be right-aligned in the top-right corner of the card
        let dots_btn = boxes
            .iter()
            .find(|b| matches!(&b.node, WidgetNode::Button { label, .. } if label == "..."))
            .expect("missing '...' card trigger button");
        assert!(
            dots_btn.x > card.x + card.width * 0.45,
            "card trigger '...' must be right-aligned in the upper-right of the portrait card"
        );

        let add_menu = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_ADD_MENU))
            .expect("missing add menu");
        let ctx_menu = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_CONTEXT_MENU))
            .expect("missing context menu");
        assert!(add_menu.z_index > 0);
        assert!(ctx_menu.z_index > add_menu.z_index);

        let connect_item = boxes
            .iter()
            .find(|b| matches!(&b.node, WidgetNode::Button { label, .. } if label == "Connect"))
            .expect("missing Connect menu item");
        let configure_item = boxes
            .iter()
            .find(|b| matches!(&b.node, WidgetNode::Button { label, .. } if label == "Configure"))
            .expect("missing Configure menu item");
        assert!(
            (connect_item.width - configure_item.width).abs() < 1.0,
            "menu items inside context menu must stretch to uniform width"
        );
        assert_eq!(
            hit_test_message(
                &boxes,
                connect_item.x + connect_item.width * 0.5,
                connect_item.y + connect_item.height * 0.5
            ),
            Some(TestMsg::ConnectCard(1))
        );
    }

    #[test]
    fn test_modal_form_rows_tab_frame_footer_and_split_drawer_layout() {
        use crate::widget::{checkbox, nav_button, segmented_button};

        let tree: Element<'_, TestMsg> = container(
            CLASS_ROOT_SHELL,
            column(
                10,
                vec![
                    container(
                        CLASS_SETTINGS_DRAWER,
                        row(
                            12,
                            vec![
                                container(
                                    CLASS_DRAWER_NAV,
                                    column(
                                        6,
                                        vec![
                                            nav_button("General")
                                                .icon("preferences-system-symbolic")
                                                .active(true)
                                                .on_press(TestMsg::SelectDrawerItem)
                                                .into(),
                                            nav_button("Resolution | Scaling")
                                                .icon("video-display-symbolic")
                                                .into(),
                                            nav_button("VPN").icon("network-vpn-symbolic").into(),
                                        ],
                                    ),
                                ),
                                container(
                                    CLASS_DRAWER_CONTENT,
                                    column(8, vec![text("Settings: General").size(16).into()]),
                                ),
                            ],
                        ),
                    ),
                    container(
                        CLASS_ADD_DESKTOP_MODAL,
                        column(
                            10,
                            vec![
                                text("Add Desktop").size(18).into(),
                                container(
                                    CLASS_FORM_ROW,
                                    row(
                                        10,
                                        vec![
                                            text("Connection:").into(),
                                            text_input("Host name or IP address", "").into(),
                                        ],
                                    ),
                                ),
                                container(
                                    CLASS_MODAL_TABS,
                                    row(
                                        6,
                                        vec![
                                            segmented_button("General").selected(true).into(),
                                            segmented_button("Display").into(),
                                            segmented_button("Devices | Audio").into(),
                                            segmented_button("Folders").into(),
                                        ],
                                    ),
                                ),
                                container(
                                    CLASS_MODAL_TAB_FRAME,
                                    column(
                                        8,
                                        vec![
                                            container(
                                                CLASS_FORM_ROW,
                                                row(
                                                    10,
                                                    vec![
                                                        text("Friendly Name:").into(),
                                                        text_input("Optional local only name", "")
                                                            .into(),
                                                    ],
                                                ),
                                            ),
                                            checkbox(
                                                "Reconnect if the session is dropped",
                                                true,
                                                TestMsg::ToggleReconnect(false),
                                            ),
                                        ],
                                    ),
                                ),
                                container(
                                    CLASS_MODAL_FOOTER,
                                    row(
                                        8,
                                        vec![
                                            button("Cancel").on_press(TestMsg::CancelModal).into(),
                                            button("Add").on_press(TestMsg::SaveModal).into(),
                                        ],
                                    ),
                                ),
                            ],
                        ),
                    ),
                ],
            ),
        );

        let boxes = compute_layout(&tree, 1440.0, 900.0);

        // 1. Verify two-pane split drawer layout and that long nav labels ("Resolution | Scaling") do not overflow CLASS_DRAWER_NAV
        let drawer_nav = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_DRAWER_NAV))
            .expect("missing drawer nav pane");
        let drawer_content = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_DRAWER_CONTENT))
            .expect("missing drawer content pane");
        assert!(
            drawer_content.x >= drawer_nav.x + drawer_nav.width,
            "drawer content pane must sit to the right of drawer nav pane"
        );
        assert!(
            drawer_content.width > drawer_nav.width,
            "drawer content pane must expand wider than drawer nav pane"
        );
        let res_nav_btn = boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    WidgetNode::Button { label, .. } if label == "Resolution | Scaling"
                )
            })
            .expect("missing Resolution | Scaling nav button");
        assert!(
            res_nav_btn.x + res_nav_btn.width <= drawer_nav.x + drawer_nav.width + 0.5,
            "Resolution | Scaling nav button (right={}) must not overflow drawer nav (right={})",
            res_nav_btn.x + res_nav_btn.width,
            drawer_nav.x + drawer_nav.width
        );

        // 2. Verify inline modal form row places label on left and input on right
        let conn_label = boxes
            .iter()
            .find(
                |b| matches!(&b.node, WidgetNode::Text { content, .. } if content == "Connection:"),
            )
            .expect("missing Connection: label");
        let host_input = boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    WidgetNode::TextInput { placeholder, .. }
                        if placeholder == "Host name or IP address"
                )
            })
            .expect("missing Host input");
        assert!(
            host_input.x > conn_label.x + conn_label.width,
            "form row must position input to the right of its label"
        );

        // 3. Verify equal-width modal tabs, tab frame, and right-aligned modal footer
        let modal_box = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_ADD_DESKTOP_MODAL))
            .expect("missing modal");
        let tabs_box = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_MODAL_TABS))
            .expect("missing modal tabs");
        let general_tab = boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    WidgetNode::Button {
                        label,
                        style: crate::ButtonStyle::Segmented,
                        ..
                    } if label == "General"
                )
            })
            .expect("missing General tab");
        let audio_tab = boxes
            .iter()
            .find(|b| {
                matches!(
                    &b.node,
                    WidgetNode::Button {
                        label,
                        style: crate::ButtonStyle::Segmented,
                        ..
                    } if label == "Devices | Audio"
                )
            })
            .expect("missing Devices | Audio tab");
        assert!(
            (general_tab.width - audio_tab.width).abs() < 1.0,
            "segmented tabs in CLASS_MODAL_TABS must divide tab bar into equal widths"
        );

        let frame_box = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_MODAL_TAB_FRAME))
            .expect("missing modal tab frame");
        let footer_box = boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_MODAL_FOOTER))
            .expect("missing modal footer");
        assert!(frame_box.y >= tabs_box.y + tabs_box.height * 0.5);
        assert!(footer_box.y >= frame_box.y + frame_box.height);

        let add_btn = boxes
            .iter()
            .find(|b| matches!(&b.node, WidgetNode::Button { label, .. } if label == "Add"))
            .expect("missing Add button");
        assert!(
            add_btn.x > modal_box.x + modal_box.width * 0.5,
            "Add button in modal footer must be right-aligned in the modal"
        );
    }

    #[test]
    fn test_nested_popup_menu_anchoring_and_scrollable_translation() {
        use crate::widget::menu_button;

        // 1. A context menu nested inside a card at (0, 0) must anchor to the card, not jump to viewport (345, 176)
        let standalone_card: Element<'_, TestMsg> = container(
            CLASS_CONNECTION_CARD,
            column(
                8,
                vec![
                    text("Device Friendly Name (Local)").into(),
                    container(
                        CLASS_CONTEXT_MENU,
                        column(
                            4,
                            vec![
                                menu_button("Connect")
                                    .on_press(TestMsg::ConnectCard(0))
                                    .into(),
                            ],
                        ),
                    ),
                ],
            ),
        );
        let card_boxes = compute_layout(&standalone_card, 1440.0, 900.0);
        let card_box = &card_boxes[0];
        let nested_menu = card_boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_CONTEXT_MENU))
            .expect("missing nested context menu");
        assert!(
            nested_menu.x >= card_box.x && nested_menu.x < card_box.x + card_box.width,
            "nested context menu (x={}) must stay anchored to its parent card at x={}..{}",
            nested_menu.x,
            card_box.x,
            card_box.x + card_box.width
        );

        // 2. When multiple cards are inside a Scrollable and scrolled, a nested context menu scrolls with its parent card
        let mut cards: Vec<Element<'_, TestMsg>> = Vec::new();
        for idx in 0..9 {
            let mut card_children: Vec<Element<'_, TestMsg>> =
                vec![text(format!("Card {idx}")).into()];
            if idx == 4 {
                card_children.push(container(
                    CLASS_CONTEXT_MENU,
                    column(
                        4,
                        vec![
                            menu_button("Connect")
                                .on_press(TestMsg::ConnectCard(idx))
                                .into(),
                        ],
                    ),
                ));
            }
            cards.push(container(CLASS_CONNECTION_CARD, column(8, card_children)));
        }

        let scroll_tree: Element<'_, TestMsg> = container(
            CLASS_ROOT_SHELL,
            column(10, vec![scrollable(grid(3, 16, cards))]),
        );

        let unscrolled = scroll_tree.layout_scene(1440.0, 500.0);
        let menu_before = unscrolled
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_CONTEXT_MENU))
            .expect("missing context menu before scroll")
            .y;

        let scrolled = unscrolled.with_scroll_offset(120.0);
        let menu_after = scrolled
            .boxes
            .iter()
            .find(|b| b.class.as_deref() == Some(CLASS_CONTEXT_MENU))
            .expect("missing context menu after scroll")
            .y;
        assert!(
            (menu_before - menu_after - 120.0).abs() < 1.0,
            "nested context menu inside Scrollable must translate with scroll_offset_y (before={menu_before}, after={menu_after})"
        );
        let connect_after = scrolled
            .boxes
            .iter()
            .find(|b| matches!(&b.node, WidgetNode::Button { label, .. } if label == "Connect"))
            .expect("missing Connect button after scroll");
        assert_eq!(
            scrolled.hit_test_message(
                connect_after.x + connect_after.width * 0.5,
                connect_after.y + connect_after.height * 0.5
            ),
            Some(TestMsg::ConnectCard(4))
        );
    }
}
