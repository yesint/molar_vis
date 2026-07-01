//! Shared egui widget helpers used across the app submodules.
use super::*;


/// A compact icon button: frameless at rest, with a background highlight on
/// hover, plus a tooltip. Implemented via `selectable_label` (always unselected)
/// because the theme overrides text color, so a frameless `Button` would show no
/// hover feedback, whereas `selectable_label` highlights its background.
pub(super) fn icon_button(ui: &mut egui::Ui, glyph: &str, hover: &str) -> egui::Response {
    ui.selectable_label(false, glyph).on_hover_text(hover)
}

/// Tighten spacing for a group of action icons (call first in the group's `ui`).
pub(super) fn compact_actions(ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing.x = 2.0;
    ui.spacing_mut().button_padding = egui::vec2(3.0, 1.0);
}

/// Widest of `labels` in the picker-button font. A picker button reserves this so it
/// keeps a **constant width** as the selection changes — a wider label must not grow
/// the button and reflow/resize the whole panel. Measured once per picker per frame.
pub(super) fn max_label_width<'a>(ui: &egui::Ui, labels: impl Iterator<Item = &'a str>) -> f32 {
    let txt = ui.visuals().text_color();
    labels
        .map(|l| {
            ui.painter()
                .layout_no_wrap(l.to_owned(), egui::FontId::proportional(14.0), txt)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max)
}

/// A dropdown button showing a drawn icon + a text label + a caret. `label_w` is the
/// width reserved for the label (pass [`max_label_width`] of all options so the button
/// doesn't change size with the selection). `draw_icon` paints into the given rect;
/// returns the click response (drive a `Popup` off it).
pub(super) fn picker_button(
    ui: &mut egui::Ui,
    label: &str,
    label_w: f32,
    draw_icon: impl FnOnce(&egui::Painter, egui::Rect),
) -> egui::Response {
    let txt = ui.visuals().text_color();
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), egui::FontId::proportional(14.0), txt);
    let (icon_w, caret_w, pad, gap) = (26.0_f32, 11.0_f32, 5.0_f32, 5.0_f32);
    // Reserve the widest option's label width (fixed button size); the current label is
    // drawn left-aligned within it.
    let w = pad + icon_w + gap + label_w.max(galley.size().x) + gap + caret_w + pad;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 20.0), egui::Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + pad, rect.center().y - 8.0),
        egui::vec2(icon_w, 16.0),
    );
    draw_icon(ui.painter(), icon_rect);
    ui.painter().galley(
        egui::pos2(icon_rect.right() + gap, rect.center().y - galley.size().y * 0.5),
        galley,
        txt,
    );
    ui.painter().text(
        egui::pos2(rect.right() - pad - caret_w * 0.5, rect.center().y),
        egui::Align2::CENTER_CENTER,
        icon::CARET_DOWN,
        egui::FontId::proportional(10.0),
        txt,
    );
    resp
}

/// One control in the viewport overlay toolbar: a fixed-height framed button
/// whose content (an icon glyph, or `label + caret`) is centered by its **ink**
/// bounds (`Galley::mesh_bounds`), not the font line-box. `ui.button` /
/// `selectable_label` center the line-box, so Phosphor glyphs with different
/// metrics (the depth-cue lines sat low, the cube high) looked vertically
/// ragged; ink-centering lines them up. `active` paints the selection fill
/// (toggle / open state). Returns the response — drive a `Popup::menu` off it.
pub(super) fn overlay_button(ui: &mut egui::Ui, content: &str, active: bool) -> egui::Response {
    const H: f32 = 26.0;
    const R: f32 = 4.0;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let txt = ui.visuals().text_color();
    let galley = ui.painter().layout_no_wrap(content.to_owned(), font, txt);
    let w = (galley.size().x + 14.0).max(H);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, H), egui::Sense::click());
    let vis = ui.style().interact_selectable(&resp, active);
    let fill = if active {
        ui.visuals().selection.bg_fill
    } else {
        vis.weak_bg_fill
    };
    ui.painter().rect_filled(rect, R, fill);
    if vis.bg_stroke.width > 0.0 {
        ui.painter()
            .rect_stroke(rect, R, vis.bg_stroke, egui::StrokeKind::Inside);
    }
    // Center the glyph/label by its ink, so it sits dead-centre regardless of the
    // font's per-glyph vertical metrics.
    let ink = galley.mesh_bounds;
    ui.painter()
        .galley(rect.center() - ink.center().to_vec2(), galley, txt);
    resp
}

