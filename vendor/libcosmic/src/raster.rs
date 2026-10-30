//! `tiny-skia` (`0.12.0`) Software/Vector Scene Rasterizer & Embedded ASCII Bitmap Font Engine
//!
//! Paints a positioned [`LayoutScene`] (or [`Element`] / [`Application`]) into an RGBA8
//! [`tiny_skia::Pixmap`] using [`cosmic::Theme`](crate::Theme):
//! - Dark and light surface palettes
//! - Cyan `accent_rgb` highlights
//! - Frosted glass modal and context drawer overlays (`frosted_blur_radius` & `frosted_opacity`)
//! - Rounded connection cards, buttons, text inputs with focus border & caret, togglers,
//!   dropdowns with vector chevrons, icons, and dividers
//! - Embedded 5x7 printable ASCII (`0x20..=0x7E`) bitmap glyph renderer with zero external font dependencies

pub use tiny_skia::{
    self, Color, Mask, Paint, PathBuilder, Pixmap, PixmapMut, PremultipliedColorU8,
};

use tiny_skia::{FillRule, LineCap, LineJoin, Stroke, Transform};

use crate::layout::{
    CLASS_ADD_DESKTOP_MODAL, CLASS_ADD_MENU, CLASS_CONNECTION_CARD, CLASS_CONTEXT_MENU,
    CLASS_DRAWER_CONTENT, CLASS_DRAWER_NAV, CLASS_FORM_ROW, CLASS_GROUP_BADGE, CLASS_GROUP_SECTION,
    CLASS_HEADER, CLASS_MODAL_FOOTER, CLASS_MODAL_TAB_FRAME, CLASS_MODAL_TABS, CLASS_ROOT_SHELL,
    CLASS_SEARCH_PILL, CLASS_SEGMENTED_PILL, CLASS_SIDEBAR, CLASS_TOOLBAR, CLASS_TOP_TABBAR,
    LayoutBox, LayoutScene, Rect, compute_clip_rects, is_overlay_container_class,
    is_popup_menu_class, widget_subtree_count,
};
use crate::{Application, Element, Theme, WidgetNode};

/// Width of each character cell in the embedded 5x7 ASCII bitmap font (in base pixels).
pub const GLYPH_WIDTH: u32 = 5;

/// Height of each character cell in the embedded 5x7 ASCII bitmap font (in base pixels).
pub const GLYPH_HEIGHT: u32 = 7;

/// Horizontal advance (5 glyph columns + 1 inter-character gap column) in base pixels.
pub const GLYPH_ADVANCE: u32 = 6;

/// Resolved RGBA8 color palette derived from [`Theme`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemePalette {
    pub window_bg: [u8; 4],
    pub header_bg: [u8; 4],
    pub sidebar_bg: [u8; 4],
    pub group_bg: [u8; 4],
    pub card_bg: [u8; 4],
    pub card_border: [u8; 4],
    pub button_bg: [u8; 4],
    pub button_border: [u8; 4],
    pub input_bg: [u8; 4],
    pub input_border: [u8; 4],
    pub text_primary: [u8; 4],
    pub text_muted: [u8; 4],
    pub text_on_accent: [u8; 4],
    pub divider: [u8; 4],
    pub toggler_off_bg: [u8; 4],
    pub toggler_thumb: [u8; 4],
    pub accent: [u8; 4],
    pub accent_muted: [u8; 4],
    pub frosted_surface: [u8; 4],
    pub modal_scrim: [u8; 4],
}

impl ThemePalette {
    /// Derives a complete RGBA8 [`ThemePalette`] from a [`Theme`].
    pub fn from_theme(theme: &Theme) -> Self {
        let ar = (theme.accent_rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8;
        let ag = (theme.accent_rgb[1].clamp(0.0, 1.0) * 255.0).round() as u8;
        let ab = (theme.accent_rgb[2].clamp(0.0, 1.0) * 255.0).round() as u8;
        let frosted_alpha = (theme.frosted_opacity.clamp(0.15, 0.98) * 255.0).round() as u8;

        if theme.is_dark {
            Self {
                window_bg: [20, 22, 28, 255],
                header_bg: [28, 32, 41, 255],
                sidebar_bg: [26, 30, 38, 255],
                group_bg: [24, 27, 35, 255],
                card_bg: [36, 41, 52, 255],
                card_border: [64, 73, 90, 255],
                button_bg: [48, 55, 70, 255],
                button_border: [82, 93, 114, 255],
                input_bg: [16, 19, 25, 255],
                input_border: [72, 82, 102, 255],
                text_primary: [238, 242, 248, 255],
                text_muted: [146, 156, 174, 255],
                text_on_accent: [12, 16, 22, 255],
                divider: [60, 68, 84, 255],
                toggler_off_bg: [58, 66, 82, 255],
                toggler_thumb: [245, 248, 252, 255],
                accent: [ar, ag, ab, 255],
                accent_muted: [ar, ag, ab, 96],
                frosted_surface: [28, 33, 44, frosted_alpha],
                modal_scrim: [8, 10, 14, 132],
            }
        } else {
            Self {
                window_bg: [242, 244, 248, 255],
                header_bg: [228, 233, 241, 255],
                sidebar_bg: [232, 236, 244, 255],
                group_bg: [236, 240, 246, 255],
                card_bg: [255, 255, 255, 255],
                card_border: [198, 206, 220, 255],
                button_bg: [220, 226, 236, 255],
                button_border: [178, 188, 206, 255],
                input_bg: [255, 255, 255, 255],
                input_border: [172, 182, 200, 255],
                text_primary: [24, 28, 36, 255],
                text_muted: [104, 114, 132, 255],
                text_on_accent: [250, 252, 255, 255],
                divider: [200, 208, 222, 255],
                toggler_off_bg: [186, 194, 208, 255],
                toggler_thumb: [255, 255, 255, 255],
                accent: [ar, ag, ab, 255],
                accent_muted: [ar, ag, ab, 84],
                frosted_surface: [244, 247, 252, frosted_alpha],
                modal_scrim: [20, 26, 36, 88],
            }
        }
    }
}

impl<M: Clone> LayoutScene<M> {
    /// Rasterizes this positioned [`LayoutScene`] into a newly allocated RGBA8 [`Pixmap`] using `theme`.
    pub fn rasterize(&self, theme: &Theme) -> Pixmap {
        rasterize_scene(self, theme)
    }

