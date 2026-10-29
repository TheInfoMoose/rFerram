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
    CLASS_ADD_DESKTOP_MODAL, LayoutBox, LayoutScene, Rect, compute_clip_rects,
    is_overlay_container_class, widget_subtree_count,
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
        WidgetNode::Button { label, .. } => {
            paint_button(pixmap, bounds, label, palette, is_hovered, clip_rect, mask);
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
            label, is_checked, ..
        } => {
            paint_toggler(pixmap, bounds, label, *is_checked, palette, clip_rect, mask);
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
        "rferram-root-shell" => {
            fill_rounded_rect_masked(pixmap, bounds, 0.0, palette.window_bg, mask);
        }
        "rferram-header" => {
            fill_rounded_rect_masked(pixmap, bounds, 8.0, palette.header_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, 8.0, 1.0, palette.card_border, mask);
        }
        "rferram-sidebar" => {
            fill_rounded_rect_masked(pixmap, bounds, 8.0, palette.sidebar_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, 8.0, 1.0, palette.card_border, mask);
        }
        "rferram-group-section" => {
            fill_rounded_rect_masked(pixmap, bounds, 10.0, palette.group_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, 10.0, 1.0, palette.card_border, mask);
        }
        "rferram-connection-card" => {
            fill_rounded_rect_masked(pixmap, bounds, 10.0, palette.card_bg, mask);
            stroke_rounded_rect_masked(pixmap, bounds, 10.0, 1.25, palette.card_border, mask);
            // Subtle accent left indicator bar on each connection card
            let bar = Rect::new(
                bounds.x + 2.0,
                bounds.y + 8.0,
                3.0,
                (bounds.height - 16.0).max(4.0),
            );
            fill_rounded_rect_masked(pixmap, bar, 1.5, palette.accent, mask);
        }
        _ => {}
    }
}

fn is_primary_accent_button(label: &str) -> bool {
    matches!(label, "+" | "Add" | "Connect" | "*")
}

fn paint_button(
    pixmap: &mut Pixmap,
    bounds: Rect,
    label: &str,
    palette: &ThemePalette,
    is_hovered: bool,
    clip_rect: Option<Rect>,
    mask: Option<&Mask>,
) {
    let is_primary = is_primary_accent_button(label);
    let fill_color = if is_primary {
        palette.accent
    } else if is_hovered {
        palette.accent_muted
    } else {
        palette.button_bg
    };
    let border_color = if is_primary || is_hovered {
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

    let font_size = 13u16;
    let text_w = measure_text_width(label, font_size);
    let text_h = glyph_pixel_height(font_size);
    let tx = bounds.x + ((bounds.width - text_w) * 0.5).max(4.0);
    let ty = bounds.y + ((bounds.height - text_h) * 0.5).max(2.0);
    let inner_clip = inset_rect(bounds, 2.0);
    let effective_clip = clip_rect.map_or(inner_clip, |c| inner_clip.intersect_clamped(c));
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

fn paint_toggler(
    pixmap: &mut Pixmap,
    bounds: Rect,
    label: &str,
    is_checked: bool,
    palette: &ThemePalette,
    clip_rect: Option<Rect>,
    mask: Option<&Mask>,
) {
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

    if !label.is_empty() {
        let font_size = 13u16;
        let text_h = glyph_pixel_height(font_size);
        let tx = switch_x + switch_w + 8.0;
        let ty = bounds.y + ((bounds.height - text_h) * 0.5).max(2.0);
        let effective_clip = clip_rect.map_or(bounds, |c| bounds.intersect_clamped(c));
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

fn paint_icon(
    pixmap: &mut Pixmap,
    bounds: Rect,
    name: &str,
    _size: u16,
    palette: &ThemePalette,
    mask: Option<&Mask>,
) {
    let icon_rect = inset_rect(bounds, 2.0);
    if name.contains("folder") {
        // Draw folder tab + body
        let tab_rect = Rect::new(
            icon_rect.x,
            icon_rect.y,
            (icon_rect.width * 0.45).max(4.0),
            (icon_rect.height * 0.28).max(3.0),
        );
        let body_rect = Rect::new(
            icon_rect.x,
            icon_rect.y + tab_rect.height * 0.6,
            icon_rect.width,
            (icon_rect.height - tab_rect.height * 0.6).max(4.0),
        );
        fill_rounded_rect_masked(pixmap, tab_rect, 2.0, palette.accent, mask);
        fill_rounded_rect_masked(pixmap, body_rect, 3.0, palette.accent, mask);
    } else {
        fill_rounded_rect_masked(pixmap, icon_rect, 4.0, palette.accent_muted, mask);
        stroke_rounded_rect_masked(pixmap, icon_rect, 4.0, 1.5, palette.accent, mask);
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
}