/// A plain text label vertically centered the same way `overlay_button` centers its
/// glyph (by ink bounds, at the toolbar button height) — so a label sitting next to
/// `overlay_button` dropdowns lines up with them instead of riding high/low.
pub(super) fn toolbar_label(ui: &mut egui::Ui, text: &str) {
    const H: f32 = 26.0;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let col = ui.visuals().text_color();
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, col);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(galley.size().x, H), egui::Sense::hover());
    let ink = galley.mesh_bounds;
    ui.painter()
        .galley(rect.center() - ink.center().to_vec2(), galley, col);
}

/// Parameter controls for a representation, shown inline under its row as a tidy
/// two-column table (parameter name on the left, control on the right).
/// Returns `true` if a render-only change was made (periodic-image params) so the
/// caller can flag the viewport dirty; geometry changes set `rep.geom_dirty`
/// directly. `has_box` gates the **Periodic** tab (only meaningful with a box).
/// The app's standard **tab bar**: underline-style tabs (the selected tab is bold
/// with an accent underline; the others are weak, clickable text) instead of
/// disconnected toggle buttons. Sets `*current` to the clicked tab and returns
/// whether the selection changed. Use this for *all* tabbed UIs so they look the
/// same (rep settings, the delete-frames dialog, …).
pub(super) fn tab_bar<T: Copy + PartialEq>(ui: &mut egui::Ui, current: &mut T, tabs: &[(T, &str)]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        for &(tab, label) in tabs {
            let selected = *current == tab;
            let txt = if selected {
                egui::RichText::new(label).strong()
            } else {
                egui::RichText::new(label).color(ui.visuals().weak_text_color())
            };
            let resp = ui
                .add(egui::Label::new(txt).sense(egui::Sense::click()))
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.clicked() && !selected {
                *current = tab;
                changed = true;
            }
            if selected {
                let r = resp.rect;
                ui.painter().hline(
                    r.x_range(),
                    r.bottom() + 2.0,
                    egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
                );
            }
        }
    });
    changed
}

/// A value slider with a numeric edit box beside it (the "[slider] [edit]"
/// pattern), both bound to the same value; `enabled` greys both out.
pub(super) fn slider_with_edit(
    ui: &mut egui::Ui,
    v: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    enabled: bool,
) {
    ui.horizontal(|ui| {
        ui.add_enabled(
            enabled,
            egui::Slider::new(v, range.clone()).show_value(false),
        );
        ui.add_enabled(
            enabled,
            egui::DragValue::new(v).speed(0.01).range(range).fixed_decimals(2),
        );
    });
}

/// A color "selector": a swatch button that opens (on click, downward) a popup
/// holding a full color picker. A nested `Popup::menu` so it stays within the parent
/// `CloseOnClickOutside` menu's hierarchy, and `CloseOnClickOutside` itself so
/// dragging the picker doesn't dismiss it. `c` is linear RGBA 0..1; the picker works
/// in sRGB `Color32`, converted through `egui::Rgba` so the swatch is WYSIWYG.
pub(super) fn color_submenu(ui: &mut egui::Ui, _id: &str, c: &mut [f32; 4]) {
    let mut col: egui::Color32 =
        egui::Rgba::from_rgba_unmultiplied(c[0], c[1], c[2], 1.0).into();
    let header = ui.add(egui::Button::new("").fill(col).min_size(egui::vec2(30.0, 16.0)));
    egui::Popup::menu(&header)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            // Fixed width so the picker doesn't resize as its contents change.
            ui.set_min_width(230.0);
            ui.set_max_width(230.0);
            if egui::color_picker::color_picker_color32(
                ui,
                &mut col,
                egui::color_picker::Alpha::Opaque,
            ) {
                let lin = egui::Rgba::from(col);
                *c = [lin.r(), lin.g(), lin.b(), 1.0];
            }
        });
}