    /// Rasterizes this positioned [`LayoutScene`] into an existing [`Pixmap`] using `theme`.
    pub fn rasterize_into(&self, pixmap: &mut Pixmap, theme: &Theme) {
        rasterize_scene_into(self, pixmap, theme);
    }
}

/// Rasterizes a positioned [`LayoutScene`] into a newly allocated RGBA8 [`Pixmap`] using `theme`.
pub fn rasterize_scene<M: Clone>(scene: &LayoutScene<M>, theme: &Theme) -> Pixmap {
    let width = scene.width();
    let height = scene.height();
    let mut pixmap =
        Pixmap::new(width, height).expect("LayoutScene dimensions are guaranteed >= 1x1");
    rasterize_scene_into(scene, &mut pixmap, theme);
    pixmap
}

/// Rasterizes a slice of [`LayoutBox`] items into an RGBA8 [`Pixmap`] at `(viewport_width, viewport_height)`.
pub fn rasterize_boxes<M: Clone>(
    boxes: &[LayoutBox<M>],
    viewport_width: u32,
    viewport_height: u32,
    theme: &Theme,
) -> Pixmap {
    let scene = LayoutScene::new(
        viewport_width.max(1) as f32,
        viewport_height.max(1) as f32,
        boxes.to_vec(),
    );
    rasterize_scene(&scene, theme)
}

/// Lays out and rasterizes a declarative [`Element`] tree at `(viewport_width, viewport_height)`.
pub fn rasterize_element<M: Clone>(
    element: &Element<'_, M>,
    viewport_width: u32,
    viewport_height: u32,
    theme: &Theme,
) -> Pixmap {
    let scene = LayoutScene::from_element(
        element,
        viewport_width.max(1) as f32,
        viewport_height.max(1) as f32,
    );
    rasterize_scene(&scene, theme)
}

/// Lays out and rasterizes a [`cosmic::Application`](crate::Application) view at `(viewport_width, viewport_height)`
/// using the application's configured [`Theme`].
pub fn rasterize_application<A: Application>(
    app: &A,
    viewport_width: u32,
    viewport_height: u32,
) -> Pixmap {
    let view = app.view();
    let theme = &app.core().theme;
    rasterize_element(&view, viewport_width, viewport_height, theme)
}

/// Selects the default focused `TextInput` widget ID when `scene.focused_widget_id` is `None`:
/// prefers the highest `z_index` layer containing a `TextInput`, and within that layer selects
/// the first `TextInput` with a non-empty `value`, or else the first `TextInput` in tree order.
fn default_focused_text_input_id<M>(boxes: &[LayoutBox<M>]) -> Option<usize> {
    let top_input_z = boxes
        .iter()
        .filter(|b| matches!(b.node, WidgetNode::TextInput { .. }))
        .map(|b| b.z_index)
        .max()?;

    boxes
        .iter()
        .find(|b| {
            b.z_index == top_input_z
                && matches!(&b.node, WidgetNode::TextInput { value, .. } if !value.is_empty())
        })
        .or_else(|| {
            boxes.iter().find(|b| {
                b.z_index == top_input_z && matches!(b.node, WidgetNode::TextInput { .. })
            })
        })
        .map(|b| b.id)
}

/// Paints a positioned [`LayoutScene`] into an existing [`Pixmap`] using `theme`.
pub fn rasterize_scene_into<M: Clone>(scene: &LayoutScene<M>, pixmap: &mut Pixmap, theme: &Theme) {
    let palette = ThemePalette::from_theme(theme);
    pixmap.fill(to_skia_color(palette.window_bg));

    if scene.boxes.is_empty() {
        return;
    }

    let effective_focus_id = scene
        .focused_widget_id
        .or_else(|| default_focused_text_input_id(&scene.boxes));

    let clips = compute_clip_rects(&scene.boxes);

    // Stably sort boxes by ascending `z_index` so background flow (`z_index == 0`) paints before
    // context drawers (`z_index == 20..25`) and modal dialogs (`z_index == 30`).
    let mut ordered_indices: Vec<usize> = (0..scene.boxes.len()).collect();
    ordered_indices.sort_by_key(|&idx| scene.boxes[idx].z_index);

    let mut scrollbars_to_paint: Vec<(u32, Rect, f32, Option<Rect>)> = Vec::new();

    for idx in ordered_indices {
        let lbox = &scene.boxes[idx];
        let clip_rect = clips[idx];
        if matches!(lbox.node, WidgetNode::Scrollable { .. }) {
            let bounds = lbox.bounds();
            let subtree_len = widget_subtree_count(&lbox.node);
            let end = (idx + subtree_len).min(scene.boxes.len());
            let content_bottom = scene.boxes[(idx + 1)..end]
                .iter()
                .filter(|b| b.z_index == lbox.z_index)
                .map(|b| b.y + b.height)
                .fold(bounds.y + bounds.height, f32::max);
            let content_height = (content_bottom - bounds.y).max(bounds.height);
            if content_height > bounds.height + 1.0 {
                scrollbars_to_paint.push((lbox.z_index, bounds, content_height, clip_rect));
            }
        }

        let is_focused = effective_focus_id == Some(lbox.id);
        let is_hovered = scene.hovered_widget_id == Some(lbox.id);
        paint_layout_box(
            pixmap,
            lbox,
            theme,
            &palette,
            is_focused,
            is_hovered,
            scene.caret_position,
            clip_rect,
        );
    }

    // Paint scrollbar tracks and proportional thumbs for any overflowing Scrollable containers
    for (_z, scroll_bounds, content_height, parent_clip) in scrollbars_to_paint {
        paint_scrollbar(
            pixmap,
            scroll_bounds,
            content_height,
            scene.scroll_offset_y,
            parent_clip,
            &palette,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_layout_box<M>(
    pixmap: &mut Pixmap,
    lbox: &LayoutBox<M>,
    theme: &Theme,
    palette: &ThemePalette,
    is_focused: bool,
    is_hovered: bool,
    caret_position: Option<usize>,
    clip_rect: Option<Rect>,
) {
    if lbox.width <= 0.5 || lbox.height <= 0.5 {
        return;
    }
    let bounds = lbox.bounds();

    let mut mask_storage: Option<Mask> = None;
    if let Some(clip) = clip_rect {
        let visible = bounds.intersect_clamped(clip);
        if visible.width <= 0.5 || visible.height <= 0.5 {
            return;
        }
        if !clip.contains_rect(bounds) {
            mask_storage = build_clip_mask(pixmap.width(), pixmap.height(), clip);
        }
    }
    let mask = mask_storage.as_ref();

    match &lbox.node {
        WidgetNode::Container { class, .. } => {
            paint_container(pixmap, bounds, class, theme, palette, mask);
        }
        WidgetNode::Text { content, size } => {
            let text_y = bounds.y + ((bounds.height - glyph_pixel_height(*size)) * 0.5).max(0.0);
            let text_clip = clip_rect.map_or(bounds, |c| bounds.intersect_clamped(c));
            draw_text(
                pixmap,
                content,
                bounds.x,
                text_y,
                *size,
                palette.text_primary,
                Some(text_clip),
            );
        }
        WidgetNode::Button {
            label,
            icon_name,
            style,
            is_active,
            ..
        } => {
            paint_button(
                pixmap,
                bounds,
                label,
                icon_name.as_deref(),
                *style,
                *is_active,
                palette,
                is_hovered,
                clip_rect,
                mask,
            );
        }
        WidgetNode::TextInput {
            placeholder, value, ..
        } => {
            paint_text_input(
                pixmap,
                bounds,
                placeholder,
                value,
                palette,
                is_focused,
                caret_position,
                clip_rect,
                mask,
            );
        }
        WidgetNode::Toggler {
            label,
            is_checked,
            style,
            ..
        } => {
            paint_toggler(
                pixmap,
                bounds,
                label,
                *is_checked,
                *style,
                palette,
                clip_rect,
                mask,
            );
        }
        WidgetNode::Dropdown {
            options, selected, ..
        } => {
            let selected_label = selected
                .and_then(|idx| options.get(idx))
                .or_else(|| options.first())
                .map(String::as_str)
                .unwrap_or("");
            paint_dropdown(pixmap, bounds, selected_label, palette, clip_rect, mask);
        }
        WidgetNode::Icon { name, size } => {
            paint_icon(pixmap, bounds, name, *size, palette, mask);
        }
        WidgetNode::Divider => {
            paint_divider(pixmap, bounds, palette, mask);
        }
        WidgetNode::Row { .. }
        | WidgetNode::Column { .. }
        | WidgetNode::Grid { .. }
        | WidgetNode::Scrollable { .. } => {}
    }
}

fn build_clip_mask(width: u32, height: u32, clip: Rect) -> Option<Mask> {
    let mut mask = Mask::new(width, height)?;
    let path = build_rounded_rect_path(clip, 0.0)?;
    mask.fill_path(&path, FillRule::Winding, false, Transform::identity());
    Some(mask)
}

fn paint_scrollbar(
    pixmap: &mut Pixmap,
    bounds: Rect,
    content_height: f32,
    scroll_offset_y: f32,
    parent_clip: Option<Rect>,
    palette: &ThemePalette,
) {
    let visible = parent_clip.map_or(bounds, |c| bounds.intersect_clamped(c));
    if visible.width <= 8.0 || visible.height <= 8.0 {
        return;
    }
    let mask_storage = parent_clip.and_then(|c| {
        if c.contains_rect(bounds) {
            None
        } else {
            build_clip_mask(pixmap.width(), pixmap.height(), c)
        }
    });
    let mask = mask_storage.as_ref();

    let track_w = 6.0_f32;
    let track_x = bounds.x + bounds.width - track_w - 2.0;
    let track_y = bounds.y + 4.0;
    let track_h = (bounds.height - 8.0).max(8.0);
    let track_rect = Rect::new(track_x, track_y, track_w, track_h);
    fill_rounded_rect_masked(pixmap, track_rect, 3.0, palette.divider, mask);

    let total_h = (content_height + scroll_offset_y.max(0.0)).max(bounds.height);
    let ratio = (bounds.height / total_h).clamp(0.1, 1.0);
    let thumb_h = (track_h * ratio).max(16.0).min(track_h);
    let max_scroll = (total_h - bounds.height).max(1.0);
    let scroll_progress = (scroll_offset_y / max_scroll).clamp(0.0, 1.0);
    let thumb_y = track_y + (track_h - thumb_h) * scroll_progress;
    let thumb_rect = Rect::new(track_x, thumb_y, track_w, thumb_h);
    fill_rounded_rect_masked(pixmap, thumb_rect, 3.0, palette.accent, mask);
}

fn paint_container(
    pixmap: &mut Pixmap,
    bounds: Rect,
    class: &str,
    theme: &Theme,
    palette: &ThemePalette,
    mask: Option<&Mask>,
) {
    if is_popup_menu_class(class) {
        let blur_radius = (theme.frosted_blur_radius.round() as u32).clamp(2, 10);
        blur_pixmap_region(pixmap, bounds, blur_radius);

        if class == CLASS_CONTEXT_MENU {
            let radius = 6.0_f32;
            fill_rounded_rect_masked(pixmap, bounds, radius, palette.frosted_surface, mask);
            stroke_rounded_rect_masked(pixmap, bounds, radius, 1.25, palette.card_border, mask);
            let inner = inset_rect(bounds, 1.5);
            stroke_rounded_rect_masked(pixmap, inner, 4.5, 1.0, palette.accent_muted, mask);

            // Separator rule below the primary "Connect" action row matching the mockup popup
            let sep_y = (bounds.y + (bounds.height * 0.24).clamp(24.0, 38.0))
                .min(bounds.y + bounds.height - 4.0);
            let sep_rect = Rect::new(bounds.x + 8.0, sep_y, (bounds.width - 16.0).max(4.0), 1.0);
            fill_rounded_rect_masked(pixmap, sep_rect, 0.0, palette.divider, mask);
        } else if class == CLASS_ADD_MENU {
            // CLASS_ADD_MENU ("Add / Import / Export" 3-row boxed popup)
            let radius = 4.0_f32;
            fill_rounded_rect_masked(pixmap, bounds, radius, palette.frosted_surface, mask);
            stroke_rounded_rect_masked(pixmap, bounds, radius, 1.25, palette.card_border, mask);
            let top_strip = Rect::new(
                bounds.x + 6.0,
                bounds.y + 1.0,
                (bounds.width - 12.0).max(4.0),
                2.0_f32.min(bounds.height),
            );
            fill_rounded_rect_masked(pixmap, top_strip, 1.0, palette.accent, mask);
            for frac in [1.0_f32 / 3.0, 2.0_f32 / 3.0] {
                let row_sep = Rect::new(
                    bounds.x + 4.0,
                    bounds.y + bounds.height * frac,
                    (bounds.width - 8.0).max(4.0),
                    1.0,
                );
                fill_rounded_rect_masked(pixmap, row_sep, 0.0, palette.divider, mask);
            }
        }
        return;
    }

    if is_overlay_container_class(class) {
        if class == CLASS_ADD_DESKTOP_MODAL {
            // Dim the full viewport behind the modal dialog
            let full_rect = Rect::new(0.0, 0.0, pixmap.width() as f32, pixmap.height() as f32);
            fill_rounded_rect(pixmap, full_rect, 0.0, palette.modal_scrim);
        }

        // Apply software frosted-glass blur to the underlying framebuffer pixels within the overlay bounds
        let blur_radius = (theme.frosted_blur_radius.round() as u32).clamp(2, 12);
        blur_pixmap_region(pixmap, bounds, blur_radius);

        let radius = if class == CLASS_ADD_DESKTOP_MODAL {
            12.0
        } else {
            8.0
        };
        fill_rounded_rect_masked(pixmap, bounds, radius, palette.frosted_surface, mask);
        stroke_rounded_rect_masked(pixmap, bounds, radius, 1.5, palette.accent, mask);

        // Top cyan accent highlight strip along the modal/drawer header edge
        let strip_h = 3.0_f32.min(bounds.height);
        let strip_rect = Rect::new(
            bounds.x + 8.0,
            bounds.y + 1.0,
            (bounds.width - 16.0).max(4.0),
            strip_h,
        );
        fill_rounded_rect_masked(pixmap, strip_rect, 1.5, palette.accent, mask);
        return;
    }

    match class {
        CLASS_ROOT_SHELL => {
            fill_rounded_rect_masked(pixmap, bounds, 0.0, palette.window_bg, mask);
        }
        CLASS_HEADER => {
            fill_rounded_rect_masked(pixmap, bounds, 8.0, palette.header_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, 8.0, 1.0, palette.card_border, mask);
        }
        CLASS_TOP_TABBAR => {
            fill_rounded_rect_masked(pixmap, bounds, 6.0, palette.input_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, 6.0, 1.0, palette.card_border, mask);

            // Active "Connections" tab surface on the left side of the top tabbar
            let tab_w = 132.0_f32.min((bounds.width * 0.42).max(24.0));
            let tab_h = (bounds.height - 6.0).max(4.0);
            let tab_rect = Rect::new(bounds.x + 4.0, bounds.y + 3.0, tab_w, tab_h);
            fill_rounded_rect_masked(pixmap, tab_rect, 5.0, palette.card_bg, mask);
            stroke_rounded_rect_masked(pixmap, tab_rect, 5.0, 1.0, palette.card_border, mask);

            let tab_indicator = Rect::new(
                tab_rect.x + 8.0,
                (tab_rect.y + tab_rect.height - 2.0).max(tab_rect.y),
                (tab_rect.width - 16.0).max(4.0),
                2.0,
            );
            fill_rounded_rect_masked(pixmap, tab_indicator, 1.0, palette.accent, mask);
        }
        CLASS_TOOLBAR => {
            fill_rounded_rect_masked(pixmap, bounds, 6.0, palette.header_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, 6.0, 1.0, palette.card_border, mask);
            let bottom_rule = Rect::new(
                bounds.x + 4.0,
                (bounds.y + bounds.height - 1.5).max(bounds.y),
                (bounds.width - 8.0).max(4.0),
                1.0,
            );
            fill_rounded_rect_masked(pixmap, bottom_rule, 0.0, palette.divider, mask);
        }
        CLASS_SEGMENTED_PILL => {
            let radius = (bounds.height * 0.5).min(18.0);
            fill_rounded_rect_masked(pixmap, bounds, radius, palette.input_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, radius, 1.2, palette.input_border, mask);
            let mid_x = bounds.x + bounds.width * 0.5 - 0.5;
            let sep_rect = Rect::new(mid_x, bounds.y + 5.0, 1.0, (bounds.height - 10.0).max(4.0));
            fill_rounded_rect_masked(pixmap, sep_rect, 0.0, palette.divider, mask);
        }
        CLASS_SEARCH_PILL => {
            let radius = (bounds.height * 0.5).min(18.0);
            fill_rounded_rect_masked(pixmap, bounds, radius, palette.input_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, radius, 1.25, palette.input_border, mask);
            let inner = inset_rect(bounds, 1.5);
            stroke_rounded_rect_masked(
                pixmap,
                inner,
                (radius - 1.5).max(2.0),
                1.0,
                palette.card_border,
                mask,
            );
        }
        CLASS_SIDEBAR => {
            fill_rounded_rect_masked(pixmap, bounds, 8.0, palette.sidebar_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, 8.0, 1.0, palette.card_border, mask);
        }
        CLASS_GROUP_SECTION => {
            fill_rounded_rect_masked(pixmap, bounds, 10.0, palette.group_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, 10.0, 1.0, palette.card_border, mask);
        }
        CLASS_GROUP_BADGE => {
            fill_rounded_rect_masked(pixmap, bounds, 6.0, palette.card_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, 6.0, 1.2, palette.card_border, mask);
            let badge_strip = Rect::new(
                bounds.x + 6.0,
                (bounds.y + bounds.height - 2.5).max(bounds.y),
                (bounds.width - 12.0).max(4.0),
                2.0,
            );
            fill_rounded_rect_masked(pixmap, badge_strip, 1.0, palette.accent, mask);
        }
        CLASS_CONNECTION_CARD => {
            let outer_r = 16.0_f32;
            fill_rounded_rect_masked(pixmap, bounds, outer_r, palette.card_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, outer_r, 1.5, palette.card_border, mask);

            // Inner second border ("double border" matching rFerramLayout-PNG.drawio.png)
            let inner_bounds = inset_rect(bounds, 3.5);
            let inner_r = (outer_r - 3.0).max(4.0);
            stroke_rounded_rect_masked(
                pixmap,
                inner_bounds,
                inner_r,
                1.0,
                palette.input_border,
                mask,
            );

            // Curved upper preview sheen across the upper half of the portrait card
            paint_card_upper_preview_sheen(pixmap, bounds, inner_r, theme, palette, mask);

            // Subtle accent left indicator bar on each connection card
            let bar = Rect::new(
                bounds.x + 2.0,
                bounds.y + 12.0,
                2.5,
                (bounds.height - 24.0).max(4.0),
            );
            fill_rounded_rect_masked(pixmap, bar, 1.25, palette.accent, mask);
        }
        CLASS_FORM_ROW => {
            fill_rounded_rect_masked(pixmap, bounds, 5.0, palette.group_bg, mask);
            let bottom_guide = Rect::new(
                bounds.x + 4.0,
                (bounds.y + bounds.height - 1.0).max(bounds.y),
                (bounds.width - 8.0).max(2.0),
                1.0,
            );
            fill_rounded_rect_masked(pixmap, bottom_guide, 0.0, palette.divider, mask);
        }
        CLASS_MODAL_TABS => {
            let radius = (bounds.height * 0.5).min(16.0);
            fill_rounded_rect_masked(pixmap, bounds, radius, palette.input_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, radius, 1.2, palette.card_border, mask);
            for frac in [0.25_f32, 0.50, 0.75] {
                let tick = Rect::new(
                    bounds.x + bounds.width * frac,
                    bounds.y + 4.0,
                    1.0,
                    (bounds.height - 8.0).max(4.0),
                );
                fill_rounded_rect_masked(pixmap, tick, 0.0, palette.divider, mask);
            }
        }
        CLASS_MODAL_TAB_FRAME => {
            fill_rounded_rect_masked(pixmap, bounds, 8.0, palette.group_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, 8.0, 1.25, palette.card_border, mask);
            let inner = inset_rect(bounds, 2.5);
            stroke_rounded_rect_masked(pixmap, inner, 6.0, 1.0, palette.divider, mask);
        }
        CLASS_MODAL_FOOTER => {
            let top_rule = Rect::new(
                bounds.x + 2.0,
                bounds.y + 1.0,
                (bounds.width - 4.0).max(2.0),
                1.0,
            );
            fill_rounded_rect_masked(pixmap, top_rule, 0.0, palette.divider, mask);

            let pill_w = 148.0_f32.min((bounds.width - 8.0).max(16.0));
            let pill_h = (bounds.height - 6.0).max(8.0);
            let pill_rect = Rect::new(
                (bounds.x + bounds.width - pill_w - 4.0).max(bounds.x),
                bounds.y + 4.0,
                pill_w,
                pill_h,
            );
            fill_rounded_rect_masked(pixmap, pill_rect, 6.0, palette.input_bg, mask);
            stroke_rounded_rect_masked(pixmap, pill_rect, 6.0, 1.0, palette.card_border, mask);
            let mid_sep = Rect::new(
                pill_rect.x + pill_rect.width * 0.5,
                pill_rect.y + 4.0,
                1.0,
                (pill_rect.height - 8.0).max(2.0),
            );
            fill_rounded_rect_masked(pixmap, mid_sep, 0.0, palette.divider, mask);
        }
        CLASS_DRAWER_NAV => {
            fill_rounded_rect_masked(pixmap, bounds, 6.0, palette.sidebar_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, 6.0, 1.0, palette.card_border, mask);
            let right_sep = Rect::new(
                (bounds.x + bounds.width - 2.0).max(bounds.x),
                bounds.y + 4.0,
                1.5,
                (bounds.height - 8.0).max(4.0),
            );
            fill_rounded_rect_masked(pixmap, right_sep, 0.0, palette.divider, mask);
        }
        CLASS_DRAWER_CONTENT => {
            fill_rounded_rect_masked(pixmap, bounds, 6.0, palette.group_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, 6.0, 1.0, palette.card_border, mask);
            let inner = inset_rect(bounds, 2.0);
            stroke_rounded_rect_masked(pixmap, inner, 4.5, 1.0, palette.input_border, mask);
        }
        _ => {}
    }
}

fn paint_card_upper_preview_sheen(
    pixmap: &mut Pixmap,
    card_bounds: Rect,
    inner_radius: f32,
    theme: &Theme,
    palette: &ThemePalette,
    mask: Option<&Mask>,
) {
    let sheen_box = inset_rect(card_bounds, 5.0);
    if sheen_box.width <= 8.0 || sheen_box.height <= 12.0 {
        return;
    }

    let x0 = sheen_box.x;
    let y0 = sheen_box.y;
    let x1 = x0 + sheen_box.width;
    let cx = x0 + sheen_box.width * 0.5;
    let r = (inner_radius - 1.5)
        .clamp(2.0, sheen_box.width * 0.4)
        .clamp(2.0, sheen_box.height * 0.3);
    let y_edge = y0 + sheen_box.height * 0.44;
    let y_ctrl = y0 + sheen_box.height * 0.58;

    let mut pb = PathBuilder::new();
    pb.move_to(x0 + r, y0);
    pb.line_to(x1 - r, y0);
    pb.quad_to(x1, y0, x1, y0 + r);
    pb.line_to(x1, y_edge);
    pb.quad_to(cx, y_ctrl, x0, y_edge);
    pb.line_to(x0, y0 + r);
    pb.quad_to(x0, y0, x0 + r, y0);
    pb.close();

    if let Some(sheen_path) = pb.finish() {
        let sheen_rgba = if theme.is_dark {
            [255, 255, 255, 22]
        } else {
            [20, 32, 52, 16]
        };
        fill_vector_path(pixmap, &sheen_path, sheen_rgba, mask);
    }

    // Subtle curved horizon stroke along the lower arc of the preview sheen
    let mut arc_pb = PathBuilder::new();
    arc_pb.move_to(x0 + 1.0, y_edge);
    arc_pb.quad_to(cx, y_ctrl, x1 - 1.0, y_edge);
    if let Some(arc_path) = arc_pb.finish() {
        stroke_vector_path(pixmap, &arc_path, 1.0, palette.divider, mask);
    }
}

fn is_primary_accent_button(label: &str) -> bool {
    matches!(label, "+" | "Add" | "Connect" | "*")
}

fn resolve_shorthand_icon_name(name_or_label: &str) -> Option<&str> {
    match name_or_label {
        "+" => Some("add-symbolic"),
        "?" => Some("help-browser-symbolic"),
        "..." => Some("view-more-symbolic"),
        "*" => Some("emblem-favorite-symbolic"),
        s if s.ends_with("-symbolic") || s.contains("rFerram") || s.contains("rferram") => Some(s),
        _ => None,
    }
}

fn icon_palette_for_accent_fill(palette: &ThemePalette) -> ThemePalette {
    let mut contrasted = *palette;
    contrasted.accent = palette.text_on_accent;
    contrasted.accent_muted = [
        palette.text_on_accent[0],
        palette.text_on_accent[1],
        palette.text_on_accent[2],
        96,
    ];
    contrasted
}

#[allow(clippy::too_many_arguments)]
fn paint_button(
    pixmap: &mut Pixmap,
    bounds: Rect,
    label: &str,
    icon_name: Option<&str>,
    style: crate::ButtonStyle,
    is_active: bool,
    palette: &ThemePalette,
    is_hovered: bool,
    clip_rect: Option<Rect>,
    mask: Option<&Mask>,
) {
    let font_size = 13u16;
    let text_h = glyph_pixel_height(font_size);
    let inner_clip = inset_rect(bounds, 2.0);
    let effective_clip = clip_rect.map_or(inner_clip, |c| inner_clip.intersect_clamped(c));

    match style {
        crate::ButtonStyle::Icon | crate::ButtonStyle::CardTrigger => {
            let is_card = style == crate::ButtonStyle::CardTrigger;
            let radius = (bounds.height * 0.5).min(if is_card { 7.0 } else { 8.0 });
            let fill_color = if is_active {
                palette.accent
            } else if is_hovered {
                palette.accent_muted
            } else if is_card {
                palette.input_bg
            } else {
                palette.button_bg
            };
            let border_color = if is_active || is_hovered {
                palette.accent
            } else if is_card {
                palette.card_border
            } else {
                palette.button_border
            };

            fill_rounded_rect_masked(pixmap, bounds, radius, fill_color, mask);
            stroke_rounded_rect_masked(pixmap, bounds, radius, 1.2, border_color, mask);

            let resolved_icon = icon_name.or_else(|| resolve_shorthand_icon_name(label));
            if let Some(ic_name) = resolved_icon {
                let side = bounds.width.min(bounds.height).clamp(12.0, 22.0);
                let ic_rect = Rect::new(
                    bounds.x + (bounds.width - side) * 0.5,
                    bounds.y + (bounds.height - side) * 0.5,
                    side,
                    side,
                );
                let ic_palette = if is_active {
                    icon_palette_for_accent_fill(palette)
                } else {
                    *palette
                };
                paint_icon(
                    pixmap,
                    ic_rect,
                    ic_name,
                    side.round() as u16,
                    &ic_palette,
                    mask,
                );
            } else if !label.is_empty() {
                let text_w = measure_text_width(label, font_size);
                let tx = bounds.x + ((bounds.width - text_w) * 0.5).max(3.0);
                let ty = bounds.y + ((bounds.height - text_h) * 0.5).max(2.0);
                let text_color = if is_active {
                    palette.text_on_accent
                } else {
                    palette.text_primary
                };
                draw_text(
                    pixmap,
                    label,
                    tx,
                    ty,
                    font_size,
                    text_color,
                    Some(effective_clip),
                );
            }
        }
        crate::ButtonStyle::Segmented => {
            let fill_color = if is_active {
                palette.accent_muted
            } else if is_hovered {
                palette.button_bg
            } else {
                palette.input_bg
            };
            let border_color = if is_active || is_hovered {
                palette.accent
            } else {
                palette.input_border
            };
            let text_color = if is_active || is_hovered {
                palette.text_primary
            } else {
                palette.text_muted
            };

            fill_rounded_rect_masked(pixmap, bounds, 6.0, fill_color, mask);
            stroke_rounded_rect_masked(
                pixmap,
                bounds,
                6.0,
                if is_active { 1.5 } else { 1.0 },
                border_color,
                mask,
            );

            if is_active {
                let bar_w = (bounds.width - 12.0).max(6.0);
                let bar_rect = Rect::new(
                    bounds.x + (bounds.width - bar_w) * 0.5,
                    (bounds.y + bounds.height - 3.0).max(bounds.y),
                    bar_w,
                    2.5,
                );
                fill_rounded_rect_masked(pixmap, bar_rect, 1.25, palette.accent, mask);
            }

            let mut text_x_offset = 0.0_f32;
            if let Some(ic_name) = icon_name {
                let ic_side = 16.0_f32.min(bounds.height - 6.0).max(10.0);
                if label.is_empty() || label == ic_name {
                    let ic_rect = Rect::new(
                        bounds.x + (bounds.width - ic_side) * 0.5,
                        bounds.y + (bounds.height - ic_side) * 0.5,
                        ic_side,
                        ic_side,
                    );
                    paint_icon(pixmap, ic_rect, ic_name, 16, palette, mask);
                    return;
                }
                let ic_rect = Rect::new(
                    bounds.x + 6.0,
                    bounds.y + (bounds.height - ic_side) * 0.5,
                    ic_side,
                    ic_side,
                );
                paint_icon(pixmap, ic_rect, ic_name, 16, palette, mask);
                text_x_offset = ic_side + 6.0;
            }

            let text_w = measure_text_width(label, font_size);
            let tx =
                bounds.x + text_x_offset + ((bounds.width - text_x_offset - text_w) * 0.5).max(4.0);
            let ty = bounds.y + ((bounds.height - text_h) * 0.5).max(2.0);
            draw_text(
                pixmap,
                label,
                tx,
                ty,
                font_size,
                text_color,
                Some(effective_clip),
            );
        }
        crate::ButtonStyle::NavRow | crate::ButtonStyle::MenuItem => {
            let is_menu = style == crate::ButtonStyle::MenuItem;
            let radius = if is_menu { 4.0 } else { 6.0 };
            let fill_color = if is_active || is_hovered {
                palette.accent_muted
            } else if is_menu {
                palette.card_bg
            } else {
                palette.sidebar_bg
            };
            let border_color = if is_active || is_hovered {
                palette.accent
            } else if is_menu {
                palette.divider
            } else {
                palette.card_border
            };

            fill_rounded_rect_masked(pixmap, bounds, radius, fill_color, mask);
            stroke_rounded_rect_masked(pixmap, bounds, radius, 1.0, border_color, mask);

            if is_active {
                let bar_rect = Rect::new(
                    bounds.x + 2.0,
                    bounds.y + 5.0,
                    3.0,
                    (bounds.height - 10.0).max(4.0),
                );
                fill_rounded_rect_masked(pixmap, bar_rect, 1.5, palette.accent, mask);
            }

            let mut tx = bounds.x + 10.0;
            if let Some(ic_name) = icon_name {
                let ic_side = 16.0_f32.min(bounds.height - 6.0).max(10.0);
                let ic_rect = Rect::new(
                    bounds.x + 8.0,
                    bounds.y + (bounds.height - ic_side) * 0.5,
                    ic_side,
                    ic_side,
                );
                paint_icon(pixmap, ic_rect, ic_name, 16, palette, mask);
                if label.is_empty() || label == ic_name {
                    return;
                }
                tx = bounds.x + 8.0 + ic_side + 8.0;
            }

            let ty = bounds.y + ((bounds.height - text_h) * 0.5).max(2.0);
            draw_text(
                pixmap,
                label,
                tx,
                ty,
                font_size,
                palette.text_primary,
                Some(effective_clip),
            );
        }
        crate::ButtonStyle::Primary | crate::ButtonStyle::Standard => {
            let is_primary =
                style == crate::ButtonStyle::Primary || is_primary_accent_button(label);
            let fill_color = if is_primary {
                palette.accent
            } else if is_active || is_hovered {
                palette.accent_muted
            } else {
                palette.button_bg
            };
            let border_color = if is_primary || is_active || is_hovered {
                palette.accent
            } else {
                palette.button_border
            };
            let text_color = if is_primary {
                palette.text_on_accent
            } else {
                palette.text_primary
            };

            fill_rounded_rect_masked(pixmap, bounds, 6.0, fill_color, mask);
            stroke_rounded_rect_masked(pixmap, bounds, 6.0, 1.2, border_color, mask);

            let mut text_x_offset = 0.0_f32;
            if let Some(ic_name) = icon_name {
                let ic_side = 16.0_f32.min(bounds.height - 6.0).max(10.0);
                let ic_palette = if is_primary {
                    icon_palette_for_accent_fill(palette)
                } else {
                    *palette
                };
                if label.is_empty() || label == ic_name {
                    let ic_rect = Rect::new(
                        bounds.x + (bounds.width - ic_side) * 0.5,
                        bounds.y + (bounds.height - ic_side) * 0.5,
                        ic_side,
                        ic_side,
                    );
                    paint_icon(pixmap, ic_rect, ic_name, 16, &ic_palette, mask);
                    return;
                }
                let ic_rect = Rect::new(
                    bounds.x + 6.0,
                    bounds.y + (bounds.height - ic_side) * 0.5,
                    ic_side,
                    ic_side,
                );
                paint_icon(pixmap, ic_rect, ic_name, 16, &ic_palette, mask);
                text_x_offset = ic_side + 6.0;
            }

            let text_w = measure_text_width(label, font_size);
            let tx =
                bounds.x + text_x_offset + ((bounds.width - text_x_offset - text_w) * 0.5).max(4.0);
            let ty = bounds.y + ((bounds.height - text_h) * 0.5).max(2.0);
            draw_text(
                pixmap,
                label,
                tx,
                ty,
                font_size,
                text_color,
                Some(effective_clip),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_text_input(
    pixmap: &mut Pixmap,
    bounds: Rect,
    placeholder: &str,
    value: &str,
    palette: &ThemePalette,
    is_focused: bool,
    caret_position: Option<usize>,
    clip_rect: Option<Rect>,
    mask: Option<&Mask>,
) {
    fill_rounded_rect_masked(pixmap, bounds, 6.0, palette.input_bg, mask);
    if is_focused {
        stroke_rounded_rect_masked(pixmap, bounds, 6.0, 2.0, palette.accent, mask);
    } else {
        stroke_rounded_rect_masked(pixmap, bounds, 6.0, 1.0, palette.input_border, mask);
    }

    let font_size = 13u16;
    let pad_x = 8.0_f32;
    let text_h = glyph_pixel_height(font_size);
    let ty = bounds.y + ((bounds.height - text_h) * 0.5).max(2.0);
    let inner_clip = inset_rect(bounds, 3.0);
    let effective_clip = clip_rect.map_or(inner_clip, |c| inner_clip.intersect_clamped(c));

    let (display_str, color) = if !value.is_empty() {
        (value, palette.text_primary)
    } else {
        (placeholder, palette.text_muted)
    };

    if !display_str.is_empty() {
        draw_text(
            pixmap,
            display_str,
            bounds.x + pad_x,
            ty,
            font_size,
            color,
            Some(effective_clip),
        );
    }

    // Draw caret bar when focused
    if is_focused {
        let char_len = value.chars().count();
        let caret_chars = caret_position.unwrap_or(char_len).min(char_len);
        let prefix: String = value.chars().take(caret_chars).collect();
        let prefix_w = measure_text_width(&prefix, font_size);
        let caret_x = (bounds.x + pad_x + prefix_w).min(bounds.x + bounds.width - 5.0);
        let caret_h = (bounds.height - 10.0).max(10.0);
        let caret_y = bounds.y + (bounds.height - caret_h) * 0.5;
        let caret_rect = Rect::new(caret_x, caret_y, 2.0, caret_h);
        fill_rounded_rect_masked(pixmap, caret_rect, 0.5, palette.accent, mask);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_toggler(
    pixmap: &mut Pixmap,
    bounds: Rect,
    label: &str,
    is_checked: bool,
    style: crate::TogglerStyle,
    palette: &ThemePalette,
    clip_rect: Option<Rect>,
    mask: Option<&Mask>,
) {
    let label_x = match style {
        crate::TogglerStyle::Checkbox => {
            let box_size = 16.0_f32.min(bounds.height - 4.0).max(12.0);
            let box_x = bounds.x + 2.0;
            let box_y = bounds.y + (bounds.height - box_size) * 0.5;
            let box_rect = Rect::new(box_x, box_y, box_size, box_size);
            if is_checked {
                fill_rounded_rect_masked(pixmap, box_rect, 3.5, palette.accent, mask);
                stroke_rounded_rect_masked(pixmap, box_rect, 3.5, 1.2, palette.accent, mask);

                // Anti-aliased vector checkmark inside the checked box
                let mut pb = PathBuilder::new();
                pb.move_to(box_x + box_size * 0.22, box_y + box_size * 0.52);
                pb.line_to(box_x + box_size * 0.44, box_y + box_size * 0.74);
                pb.line_to(box_x + box_size * 0.78, box_y + box_size * 0.28);
                if let Some(path) = pb.finish() {
                    stroke_vector_path(pixmap, &path, 2.0, palette.text_on_accent, mask);
                }
            } else {
                fill_rounded_rect_masked(pixmap, box_rect, 3.5, palette.input_bg, mask);
                stroke_rounded_rect_masked(pixmap, box_rect, 3.5, 1.2, palette.input_border, mask);
            }
            box_x + box_size + 8.0
        }
        crate::TogglerStyle::Switch => {
            let switch_w = 34.0_f32.min(bounds.width * 0.45).max(20.0);
            let switch_h = 18.0_f32.min(bounds.height - 4.0).max(12.0);
            let switch_x = bounds.x + 2.0;
            let switch_y = bounds.y + (bounds.height - switch_h) * 0.5;
            let switch_rect = Rect::new(switch_x, switch_y, switch_w, switch_h);
            let track_color = if is_checked {
                palette.accent
            } else {
                palette.toggler_off_bg
            };

            fill_rounded_rect_masked(pixmap, switch_rect, switch_h * 0.5, track_color, mask);
            stroke_rounded_rect_masked(
                pixmap,
                switch_rect,
                switch_h * 0.5,
                1.0,
                palette.card_border,
                mask,
            );

            let thumb_d = (switch_h - 4.0).max(6.0);
            let thumb_x = if is_checked {
                switch_x + switch_w - thumb_d - 2.0
            } else {
                switch_x + 2.0
            };
            let thumb_y = switch_y + (switch_h - thumb_d) * 0.5;
            let thumb_rect = Rect::new(thumb_x, thumb_y, thumb_d, thumb_d);
            fill_rounded_rect_masked(
                pixmap,
                thumb_rect,
                thumb_d * 0.5,
                palette.toggler_thumb,
                mask,
            );
            switch_x + switch_w + 8.0
        }
    };

    if !label.is_empty() {
        let font_size = 13u16;
        let text_h = glyph_pixel_height(font_size);
        let ty = bounds.y + ((bounds.height - text_h) * 0.5).max(2.0);
        let effective_clip = clip_rect.map_or(bounds, |c| bounds.intersect_clamped(c));
        draw_text(
            pixmap,
            label,
            label_x,
            ty,
            font_size,
            palette.text_primary,
            Some(effective_clip),
        );
    }
}

fn paint_dropdown(
    pixmap: &mut Pixmap,
    bounds: Rect,
    selected_label: &str,
    palette: &ThemePalette,
    clip_rect: Option<Rect>,
    mask: Option<&Mask>,
) {
    fill_rounded_rect_masked(pixmap, bounds, 6.0, palette.input_bg, mask);
    stroke_rounded_rect_masked(pixmap, bounds, 6.0, 1.0, palette.input_border, mask);

    let font_size = 13u16;
    let text_h = glyph_pixel_height(font_size);
    let tx = bounds.x + 8.0;
    let ty = bounds.y + ((bounds.height - text_h) * 0.5).max(2.0);
    let text_clip = Rect::new(
        bounds.x + 2.0,
        bounds.y + 2.0,
        (bounds.width - 24.0).max(8.0),
        (bounds.height - 4.0).max(4.0),
    );
    let effective_clip = clip_rect.map_or(text_clip, |c| text_clip.intersect_clamped(c));
    draw_text(
        pixmap,
        selected_label,
        tx,
        ty,
        font_size,
        palette.text_primary,
        Some(effective_clip),
    );

    // Draw vector chevron arrow on the right side of the dropdown
    let cx = bounds.x + bounds.width - 13.0;
    let cy = bounds.y + bounds.height * 0.5;
    let mut pb = PathBuilder::new();
    pb.move_to(cx - 4.0, cy - 2.0);
    pb.line_to(cx, cy + 2.5);
    pb.line_to(cx + 4.0, cy - 2.0);
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color(to_skia_color(palette.accent));
        paint.anti_alias = true;
        let stroke = Stroke {
            width: 1.8,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), mask);
    }
}

/// Rasterizes the theme-matched rFerram stained-glass `"rF"` emblem into an RGBA8 [`Pixmap`]
/// of dimensions `size x size` using the active [`Theme`] palette (never the full-color desktop icon).
pub fn rasterize_themed_app_icon(size: u32, theme: &Theme) -> Pixmap {
    let safe_size = size.max(16);
    let mut pixmap = Pixmap::new(safe_size, safe_size).expect("icon size is >= 16x16");
    let palette = ThemePalette::from_theme(theme);
    let bounds = Rect::new(0.0, 0.0, safe_size as f32, safe_size as f32);
    paint_rferram_themed_logo(&mut pixmap, bounds, palette, None);
    pixmap
}

fn paint_rferram_themed_logo(
    pixmap: &mut Pixmap,
    bounds: Rect,
    palette: ThemePalette,
    mask: Option<&Mask>,
) {
    let icon_rect = inset_rect(bounds, 1.0);
    let side = icon_rect.width.min(icon_rect.height).max(8.0);
    let radius = (side * 0.22).clamp(3.0, 16.0);

    // 1. Base surface matching the active theme's dark/light input surface
    fill_rounded_rect_masked(pixmap, icon_rect, radius, palette.input_bg, mask);

    // 2. Stained-glass facet panes tinted with the theme's muted accent & card surface
    let half_w = icon_rect.width * 0.5;
    let half_h = icon_rect.height * 0.5;
    let top_left_pane = Rect::new(
        icon_rect.x + 2.0,
        icon_rect.y + 2.0,
        (half_w - 2.0).max(2.0),
        (half_h - 2.0).max(2.0),
    );
    let bottom_right_pane = Rect::new(
        icon_rect.x + half_w,
        icon_rect.y + half_h,
        (half_w - 2.0).max(2.0),
        (half_h - 2.0).max(2.0),
    );
    fill_rounded_rect_masked(
        pixmap,
        top_left_pane,
        (radius * 0.6).max(2.0),
        palette.accent_muted,
        mask,
    );
    fill_rounded_rect_masked(
        pixmap,
        bottom_right_pane,
        (radius * 0.6).max(2.0),
        palette.card_bg,
        mask,
    );

    // 3. Lead-came / iron framework lattice lines
    let mut pb = PathBuilder::new();
    let cx = icon_rect.x + half_w;
    let cy = icon_rect.y + half_h;
    pb.move_to(icon_rect.x + 2.0, cy);
    pb.line_to(icon_rect.x + icon_rect.width - 2.0, cy);
    pb.move_to(cx, icon_rect.y + 2.0);
    pb.line_to(cx, icon_rect.y + icon_rect.height - 2.0);
    pb.move_to(icon_rect.x + 4.0, icon_rect.y + 4.0);
    pb.line_to(
        icon_rect.x + icon_rect.width - 4.0,
        icon_rect.y + icon_rect.height - 4.0,
    );
    if let Some(lattice_path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color(to_skia_color(palette.divider));
        paint.anti_alias = true;
        let stroke = Stroke {
            width: (side * 0.045).clamp(1.0, 2.5),
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        };
        pixmap.stroke_path(&lattice_path, &paint, &stroke, Transform::identity(), mask);
    }

    // 4. Outer stained-glass iron frame in theme accent & border
    stroke_rounded_rect_masked(
        pixmap,
        icon_rect,
        radius,
        (side * 0.065).clamp(1.2, 3.0),
        palette.accent,
        mask,
    );

    // 5. "rF" monogram rendered in theme accent / primary text color
    let font_size = ((side * 0.42).round() as u16).max(10);
    let label = "rF";
    let text_w = measure_text_width(label, font_size);
    let text_h = glyph_pixel_height(font_size);
    let tx = icon_rect.x + ((icon_rect.width - text_w) * 0.5).max(1.0);
    let ty = icon_rect.y + ((icon_rect.height - text_h) * 0.5).max(1.0);
    draw_text(
        pixmap,
        label,
        tx,
        ty,
        font_size,
        palette.accent,
        Some(icon_rect),
    );
}

/// Canonical list of all Freedesktop / COSMIC symbolic icons specified in `UI_Fix.md`
/// and `rFerramLayout-PNG.drawio.png`.
pub const SYMBOLIC_ICON_NAMES: &[&str] = &[
    "com.theinfomoose.rFerram-symbolic",
    "add-symbolic",
    "view-grid-symbolic",
    "view-list-symbolic",
    "view-sort-descending-symbolic",
    "system-search-symbolic",
    "preferences-system-symbolic",
    "help-browser-symbolic",
    "emblem-favorite-symbolic",
    "folder-symbolic",
    "folder-remote-symbolic",
    "object-select-symbolic",
    "input-keyboard-symbolic",
    "video-display-symbolic",
    "user-bookmarks-symbolic",
    "network-vpn-symbolic",
    "document-open-symbolic",
    "tools-symbolic",
    "dialog-warning-symbolic",
    "lab-symbolic",
    "view-more-symbolic",
    "avatar-default-symbolic",
];

fn stroke_vector_path(
    pixmap: &mut Pixmap,
    path: &tiny_skia::Path,
    stroke_width: f32,
    rgba: [u8; 4],
    mask: Option<&Mask>,
) {
    let mut paint = Paint::default();
    paint.set_color(to_skia_color(rgba));
    paint.anti_alias = true;
    let stroke = Stroke {
        width: stroke_width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    };
    pixmap.stroke_path(path, &paint, &stroke, Transform::identity(), mask);
}

fn fill_vector_path(
    pixmap: &mut Pixmap,
    path: &tiny_skia::Path,
    rgba: [u8; 4],
    mask: Option<&Mask>,
) {
    let mut paint = Paint::default();
    paint.set_color(to_skia_color(rgba));
    paint.anti_alias = true;
    pixmap.fill_path(path, &paint, FillRule::Winding, Transform::identity(), mask);
}

fn push_circle_subpath(pb: &mut PathBuilder, cx: f32, cy: f32, r: f32) {
    if r <= 0.1 {
        return;
    }
    let k = r * 0.552_284_8;
    pb.move_to(cx, cy - r);
    pb.cubic_to(cx + k, cy - r, cx + r, cy - k, cx + r, cy);
    pb.cubic_to(cx + r, cy + k, cx + k, cy + r, cx, cy + r);
    pb.cubic_to(cx - k, cy + r, cx - r, cy + k, cx - r, cy);
    pb.cubic_to(cx - r, cy - k, cx - k, cy - r, cx, cy - r);
    pb.close();
}

fn paint_icon(
    pixmap: &mut Pixmap,
    bounds: Rect,
    name: &str,
    size: u16,
    palette: &ThemePalette,
    mask: Option<&Mask>,
) {
    if name.contains("rFerram") || name.contains("rferram") {
        paint_rferram_themed_logo(pixmap, bounds, *palette, mask);
        return;
    }

    let max_side = bounds.width.min(bounds.height).max(1.0);
    let target_side = if size > 0 {
        f32::from(size).min(max_side).max(1.0)
    } else {
        max_side
    };
    let square_bounds = Rect::new(
        bounds.x + (bounds.width - target_side) * 0.5,
        bounds.y + (bounds.height - target_side) * 0.5,
        target_side,
        target_side,
    );
    let pad = (target_side * 0.1).min(2.0);
    let icon_rect = inset_rect(square_bounds, pad);
    let x = icon_rect.x;
    let y = icon_rect.y;
    let w = icon_rect.width.max(1.0);
    let h = icon_rect.height.max(1.0);
    let cx = x + w * 0.5;
    let cy = y + h * 0.5;
    let s = w.min(h);
    let sw = (s * 0.115).clamp((s * 0.18).min(1.35), 2.5);

    match name {
        "add-symbolic" | "list-add-symbolic" => {
            let arm = s * 0.36;
            let mut pb = PathBuilder::new();
            pb.move_to(cx - arm, cy);
            pb.line_to(cx + arm, cy);
            pb.move_to(cx, cy - arm);
            pb.line_to(cx, cy + arm);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw * 1.15, palette.accent, mask);
            }
        }
        "view-grid-symbolic" => {
            let cell_w = w * 0.36;
            let cell_h = h * 0.36;
            let gap_x = (w - cell_w * 2.0) * 0.38;
            let gap_y = (h - cell_h * 2.0) * 0.38;
            let x0 = cx - cell_w - gap_x * 0.5;
            let x1 = cx + gap_x * 0.5;
            let y0 = cy - cell_h - gap_y * 0.5;
            let y1 = cy + gap_y * 0.5;
            for (rx, ry) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
                let cell = Rect::new(rx, ry, cell_w, cell_h);
                fill_rounded_rect_masked(pixmap, cell, 1.8, palette.accent_muted, mask);
                stroke_rounded_rect_masked(pixmap, cell, 1.8, sw * 0.85, palette.accent, mask);
            }
        }
        "view-list-symbolic" => {
            let mut pb = PathBuilder::new();
            for frac in [0.24_f32, 0.50, 0.76] {
                let ry = y + h * frac;
                push_circle_subpath(&mut pb, x + w * 0.18, ry, (s * 0.075).max(sw * 0.45));
                pb.move_to(x + w * 0.36, ry);
                pb.line_to(x + w * 0.88, ry);
            }
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw, palette.accent, mask);
            }
        }
        "view-sort-descending-symbolic" => {
            let mut pb = PathBuilder::new();
            // Left vertical arrow shaft + bottom arrowhead
            let ax = x + w * 0.25;
            pb.move_to(ax, y + h * 0.16);
            pb.line_to(ax, y + h * 0.82);
            pb.move_to(ax - w * 0.13, y + h * 0.66);
            pb.line_to(ax, y + h * 0.84);
            pb.line_to(ax + w * 0.13, y + h * 0.66);
            // Right descending horizontal filter bars
            pb.move_to(x + w * 0.48, y + h * 0.24);
            pb.line_to(x + w * 0.90, y + h * 0.24);
            pb.move_to(x + w * 0.48, y + h * 0.50);
            pb.line_to(x + w * 0.78, y + h * 0.50);
            pb.move_to(x + w * 0.48, y + h * 0.76);
            pb.line_to(x + w * 0.66, y + h * 0.76);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw, palette.accent, mask);
            }
        }
        "system-search-symbolic" | "edit-find-symbolic" => {
            let lcx = x + w * 0.42;
            let lcy = y + h * 0.42;
            let lr = s * 0.26;
            let mut pb = PathBuilder::new();
            push_circle_subpath(&mut pb, lcx, lcy, lr);
            pb.move_to(lcx + lr * 0.72, lcy + lr * 0.72);
            pb.line_to(x + w * 0.86, y + h * 0.86);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw, palette.accent, mask);
            }
        }
        "preferences-system-symbolic" | "emblem-system-symbolic" => {
            let r_outer = s * 0.27;
            let r_inner = s * 0.11;
            let mut pb = PathBuilder::new();
            push_circle_subpath(&mut pb, cx, cy, r_outer);
            push_circle_subpath(&mut pb, cx, cy, r_inner);
            for i in 0..8 {
                let angle = (i as f32) * std::f32::consts::FRAC_PI_4;
                let (sin_a, cos_a) = angle.sin_cos();
                pb.move_to(cx + cos_a * (r_outer - 0.5), cy + sin_a * (r_outer - 0.5));
                pb.line_to(cx + cos_a * (s * 0.42), cy + sin_a * (s * 0.42));
            }
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw * 0.95, palette.accent, mask);
            }
        }
        "help-browser-symbolic" | "dialog-question-symbolic" => {
            let mut ring_pb = PathBuilder::new();
            push_circle_subpath(&mut ring_pb, cx, cy, s * 0.43);
            if let Some(ring_path) = ring_pb.finish() {
                stroke_vector_path(pixmap, &ring_path, sw * 0.85, palette.accent_muted, mask);
            }

            let mut pb = PathBuilder::new();
            pb.move_to(cx - s * 0.14, cy - s * 0.14);
            pb.quad_to(cx, cy - s * 0.32, cx + s * 0.15, cy - s * 0.12);
            pb.quad_to(cx + s * 0.15, cy - s * 0.01, cx, cy + s * 0.08);
            pb.line_to(cx, cy + s * 0.13);
            push_circle_subpath(&mut pb, cx, cy + s * 0.26, sw * 0.55);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw, palette.accent, mask);
            }
        }
        "emblem-favorite-symbolic" | "starred-symbolic" | "non-starred-symbolic" => {
            let r_out = s * 0.43;
            let r_in = s * 0.18;
            let mut pb = PathBuilder::new();
            for i in 0..10 {
                let angle =
                    -std::f32::consts::FRAC_PI_2 + (i as f32) * (std::f32::consts::PI / 5.0);
                let r = if i % 2 == 0 { r_out } else { r_in };
                let px = cx + angle.cos() * r;
                let py = cy + angle.sin() * r;
                if i == 0 {
                    pb.move_to(px, py);
                } else {
                    pb.line_to(px, py);
                }
            }
            pb.close();
            if let Some(path) = pb.finish() {
                if name != "non-starred-symbolic" {
                    fill_vector_path(pixmap, &path, palette.accent_muted, mask);
                }
                stroke_vector_path(pixmap, &path, sw * 0.9, palette.accent, mask);
            }
        }
        "folder-symbolic" => {
            let mut pb = PathBuilder::new();
            pb.move_to(x + w * 0.10, y + h * 0.24);
            pb.line_to(x + w * 0.40, y + h * 0.24);
            pb.line_to(x + w * 0.48, y + h * 0.35);
            pb.line_to(x + w * 0.90, y + h * 0.35);
            pb.line_to(x + w * 0.90, y + h * 0.82);
            pb.line_to(x + w * 0.10, y + h * 0.82);
            pb.close();
            if let Some(path) = pb.finish() {
                fill_vector_path(pixmap, &path, palette.accent_muted, mask);
                stroke_vector_path(pixmap, &path, sw, palette.accent, mask);
            }
        }
        "folder-remote-symbolic" => {
            let mut pb = PathBuilder::new();
            pb.move_to(x + w * 0.12, y + h * 0.16);
            pb.line_to(x + w * 0.40, y + h * 0.16);
            pb.line_to(x + w * 0.48, y + h * 0.26);
            pb.line_to(x + w * 0.88, y + h * 0.26);
            pb.line_to(x + w * 0.88, y + h * 0.66);
            pb.line_to(x + w * 0.12, y + h * 0.66);
            pb.close();
            if let Some(folder_path) = pb.finish() {
                fill_vector_path(pixmap, &folder_path, palette.accent_muted, mask);
                stroke_vector_path(pixmap, &folder_path, sw * 0.95, palette.accent, mask);
            }
            // Remote network stem and horizontal bus with center node
            let mut net_pb = PathBuilder::new();
            net_pb.move_to(cx, y + h * 0.66);
            net_pb.line_to(cx, y + h * 0.86);
            net_pb.move_to(x + w * 0.22, y + h * 0.86);
            net_pb.line_to(x + w * 0.78, y + h * 0.86);
            push_circle_subpath(&mut net_pb, cx, y + h * 0.86, sw * 0.75);
            if let Some(net_path) = net_pb.finish() {
                stroke_vector_path(pixmap, &net_path, sw, palette.accent, mask);
            }
        }
        "object-select-symbolic" | "emblem-ok-symbolic" => {
            let mut pb = PathBuilder::new();
            push_circle_subpath(&mut pb, cx, cy, s * 0.42);
            pb.move_to(x + w * 0.27, cy + h * 0.02);
            pb.line_to(x + w * 0.44, y + h * 0.69);
            pb.line_to(x + w * 0.75, y + h * 0.33);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw, palette.accent, mask);
            }
        }
        "input-keyboard-symbolic" => {
            let chassis = Rect::new(x + w * 0.08, y + h * 0.22, w * 0.84, h * 0.56);
            fill_rounded_rect_masked(pixmap, chassis, 2.5, palette.accent_muted, mask);
            stroke_rounded_rect_masked(pixmap, chassis, 2.5, sw * 0.95, palette.accent, mask);

            let mut pb = PathBuilder::new();
            let key_half = (w * 0.04).min(0.8);
            for col in [0.24_f32, 0.42, 0.58, 0.76] {
                let kx = x + w * col;
                pb.move_to(kx - key_half, y + h * 0.38);
                pb.line_to(kx + key_half, y + h * 0.38);
                pb.move_to(kx - key_half, y + h * 0.51);
                pb.line_to(kx + key_half, y + h * 0.51);
            }
            // Spacebar row
            pb.move_to(x + w * 0.30, y + h * 0.65);
            pb.line_to(x + w * 0.70, y + h * 0.65);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw * 0.9, palette.accent, mask);
            }
        }
        "video-display-symbolic" | "computer-symbolic" => {
            let screen = Rect::new(x + w * 0.10, y + h * 0.14, w * 0.80, h * 0.52);
            fill_rounded_rect_masked(pixmap, screen, 2.5, palette.accent_muted, mask);
            stroke_rounded_rect_masked(pixmap, screen, 2.5, sw, palette.accent, mask);

            let mut pb = PathBuilder::new();
            pb.move_to(cx, y + h * 0.66);
            pb.line_to(cx, y + h * 0.84);
            pb.move_to(cx - w * 0.24, y + h * 0.84);
            pb.line_to(cx + w * 0.24, y + h * 0.84);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw, palette.accent, mask);
            }
        }
        "user-bookmarks-symbolic" => {
            // Hierarchical group tree / bookmark nodes matching rFerramLayout-PNG.drawio.png
            let node_w = w * 0.24;
            let node_h = h * 0.20;
            let top_node = Rect::new(cx - node_w * 0.5, y + h * 0.12, node_w, node_h);
            let left_node = Rect::new(x + w * 0.14, y + h * 0.68, node_w, node_h);
            let right_node = Rect::new(x + w * 0.62, y + h * 0.68, node_w, node_h);
            for nr in [top_node, left_node, right_node] {
                fill_rounded_rect_masked(pixmap, nr, 1.5, palette.accent_muted, mask);
                stroke_rounded_rect_masked(pixmap, nr, 1.5, sw * 0.85, palette.accent, mask);
            }
            let mut pb = PathBuilder::new();
            pb.move_to(cx, y + h * 0.32);
            pb.line_to(cx, y + h * 0.50);
            pb.move_to(x + w * 0.26, y + h * 0.50);
            pb.line_to(x + w * 0.74, y + h * 0.50);
            pb.move_to(x + w * 0.26, y + h * 0.50);
            pb.line_to(x + w * 0.26, y + h * 0.68);
            pb.move_to(x + w * 0.74, y + h * 0.50);
            pb.line_to(x + w * 0.74, y + h * 0.68);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw * 0.85, palette.accent, mask);
            }
        }
        "network-vpn-symbolic" => {
            // VPN tunnel portal bracket + directional arrow/shield matching rFerramLayout-PNG.drawio.png
            let mut pb = PathBuilder::new();
            // Right portal bracket
            pb.move_to(x + w * 0.46, y + h * 0.16);
            pb.line_to(x + w * 0.84, y + h * 0.16);
            pb.line_to(x + w * 0.84, y + h * 0.84);
            pb.line_to(x + w * 0.46, y + h * 0.84);
            // Tunnel entry arrow from left into portal
            pb.move_to(x + w * 0.12, cy);
            pb.line_to(x + w * 0.64, cy);
            pb.move_to(x + w * 0.46, cy - h * 0.18);
            pb.line_to(x + w * 0.66, cy);
            pb.line_to(x + w * 0.46, cy + h * 0.18);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw, palette.accent, mask);
            }
        }
        "document-open-symbolic" | "x-office-document-symbolic" => {
            let mut pb = PathBuilder::new();
            // Page outline with top-right folded dog-ear corner
            pb.move_to(x + w * 0.20, y + h * 0.12);
            pb.line_to(x + w * 0.62, y + h * 0.12);
            pb.line_to(x + w * 0.82, y + h * 0.32);
            pb.line_to(x + w * 0.82, y + h * 0.88);
            pb.line_to(x + w * 0.20, y + h * 0.88);
            pb.close();
            // Dog-ear crease
            pb.move_to(x + w * 0.62, y + h * 0.12);
            pb.line_to(x + w * 0.62, y + h * 0.32);
            pb.line_to(x + w * 0.82, y + h * 0.32);
            // Document text lines
            pb.move_to(x + w * 0.32, y + h * 0.48);
            pb.line_to(x + w * 0.70, y + h * 0.48);
            pb.move_to(x + w * 0.32, y + h * 0.64);
            pb.line_to(x + w * 0.70, y + h * 0.64);
            pb.move_to(x + w * 0.32, y + h * 0.76);
            pb.line_to(x + w * 0.56, y + h * 0.76);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw * 0.9, palette.accent, mask);
            }
        }
        "tools-symbolic" | "preferences-other-symbolic" => {
            let mut pb = PathBuilder::new();
            // Diagonal wrench shaft (bottom-left to top-right) + open jaw
            pb.move_to(x + w * 0.18, y + h * 0.82);
            pb.line_to(x + w * 0.68, y + h * 0.32);
            pb.move_to(x + w * 0.60, y + h * 0.20);
            pb.quad_to(x + w * 0.84, y + h * 0.16, x + w * 0.80, y + h * 0.40);
            // Crossed screwdriver shaft (top-left to bottom-right) + handle
            pb.move_to(x + w * 0.20, y + h * 0.20);
            pb.line_to(x + w * 0.82, y + h * 0.82);
            pb.move_to(x + w * 0.14, y + h * 0.26);
            pb.line_to(x + w * 0.26, y + h * 0.14);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw, palette.accent, mask);
            }
        }
        "dialog-warning-symbolic" | "dialog-error-symbolic" => {
            let mut pb = PathBuilder::new();
            // Warning triangle outline
            pb.move_to(cx, y + h * 0.12);
            pb.line_to(x + w * 0.88, y + h * 0.84);
            pb.line_to(x + w * 0.12, y + h * 0.84);
            pb.close();
            // Exclamation mark stem + bottom dot
            pb.move_to(cx, y + h * 0.38);
            pb.line_to(cx, y + h * 0.62);
            push_circle_subpath(&mut pb, cx, y + h * 0.74, sw * 0.55);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw * 0.95, palette.accent, mask);
            }
        }
        "lab-symbolic" | "applications-science-symbolic" => {
            // Erlenmeyer flask matching Feature Request icon in rFerramLayout-PNG.drawio.png
            let mut pb = PathBuilder::new();
            // Top rim
            pb.move_to(cx - w * 0.16, y + h * 0.14);
            pb.line_to(cx + w * 0.16, y + h * 0.14);
            // Neck & conical body
            pb.move_to(cx - w * 0.10, y + h * 0.14);
            pb.line_to(cx - w * 0.10, y + h * 0.38);
            pb.line_to(x + w * 0.16, y + h * 0.86);
            pb.line_to(x + w * 0.84, y + h * 0.86);
            pb.line_to(cx + w * 0.10, y + h * 0.38);
            pb.line_to(cx + w * 0.10, y + h * 0.14);
            // Liquid level line inside flask
            pb.move_to(x + w * 0.28, y + h * 0.66);
            pb.line_to(x + w * 0.72, y + h * 0.66);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw * 0.95, palette.accent, mask);
            }
        }
        "view-more-symbolic" | "view-more-horizontal-symbolic" => {
            let dot_r = (s * 0.085).max(sw * 0.5);
            let mut pb = PathBuilder::new();
            for dx in [-w * 0.28, 0.0, w * 0.28] {
                push_circle_subpath(&mut pb, cx + dx, cy, dot_r);
            }
            if let Some(path) = pb.finish() {
                fill_vector_path(pixmap, &path, palette.accent, mask);
                stroke_vector_path(pixmap, &path, sw * 0.6, palette.accent, mask);
            }
        }
        "avatar-default-symbolic" | "system-users-symbolic" | "user-symbolic" => {
            let mut pb = PathBuilder::new();
            push_circle_subpath(&mut pb, cx, y + h * 0.32, s * 0.18);
            pb.move_to(x + w * 0.20, y + h * 0.84);
            pb.quad_to(cx, y + h * 0.54, x + w * 0.80, y + h * 0.84);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw, palette.accent, mask);
            }
        }
        "window-close-symbolic" => {
            let arm = s * 0.30;
            let mut pb = PathBuilder::new();
            pb.move_to(cx - arm, cy - arm);
            pb.line_to(cx + arm, cy + arm);
            pb.move_to(cx + arm, cy - arm);
            pb.line_to(cx - arm, cy + arm);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw, palette.accent, mask);
            }
        }
        _ => {
            // Deterministic vector diamond + hash-varied radial spokes for any custom symbolic icon name
            let mut hash = 2166136261u32;
            for b in name.bytes() {
                hash = (hash ^ u32::from(b)).wrapping_mul(16777619);
            }
            let r = s * 0.38;
            let mut pb = PathBuilder::new();
            pb.move_to(cx, cy - r);
            pb.line_to(cx + r, cy);
            pb.line_to(cx, cy + r);
            pb.line_to(cx - r, cy);
            pb.close();
            let angle = ((hash % 360) as f32).to_radians();
            pb.move_to(cx - angle.cos() * r * 0.6, cy - angle.sin() * r * 0.6);
            pb.line_to(cx + angle.cos() * r * 0.6, cy + angle.sin() * r * 0.6);
            if let Some(path) = pb.finish() {
                stroke_vector_path(pixmap, &path, sw, palette.accent, mask);
            }
        }
    }
}

fn paint_divider(pixmap: &mut Pixmap, bounds: Rect, palette: &ThemePalette, mask: Option<&Mask>) {
    let y = bounds.y + bounds.height * 0.5;
    let line_rect = Rect::new(bounds.x, y - 0.5, bounds.width.max(1.0), 1.0);
    fill_rounded_rect_masked(pixmap, line_rect, 0.0, palette.divider, mask);
}

fn inset_rect(r: Rect, pad: f32) -> Rect {
    Rect::new(
        r.x + pad,
        r.y + pad,
        (r.width - pad * 2.0).max(1.0),
        (r.height - pad * 2.0).max(1.0),
    )
}

fn to_skia_color(rgba: [u8; 4]) -> Color {
    Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3])
}

/// Builds a rounded rectangle [`tiny_skia::Path`] for `(x, y, width, height)` and corner `radius`.
pub fn build_rounded_rect_path(rect: Rect, radius: f32) -> Option<tiny_skia::Path> {
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return None;
    }
    let r = radius
        .clamp(0.0, rect.width * 0.5)
        .clamp(0.0, rect.height * 0.5);

    if r <= 0.25 {
        let skia_rect = tiny_skia::Rect::from_xywh(rect.x, rect.y, rect.width, rect.height)?;
        return Some(PathBuilder::from_rect(skia_rect));
    }

    let x0 = rect.x;
    let y0 = rect.y;
    let x1 = rect.x + rect.width;
    let y1 = rect.y + rect.height;

    let mut pb = PathBuilder::new();
    pb.move_to(x0 + r, y0);
    pb.line_to(x1 - r, y0);
    pb.quad_to(x1, y0, x1, y0 + r);
    pb.line_to(x1, y1 - r);
    pb.quad_to(x1, y1, x1 - r, y1);
    pb.line_to(x0 + r, y1);
    pb.quad_to(x0, y1, x0, y1 - r);
    pb.line_to(x0, y0 + r);
    pb.quad_to(x0, y0, x0 + r, y0);
    pb.close();
    pb.finish()
}

fn fill_rounded_rect(pixmap: &mut Pixmap, rect: Rect, radius: f32, rgba: [u8; 4]) {
    fill_rounded_rect_masked(pixmap, rect, radius, rgba, None);
}

fn fill_rounded_rect_masked(
    pixmap: &mut Pixmap,
    rect: Rect,
    radius: f32,
    rgba: [u8; 4],
    mask: Option<&Mask>,
) {
    let Some(path) = build_rounded_rect_path(rect, radius) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(to_skia_color(rgba));
    paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        mask,
    );
}

fn stroke_rounded_rect_masked(
    pixmap: &mut Pixmap,
    rect: Rect,
    radius: f32,
    stroke_width: f32,
    rgba: [u8; 4],
    mask: Option<&Mask>,
) {
    let half = stroke_width * 0.5;
    let inset = Rect::new(
        rect.x + half,
        rect.y + half,
        (rect.width - stroke_width).max(1.0),
        (rect.height - stroke_width).max(1.0),
    );
    let Some(path) = build_rounded_rect_path(inset, (radius - half).max(0.0)) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(to_skia_color(rgba));
    paint.anti_alias = true;
    let stroke = Stroke {
        width: stroke_width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), mask);
}

/// Applies a fast 2-pass separable box blur to the rectangular region `bounds` in `pixmap`
/// to simulate the COSMIC frosted glass shader backdrop blur.
fn blur_pixmap_region(pixmap: &mut Pixmap, bounds: Rect, radius: u32) {
    if radius == 0 || bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }
    let pw = pixmap.width() as i32;
    let ph = pixmap.height() as i32;
    let x0 = (bounds.x.floor() as i32).clamp(0, pw);
    let y0 = (bounds.y.floor() as i32).clamp(0, ph);
    let x1 = ((bounds.x + bounds.width).ceil() as i32).clamp(0, pw);
    let y1 = ((bounds.y + bounds.height).ceil() as i32).clamp(0, ph);

    if x1 - x0 <= 2 || y1 - y0 <= 2 {
        return;
    }
    let rw = (x1 - x0) as usize;
    let rh = (y1 - y0) as usize;

    let rad = (radius as i32).clamp(1, 8);
    let stride = pw as usize * 4;
    let data = pixmap.data_mut();

    let mut temp = vec![[0u8; 4]; rw * rh];

    // Horizontal blur pass into `temp`
    for ry in 0..rh {
        let sy = (y0 as usize + ry) * stride;
        for rx in 0..rw {
            let mut sr = 0u32;
            let mut sg = 0u32;
            let mut sb = 0u32;
            let mut sa = 0u32;
            let mut count = 0u32;
            for dx in -rad..=rad {
                let nx = (rx as i32 + dx).clamp(0, rw as i32 - 1) as usize;
                let off = sy + (x0 as usize + nx) * 4;
                sr += u32::from(data[off]);
                sg += u32::from(data[off + 1]);
                sb += u32::from(data[off + 2]);
                sa += u32::from(data[off + 3]);
                count += 1;
            }
            temp[ry * rw + rx] = [
                (sr / count) as u8,
                (sg / count) as u8,
                (sb / count) as u8,
                (sa / count) as u8,
            ];
        }
    }

    // Vertical blur pass back into `data`
    for ry in 0..rh {
        let dy = (y0 as usize + ry) * stride;
        for rx in 0..rw {
            let mut sr = 0u32;
            let mut sg = 0u32;
            let mut sb = 0u32;
            let mut sa = 0u32;
            let mut count = 0u32;
            for ky in -rad..=rad {
                let ny = (ry as i32 + ky).clamp(0, rh as i32 - 1) as usize;
                let px = temp[ny * rw + rx];
                sr += u32::from(px[0]);
                sg += u32::from(px[1]);
                sb += u32::from(px[2]);
                sa += u32::from(px[3]);
                count += 1;
            }
            let off = dy + (x0 as usize + rx) * 4;
            data[off] = (sr / count) as u8;
            data[off + 1] = (sg / count) as u8;
            data[off + 2] = (sb / count) as u8;
            data[off + 3] = (sa / count) as u8;
        }
    }
}

fn glyph_scale_for_size(font_size: u16) -> u32 {
    if font_size >= 18 { 2 } else { 1 }
}

fn glyph_pixel_height(font_size: u16) -> f32 {
    (GLYPH_HEIGHT * glyph_scale_for_size(font_size)) as f32
}

/// Measures the rendered horizontal width in pixels for `text` at `font_size` using the embedded
/// 5x7 ASCII bitmap font.
pub fn measure_text_width(text: &str, font_size: u16) -> f32 {
    let count = text.chars().count() as u32;
    if count == 0 {
        return 0.0;
    }
    let scale = glyph_scale_for_size(font_size);
    ((count * GLYPH_ADVANCE - 1) * scale) as f32
}

/// Blends a straight (unpremultiplied) RGBA8 color `[sr, sg, sb, sa]` over a destination
/// [`PremultipliedColorU8`] pixel using standard source-over Porter-Duff compositing.
fn blend_straight_rgba_over(dst: PremultipliedColorU8, rgba: [u8; 4]) -> PremultipliedColorU8 {
    let sa = u32::from(rgba[3]);
    if sa == 255 {
        return PremultipliedColorU8::from_rgba(rgba[0], rgba[1], rgba[2], 255)
            .expect("opaque alpha 255 always satisfies premultiplied invariant");
    }
    if sa == 0 {
        return dst;
    }
    let inv_sa = 255 - sa;
    let psr = (u32::from(rgba[0]) * sa + 127) / 255;
    let psg = (u32::from(rgba[1]) * sa + 127) / 255;
    let psb = (u32::from(rgba[2]) * sa + 127) / 255;

    let out_a = (sa + (u32::from(dst.alpha()) * inv_sa + 127) / 255).min(255) as u8;
    let out_r = (psr + (u32::from(dst.red()) * inv_sa + 127) / 255).min(u32::from(out_a)) as u8;
    let out_g = (psg + (u32::from(dst.green()) * inv_sa + 127) / 255).min(u32::from(out_a)) as u8;
    let out_b = (psb + (u32::from(dst.blue()) * inv_sa + 127) / 255).min(u32::from(out_a)) as u8;

    PremultipliedColorU8::from_rgba(out_r, out_g, out_b, out_a)
        .expect("clamped channels <= out_a always satisfy premultiplied invariant")
}

/// Draws `text` into `pixmap` at `(x, y)` using the embedded 5x7 ASCII bitmap glyph renderer.
/// Returns the total number of foreground glyph pixels written.
pub fn draw_text(
    pixmap: &mut Pixmap,
    text: &str,
    x: f32,
    y: f32,
    font_size: u16,
    color: [u8; 4],
    clip_bounds: Option<Rect>,
) -> u32 {
    if text.is_empty() || color[3] == 0 {
        return 0;
    }

    let scale = glyph_scale_for_size(font_size) as i32;
    let pw = pixmap.width() as i32;
    let ph = pixmap.height() as i32;

    let (clip_x0, clip_y0, clip_x1, clip_y1) = if let Some(clip) = clip_bounds {
        (
            (clip.x.floor() as i32).clamp(0, pw),
            (clip.y.floor() as i32).clamp(0, ph),
            ((clip.x + clip.width).ceil() as i32).clamp(0, pw),
            ((clip.y + clip.height).ceil() as i32).clamp(0, ph),
        )
    } else {
        (0, 0, pw, ph)
    };

    if clip_x0 >= clip_x1 || clip_y0 >= clip_y1 {
        return 0;
    }

    let base_y = y.round() as i32;
    let mut cursor_x = x.round() as i32;
    let advance_px = (GLYPH_ADVANCE as i32) * scale;
    let mut painted_pixels = 0u32;
    let pixels = pixmap.pixels_mut();

    for ch in text.chars() {
        if cursor_x >= clip_x1 {
            break;
        }
        let rows = ascii_glyph_5x7(ch);
        for (row_idx, &row_bits) in rows.iter().enumerate() {
            if row_bits == 0 {
                continue;
            }
            for col_idx in 0..5i32 {
                let mask = 1u8 << (4 - col_idx);
                if (row_bits & mask) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    let py = base_y + (row_idx as i32) * scale + sy;
                    if py < clip_y0 || py >= clip_y1 {
                        continue;
                    }
                    let row_offset = (py as usize) * (pw as usize);
                    for sx in 0..scale {
                        let px = cursor_x + col_idx * scale + sx;
                        if px >= clip_x0 && px < clip_x1 {
                            let idx = row_offset + (px as usize);
                            pixels[idx] = blend_straight_rgba_over(pixels[idx], color);
                            painted_pixels += 1;
                        }
                    }
                }
            }
        }
        cursor_x += advance_px;
    }

    painted_pixels
}

/// Returns the 7-row (5 bits per row, MSB = leftmost column `0b10000`) bitmap for printable
/// ASCII characters `0x20..=0x7E`, or a clean outlined box for non-ASCII characters.
pub const fn ascii_glyph_5x7(ch: char) -> [u8; 7] {
    match ch {
        ' ' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000,
        ],
        '!' => [
            0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100,
        ],
        '"' => [
            0b01010, 0b01010, 0b01010, 0b00000, 0b00000, 0b00000, 0b00000,
        ],
        '#' => [
            0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010,
        ],
        '$' => [
            0b00100, 0b01111, 0b10100, 0b01110, 0b00101, 0b11110, 0b00100,
        ],
        '%' => [
            0b11000, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b00011,
        ],
        '&' => [
            0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101,
        ],
        '\'' => [
            0b00100, 0b00100, 0b01000, 0b00000, 0b00000, 0b00000, 0b00000,
        ],
        '(' => [
            0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010,
        ],
        ')' => [
            0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000,
        ],
        '*' => [
            0b00000, 0b00100, 0b10101, 0b01110, 0b10101, 0b00100, 0b00000,
        ],
        '+' => [
            0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000,
        ],
        ',' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100, 0b01000,
        ],
        '-' => [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        '.' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100,
        ],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
        ],
        ':' => [
            0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000,
        ],
        ';' => [
            0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b01000,
        ],
        '<' => [
            0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010,
        ],
        '=' => [
            0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000,
        ],
        '>' => [
            0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000,
        ],
        '?' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100,
        ],
        '@' => [
            0b01110, 0b10001, 0b00001, 0b01101, 0b10101, 0b10101, 0b01110,
        ],
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '[' => [
            0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110,
        ],
        '\\' => [
            0b10000, 0b01000, 0b01000, 0b00100, 0b00010, 0b00010, 0b00001,
        ],
        ']' => [
            0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110,
        ],
        '^' => [
            0b00100, 0b01010, 0b10001, 0b00000, 0b00000, 0b00000, 0b00000,
        ],
        '_' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111,
        ],
        '`' => [
            0b01000, 0b00100, 0b00010, 0b00000, 0b00000, 0b00000, 0b00000,
        ],
        'a' => [
            0b00000, 0b00000, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111,
        ],
        'b' => [
            0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b11110,
        ],
        'c' => [
            0b00000, 0b00000, 0b01110, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'd' => [
            0b00001, 0b00001, 0b01101, 0b10011, 0b10001, 0b10001, 0b01111,
        ],
        'e' => [
            0b00000, 0b00000, 0b01110, 0b10001, 0b11111, 0b10000, 0b01110,
        ],
        'f' => [
            0b00110, 0b01001, 0b01000, 0b11100, 0b01000, 0b01000, 0b01000,
        ],
        'g' => [
            0b00000, 0b01111, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110,
        ],
        'h' => [
            0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001,
        ],
        'i' => [
            0b00100, 0b00000, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'j' => [
            0b00010, 0b00000, 0b00110, 0b00010, 0b00010, 0b10010, 0b01100,
        ],
        'k' => [
            0b10000, 0b10000, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010,
        ],
        'l' => [
            0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'm' => [
            0b00000, 0b00000, 0b11010, 0b10101, 0b10101, 0b10001, 0b10001,
        ],
        'n' => [
            0b00000, 0b00000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001,
        ],
        'o' => [
            0b00000, 0b00000, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'p' => [
            0b00000, 0b00000, 0b11110, 0b10001, 0b11110, 0b10000, 0b10000,
        ],
        'q' => [
            0b00000, 0b00000, 0b01101, 0b10011, 0b01111, 0b00001, 0b00001,
        ],
        'r' => [
            0b00000, 0b00000, 0b10110, 0b11001, 0b10000, 0b10000, 0b10000,
        ],
        's' => [
            0b00000, 0b00000, 0b01110, 0b10000, 0b01110, 0b00001, 0b11110,
        ],
        't' => [
            0b01000, 0b01000, 0b11100, 0b01000, 0b01000, 0b01001, 0b00110,
        ],
        'u' => [
            0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b10011, 0b01101,
        ],
        'v' => [
            0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'w' => [
            0b00000, 0b00000, 0b10001, 0b10001, 0b10101, 0b10101, 0b01010,
        ],
        'x' => [
            0b00000, 0b00000, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001,
        ],
        'y' => [
            0b00000, 0b00000, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110,
        ],
        'z' => [
            0b00000, 0b00000, 0b11111, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '{' => [
            0b00010, 0b00100, 0b00100, 0b01000, 0b00100, 0b00100, 0b00010,
        ],
        '|' => [
            0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        '}' => [
            0b01000, 0b00100, 0b00100, 0b00010, 0b00100, 0b00100, 0b01000,
        ],
        '~' => [
            0b00000, 0b01000, 0b10101, 0b00010, 0b00000, 0b00000, 0b00000,
        ],
        _ => [
            0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{CLASS_HELP_DRAWER, CLASS_SETTINGS_DRAWER};
    use crate::widget::{
        button, column, container, divider, dropdown, grid, icon, row, scrollable, text,
        text_input, toggler,
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum RasterTestMsg {
        Action,
        Toggle(bool),
    }

    #[test]
    fn test_ascii_glyph_table_covers_all_printable_ascii() {
        for byte in 0x21u8..=0x7Eu8 {
            let ch = byte as char;
            let glyph = ascii_glyph_5x7(ch);
            assert!(
                glyph.iter().any(|&row| row != 0),
                "printable ASCII char {:?} (0x{:02X}) must have non-empty bitmap rows",
                ch,
                byte
            );
        }
        assert_eq!(ascii_glyph_5x7(' '), [0; 7]);
        // Non-ASCII fallback produces an outlined 5x7 box
        assert_eq!(
            ascii_glyph_5x7('\u{1F5A5}'),
            [
                0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111
            ]
        );
    }

    #[test]
    fn test_draw_text_and_clipping_bounds() {
        let mut pixmap = Pixmap::new(120, 40).unwrap();
        pixmap.fill(Color::from_rgba8(20, 22, 28, 255));

        let written = draw_text(
            &mut pixmap,
            "rFerram 10.0.10.15:4433",
            4.0,
            6.0,
            14,
            [238, 242, 248, 255],
            Some(Rect::new(0.0, 0.0, 120.0, 40.0)),
        );
        assert!(written > 50);

        // Empty text and zero-alpha color write 0 pixels
        assert_eq!(
            draw_text(&mut pixmap, "", 4.0, 6.0, 14, [255, 255, 255, 255], None),
            0
        );
        assert_eq!(
            draw_text(
                &mut pixmap,
                "hidden",
                4.0,
                6.0,
                14,
                [255, 255, 255, 0],
                None
            ),
            0
        );
    }

    #[test]
    fn test_rasterize_scene_dark_and_light_themes_with_overlays() {
        let tree: Element<'_, RasterTestMsg> = container(
            "rferram-root-shell",
            column(
                10,
                vec![
                    container(
                        "rferram-header",
                        row(
                            8,
                            vec![
                                text("Connections").size(18).into(),
                                button("+").on_press(RasterTestMsg::Action).into(),
                                text_input("Search connections...", "macOS").into(),
                            ],
                        ),
                    ),
                    scrollable(container(
                        "rferram-group-section",
                        grid(
                            2,
                            12,
                            vec![container(
                                "rferram-connection-card",
                                column(
                                    6,
                                    vec![
                                        text("macOS Workstation M3").size(15).into(),
                                        divider(),
                                        toggler("Reconnect", true, RasterTestMsg::Toggle(false)),
                                        dropdown(
                                            vec!["None".to_string(), "WireGuard".to_string()],
                                            Some(1),
                                        ),
                                        icon("folder-remote-symbolic", 24),
                                        button("Connect").on_press(RasterTestMsg::Action).into(),
                                    ],
                                ),
                            )],
                        ),
                    )),
                    container(
                        CLASS_SETTINGS_DRAWER,
                        column(8, vec![text("Settings: General").size(16).into()]),
                    ),
                    container(
                        CLASS_HELP_DRAWER,
                        column(8, vec![text("Help: Documentation").size(16).into()]),
                    ),
                    container(
                        CLASS_ADD_DESKTOP_MODAL,
                        column(
                            8,
                            vec![
                                text("Add Desktop").size(18).into(),
                                text_input("Host name or IP address", "10.0.0.1:4433").into(),
                                button("Add").on_press(RasterTestMsg::Action).into(),
                            ],
                        ),
                    ),
                ],
            ),
        );

        let scene = tree
            .layout_scene(1440.0, 900.0)
            .with_caret_position(Some(4));
        assert_eq!(scene.width(), 1440);
        assert_eq!(scene.height(), 900);

        let dark_pixmap = scene.rasterize(&Theme::dark());
        assert_eq!(dark_pixmap.width(), 1440);
        assert_eq!(dark_pixmap.height(), 900);

        let light_pixmap = scene.rasterize(&Theme::light());
        assert_eq!(light_pixmap.width(), 1440);
        assert_eq!(light_pixmap.height(), 900);

        // Dark and light theme renders must differ in pixel output
        assert_ne!(dark_pixmap.data(), light_pixmap.data());
    }

    #[test]
    fn test_rasterize_edge_cases_empty_and_tiny_viewports() {
        // Empty scene at 0x0 clamps to 1x1 without panicking
        let empty_scene: LayoutScene<RasterTestMsg> = LayoutScene::new(0.0, 0.0, Vec::new());
        assert_eq!(empty_scene.width(), 1);
        assert_eq!(empty_scene.height(), 1);
        let px = empty_scene.rasterize(&Theme::dark());
        assert_eq!(px.width(), 1);
        assert_eq!(px.height(), 1);

        // Out-of-bounds text clipping returns 0 pixels safely
        let mut small_px = Pixmap::new(16, 16).unwrap();
        assert_eq!(
            draw_text(
                &mut small_px,
                "Offscreen",
                200.0,
                200.0,
                14,
                [255, 255, 255, 255],
                Some(Rect::new(0.0, 0.0, 16.0, 16.0)),
            ),
            0
        );

        // Translucent straight RGBA8 text (where R/G/B > Alpha) blends cleanly and writes > 0 pixels
        let mut blend_px = Pixmap::new(120, 32).unwrap();
        blend_px.fill(Color::from_rgba8(20, 22, 28, 255));
        let translucent_written = draw_text(
            &mut blend_px,
            "Translucent",
            4.0,
            6.0,
            14,
            [238, 242, 248, 128],
            None,
        );
        assert!(
            translucent_written > 20,
            "translucent straight RGBA8 text must paint blended glyph pixels"
        );
    }

    #[test]
    fn test_nested_scrollable_overflow_clipping_and_topmost_focus() {
        let mut rows: Vec<Element<'_, RasterTestMsg>> = Vec::new();
        for idx in 0..28 {
            rows.push(
                button(format!("Scroll Row {idx}"))
                    .on_press(RasterTestMsg::Action)
                    .into(),
            );
        }

        let tree: Element<'_, RasterTestMsg> = container(
            "rferram-root-shell",
            column(
                8,
                vec![
                    scrollable(scrollable(column(6, rows))),
                    container(
                        CLASS_ADD_DESKTOP_MODAL,
                        column(
                            8,
                            vec![
                                text_input("Host name or IP address", "10.42.0.10:4433").into(),
                                text_input("Optional local only name", "").into(),
                            ],
                        ),
                    ),
                ],
            ),
        );

        let scene_caret_0 = tree.layout_scene(800.0, 480.0).with_caret_position(Some(0));
        let scene_caret_8 = tree.layout_scene(800.0, 480.0).with_caret_position(Some(8));

        let px_0 = scene_caret_0.rasterize(&Theme::dark());
        let px_8 = scene_caret_8.rasterize(&Theme::dark());
        assert_eq!(px_0.width(), 800);
        assert_eq!(px_0.height(), 480);
        // Because the first modal TextInput ("10.42.0.10:4433") is auto-focused rather than the
        // trailing empty TextInput, changing caret_position from 0 to 8 moves the rendered caret bar.
        assert_ne!(px_0.data(), px_8.data());
    }

    #[test]
    fn test_all_symbolic_icons_render_distinct_vector_pixels() {
        assert!(
            SYMBOLIC_ICON_NAMES.len() >= 19,
            "expected at least 19 symbolic icon names in SYMBOLIC_ICON_NAMES"
        );

        let theme = Theme::dark();
        let palette = ThemePalette::from_theme(&theme);
        let bg = to_skia_color(palette.window_bg);

        let mut rendered_icons: Vec<(&str, Vec<u8>)> =
            Vec::with_capacity(SYMBOLIC_ICON_NAMES.len());

        for &icon_name in SYMBOLIC_ICON_NAMES {
            // Also verify icon::from_name builder produces the expected Icon WidgetNode
            let elem: Element<'_, RasterTestMsg> = icon::from_name(icon_name).size(24).into();
            let px = elem.rasterize(32, 32, &theme);
            assert_eq!(px.width(), 32);
            assert_eq!(px.height(), 32);

            let mut baseline = Pixmap::new(32, 32).unwrap();
            baseline.fill(bg);
            assert_ne!(
                px.data(),
                baseline.data(),
                "symbolic icon {icon_name:?} must paint non-background vector pixels"
            );

            for (prev_name, prev_data) in &rendered_icons {
                assert_ne!(
                    px.data(),
                    prev_data.as_slice(),
                    "symbolic icon {icon_name:?} must render distinct vector pixels from {prev_name:?}"
                );
            }
            rendered_icons.push((icon_name, px.data().to_vec()));

            // Verify sub-16px layout sizing (12x12) and sub-6px (4x4) bounds containment
            let small_elem: Element<'_, RasterTestMsg> = icon::from_name(icon_name).size(12).into();
            let small_boxes = small_elem.layout(64.0, 64.0);
            assert_eq!(small_boxes.len(), 1);
            assert_eq!((small_boxes[0].width, small_boxes[0].height), (12.0, 12.0));

            if !icon_name.contains("rFerram") {
                let mut tiny_canvas = Pixmap::new(24, 24).unwrap();
                tiny_canvas.fill(bg);
                paint_icon(
                    &mut tiny_canvas,
                    Rect::new(10.0, 10.0, 4.0, 4.0),
                    icon_name,
                    4,
                    &palette,
                    None,
                );
                // Pixels outside the 4x4 target region (with 1px anti-aliasing margin: 9..15) must remain untouched
                for y in 0..24u32 {
                    for x in 0..24u32 {
                        if !(9..15).contains(&x) || !(9..15).contains(&y) {
                            let p = tiny_canvas.pixel(x, y).unwrap();
                            assert_eq!(
                                (p.red(), p.green(), p.blue()),
                                (
                                    palette.window_bg[0],
                                    palette.window_bg[1],
                                    palette.window_bg[2]
                                ),
                                "sub-6px icon {icon_name:?} overflowed at ({x}, {y})"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn test_button_styles_and_checkbox_toggler_rendering() {
        use crate::widget::{
            ButtonStyle, TogglerStyle, checkbox, icon_button, menu_button, nav_button,
            segmented_button,
        };

        let theme = Theme::dark();
        let palette = ThemePalette::from_theme(&theme);

        // 1. Verify each ButtonStyle renders pairwise distinct pixels for the same label & icon
        let styles = [
            ButtonStyle::Standard,
            ButtonStyle::Primary,
            ButtonStyle::Icon,
            ButtonStyle::Segmented,
            ButtonStyle::NavRow,
            ButtonStyle::CardTrigger,
            ButtonStyle::MenuItem,
        ];
        let mut rendered_styles: Vec<(ButtonStyle, Vec<u8>)> = Vec::with_capacity(styles.len());
        for style in styles {
            let elem: Element<'_, RasterTestMsg> = button("Action")
                .style(style)
                .icon("preferences-system-symbolic")
                .on_press(RasterTestMsg::Action)
                .into();
            let px = elem.rasterize(160, 44, &theme);
            assert_eq!(px.width(), 160);
            assert_eq!(px.height(), 44);
            for (prev_style, prev_data) in &rendered_styles {
                assert_ne!(
                    px.data(),
                    prev_data.as_slice(),
                    "ButtonStyle::{style:?} must render distinctly from ButtonStyle::{prev_style:?}"
                );
            }
            rendered_styles.push((style, px.data().to_vec()));
        }

        // Verify active icon buttons and primary buttons paint contrasted (text_on_accent) icon strokes
        let active_add_btn: Element<'_, RasterTestMsg> = icon_button("add-symbolic")
            .active(true)
            .on_press(RasterTestMsg::Action)
            .into();
        let active_add_px = active_add_btn.rasterize(40, 40, &theme);
        let has_contrasted_icon_pixel = active_add_px.pixels().iter().any(|px| {
            px.red() == palette.text_on_accent[0]
                && px.green() == palette.text_on_accent[1]
                && px.blue() == palette.text_on_accent[2]
        });
        assert!(
            has_contrasted_icon_pixel,
            "active Icon button must paint symbolic icon in text_on_accent color over accent fill"
        );

        // Verify icon-only segmented/primary buttons stay compact in layout
        let seg_icon_only: Element<'_, RasterTestMsg> = icon_button("view-grid-symbolic")
            .style(ButtonStyle::Segmented)
            .into();
        let seg_boxes = seg_icon_only.layout(200.0, 44.0);
        assert_eq!(seg_boxes[0].width, 36.0);

        // 2. Verify icon_button, nav_button, segmented_button, and menu_button builders + collect_text
        let ic_btn: Element<'_, RasterTestMsg> = icon_button("add-symbolic")
            .label("+")
            .on_press(RasterTestMsg::Action)
            .into();
        assert_eq!(
            ic_btn.collect_text(),
            vec!["+".to_string(), "add-symbolic".to_string()]
        );

        let nav_btn: Element<'_, RasterTestMsg> = nav_button("Favorites")
            .icon("emblem-favorite-symbolic")
            .active(true)
            .on_press(RasterTestMsg::Action)
            .into();
        assert_eq!(
            nav_btn.collect_text(),
            vec![
                "Favorites".to_string(),
                "emblem-favorite-symbolic".to_string()
            ]
        );

        let seg_btn: Element<'_, RasterTestMsg> = segmented_button("General")
            .selected(true)
            .on_press(RasterTestMsg::Action)
            .into();
        let menu_btn: Element<'_, RasterTestMsg> = menu_button("Connect")
            .icon("object-select-symbolic")
            .on_press(RasterTestMsg::Action)
            .into();

        for el in [ic_btn, nav_btn, seg_btn, menu_btn] {
            let px = el.rasterize(180, 44, &theme);
            assert_eq!(px.width(), 180);
            assert_eq!(px.height(), 44);
        }

        // 3. Verify TogglerStyle::Checkbox vs TogglerStyle::Switch and checked vs unchecked
        let chk_unchecked: Element<'_, RasterTestMsg> = checkbox(
            "Reconnect if the session is dropped",
            false,
            RasterTestMsg::Toggle(true),
        );
        let chk_checked: Element<'_, RasterTestMsg> = checkbox(
            "Reconnect if the session is dropped",
            true,
            RasterTestMsg::Toggle(false),
        );
        let sw_checked: Element<'_, RasterTestMsg> = toggler(
            "Reconnect if the session is dropped",
            true,
            RasterTestMsg::Toggle(false),
        );

        assert!(matches!(
            chk_checked.node,
            WidgetNode::Toggler {
                style: TogglerStyle::Checkbox,
                is_checked: true,
                ..
            }
        ));
        assert!(matches!(
            sw_checked.node,
            WidgetNode::Toggler {
                style: TogglerStyle::Switch,
                is_checked: true,
                ..
            }
        ));

        let px_chk_off = chk_unchecked.rasterize(320, 36, &theme);
        let px_chk_on = chk_checked.rasterize(320, 36, &theme);
        let px_sw_on = sw_checked.rasterize(320, 36, &theme);
        assert_ne!(
            px_chk_off.data(),
            px_chk_on.data(),
            "checked checkbox must paint accent fill and checkmark distinct from unchecked checkbox"
        );
        assert_ne!(
            px_chk_on.data(),
            px_sw_on.data(),
            "TogglerStyle::Checkbox must render distinctly from TogglerStyle::Switch"
        );
    }

    #[test]
    fn test_mockup_container_classes_and_card_sheen_rasterization() {
        let dark_theme = Theme::dark();
        let light_theme = Theme::light();
        let dark_palette = ThemePalette::from_theme(&dark_theme);
        let bg = to_skia_color(dark_palette.window_bg);

        let container_classes = [
            CLASS_TOP_TABBAR,
            CLASS_TOOLBAR,
            CLASS_SEGMENTED_PILL,
            CLASS_SEARCH_PILL,
            CLASS_GROUP_BADGE,
            CLASS_CONNECTION_CARD,
            CLASS_CONTEXT_MENU,
            CLASS_ADD_MENU,
            CLASS_FORM_ROW,
            CLASS_MODAL_TABS,
            CLASS_MODAL_TAB_FRAME,
            CLASS_MODAL_FOOTER,
            CLASS_DRAWER_NAV,
            CLASS_DRAWER_CONTENT,
        ];

        for &class in &container_classes {
            let elem: Element<'_, RasterTestMsg> = container(class, text("Sample"));
            let dark_px = elem.rasterize(240, 260, &dark_theme);
            let light_px = elem.rasterize(240, 260, &light_theme);
            assert_eq!((dark_px.width(), dark_px.height()), (240, 260));
            assert_eq!((light_px.width(), light_px.height()), (240, 260));
            assert_ne!(
                dark_px.data(),
                light_px.data(),
                "container class {class:?} must adapt between dark and light themes"
            );

            // Also verify direct paint_container paints non-background surface/border pixels
            let mut canvas = Pixmap::new(220, 240).unwrap();
            canvas.fill(bg);
            let mut baseline = Pixmap::new(220, 240).unwrap();
            baseline.fill(bg);

            paint_container(
                &mut canvas,
                Rect::new(10.0, 10.0, 200.0, 220.0),
                class,
                &dark_theme,
                &dark_palette,
                None,
            );
            assert_ne!(
                canvas.data(),
                baseline.data(),
                "paint_container for {class:?} must paint visible vector pixels"
            );
        }

        // Verify portrait connection card upper preview sheen makes upper-half interior pixels
        // brighter than lower-half interior card_bg pixels in Dark Theme
        let mut card_canvas = Pixmap::new(220, 260).unwrap();
        card_canvas.fill(bg);
        paint_container(
            &mut card_canvas,
            Rect::new(10.0, 10.0, 200.0, 240.0),
            CLASS_CONNECTION_CARD,
            &dark_theme,
            &dark_palette,
            None,
        );
        let upper_px = card_canvas
            .pixel(110, 50)
            .expect("upper preview sheen pixel");
        let lower_px = card_canvas.pixel(110, 200).expect("lower card body pixel");
        assert!(
            upper_px.red() > lower_px.red()
                && upper_px.green() > lower_px.green()
                && upper_px.blue() > lower_px.blue(),
            "portrait connection card upper preview sheen {:?} must be brighter than lower card body {:?}",
            (upper_px.red(), upper_px.green(), upper_px.blue()),
            (lower_px.red(), lower_px.green(), lower_px.blue())
        );

        // Edge case: tiny 4x4 container bounds must not panic on any container class
        let mut tiny_canvas = Pixmap::new(16, 16).unwrap();
        tiny_canvas.fill(bg);
        for &class in &container_classes {
            paint_container(
                &mut tiny_canvas,
                Rect::new(2.0, 2.0, 4.0, 4.0),
                class,
                &dark_theme,
                &dark_palette,
                None,
            );
        }
    }
}
