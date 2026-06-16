//! The eframe application: owns UI state, the camera, the scene (molecules and
//! their representations), and the 3D renderer. Lays out the VMD-style left
//! control panel (Scene → Molecules → Representations → Rep controls) plus the
//! central 3D viewport, and only re-renders the scene when something changed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use eframe::egui;
use molar::prelude::{ParticleIterProvider, SsAlgorithm, State};

use crate::camera::{Camera, Projection};
use crate::color::ColorMethod;
use crate::data;
use crate::geometry::{self, RepKind, RepParams};
use crate::history::{EditState, History};
use crate::launch::AppLaunch;
use crate::material::Material;
use crate::pick::{self, PickMode, SelectionMode};
use crate::render::SceneRenderer;
use crate::scene::{self, MolId, Representation, Scene, SettingsTab};
use crate::secstruct::SsMap;
use crate::trajectory::{LoadMode, LoadMsg, LoadOptions, LoopMode, Trajectory};

use egui_phosphor::regular as icon;

/// A compact icon button: frameless at rest, with a background highlight on
/// hover, plus a tooltip. Implemented via `selectable_label` (always unselected)
/// because the theme overrides text color, so a frameless `Button` would show no
/// hover feedback, whereas `selectable_label` highlights its background.
fn icon_button(ui: &mut egui::Ui, glyph: &str, hover: &str) -> egui::Response {
    ui.selectable_label(false, glyph).on_hover_text(hover)
}

/// Tighten spacing for a group of action icons (call first in the group's `ui`).
fn compact_actions(ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing.x = 2.0;
    ui.spacing_mut().button_padding = egui::vec2(3.0, 1.0);
}

/// Overlay a red border + a right-justified "⚠ 0!" on a selection field whose
/// selection is valid but matched **zero atoms** (molar's "empty" error, surfaced
/// as a non-destructive warning — the text stays editable).
fn mark_empty_selection(ui: &egui::Ui, rect: egui::Rect) {
    let red = egui::Color32::from_rgb(220, 90, 90);
    let painter = ui.painter();
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.5, red),
        egui::StrokeKind::Inside,
    );
    painter.text(
        egui::pos2(rect.right() - 6.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        format!("{} 0!", icon::WARNING),
        egui::FontId::proportional(13.0),
        red,
    );
}

/// The blue glow ring shared by hover-picking and the active-selection highlight:
/// a faint thick halo fading inward to a bright thin core, centered at `center`
/// with core pixel radius `rpx`.
fn draw_glow_ring(painter: &egui::Painter, center: egui::Pos2, rpx: f32) {
    let glow = |a: u8| egui::Color32::from_rgba_unmultiplied(130, 215, 255, a);
    painter.circle_stroke(center, rpx + 4.0, egui::Stroke::new(6.0, glow(35)));
    painter.circle_stroke(center, rpx + 1.5, egui::Stroke::new(3.0, glow(95)));
    painter.circle_stroke(center, rpx, egui::Stroke::new(1.8, glow(235)));
}

/// Draw the hover-pick highlight over the viewport: a glowing outline ring at the
/// hovered atom's **displayed** position (sized to the rep's sphere radius) plus a
/// lower-left info box with the atom's identity and **real** coordinates (nm).
fn draw_pick_overlay(
    ui: &egui::Ui,
    rect: egui::Rect,
    camera: &Camera,
    aspect: f32,
    hit: &crate::pick::PickHit,
) {
    let vp = camera.proj(aspect) * camera.view();
    let project = |w: glam::Vec3| -> Option<egui::Pos2> {
        let c = vp * w.extend(1.0);
        if c.w <= 0.0 {
            return None;
        }
        let nx = c.x / c.w;
        let ny = c.y / c.w;
        Some(egui::pos2(
            rect.left() + (nx * 0.5 + 0.5) * rect.width(),
            rect.top() + (1.0 - (ny * 0.5 + 0.5)) * rect.height(),
        ))
    };
    let Some(center) = project(hit.display) else {
        return;
    };
    // Projected pixel radius: project a point one world-radius to the camera's right.
    let right = camera.orientation * glam::Vec3::X;
    let rpx = project(hit.display + right * hit.radius)
        .map(|e| (e - center).length())
        .unwrap_or(6.0)
        .clamp(3.0, rect.width());

    let painter = ui.painter_at(rect);
    draw_glow_ring(&painter, center, rpx);

    // Lower-left info box: "name resname resid" / "x, y, z" (real coords, nm).
    let font = egui::FontId::monospace(13.0);
    let tc = egui::Color32::from_gray(240);
    let l1 = format!("{} {} {}", hit.name, hit.resname, hit.resid);
    let l2 = format!("{:.3}, {:.3}, {:.3}", hit.real.x, hit.real.y, hit.real.z);
    let g1 = painter.layout_no_wrap(l1, font.clone(), tc);
    let g2 = painter.layout_no_wrap(l2, font, tc);
    let (s1, s2) = (g1.size(), g2.size());
    let pad = 6.0;
    let w = s1.x.max(s2.x) + pad * 2.0;
    let h = s1.y + s2.y + pad * 2.0;
    let x = rect.left() + 8.0;
    let y = rect.bottom() - 8.0 - h;
    let box_rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h));
    painter.rect_filled(box_rect, 4.0, egui::Color32::from_black_alpha(180));
    painter.rect_stroke(
        box_rect,
        4.0,
        egui::Stroke::new(1.0, egui::Color32::from_gray(120)),
        egui::StrokeKind::Inside,
    );
    painter.galley(egui::pos2(x + pad, y + pad), g1, tc);
    painter.galley(egui::pos2(x + pad, y + pad + s1.y), g2, tc);
}

/// Draw a VMD-style orientation-axes gizmo into the chosen corner of `rect`.
/// The three world axes (X red, Y green, Z blue) rotate with the camera; only the
/// positive directions are drawn, labelled, and depth-sorted so nearer axes sit on top.
fn draw_axes_overlay(ui: &egui::Ui, rect: egui::Rect, camera: &Camera, corner: Corner) {
    let len = 26.0; // axis length, px
    let label_gap = 12.0; // px between an axis tip and its label
    let margin = len + label_gap + 10.0; // keep the gizmo + labels inside the rect
    let center = match corner {
        Corner::TopLeft => rect.left_top() + egui::vec2(margin, margin),
        Corner::TopRight => rect.right_top() + egui::vec2(-margin, margin),
        Corner::BottomLeft => rect.left_bottom() + egui::vec2(margin, -margin),
        Corner::BottomRight => rect.right_bottom() + egui::vec2(-margin, -margin),
    };
    // World axis → view space: orientation rotates view→world, so its conjugate
    // maps world→view. Screen y is down, hence the -y. View +z points at the eye.
    let to_view = camera.orientation.conjugate();
    let axes = [
        (glam::Vec3::X, egui::Color32::from_rgb(235, 70, 70), "x"),
        (glam::Vec3::Y, egui::Color32::from_rgb(70, 210, 70), "y"),
        (glam::Vec3::Z, egui::Color32::from_rgb(90, 120, 255), "z"),
    ];
    let mut drawn: Vec<(f32, egui::Pos2, egui::Color32, &str)> = axes
        .iter()
        .map(|&(dir, col, lbl)| {
            let v = to_view * dir;
            let tip = center + egui::vec2(v.x, -v.y) * len;
            (v.z, tip, col, lbl)
        })
        .collect();
    // Draw far axes first so near ones overlap them.
    drawn.sort_by(|a, b| a.0.total_cmp(&b.0));

    let painter = ui.painter_at(rect);
    for &(_, tip, col, lbl) in &drawn {
        painter.line_segment([center, tip], egui::Stroke::new(2.0, col));
        // Small head + label, set a constant gap beyond the tip (so foreshortened
        // axes don't bunch their labels against the gizmo).
        painter.circle_filled(tip, 2.0, col);
        let dir = tip - center;
        let label_pos = if dir.length() > 1e-3 {
            tip + dir.normalized() * label_gap
        } else {
            tip + egui::vec2(0.0, -label_gap)
        };
        painter.text(
            label_pos,
            egui::Align2::CENTER_CENTER,
            lbl,
            egui::FontId::proportional(13.0),
            col,
        );
    }
}

/// Draw a small vector icon depicting a representation style into `rect`.
fn paint_style_icon(painter: &egui::Painter, rect: egui::Rect, kind: RepKind, color: egui::Color32) {
    use egui::{Pos2, Stroke, Vec2};
    let c = rect.center();
    let r = rect.height() * 0.5;
    let hw = rect.width() * 0.5;
    match kind {
        RepKind::Vdw => {
            painter.circle_filled(c, r * 0.9, color);
        }
        RepKind::BallAndStick => {
            let off = Vec2::new(hw * 0.6, 0.0);
            painter.line_segment([c - off, c + off], Stroke::new(r * 0.5, color));
            painter.circle_filled(c - off, r * 0.45, color);
            painter.circle_filled(c + off, r * 0.45, color);
        }
        RepKind::Licorice => {
            let off = Vec2::new(hw * 0.55, 0.0);
            let rod = r * 0.55;
            painter.line_segment([c - off, c + off], Stroke::new(rod * 2.0, color));
            painter.circle_filled(c - off, rod, color);
            painter.circle_filled(c + off, rod, color);
        }
        RepKind::Lines => {
            // An irregular squiggle (uneven node spacing + heights), so it reads
            // as a hand-drawn line rather than a regular "M"/"W".
            let s = Stroke::new(1.5, color);
            let nodes = [
                (-0.95_f32, 0.30_f32),
                (-0.55, -0.50),
                (-0.10, 0.10),
                (0.35, -0.30),
                (0.62, 0.45),
                (0.95, -0.15),
            ];
            let pts: Vec<Pos2> = nodes
                .iter()
                .map(|&(x, y)| Pos2::new(c.x + x * hw, c.y + y * r))
                .collect();
            for seg in pts.windows(2) {
                painter.line_segment([seg[0], seg[1]], s);
            }
        }
        RepKind::Cartoon => {
            // A flat β-arrow: rectangular body + triangular head.
            let bh = r * 0.42;
            let x0 = c.x - hw * 0.92;
            let xbody = c.x + hw * 0.15;
            painter.rect_filled(
                egui::Rect::from_min_max(Pos2::new(x0, c.y - bh), Pos2::new(xbody, c.y + bh)),
                0.0,
                color,
            );
            let head = vec![
                Pos2::new(xbody, c.y - bh * 2.0),
                Pos2::new(c.x + hw * 0.92, c.y),
                Pos2::new(xbody, c.y + bh * 2.0),
            ];
            painter.add(egui::Shape::convex_polygon(head, color, Stroke::NONE));
        }
        RepKind::Surface => {
            // A blob: three overlapping circles that fuse into one smooth outline.
            painter.circle_filled(c - Vec2::new(hw * 0.42, -r * 0.10), r * 0.62, color);
            painter.circle_filled(c + Vec2::new(hw * 0.40, r * 0.18), r * 0.58, color);
            painter.circle_filled(c + Vec2::new(hw * 0.02, -r * 0.30), r * 0.50, color);
        }
    }
}

/// A clickable icon+label row inside the style dropdown. Returns true if clicked.
fn style_option(ui: &mut egui::Ui, kind: RepKind, selected: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(150.0, 22.0), egui::Sense::click());
    if selected || resp.hovered() {
        ui.painter()
            .rect_filled(rect, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let color = ui.visuals().text_color();
    let icon_rect =
        egui::Rect::from_min_size(rect.left_top() + egui::vec2(4.0, 2.0), egui::vec2(26.0, 18.0));
    paint_style_icon(ui.painter(), icon_rect, kind, color);
    ui.painter().text(
        egui::pos2(icon_rect.right() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        kind.label(),
        egui::FontId::proportional(15.0),
        color,
    );
    resp.clicked()
}

/// A dropdown button showing a drawn icon + a text label + a caret. `draw_icon`
/// paints into the given rect; returns the click response (drive a `Popup` off it).
fn picker_button(
    ui: &mut egui::Ui,
    label: &str,
    draw_icon: impl FnOnce(&egui::Painter, egui::Rect),
) -> egui::Response {
    let txt = ui.visuals().text_color();
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), egui::FontId::proportional(14.0), txt);
    let (icon_w, caret_w, pad, gap) = (26.0_f32, 11.0_f32, 5.0_f32, 5.0_f32);
    let w = pad + icon_w + gap + galley.size().x + gap + caret_w + pad;
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
fn overlay_button(ui: &mut egui::Ui, content: &str, active: bool) -> egui::Response {
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

/// A drawn style-icon + label button that opens a dropdown of style options.
fn style_picker(ui: &mut egui::Ui, rep: &mut Representation) {
    let color = ui.visuals().text_color();
    let kind = rep.kind;
    let resp = picker_button(ui, kind.label(), |p, r| paint_style_icon(p, r, kind, color));

    egui::Popup::menu(&resp).show(|ui| {
        for kind in RepKind::ALL {
            if style_option(ui, kind, kind == rep.kind) {
                rep.kind = kind;
                rep.params = RepParams::for_kind(kind);
                rep.geom_dirty = true;
                ui.close();
            }
        }
    });
}

/// Draw a small icon depicting a color scheme into `rect` (uses the scheme's own
/// colors, so unlike the style icons it is not theme-tinted).
fn paint_color_icon(painter: &egui::Painter, rect: egui::Rect, method: ColorMethod) {
    use crate::color;
    use egui::{pos2, Color32, Stroke};
    let rgb3 = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]);
    let rgb4 = |c: [u8; 4]| Color32::from_rgb(c[0], c[1], c[2]);
    // Fill `rect` with a rainbow gradient (background for text-on-rainbow icons).
    let rainbow_bg = || {
        let n = 14usize;
        let w = rect.width() / n as f32;
        for i in 0..n {
            let c = color::rainbow(i as f32 / (n - 1) as f32);
            let x0 = rect.left() + i as f32 * w;
            painter.rect_filled(
                egui::Rect::from_min_max(pos2(x0, rect.top()), pos2(x0 + w + 1.0, rect.bottom())),
                0.0,
                Color32::from_rgb(c[0], c[1], c[2]),
            );
        }
    };

    match method {
        ColorMethod::Element => {
            // CPK dots: carbon (grey), oxygen (red), nitrogen (blue).
            let r = rect.height() * 0.22;
            let y = rect.center().y;
            let w = rect.width();
            for (k, an) in [0.22_f32, 0.5, 0.78].iter().zip([6u8, 8, 7]) {
                painter.circle_filled(pos2(rect.left() + k * w, y), r, rgb4(color::element_color(an)));
            }
        }
        ColorMethod::Chain => {
            // Interlocking colored chain links.
            let r = rect.height() * 0.34;
            let y = rect.center().y;
            let cols = [color::PALETTE[0], color::PALETTE[6], color::PALETTE[2]];
            let step = (rect.width() - r * 2.0) / (cols.len() as f32 - 1.0).max(1.0);
            for (i, c) in cols.iter().enumerate() {
                let x = rect.left() + r + step * i as f32;
                painter.circle_stroke(pos2(x, y), r, Stroke::new(2.0, rgb3(*c)));
            }
        }
        ColorMethod::ResId => {
            // A backbone (horizontal line) with two residues hanging off it: one
            // up-left, one down-right, each a different color (color-by-residue).
            let line = Stroke::new(1.5, Color32::from_gray(180));
            let mid = rect.center().y;
            painter.line_segment(
                [pos2(rect.left() + 2.0, mid), pos2(rect.right() - 2.0, mid)],
                line,
            );
            let x1 = rect.left() + rect.width() * 0.33;
            let top = rect.top() + 2.5;
            painter.line_segment([pos2(x1, mid), pos2(x1, top)], line);
            painter.circle_filled(pos2(x1, top), 2.6, rgb3(color::PALETTE[0]));
            let x2 = rect.left() + rect.width() * 0.67;
            let bot = rect.bottom() - 2.5;
            painter.line_segment([pos2(x2, mid), pos2(x2, bot)], line);
            painter.circle_filled(pos2(x2, bot), 2.6, rgb3(color::PALETTE[3]));
        }
        ColorMethod::ResName => {
            // "ALA" on a rainbow background.
            rainbow_bg();
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "ALA",
                egui::FontId::proportional(9.0),
                Color32::BLACK,
            );
        }
        ColorMethod::Index => {
            // "123" with each digit a different (rainbow) color.
            let digits = ["1", "2", "3"];
            let ts = [0.0_f32, 0.45, 0.85];
            let w = rect.width() / 3.0;
            for i in 0..3 {
                painter.text(
                    pos2(rect.left() + w * (i as f32 + 0.5), rect.center().y),
                    egui::Align2::CENTER_CENTER,
                    digits[i],
                    egui::FontId::proportional(12.0),
                    rgb4(color::rainbow(ts[i])),
                );
            }
        }
        ColorMethod::Beta => {
            // "B" on a rainbow background.
            rainbow_bg();
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "B",
                egui::FontId::proportional(12.0),
                Color32::BLACK,
            );
        }
        ColorMethod::SecStruct => {
            // A ribbon: magenta helix body + yellow sheet arrowhead.
            let mid = rect.center().y;
            let h = rect.height() * 0.26;
            let x0 = rect.left() + 1.0;
            let xbody = rect.left() + rect.width() * 0.58;
            painter.rect_filled(
                egui::Rect::from_min_max(pos2(x0, mid - h), pos2(xbody, mid + h)),
                0.0,
                Color32::from_rgb(233, 43, 142),
            );
            let head = vec![
                pos2(xbody, mid - h * 2.0),
                pos2(rect.right() - 1.0, mid),
                pos2(xbody, mid + h * 2.0),
            ];
            painter.add(egui::Shape::convex_polygon(
                head,
                Color32::from_rgb(255, 200, 40),
                Stroke::NONE,
            ));
        }
        ColorMethod::Solid(c) => {
            // A filled swatch of the chosen color, with a subtle border.
            let sw = rect.shrink(2.0);
            painter.rect_filled(sw, 2.0, rgb4(c));
            painter.rect_stroke(
                sw,
                2.0,
                Stroke::new(1.0, Color32::from_gray(90)),
                egui::StrokeKind::Inside,
            );
        }
    }
}

/// A clickable icon+label row inside the color dropdown. Returns its `Response`.
/// When `arrow` is set it draws a right-pointing submenu indicator (used for the
/// `Solid` row, which opens the color submenu).
fn color_option(ui: &mut egui::Ui, method: ColorMethod, selected: bool, arrow: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(172.0, 22.0), egui::Sense::click());
    if selected || resp.hovered() {
        ui.painter()
            .rect_filled(rect, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let icon_rect =
        egui::Rect::from_min_size(rect.left_top() + egui::vec2(4.0, 2.0), egui::vec2(26.0, 18.0));
    paint_color_icon(ui.painter(), icon_rect, method);
    ui.painter().text(
        egui::pos2(icon_rect.right() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        method.label(),
        egui::FontId::proportional(15.0),
        ui.visuals().text_color(),
    );
    if arrow {
        ui.painter().text(
            egui::pos2(rect.right() - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "⏵",
            egui::FontId::proportional(13.0),
            ui.visuals().weak_text_color(),
        );
    }
    resp
}

/// Predefined color swatches for the `Solid` color submenu (6 per row).
const SOLID_SWATCHES: [[u8; 4]; 18] = [
    [230, 50, 50, 255],   // red
    [255, 140, 0, 255],   // orange
    [240, 220, 40, 255],  // yellow
    [60, 200, 80, 255],   // green
    [60, 200, 210, 255],  // cyan
    [60, 110, 240, 255],  // blue
    [150, 80, 220, 255],  // purple
    [220, 60, 200, 255],  // magenta
    [240, 130, 180, 255], // pink
    [30, 150, 150, 255],  // teal
    [170, 220, 40, 255],  // lime
    [40, 60, 150, 255],   // navy
    [245, 245, 245, 255], // white
    [200, 200, 200, 255], // light grey
    [140, 140, 140, 255], // grey
    [80, 80, 80, 255],    // dark grey
    [25, 25, 25, 255],    // near-black
    [150, 90, 50, 255],   // brown
];

/// A small clickable color swatch in the `Solid` submenu grid. Highlights on
/// hover and rings the currently-selected color.
fn swatch_button(ui: &mut egui::Ui, c: [u8; 4], selected: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
    ui.painter()
        .rect_filled(rect, 2.0, egui::Color32::from_rgb(c[0], c[1], c[2]));
    let stroke = if selected {
        egui::Stroke::new(2.0, ui.visuals().selection.bg_fill)
    } else if resp.hovered() {
        egui::Stroke::new(1.5, ui.visuals().widgets.hovered.fg_stroke.color)
    } else {
        egui::Stroke::new(1.0, egui::Color32::from_gray(80))
    };
    ui.painter()
        .rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
    resp.on_hover_text(format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]))
        .clicked()
}

/// The current solid color, or the default if the rep isn't colored by Solid.
fn current_solid(method: ColorMethod) -> [u8; 4] {
    match method {
        ColorMethod::Solid(c) => c,
        _ => crate::color::DEFAULT_SOLID,
    }
}

/// A drawn color-scheme icon + label button that opens a dropdown of options.
/// The built-in schemes pick-and-close; **`Solid` opens a submenu** with a grid of
/// preset swatches and a full color picker (changes are undoable like any other
/// coloring change).
fn color_picker(ui: &mut egui::Ui, rep: &mut Representation) {
    use egui::containers::menu::{MenuConfig, SubMenu};
    let method = rep.color;
    let resp = picker_button(ui, method.label(), |p, r| paint_color_icon(p, r, method));

    egui::Popup::menu(&resp).show(|ui| {
        // The built-in per-atom schemes: pick one and close.
        for m in ColorMethod::ALL {
            if matches!(m, ColorMethod::Solid(_)) {
                continue;
            }
            if color_option(ui, m, m == rep.color, false).clicked() {
                rep.color = m;
                rep.geom_dirty = true;
                ui.close();
            }
        }
        // Solid: a submenu with a preset-swatch grid + a full color picker.
        let active = matches!(rep.color, ColorMethod::Solid(_));
        let header = color_option(ui, ColorMethod::Solid(current_solid(rep.color)), active, true);
        SubMenu::new()
            // Keep the picker usable: clicking inside (e.g. the SV square) must
            // not close the menu — only clicking outside dismisses it.
            .config(MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside))
            .show(ui, &header, |ui| {
                let mut cur = current_solid(rep.color);
                let mut changed = false;
                ui.label(egui::RichText::new("Presets").weak().small());
                egui::Grid::new("solid_presets")
                    .spacing(egui::vec2(3.0, 3.0))
                    .show(ui, |ui| {
                        for (i, &c) in SOLID_SWATCHES.iter().enumerate() {
                            if swatch_button(ui, c, c == cur) {
                                cur = c;
                                changed = true;
                            }
                            if (i + 1) % 6 == 0 {
                                ui.end_row();
                            }
                        }
                    });
                ui.separator();
                let mut col = egui::Color32::from_rgb(cur[0], cur[1], cur[2]);
                if egui::color_picker::color_picker_color32(
                    ui,
                    &mut col,
                    egui::color_picker::Alpha::Opaque,
                ) {
                    cur = [col.r(), col.g(), col.b(), 255];
                    changed = true;
                }
                if changed {
                    rep.color = ColorMethod::Solid(cur);
                    rep.geom_dirty = true;
                }
            });
    });
}

/// Draw a small icon depicting a material: a shaded sphere whose opacity mirrors
/// the material's, with a specular highlight whose size tracks shininess.
fn paint_material_icon(painter: &egui::Painter, rect: egui::Rect, material: Material) {
    use egui::Color32;
    let p = material.params();
    let c = rect.center();
    let r = rect.height() * 0.42;
    let a = ((p.opacity * 0.85 + 0.15) * 255.0) as u8; // keep faint materials visible
    painter.circle_filled(c, r, Color32::from_rgba_unmultiplied(150, 152, 165, a));
    if p.specular > 0.35 {
        let hl = c + egui::vec2(-r * 0.32, -r * 0.34);
        let hr = r * (0.18 + p.shininess * 0.18);
        painter.circle_filled(hl, hr, Color32::from_white_alpha(235));
    }
}

/// A clickable icon+label row inside the material dropdown. Returns true if clicked.
fn material_option(ui: &mut egui::Ui, material: Material, selected: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(150.0, 22.0), egui::Sense::click());
    if selected || resp.hovered() {
        ui.painter()
            .rect_filled(rect, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let icon_rect =
        egui::Rect::from_min_size(rect.left_top() + egui::vec2(4.0, 2.0), egui::vec2(26.0, 18.0));
    paint_material_icon(ui.painter(), icon_rect, material);
    ui.painter().text(
        egui::pos2(icon_rect.right() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        material.label(),
        egui::FontId::proportional(15.0),
        ui.visuals().text_color(),
    );
    resp.clicked()
}

/// A drawn material icon + label button that opens a dropdown of materials.
/// A material change forces a geometry rebuild (the opacity/lighting are baked
/// per geometry element).
fn material_picker(ui: &mut egui::Ui, rep: &mut Representation) {
    let material = rep.material;
    let resp = picker_button(ui, material.label(), |p, r| paint_material_icon(p, r, material));

    egui::Popup::menu(&resp).show(|ui| {
        for material in Material::ALL {
            if material_option(ui, material, material == rep.material) {
                rep.material = material;
                rep.geom_dirty = true;
                ui.close();
            }
        }
    });
}

/// Parameter controls for a representation, shown inline under its row as a tidy
/// two-column table (parameter name on the left, control on the right).
/// Returns `true` if a render-only change was made (periodic-image params) so the
/// caller can flag the viewport dirty; geometry changes set `rep.geom_dirty`
/// directly. `has_box` gates the **Periodic** tab (only meaningful with a box).
fn draw_rep_params(ui: &mut egui::Ui, rep: &mut Representation, has_box: bool) -> bool {
    let mut view_dirty = false;
    // The Periodic tab only exists when the molecule has a box; if it was the
    // active tab and the box went away, fall back to Style.
    if !has_box && rep.settings_tab == SettingsTab::Periodic {
        rep.settings_tab = SettingsTab::Style;
    }
    // Tab bar: [Style] [Traj] [Periodic?] — underline-style tabs (selected = bold
    // text with an accent underline) rather than disconnected toggle buttons.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        let mut tabs = vec![(SettingsTab::Style, "Style"), (SettingsTab::Traj, "Traj")];
        if has_box {
            tabs.push((SettingsTab::Periodic, "Periodic"));
        }
        for (tab, label) in tabs {
            let selected = rep.settings_tab == tab;
            let txt = if selected {
                egui::RichText::new(label).strong()
            } else {
                egui::RichText::new(label).color(ui.visuals().weak_text_color())
            };
            let resp = ui
                .add(egui::Label::new(txt).sense(egui::Sense::click()))
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.clicked() {
                rep.settings_tab = tab;
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
    ui.separator();
    match rep.settings_tab {
        SettingsTab::Traj => {
            draw_traj_tab(ui, rep);
            return view_dirty;
        }
        SettingsTab::Periodic => {
            view_dirty |= draw_periodic_tab(ui, rep);
            return view_dirty;
        }
        SettingsTab::Style => {}
    }

    // --- [Style] tab: per-style geometry parameters. ---
    let mut changed = false;
    egui::Grid::new("rep_params")
        .num_columns(2)
        .spacing(egui::vec2(8.0, 4.0))
        .show(ui, |ui| match &mut rep.params {
            RepParams::Vdw { scale } => {
                ui.label("Sphere scale");
                changed |= ui
                    .add(egui::Slider::new(scale, 0.1..=2.0).text("× VDW radius"))
                    .changed();
                ui.end_row();
            }
            RepParams::Lines { width } => {
                ui.label("Line width (px)");
                changed |= ui.add(egui::Slider::new(width, 1.0..=10.0)).changed();
                ui.end_row();
            }
            RepParams::Licorice { bond_radius } => {
                ui.label("Bond radius (nm)");
                changed |= ui.add(egui::Slider::new(bond_radius, 0.005..=0.10)).changed();
                ui.end_row();
            }
            RepParams::BallAndStick { sphere_scale, bond_radius } => {
                ui.label("Sphere scale");
                changed |= ui.add(egui::Slider::new(sphere_scale, 0.05..=0.6)).changed();
                ui.end_row();
                ui.label("Bond radius (nm)");
                changed |= ui.add(egui::Slider::new(bond_radius, 0.005..=0.05)).changed();
                ui.end_row();
            }
            RepParams::Cartoon { coil_radius, ribbon_width, ribbon_thickness } => {
                ui.label("Coil radius (nm)");
                changed |= ui.add(egui::Slider::new(coil_radius, 0.02..=0.08)).changed();
                ui.end_row();
                ui.label("Ribbon width (nm)");
                changed |= ui.add(egui::Slider::new(ribbon_width, 0.05..=0.35)).changed();
                ui.end_row();
                ui.label("Ribbon thickness (nm)");
                changed |= ui.add(egui::Slider::new(ribbon_thickness, 0.02..=0.10)).changed();
                ui.end_row();
            }
            RepParams::Surface { probe, quality, smoothing } => {
                ui.label("Probe radius (nm)");
                changed |= ui.add(egui::Slider::new(probe, 0.0..=0.3)).changed();
                ui.end_row();
                ui.label("Quality");
                changed |= ui.add(egui::Slider::new(quality, 0..=4)).changed();
                ui.end_row();
                ui.label("Smoothing");
                changed |= ui.add(egui::Slider::new(smoothing, 0..=5)).changed();
                ui.end_row();
            }
        });

    // Secondary-structure algorithm — used by the Cartoon shape and the
    // "Structure" color scheme; offer the two sensible choices.
    if matches!(rep.kind, RepKind::Cartoon) || rep.color == ColorMethod::SecStruct {
        let label = match rep.ss_algo {
            SsAlgorithm::Dssp => "DSSP",
            SsAlgorithm::DsspGmx => "DSSP (gmx)",
            SsAlgorithm::Dss => "dss (PyMOL)",
        };
        ui.horizontal(|ui| {
            ui.label("SS algorithm");
            egui::ComboBox::from_id_salt("ss_algo")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(&mut rep.ss_algo, SsAlgorithm::Dssp, "DSSP")
                        .changed();
                    changed |= ui
                        .selectable_value(&mut rep.ss_algo, SsAlgorithm::Dss, "dss (PyMOL)")
                        .changed();
                });
        });
    }

    // Restore this style's default parameters.
    ui.add_space(2.0);
    if ui
        .button(format!("{}  Defaults", icon::ARROW_COUNTER_CLOCKWISE))
        .on_hover_text("Restore default parameters for this style")
        .clicked()
    {
        rep.params = RepParams::for_kind(rep.kind);
        changed = true;
    }

    if changed {
        rep.geom_dirty = true;
    }
    view_dirty
}

/// [Periodic] tab: render copies of the selection shifted by integer combinations
/// of the box lattice vectors `a,b,c`. Returns true if anything changed (render-only
/// — no geometry rebuild, the images are drawn under a translated camera). Only
/// shown when the molecule has a box.
fn draw_periodic_tab(ui: &mut egui::Ui, rep: &mut Representation) -> bool {
    let p = &mut rep.periodic;
    let mut changed = false;
    ui.horizontal(|ui| {
        changed |= ui
            .checkbox(&mut p.self_img, "Self")
            .on_hover_text("Show the central (un-shifted) copy")
            .changed();
        changed |= ui
            .checkbox(&mut p.show_box, "Box")
            .on_hover_text("Draw the periodic box wireframe at every shown image")
            .changed();
    });
    ui.add_space(2.0);
    // One row per axis: [−n] −x  [+n] +x  (counts of images along ±a, ±b, ±c).
    egui::Grid::new("periodic_images")
        .num_columns(4)
        .spacing(egui::vec2(6.0, 4.0))
        .show(ui, |ui| {
            for (axis, name) in [(0usize, "x"), (1, "y"), (2, "z")] {
                changed |= ui
                    .add(egui::DragValue::new(&mut p.neg[axis]).range(0..=8))
                    .changed();
                ui.label(format!("−{name}"));
                changed |= ui
                    .add(egui::DragValue::new(&mut p.pos[axis]).range(0..=8))
                    .changed();
                ui.label(format!("+{name}"));
                ui.end_row();
            }
        });
    changed
}

/// [Traj] tab of the representation settings: per-frame behavior.
fn draw_traj_tab(ui: &mut egui::Ui, rep: &mut Representation) {
    ui.checkbox(&mut rep.dynamic, "Update every frame").on_hover_text(
        "Re-evaluate the selection on every trajectory frame — needed for \
         coordinate-dependent selections like `within …`.",
    );
    // Per-frame secondary structure (Cartoon shape / SecStruct coloring only).
    if matches!(rep.kind, RepKind::Cartoon) || rep.color == ColorMethod::SecStruct {
        ui.checkbox(&mut rep.ss_per_frame, "Recompute SS every frame")
            .on_hover_text(
                "Off: compute secondary structure once and reuse it across frames \
                 (fast). On: recompute DSSP each trajectory frame (slower, but \
                 follows conformational changes).",
            );
    }
    // Trajectory smoothing: render a Savitzky–Golay blend of nearby frames. The
    // window is odd (1 = off, 3, 5, 7, …); stepped via the half-width but shown as
    // the window count.
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Smooth window");
        let mut half = rep.smooth_window.saturating_sub(1) / 2;
        let resp = ui
            .add(
                egui::DragValue::new(&mut half)
                    .range(0..=15)
                    .speed(0.05)
                    .custom_formatter(|n, _| format!("{}", (n as i64) * 2 + 1))
                    .custom_parser(|s| s.parse::<f64>().ok().map(|w| ((w - 1.0) / 2.0).max(0.0))),
            )
            .on_hover_text(
                "Render the trajectory smoothed over this many adjacent frames \
                 (odd; 1 = off): a local-polynomial (Savitzky–Golay) blend of \
                 neighbouring frames, shrunk gracefully at the trajectory ends.",
            );
        if resp.changed() {
            rep.smooth_window = half * 2 + 1;
            // Coords-only change → incremental rebuild (no DSSP / realloc).
            rep.coords_dirty = true;
        }
    });
}

/// Draw the VMD-style trajectory control bar (buttons + frame field + loop/speed)
/// and the frame slider on its own row. Returns true if the displayed frame
/// changed (so the caller re-applies the state and re-renders). Caller ensures
/// the trajectory has playback (>1 frame).
fn draw_traj_bar(ui: &mut egui::Ui, traj: &mut Trajectory) -> bool {
    let n = traj.n_frames();
    if n < 2 {
        return false;
    }
    let last = n - 1;
    let before = traj.current;

    ui.horizontal(|ui| {
        compact_actions(ui);
        if icon_button(ui, icon::SKIP_BACK, "First frame").clicked() {
            traj.set_playing(false);
            traj.set_current(0);
        }
        if icon_button(ui, icon::CARET_LEFT, "Step back").clicked() {
            traj.set_playing(false);
            traj.step(-1);
        }
        let play_glyph = if traj.playing { icon::PAUSE } else { icon::PLAY };
        if ui
            .selectable_label(traj.playing, play_glyph)
            .on_hover_text(if traj.playing { "Pause" } else { "Play" })
            .clicked()
        {
            traj.set_playing(!traj.playing);
        }
        if icon_button(ui, icon::CARET_RIGHT, "Step forward").clicked() {
            traj.set_playing(false);
            traj.step(1);
        }
        if icon_button(ui, icon::SKIP_FORWARD, "Last frame").clicked() {
            traj.set_playing(false);
            traj.set_current(last);
        }

        // Editable current-frame field + total.
        let mut cur = traj.current;
        if ui
            .add(egui::DragValue::new(&mut cur).range(0..=last))
            .on_hover_text("Current frame")
            .changed()
        {
            traj.set_current(cur);
        }
        ui.weak(format!("/ {last}"));

        // Loop / once toggle.
        let looping = traj.loop_mode == LoopMode::Loop;
        if ui
            .selectable_label(looping, icon::REPEAT)
            .on_hover_text(if looping {
                "Looping (click for play-once)"
            } else {
                "Play once (click to loop)"
            })
            .clicked()
        {
            traj.loop_mode = if looping { LoopMode::Once } else { LoopMode::Loop };
        }

        // Playback speed (frames per second).
        ui.add(
            egui::DragValue::new(&mut traj.speed_fps)
                .range(1.0..=120.0)
                .suffix(" fps")
                .fixed_decimals(0),
        )
        .on_hover_text("Playback speed");
    });

    // Slider on its own row, filling the width.
    let mut cur = traj.current;
    let resp = ui.add(egui::Slider::new(&mut cur, 0..=last).show_value(false));
    if resp.changed() {
        traj.set_playing(false);
        traj.set_current(cur);
    }
    if let Some(t) = traj.current_time() {
        resp.on_hover_text(format!("frame {} — t = {:.3}", traj.current, t));
    }

    traj.current != before
}

pub struct App {
    renderer: SceneRenderer,
    camera: Camera,
    scene: Scene,
    /// Style used for the initial representation of each loaded molecule.
    default_rep: RepKind,
    /// Camera at the last 3D render; `None` forces a render.
    last_render_camera: Option<Camera>,
    last_size: [u32; 2],
    /// Set when visibility/structure changed in a way the camera/geometry flags
    /// don't capture (forces one re-render).
    view_dirty: bool,
    status: String,
    history: History,
    /// Number of steps to undo/redo this frame (set by keyboard or the toolbar
    /// dropdowns), applied after the panel is drawn.
    pending_undo_n: Option<usize>,
    pending_redo_n: Option<usize>,
    /// `(molecule index, rep index)` whose selection field is focused/expanded.
    editing_rep: Option<(usize, usize)>,
    /// Open trajectory-load dialog, if any (one at a time).
    load_dialog: Option<LoadDialog>,
    /// In-flight background trajectory loaders, keyed by molecule (so they
    /// survive reorder/delete/undo). Drained each frame via `try_recv`.
    loaders: HashMap<MolId, Receiver<LoadMsg>>,
    /// Picking mode (top view-toolbar dropdown). `HoverInfo` shows the hovered
    /// atom's identity + real coords and glows its outline; `Lasso` drags a
    /// freehand selection polygon.
    pick_mode: PickMode,
    /// How a lasso expands its hit atoms (viewport-overlay dropdown): exact atoms,
    /// whole residues, or heavy atoms + their bonded hydrogens.
    selection_mode: SelectionMode,
    /// In-progress lasso polygon (viewport pixel coords), accumulated while
    /// dragging in `PickMode::Lasso`. Empty when not lassoing. Transient view state.
    lasso_path: Vec<egui::Pos2>,
    /// Whether the VMD-style orientation axes gizmo is shown in the viewport.
    axes_on: bool,
    /// Which viewport corner the axes gizmo is anchored to.
    axes_corner: Corner,
    /// Browser file-open channel: the async `<input type=file>` picker reads the
    /// chosen file and sends `(filename, bytes)` here; `ui()` drains it and loads
    /// the structure. Cloned per pick; the receiver is polled each frame. Wasm only.
    #[cfg(target_arch = "wasm32")]
    file_tx: std::sync::mpsc::Sender<(String, Vec<u8>)>,
    #[cfg(target_arch = "wasm32")]
    file_rx: std::sync::mpsc::Receiver<(String, Vec<u8>)>,
    /// Browser trajectory-load channel: the picker sends `(molecule, filename,
    /// bytes)` here; `ui()` drains it into an incremental [`data::traj_wasm::TrajStream`]
    /// per molecule (in `wasm_loaders`), whose frames are streamed into the
    /// trajectory a batch per frame. Wasm only.
    #[cfg(target_arch = "wasm32")]
    traj_tx: std::sync::mpsc::Sender<(MolId, String, Vec<u8>)>,
    #[cfg(target_arch = "wasm32")]
    traj_rx: std::sync::mpsc::Receiver<(MolId, String, Vec<u8>)>,
    #[cfg(target_arch = "wasm32")]
    wasm_loaders: HashMap<MolId, data::traj_wasm::TrajStream>,
}

/// A viewport corner, for anchoring the axes gizmo.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// State of the "Load trajectory" modal.
struct LoadDialog {
    mol_id: MolId,
    path: Option<PathBuf>,
    from: usize,
    /// Whether `to` bounds the read (else read to end of file).
    to_enabled: bool,
    to: usize,
    stride: usize,
    mode: LoadMode,
    error: Option<String>,
}

impl LoadDialog {
    fn new(mol_id: MolId) -> Self {
        Self {
            mol_id,
            path: None,
            from: 0,
            to_enabled: false,
            to: 0,
            stride: 1,
            mode: LoadMode::Sync,
            error: None,
        }
    }
}

/// Outcome of drawing the load dialog this frame.
enum DialogAction {
    Keep,
    Cancel,
    Load,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        launch: AppLaunch,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        crate::theme::apply(&cc.egui_ctx);

        let render_state = cc
            .wgpu_render_state
            .as_ref()
            .ok_or("wgpu render state unavailable (eframe must use the wgpu backend)")?;
        let renderer = SceneRenderer::new(render_state);

        // VMD's default style for a new molecule is Lines; override for headless
        // checks with MOLAR_VIS_DEBUG_REP=vdw|licorice|ballstick|lines.
        let default_rep = std::env::var("MOLAR_VIS_DEBUG_REP")
            .ok()
            .and_then(|s| RepKind::from_name(&s))
            .unwrap_or(RepKind::Lines);

        let mut scene = Scene::default();
        let mut status = String::new();
        for path in &launch.files {
            match data::load(path) {
                Ok(raw) => {
                    scene.add(raw, default_rep);
                }
                Err(e) => {
                    log::error!("{e}");
                    status = e;
                }
            }
        }
        if !scene.molecules.is_empty() {
            scene.selected_mol = Some(0);
            status = format!("{} molecule(s) loaded", scene.molecules.len());
        } else if status.is_empty() {
            status = "No molecules loaded.".to_string();
        }

        // Verification hook: MOLAR_VIS_DEBUG_SEL=<selection> overrides the initial
        // selection of every molecule's first rep (e.g. "name CA", "protein").
        if let Ok(sel) = std::env::var("MOLAR_VIS_DEBUG_SEL") {
            for mol in &mut scene.molecules {
                if let Some(rep) = mol.reps.first_mut() {
                    rep.sel_text = sel.clone();
                    rep.sel_dirty = true;
                }
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_COLOR sets the first rep's color scheme.
        if let Some(cm) = std::env::var("MOLAR_VIS_DEBUG_COLOR").ok().and_then(|c| {
            match c.to_ascii_lowercase().as_str() {
                "element" => Some(ColorMethod::Element),
                "chain" => Some(ColorMethod::Chain),
                "resid" => Some(ColorMethod::ResId),
                "resname" => Some(ColorMethod::ResName),
                "index" => Some(ColorMethod::Index),
                "beta" => Some(ColorMethod::Beta),
                "secstruct" | "structure" | "ss" => Some(ColorMethod::SecStruct),
                "solid" => Some(ColorMethod::Solid(crate::color::DEFAULT_SOLID)),
                _ => None,
            }
        }) {
            for mol in &mut scene.molecules {
                if let Some(rep) = mol.reps.first_mut() {
                    rep.color = cm;
                }
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_SMOOTH=<window> sets mol 0's first rep
        // trajectory smoothing window (odd; needs MOLAR_VIS_DEBUG_TRAJ to do anything).
        if let Ok(w) = std::env::var("MOLAR_VIS_DEBUG_SMOOTH") {
            if let Ok(w) = w.trim().parse::<u32>() {
                if let Some(rep) = scene.molecules.first_mut().and_then(|m| m.reps.first_mut()) {
                    rep.smooth_window = w.max(1) | 1;
                }
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_MATERIAL sets the first rep's material.
        if let Some(mat) = std::env::var("MOLAR_VIS_DEBUG_MATERIAL").ok().and_then(|m| {
            Material::ALL
                .into_iter()
                .find(|x| x.label().eq_ignore_ascii_case(&m))
        }) {
            for mol in &mut scene.molecules {
                if let Some(rep) = mol.reps.first_mut() {
                    rep.material = mat;
                }
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_ALLCOLORS lays out one rep per color
        // scheme (cycling styles) so every style/color icon is visible at once.
        if std::env::var("MOLAR_VIS_DEBUG_ALLCOLORS").is_ok() {
            for mol in &mut scene.molecules {
                mol.reps.clear();
                for (i, &cm) in ColorMethod::ALL.iter().enumerate() {
                    let mut rep =
                        Representation::new(crate::geometry::RepKind::ALL[i % 4]);
                    rep.color = cm;
                    mol.reps.push(rep);
                }
                mol.selected_rep = Some(0);
            }
        }

        // Verification hook: MOLAR_VIS_DEBUG_TRAJ=<path> loads a trajectory into
        // the first molecule (sync), bypassing the dialog; MOLAR_VIS_DEBUG_FRAME=<n>
        // selects the displayed frame so headless screenshots can confirm motion.
        #[cfg(not(target_arch = "wasm32"))]
        if let Ok(traj_path) = std::env::var("MOLAR_VIS_DEBUG_TRAJ") {
            if let Some(mol) = scene.molecules.first_mut() {
                mol.seed_frame0();
                let envn = |k: &str| std::env::var(k).ok().and_then(|s| s.parse::<usize>().ok());
                let opts = crate::trajectory::LoadOptions {
                    from: envn("MOLAR_VIS_DEBUG_TRAJ_FROM").unwrap_or(0),
                    to: envn("MOLAR_VIS_DEBUG_TRAJ_TO"),
                    stride: envn("MOLAR_VIS_DEBUG_TRAJ_STRIDE").unwrap_or(1),
                };
                match data::traj_loader::read_frames_sync(
                    std::path::Path::new(&traj_path),
                    &opts,
                    mol.n_atoms,
                ) {
                    Ok(frames) => {
                        mol.append_frames(frames);
                        let frame = std::env::var("MOLAR_VIS_DEBUG_FRAME")
                            .ok()
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(0);
                        mol.trajectory.set_current(frame);
                        if std::env::var("MOLAR_VIS_DEBUG_TRAJ_PLAY").is_ok() {
                            mol.trajectory.set_playing(true);
                        }
                        mol.apply_current_frame();
                        log::info!(
                            "debug trajectory: {} frames, showing {}",
                            mol.trajectory.n_frames(),
                            mol.trajectory.current
                        );
                    }
                    Err(e) => log::error!("debug trajectory load failed: {e}"),
                }
            }
        }

        // Verification hook: MOLAR_VIS_DEBUG_BOX=1 shows the periodic box on mol 0.
        if std::env::var("MOLAR_VIS_DEBUG_BOX").is_ok() {
            if let Some(mol) = scene.molecules.first_mut() {
                mol.show_box = true;
                mol.box_dirty = true;
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_PBC="px,py,pz" sets the +a/+b/+c periodic
        // image counts on mol 0's first rep (and shows the box), exercising the
        // dynamic-camera image rendering headlessly.
        if let Ok(spec) = std::env::var("MOLAR_VIS_DEBUG_PBC") {
            let n: Vec<u32> = spec.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if let Some(mol) = scene.molecules.first_mut() {
                if let Some(rep) = mol.reps.first_mut() {
                    rep.periodic.pos = [
                        n.first().copied().unwrap_or(0),
                        n.get(1).copied().unwrap_or(0),
                        n.get(2).copied().unwrap_or(0),
                    ];
                    rep.periodic.show_box = true;
                }
            }
        }

        let mut camera = match scene.bbox() {
            Some((min, max)) => Camera::frame_bbox(min, max),
            None => Camera::default(),
        };
        if let Ok(deg) = std::env::var("MOLAR_VIS_DEBUG_ORBIT") {
            if let Ok(d) = deg.parse::<f32>() {
                camera.orbit(d, d * 0.4);
            }
        }
        if std::env::var("MOLAR_VIS_DEBUG_ORTHO").is_ok() {
            camera.projection = Projection::Orthographic;
        }
        // Verification hook: MOLAR_VIS_DEBUG_FOCUS=<selection> zooms the camera to
        // fit that selection of mol 0 (exercises the zoom-to-selection path).
        if let Ok(sel_text) = std::env::var("MOLAR_VIS_DEBUG_FOCUS") {
            if let Some(mol) = scene.molecules.first() {
                if let Ok((_, sel)) = scene::evaluate(&mol.system, &sel_text) {
                    let (min, max) = mol.sel_bbox(&sel);
                    camera.focus_bbox(min, max);
                }
            }
        }

        // Verification hook: MOLAR_VIS_DEBUG_PENDING=<selection> stages that selection
        // as an active (pending) selection on **every** molecule — exercises the lasso
        // glow + per-molecule accept/discard UI (incl. the multi-molecule case) without
        // simulating a mouse drag.
        if let Ok(sel_text) = std::env::var("MOLAR_VIS_DEBUG_PENDING") {
            for mol in &mut scene.molecules {
                if let Ok((_, sel)) = scene::evaluate(&mol.system, &sel_text) {
                    let atoms: Vec<usize> = {
                        let bound = mol.system.bind(&sel);
                        bound.iter_particle().map(|p| p.id).collect()
                    };
                    if atoms.is_empty() {
                        continue;
                    }
                    mol.pending = Some(scene::PendingSelection { sel_text: sel_text.clone(), atoms });
                    mol.reps_open = true;
                    mol.glow_dirty = true;
                }
            }
        }

        let history = History::new(EditState::capture(&scene));

        // Verification hook: MOLAR_VIS_DEBUG_PARAMS=1 opens the first rep's gear panel.
        if std::env::var("MOLAR_VIS_DEBUG_PARAMS").is_ok() {
            if let Some(rep) = scene.molecules.first_mut().and_then(|m| m.reps.first_mut()) {
                rep.params_open = true;
            }
        }

        // Browser file-open + trajectory-load channels (the async pickers send
        // their bytes back here for `ui()` to process).
        #[cfg(target_arch = "wasm32")]
        let (file_tx, file_rx) = std::sync::mpsc::channel::<(String, Vec<u8>)>();
        #[cfg(target_arch = "wasm32")]
        let (traj_tx, traj_rx) = std::sync::mpsc::channel::<(MolId, String, Vec<u8>)>();

        Ok(Self {
            renderer,
            camera,
            scene,
            default_rep,
            last_render_camera: None,
            last_size: [0, 0],
            view_dirty: true,
            status,
            history,
            pending_undo_n: None,
            pending_redo_n: None,
            editing_rep: None,
            load_dialog: None,
            loaders: HashMap::new(),
            // MOLAR_VIS_DEBUG_PICK forces hover-info on (and picks at the viewport
            // center each frame; see draw_viewport) for headless verification.
            pick_mode: if std::env::var("MOLAR_VIS_DEBUG_PICK").is_ok() {
                PickMode::HoverInfo
            } else {
                PickMode::default()
            },
            selection_mode: match std::env::var("MOLAR_VIS_DEBUG_SELMODE").as_deref() {
                Ok("residues") => SelectionMode::Residues,
                Ok("boundh") => SelectionMode::BoundH,
                _ => SelectionMode::default(),
            },
            lasso_path: Vec::new(),
            axes_on: std::env::var("MOLAR_VIS_DEBUG_AXES").is_ok(),
            axes_corner: Corner::BottomRight,
            #[cfg(target_arch = "wasm32")]
            file_tx,
            #[cfg(target_arch = "wasm32")]
            file_rx,
            #[cfg(target_arch = "wasm32")]
            traj_tx,
            #[cfg(target_arch = "wasm32")]
            traj_rx,
            #[cfg(target_arch = "wasm32")]
            wasm_loaders: HashMap::new(),
        })
    }

    /// Recompile dirty selections and rebuild/reupload dirty geometry. Returns
    /// true if any geometry was uploaded (so the frame needs re-rendering).
    fn rebuild_dirty(&mut self, rs: &eframe::egui_wgpu::RenderState) -> bool {
        let mut changed = false;
        for mol in &mut self.scene.molecules {
            let any_rep_dirty = mol
                .reps
                .iter()
                .any(|r| r.sel_dirty || r.geom_dirty || r.coords_dirty);
            if !any_rep_dirty && !(mol.show_box && mol.box_dirty) && !mol.glow_dirty {
                continue;
            }
            // The coordinates to render: the current trajectory frame, read by
            // reference (no copy into the System), or the static structure state.
            let render_state: &State = match mol.trajectory.frames.get(mol.trajectory.current) {
                Some(frame) => frame,
                None => mol.system.state(),
            };
            let n_atoms = mol.n_atoms;
            // Whether any rep's geometry was (re)built this pass — if so and there's
            // an active selection, its glow must follow the new style/coords.
            let mut rep_geom_changed = false;
            for rep in &mut mol.reps {
                if rep.sel_dirty {
                    // Parse + evaluate the selection (against the System's own
                    // state). On error keep the previous selection/geometry and
                    // just surface the message.
                    match scene::evaluate(&mol.system, rep.sel_text.as_str()) {
                        Ok((expr, sel)) => {
                            rep.expr = Some(expr);
                            rep.sel = Some(sel);
                            rep.sel_error = None;
                            rep.sel_empty = false;
                            rep.geom_dirty = true;
                        }
                        // Valid selection that matches no atoms: not an error — drop
                        // the geometry (render nothing), keep the text, and flag the
                        // field. The viewport must re-render to clear the old mesh.
                        Err(scene::EvalError::Empty) => {
                            rep.expr = None;
                            rep.sel = None;
                            rep.sel_error = None;
                            rep.sel_empty = true;
                            rep.gpu = Default::default();
                            changed = true;
                        }
                        Err(scene::EvalError::Invalid(e)) => {
                            rep.sel_error = Some(e);
                            rep.sel_empty = false;
                        }
                    }
                    rep.sel_dirty = false;
                }
                let Some(sel) = &rep.sel else {
                    rep.geom_dirty = false;
                    rep.coords_dirty = false;
                    continue;
                };

                // Trajectory smoothing: a transient Savitzky–Golay blend of the
                // frames around `current`, computed here and dropped after the
                // build (nothing stored). Falls back to the raw current frame.
                let smoothed = (rep.smooth_window > 1)
                    .then(|| mol.trajectory.smoothed_state(rep.smooth_window))
                    .flatten();
                let state: &State = smoothed.as_ref().unwrap_or(render_state);

                if rep.geom_dirty {
                    // Full structural rebuild: (re)compute secondary structure
                    // into the cache, build geometry, recreate GPU buffers.
                    let (geom, fresh_ss) = {
                        let bound = mol.system.bind_with_state(sel, state);
                        let ss = geometry::needs_ss(&rep.params, rep.color)
                            .then(|| SsMap::compute(&bound, rep.ss_algo));
                        let geom = geometry::build(
                            &bound, n_atoms, &mol.bonds, &rep.params, rep.color, rep.material,
                            ss.as_ref(),
                        );
                        (geom, ss)
                    };
                    rep.ss_cache = fresh_ss;
                    rep.gpu = self.renderer.upload(rs, &geom);
                    rep.geom_dirty = false;
                    rep.coords_dirty = false;
                    changed = true;
                    rep_geom_changed = true;
                } else if rep.coords_dirty {
                    // Coordinates-only frame change: rebuild geometry reusing the
                    // cached secondary structure (no DSSP), then update the
                    // existing GPU buffers in place (no reallocation).
                    let geom = {
                        let bound = mol.system.bind_with_state(sel, state);
                        geometry::build(
                            &bound, n_atoms, &mol.bonds, &rep.params, rep.color, rep.material,
                            rep.ss_cache.as_ref(),
                        )
                    };
                    self.renderer.update(rs, &mut rep.gpu, &geom);
                    rep.coords_dirty = false;
                    changed = true;
                    rep_geom_changed = true;
                }
            }
            // Periodic-box wireframe: (re)build when dirty, regardless of whether
            // it's currently shown — both the molecule-level box toggle *and* a
            // rep's periodic `Box` toggle draw this geometry, and the latter isn't
            // tracked by `box_dirty`, so keep `box_gpu` ready whenever a box exists.
            // Use the current frame's box (tracks NPT box changes); fall back to the
            // structure's own box when a trajectory frame carries none.
            if mol.box_dirty {
                let pb = render_state
                    .pbox
                    .as_ref()
                    .or_else(|| mol.system.state().pbox.as_ref());
                let lines = pb.map(geometry::box_wireframe).unwrap_or_default();
                let geom = geometry::GeometryData { lines, ..Default::default() };
                mol.box_gpu = self.renderer.upload(rs, &geom);
                mol.box_dirty = false;
                changed = true;
            }

            // If any rep's geometry was rebuilt (style/selection/coords changed) and
            // there's an active selection, rebuild its glow so it follows the change.
            if rep_geom_changed && mol.pending.is_some() {
                mol.glow_dirty = true;
            }

            // Active-selection glow: rebuild the pending atoms in each rep's own
            // style (so the highlight glows in the current style), or clear it. Runs
            // after the rep loop so Cartoon reps' `ss_cache` is already populated.
            if mol.glow_dirty {
                let geom = match &mol.pending {
                    Some(pending) => build_glow(
                        &mol.system, &mol.bonds, &mol.reps, pending, render_state, n_atoms,
                    ),
                    None => geometry::GeometryData::default(),
                };
                mol.glow_gpu = self.renderer.upload(rs, &geom);
                mol.glow_dirty = false;
                changed = true;
            }
        }
        changed
    }
}

/// Build the active-selection glow geometry for one molecule: for each visible
/// rep, the rep's selection intersected with the `pending` atoms, built in that
/// rep's own style/params, merged into one geometry. The element colors/materials
/// are irrelevant (the glow shaders emit a fixed cyan Fresnel rim), so the rep's
/// own values are reused. Cartoon/SecStruct reps are skipped until their SS cache
/// exists (it's filled by the same `rebuild_dirty` pass, just before this).
fn build_glow(
    system: &molar::prelude::System,
    bonds: &[[usize; 2]],
    reps: &[Representation],
    pending: &scene::PendingSelection,
    state: &State,
    n_atoms: usize,
) -> geometry::GeometryData {
    let Some(index_str) = pick::index_selection_string(&pending.atoms) else {
        return geometry::GeometryData::default();
    };
    let mut out = geometry::GeometryData::default();
    for rep in reps {
        if !rep.visible {
            continue;
        }
        if geometry::needs_ss(&rep.params, rep.color) && rep.ss_cache.is_none() {
            continue;
        }
        // (rep selection) ∩ (pending atoms): glow only this rep's own atoms, in its
        // own style. Skip on an empty/invalid intersection.
        let combined = format!("({}) and ({})", rep.sel_text, index_str);
        let Ok((_, sel)) = scene::evaluate(system, &combined) else {
            continue;
        };
        let bound = system.bind_with_state(&sel, state);
        let mut geom = geometry::build(
            &bound, n_atoms, bonds, &rep.params, rep.color, rep.material,
            rep.ss_cache.as_ref(),
        );
        // Mesh styles (Cartoon/Surface) re-build the glow over the *subset* of
        // selected atoms, so its mesh nearly — but not exactly — coincides with the
        // parent rep's full mesh (the SS-dependent spline smoothing/cleanup diverges
        // at the subset's ends). Two near-coplanar surfaces z-fight, so the glow
        // looked patchy. Push the glow mesh a hair *outward* along its normals into a
        // thin shell just in front of the parent, so it tests cleanly above it. (The
        // glow pass writes no depth, so the back of the shell still fails the depth
        // test and stays hidden.) Impostor glows coincide exactly and need no offset.
        inflate_mesh(&mut geom.mesh, GLOW_INFLATE);
        out.append(geom);
    }
    out
}

/// World-space (nm) outward shell offset for the active-selection glow mesh — large
/// enough to dominate the sub-Ångström divergence between the subset and parent
/// cartoon splines (so no z-fighting), small enough to read as a tight halo.
const GLOW_INFLATE: f32 = 0.025;

/// Offset every mesh vertex outward along its normal by `d` nm (a thin shell).
fn inflate_mesh(mesh: &mut geometry::MeshData, d: f32) {
    for v in &mut mesh.vertices {
        v.pos[0] += v.normal[0] * d;
        v.pos[1] += v.normal[1] * d;
        v.pos[2] += v.normal[2] * d;
    }
}

/// Browser file open: create a hidden `<input type=file>` (limited to `accept`),
/// click it, and when the user picks a file read it (`Blob::array_buffer`) into a
/// `Vec<u8>` and hand `(name, bytes)` to `deliver`, then request a repaint so the
/// app processes it. Async (the dialog + the read), so this returns immediately;
/// `deliver` runs later on the main thread. Used by both the structure-open and
/// trajectory-load buttons (the latter's `deliver` tags the bytes with a molecule).
#[cfg(target_arch = "wasm32")]
fn pick_file(accept: &str, ctx: egui::Context, deliver: impl Fn(String, Vec<u8>) + Clone + 'static) {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast as _;

    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Ok(input) = document
        .create_element("input")
        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().map_err(|_| wasm_bindgen::JsValue::NULL.into()))
    else {
        return;
    };
    input.set_type("file");
    input.set_accept(accept);

    let input_for_cb = input.clone();
    let on_change = Closure::<dyn FnMut()>::new(move || {
        let Some(file) = input_for_cb.files().and_then(|f| f.get(0)) else {
            return;
        };
        let name = file.name();
        let deliver = deliver.clone();
        let ctx = ctx.clone();
        // Read the Blob asynchronously, then hand the bytes to the app.
        wasm_bindgen_futures::spawn_local(async move {
            match wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await {
                Ok(buf) => {
                    let bytes = js_sys::Uint8Array::new(&buf).to_vec();
                    deliver(name, bytes);
                    ctx.request_repaint();
                }
                Err(e) => log::error!("failed to read file: {e:?}"),
            }
        });
    });
    input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    // The closure must outlive this call (it fires later); leak it deliberately.
    on_change.forget();
    input.click();
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // No continuous repaint: egui repaints on input (incl. active drags), and
        // we re-render the 3D scene only when it actually changed (see viewport).
        let ctx = ui.ctx().clone();

        // Browser file picker results: load each (filename, bytes) the async picker
        // delivered (see `pick_file`) as a new molecule.
        #[cfg(target_arch = "wasm32")]
        while let Ok((name, bytes)) = self.file_rx.try_recv() {
            match data::load_from_bytes(&name, bytes) {
                Ok(raw) => self.add_loaded(raw),
                Err(e) => {
                    log::error!("{e}");
                    self.status = e;
                }
            }
        }

        // Browser trajectory picker results: open an incremental stream over the
        // bytes (seeding frame 0 with the structure first), to be drained below.
        #[cfg(target_arch = "wasm32")]
        while let Ok((mol_id, name, bytes)) = self.traj_rx.try_recv() {
            let Some(mol) = self.scene.molecules.iter_mut().find(|m| m.id == mol_id) else {
                continue;
            };
            mol.seed_frame0();
            let expected = mol.n_atoms;
            match data::traj_wasm::TrajStream::new(
                &name,
                bytes,
                LoadOptions::default(),
                expected,
            ) {
                Ok(stream) => {
                    self.wasm_loaders.insert(mol_id, stream);
                    self.status = format!("Loading {name}…");
                }
                Err(e) => {
                    log::error!("{e}");
                    self.status = e;
                }
            }
        }

        // Keyboard: Ctrl/Cmd+Z undo, Ctrl/Cmd+Shift+Z or Ctrl/Cmd+Y redo.
        ctx.input(|i| {
            if i.modifiers.command && i.key_pressed(egui::Key::Z) {
                if i.modifiers.shift {
                    self.pending_redo_n = Some(1);
                } else {
                    self.pending_undo_n = Some(1);
                }
            }
            if i.modifiers.command && i.key_pressed(egui::Key::Y) {
                self.pending_redo_n = Some(1);
            }
        });

        // Drain background trajectory loaders so the slider reflects arrived frames.
        self.poll_loaders();
        #[cfg(target_arch = "wasm32")]
        self.poll_wasm_loaders(&ctx);

        let panel_dirty = self.draw_left_panel(ui);
        self.view_dirty |= panel_dirty;

        // The "Load trajectory" modal floats above everything (driven from ctx).
        self.draw_load_dialog(&ctx);

        // Apply undo/redo after the panel so list indices stay stable during draw.
        let applied = match (self.pending_undo_n.take(), self.pending_redo_n.take()) {
            (Some(n), _) => self.history.undo_n(n),
            (None, Some(n)) => self.history.redo_n(n),
            (None, None) => None,
        };
        if let Some(state) = applied {
            state.apply(&mut self.scene);
            self.view_dirty = true;
        }

        // Advance playback for any playing molecule (time-based, so the fps knob
        // is honored regardless of the render rate). `tick` is a no-op unless
        // playing, and stops itself at the ends in play-once mode.
        let dt = ctx.input(|i| i.stable_dt).min(0.1) as f64;
        let mut animating = false;
        let mut frame_advanced = false;
        for mol in &mut self.scene.molecules {
            if mol.trajectory.tick(dt) {
                mol.apply_current_frame();
                frame_advanced = true;
            }
            animating |= mol.trajectory.playing;
        }
        if frame_advanced {
            self.view_dirty = true;
        }
        // Keep repainting while animating or loading; otherwise idle = 0 GPU.
        if animating || !self.loaders.is_empty() {
            ctx.request_repaint();
        }

        // View/selection controls live in a top toolbar above the viewport (right of
        // the left panel); the central panel then fills the rest with the 3D image.
        self.draw_view_toolbar(ui);
        self.draw_viewport(ui, frame);

        // Record a checkpoint once the gesture has settled (coalesces drags/typing).
        let settled = !ctx.egui_is_using_pointer() && !ctx.egui_wants_keyboard_input();
        if settled {
            self.history.maybe_record(EditState::capture(&self.scene));
        }
    }
}

impl App {
    fn draw_left_panel(&mut self, ui: &mut egui::Ui) -> bool {
        let mut view_dirty = false;
        egui::Panel::left("controls_panel")
            .resizable(true)
            .default_size(300.0)
            .size_range(egui::Rangef::new(220.0, 520.0))
            .show_inside(ui, |ui| {
                ui.add_space(8.0);

                self.draw_history_toolbar(ui);
                ui.add_space(6.0);

                // Molecules are listed directly (no "Molecules"/"Scene" headers);
                // global scene controls live in the viewport overlay instead.
                view_dirty |= self.draw_molecule_list(ui);

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(4.0);
                    let dt = ui.ctx().input(|i| i.stable_dt);
                    let fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
                    ui.weak(format!("{fps:.0} fps  ({:.1} ms/frame)", dt * 1000.0));
                });
            });
        view_dirty
    }

    /// Top toolbar over the viewport (a real panel *above* the 3D image, not a
    /// floating overlay on it): **view** controls (projection · depth-cue popup ·
    /// orientation-axes dropdown) and **selection** controls (pick mode · selection
    /// mode), grouped by a separator. The lasso modifier hint trails on the right.
    /// All buttons are the shared `overlay_button` (uniform height, framed,
    /// ink-centered glyph); dropdowns/popups hang off `egui::Popup::menu`.
    fn draw_view_toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("view_toolbar")
            .frame(
                egui::Frame::default()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(egui::Margin::symmetric(6, 4)),
            )
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);

                    // — View controls —
                    // Projection toggle (perspective ↔ orthographic).
                    let persp = self.camera.is_perspective();
                    let (glyph, tip) = if persp {
                        (icon::PERSPECTIVE, "Perspective — click for orthographic")
                    } else {
                        (icon::CUBE, "Orthographic — click for perspective")
                    };
                    if overlay_button(ui, glyph, false).on_hover_text(tip).clicked() {
                        self.camera.projection = if persp {
                            Projection::Orthographic
                        } else {
                            Projection::Perspective
                        };
                    }
                    // Depth-cue popup (button filled when the cue is enabled). The
                    // panel is a popup so the toolbar keeps a fixed height.
                    let cue_on = self.camera.depth_cue.enabled;
                    let resp = overlay_button(ui, icon::GRADIENT, cue_on).on_hover_text("Depth cue");
                    egui::Popup::menu(&resp).show(|ui| {
                        let cue = &mut self.camera.depth_cue;
                        ui.checkbox(&mut cue.enabled, "Depth cue").on_hover_text(
                            "Fade distant geometry toward the background for depth perception",
                        );
                        ui.add_enabled_ui(cue.enabled, |ui| {
                            egui::Grid::new("depth_cue")
                                .num_columns(2)
                                .spacing(egui::vec2(8.0, 4.0))
                                .show(ui, |ui| {
                                    ui.label("Strength");
                                    ui.add(
                                        egui::Slider::new(&mut cue.strength, 0.0..=1.0)
                                            .fixed_decimals(2),
                                    );
                                    ui.end_row();
                                    ui.label("Start");
                                    ui.add(
                                        egui::Slider::new(&mut cue.start, 0.0..=1.0)
                                            .fixed_decimals(2),
                                    );
                                    ui.end_row();
                                });
                        });
                    });
                    // Orientation-axes dropdown: on/off + which corner.
                    let resp = overlay_button(ui, icon::ARROWS_OUT_CARDINAL, self.axes_on)
                        .on_hover_text("Orientation axes");
                    egui::Popup::menu(&resp).show(|ui| {
                        ui.checkbox(&mut self.axes_on, "On");
                        ui.add_enabled_ui(self.axes_on, |ui| {
                            ui.add_space(2.0);
                            // 2×2 of corner radio buttons (top row, bottom row).
                            egui::Grid::new("axes_corner")
                                .spacing(egui::vec2(18.0, 6.0))
                                .show(ui, |ui| {
                                    ui.radio_value(&mut self.axes_corner, Corner::TopLeft, "");
                                    ui.radio_value(&mut self.axes_corner, Corner::TopRight, "");
                                    ui.end_row();
                                    ui.radio_value(&mut self.axes_corner, Corner::BottomLeft, "");
                                    ui.radio_value(&mut self.axes_corner, Corner::BottomRight, "");
                                    ui.end_row();
                                });
                        });
                    });

                    ui.separator();

                    // — Selection controls —
                    // Pick mode dropdown (label + caret). Off by default.
                    let pick_label = format!("{}  {}", self.pick_mode.label(), icon::CARET_DOWN);
                    let resp = overlay_button(ui, &pick_label, false).on_hover_text("Pick mode");
                    egui::Popup::menu(&resp).show(|ui| {
                        for m in [PickMode::Off, PickMode::HoverInfo, PickMode::Lasso] {
                            ui.selectable_value(&mut self.pick_mode, m, m.label());
                        }
                    });
                    // Selection-mode dropdown (how a lasso expands its hits).
                    let sel_label = format!("{}  {}", self.selection_mode.label(), icon::CARET_DOWN);
                    let resp = overlay_button(ui, &sel_label, false).on_hover_text(
                        "Selection mode — how a lasso expands its hits:\n\
                         Atoms (exact) · Residues (whole) · Bound H (heavy + bonded H)",
                    );
                    egui::Popup::menu(&resp).show(|ui| {
                        for m in [
                            SelectionMode::Atoms,
                            SelectionMode::Residues,
                            SelectionMode::BoundH,
                        ] {
                            ui.selectable_value(&mut self.selection_mode, m, m.label());
                        }
                    });

                    // In Lasso mode, a held modifier changes the set operation (or, for
                    // Alt, orbits the view) — trail the hint on the right (matches
                    // `finish_lasso`'s `LassoOp` and `draw_viewport`'s Alt orbit).
                    if self.pick_mode == PickMode::Lasso {
                        let m = ui.input(|i| i.modifiers);
                        let hint = if m.alt {
                            Some((icon::ARROWS_CLOCKWISE, "rotate view", egui::Color32::from_rgb(150, 190, 230)))
                        } else if m.shift {
                            Some((icon::PLUS_CIRCLE, "add to selection", egui::Color32::from_rgb(120, 220, 120)))
                        } else if m.command {
                            Some((icon::MINUS_CIRCLE, "subtract from selection", egui::Color32::from_rgb(230, 140, 140)))
                        } else {
                            None
                        };
                        if let Some((glyph, text, color)) = hint {
                            ui.separator();
                            ui.colored_label(color, egui::RichText::new(glyph).size(16.0));
                            ui.colored_label(color, text);
                        }
                    }
                });
            });
    }

    /// Undo/redo buttons, each with a dropdown listing the named actions on the
    /// stack; selecting an entry undoes/redoes cumulatively up to it.
    fn draw_history_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;

            // Open a structure file as a new molecule (topology+coords formats only).
            if ui
                .button(format!("{}  Open", icon::FOLDER_OPEN))
                .on_hover_text("Open a structure file as a new molecule")
                .clicked()
            {
                self.open_structure(ui.ctx());
            }
            ui.separator();

            let can_undo = self.history.can_undo();
            if ui
                .add_enabled(can_undo, egui::Button::new(icon::ARROW_COUNTER_CLOCKWISE))
                .on_hover_text("Undo (Ctrl+Z)")
                .clicked()
            {
                self.pending_undo_n = Some(1);
            }
            if can_undo {
                ui.menu_button(icon::CARET_DOWN, |ui| {
                    for d in 0..self.history.undo_len() {
                        let label = format!("{}.  {}", d + 1, self.history.undo_label(d));
                        if ui.button(label).clicked() {
                            self.pending_undo_n = Some(d + 1);
                            ui.close();
                        }
                    }
                });
            } else {
                ui.add_enabled(false, egui::Button::new(icon::CARET_DOWN));
            }

            ui.add_space(8.0);

            let can_redo = self.history.can_redo();
            if ui
                .add_enabled(can_redo, egui::Button::new(icon::ARROW_CLOCKWISE))
                .on_hover_text("Redo (Ctrl+Shift+Z)")
                .clicked()
            {
                self.pending_redo_n = Some(1);
            }
            if can_redo {
                ui.menu_button(icon::CARET_DOWN, |ui| {
                    for d in 0..self.history.redo_len() {
                        let label = format!("{}.  {}", d + 1, self.history.redo_label(d));
                        if ui.button(label).clicked() {
                            self.pending_redo_n = Some(d + 1);
                            ui.close();
                        }
                    }
                });
            } else {
                ui.add_enabled(false, egui::Button::new(icon::CARET_DOWN));
            }
        });
    }

    /// Loaded molecules. Each is a foldable block: a header row (fold caret, name,
    /// atom count, then a right-justified action group: add-rep, eye, trash), with
    /// the molecule's representations nested below when expanded.
    fn draw_molecule_list(&mut self, ui: &mut egui::Ui) -> bool {
        if self.scene.molecules.is_empty() {
            ui.weak(&self.status);
            return false;
        }
        let default_rep = self.default_rep;
        let mut view_dirty = false;
        let mut delete: Option<usize> = None;
        let mut open_load: Option<MolId> = None;
        // A camera "zoom to fit" request (whole-molecule bbox), applied after the
        // loop so it doesn't conflict with the `&mut` molecule borrow.
        let mut focus: Option<(glam::Vec3, glam::Vec3)> = None;

        for i in 0..self.scene.molecules.len() {
            let open;
            {
                let mol = &mut self.scene.molecules[i];
                ui.horizontal(|ui| {
                    let caret = if mol.reps_open { icon::CARET_DOWN } else { icon::CARET_RIGHT };
                    if ui
                        .selectable_label(false, caret)
                        .on_hover_text("Representations")
                        .clicked()
                    {
                        mol.reps_open = !mol.reps_open;
                    }
                    ui.label(mol.name.as_str());
                    ui.weak(format!("({})", mol.n_atoms));
                    // Load a trajectory into this molecule (left-aligned, by the name).
                    if icon_button(ui, icon::FOLDER_OPEN, "Load trajectory").clicked() {
                        open_load = Some(mol.id);
                    }
                    // Right-justified action group: add-rep · eye · trash.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        compact_actions(ui);
                        if icon_button(ui, icon::TRASH, "Delete molecule").clicked() {
                            delete = Some(i);
                        }
                        let eye = if mol.visible { icon::EYE } else { icon::EYE_SLASH };
                        if ui
                            .selectable_label(mol.visible, eye)
                            .on_hover_text(if mol.visible { "Hide" } else { "Show" })
                            .clicked()
                        {
                            mol.visible = !mol.visible;
                            view_dirty = true;
                        }
                        if icon_button(ui, icon::MAGNIFYING_GLASS_PLUS, "Zoom to molecule")
                            .clicked()
                        {
                            focus = Some(mol.current_bbox());
                        }
                        if ui
                            .selectable_label(mol.show_box, icon::BOUNDING_BOX)
                            .on_hover_text("Show periodic box")
                            .clicked()
                        {
                            mol.show_box = !mol.show_box;
                            mol.box_dirty = true;
                            view_dirty = true;
                        }
                        if ui
                            .button(format!("{} rep", icon::PLUS))
                            .on_hover_text("Add representation")
                            .clicked()
                        {
                            mol.reps.push(Representation::new(default_rep));
                            mol.selected_rep = Some(mol.reps.len() - 1);
                            mol.reps_open = true;
                            view_dirty = true;
                        }
                    });
                });
                // Trajectory playback controls, shown once >1 frame is loaded.
                if mol.trajectory.has_playback() {
                    ui.indent(egui::Id::new(("traj", i)), |ui| {
                        if draw_traj_bar(ui, &mut mol.trajectory) {
                            mol.apply_current_frame();
                            view_dirty = true;
                        }
                    });
                } else if self.loaders.contains_key(&mol.id) {
                    ui.indent(egui::Id::new(("traj", i)), |ui| {
                        ui.weak(format!("loading… {} frames", mol.trajectory.n_frames()));
                    });
                }
                open = mol.reps_open;
            }
            if open {
                ui.indent(egui::Id::new(("reps", i)), |ui| {
                    view_dirty |= self.draw_reps_for(ui, i);
                });
            }
            ui.add_space(4.0);
        }

        if let Some(i) = delete {
            // Park the molecule in the trash so the delete can be undone.
            let m = self.scene.molecules.remove(i);
            // Drop any in-flight loader (its background thread exits when the
            // receiver is dropped); likewise any browser streaming loader.
            self.loaders.remove(&m.id);
            #[cfg(target_arch = "wasm32")]
            self.wasm_loaders.remove(&m.id);
            self.scene.trash.insert(m.id, m);
            self.scene.clamp_selection();
            view_dirty = true;
        }
        if let Some(id) = open_load {
            // Native: the load dialog (file picker + range/stride/sync-async).
            #[cfg(not(target_arch = "wasm32"))]
            {
                self.load_dialog = Some(LoadDialog::new(id));
            }
            // Browser: pick a trajectory file and stream all its frames in (no
            // dialog; range/stride aren't offered on the web yet).
            #[cfg(target_arch = "wasm32")]
            {
                let tx = self.traj_tx.clone();
                pick_file(
                    ".xtc,.trr,.dcd,.pdb,.gro,.xyz",
                    ui.ctx().clone(),
                    move |name, bytes| {
                        let _ = tx.send((id, name, bytes));
                    },
                );
            }
        }
        if let Some((min, max)) = focus {
            self.camera.focus_bbox(min, max);
            view_dirty = true;
        }
        view_dirty
    }

    /// Open a structure file as a new molecule. Native: a synchronous `rfd` file
    /// picker → [`data::load`]. Browser: an async `<input type=file>` whose bytes
    /// come back through `file_rx` and are loaded in [`Self::ui`] via
    /// [`data::load_from_bytes`]. Only topology+coordinate formats can seed a
    /// molecule. The add is undoable (end-of-frame history checkpoint).
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
    fn open_structure(&mut self, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Structures", &["pdb", "ent", "gro", "xyz", "tpr"])
                .pick_file()
            else {
                return;
            };
            match data::load(&path) {
                Ok(raw) => self.add_loaded(raw),
                Err(e) => {
                    log::error!("{e}");
                    self.status = e;
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let tx = self.file_tx.clone();
            pick_file(
                ".pdb,.ent,.gro,.xyz,.dcd,.trr,.xtc",
                ctx.clone(),
                move |name, bytes| {
                    let _ = tx.send((name, bytes));
                },
            );
        }
    }

    /// Add a freshly loaded structure as a new molecule: select it, frame the
    /// camera if it's the first one, and flag a re-render. Shared by the native
    /// picker and the browser byte-loader.
    fn add_loaded(&mut self, raw: data::RawMolecule) {
        let was_empty = self.scene.molecules.is_empty();
        self.scene.add(raw, self.default_rep);
        self.scene.selected_mol = Some(self.scene.molecules.len() - 1);
        if was_empty {
            if let Some((min, max)) = self.scene.bbox() {
                let proj = self.camera.projection;
                self.camera = Camera::frame_bbox(min, max);
                self.camera.projection = proj;
            }
        }
        self.status = format!("{} molecule(s) loaded", self.scene.molecules.len());
        self.view_dirty = true;
    }

    /// Load the small bundled structure (2lao) so the web/GitHub-Pages demo opens
    /// to a molecule instead of an empty viewport. Wasm only (embeds the file in
    /// the binary); the native app starts empty and loads via the Open button.
    #[cfg(target_arch = "wasm32")]
    pub fn load_demo(&mut self) {
        const DEMO_PDB: &[u8] = include_bytes!("../../../tests/2lao.pdb");
        match data::load_from_bytes("2lao.pdb", DEMO_PDB.to_vec()) {
            Ok(raw) => self.add_loaded(raw),
            Err(e) => log::error!("demo load failed: {e}"),
        }
    }

    /// Render the "Load trajectory" modal (a-la VMD): file chooser + frame range
    /// / stride + sync/async, with Load/Cancel. Driven from `ctx` (egui modals
    /// take a `Context`, not a `Ui`), so it floats above the whole window.
    fn draw_load_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.load_dialog.take() else {
            return;
        };
        let mut action = DialogAction::Keep;

        let modal = egui::Modal::new(egui::Id::new("load_traj_modal")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Load trajectory");
            match self.scene.molecules.iter().find(|m| m.id == dialog.mol_id) {
                Some(mol) => {
                    ui.label(format!("Into “{}”  ({} atoms)", mol.name, mol.n_atoms));
                }
                None => {
                    ui.colored_label(
                        egui::Color32::from_rgb(240, 120, 120),
                        "molecule no longer exists",
                    );
                }
            }
            ui.separator();

            // File chooser.
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{}  Choose file…", icon::FOLDER_OPEN))
                    .clicked()
                {
                    #[cfg(not(target_arch = "wasm32"))]
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter(
                            "Trajectories",
                            &["xtc", "trr", "dcd", "pdb", "gro", "xyz", "nc", "ncdf"],
                        )
                        .pick_file()
                    {
                        dialog.path = Some(p);
                        dialog.error = None;
                    }
                }
                match &dialog.path {
                    Some(p) => {
                        ui.monospace(
                            p.file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        );
                    }
                    None => {
                        ui.weak("no file selected");
                    }
                }
            });

            // Frame range + stride.
            egui::Grid::new("traj_load_opts")
                .num_columns(2)
                .spacing(egui::vec2(8.0, 4.0))
                .show(ui, |ui| {
                    ui.label("First frame");
                    ui.add(egui::DragValue::new(&mut dialog.from));
                    ui.end_row();

                    ui.label("Last frame");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut dialog.to_enabled, "");
                        ui.add_enabled(dialog.to_enabled, egui::DragValue::new(&mut dialog.to));
                        if !dialog.to_enabled {
                            ui.weak("(to end of file)");
                        }
                    });
                    ui.end_row();

                    ui.label("Stride");
                    ui.add(egui::DragValue::new(&mut dialog.stride).range(1..=usize::MAX))
                        .on_hover_text("Keep every Nth frame");
                    ui.end_row();
                });

            ui.horizontal(|ui| {
                ui.label("Reading:");
                ui.radio_value(&mut dialog.mode, LoadMode::Sync, "Sync")
                    .on_hover_text("Read all frames now (UI blocks until done)");
                ui.radio_value(&mut dialog.mode, LoadMode::Async, "Async")
                    .on_hover_text("Read in the background; frames appear as they load");
            });

            if let Some(err) = &dialog.error {
                ui.colored_label(egui::Color32::from_rgb(240, 120, 120), err);
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(dialog.path.is_some(), egui::Button::new("Load"))
                    .clicked()
                {
                    action = DialogAction::Load;
                }
                if ui.button("Cancel").clicked() {
                    action = DialogAction::Cancel;
                }
            });
        });

        if modal.should_close() {
            action = DialogAction::Cancel;
        }

        match action {
            DialogAction::Keep => self.load_dialog = Some(dialog),
            DialogAction::Cancel => {}
            DialogAction::Load => {
                if let Err(e) = self.start_load(&dialog) {
                    dialog.error = Some(e);
                    self.load_dialog = Some(dialog); // reopen, showing the error
                }
            }
        }
    }

    /// Begin loading the dialog's file into its molecule (sync or async).
    fn start_load(&mut self, dialog: &LoadDialog) -> Result<(), String> {
        let path = dialog.path.clone().ok_or("no file selected")?;
        let opts = LoadOptions {
            from: dialog.from,
            to: dialog.to_enabled.then_some(dialog.to),
            stride: dialog.stride.max(1),
        };
        if let Some(to) = opts.to {
            if to < opts.from {
                return Err("last frame is before first frame".to_string());
            }
        }

        // Seed frame 0 with the structure coords (idempotent) and learn the count.
        let expected = {
            let mol = self
                .scene
                .molecules
                .iter_mut()
                .find(|m| m.id == dialog.mol_id)
                .ok_or("molecule no longer exists")?;
            mol.seed_frame0();
            mol.n_atoms
        };

        match dialog.mode {
            LoadMode::Sync => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let frames = data::traj_loader::read_frames_sync(&path, &opts, expected)?;
                    let added = frames.len();
                    let mol = self
                        .scene
                        .molecules
                        .iter_mut()
                        .find(|m| m.id == dialog.mol_id)
                        .ok_or("molecule no longer exists")?;
                    let first_new = mol.trajectory.frames.len();
                    mol.append_frames(frames);
                    mol.trajectory.current = first_new; // jump to first loaded frame
                    mol.apply_current_frame();
                    self.status = format!("Loaded {added} frame(s)");
                    self.view_dirty = true;
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = (&path, &opts, expected);
                    return Err("trajectory loading is not yet supported on the web".to_string());
                }
            }
            LoadMode::Async => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let rx = data::traj_loader::spawn_async(path, opts, expected);
                    self.loaders.insert(dialog.mol_id, rx);
                    self.status = "Loading trajectory…".to_string();
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = (&path, &opts, expected);
                    return Err("trajectory loading is not yet supported on the web".to_string());
                }
            }
        }
        Ok(())
    }

    /// Drain background loaders, appending streamed frames to their molecules.
    /// Non-blocking (`try_recv`); finished/errored/disconnected loaders are removed.
    fn poll_loaders(&mut self) {
        if self.loaders.is_empty() {
            return;
        }
        use std::sync::mpsc::TryRecvError;
        let ids: Vec<MolId> = self.loaders.keys().copied().collect();
        let mut finished: Vec<MolId> = Vec::new();
        for id in ids {
            loop {
                let msg = match self.loaders.get(&id) {
                    Some(rx) => rx.try_recv(),
                    None => break,
                };
                match msg {
                    Ok(LoadMsg::Frame(state)) => {
                        // Append to the molecule if it still exists; else discard.
                        if let Some(mol) =
                            self.scene.molecules.iter_mut().find(|m| m.id == id)
                        {
                            mol.push_frame(state);
                        }
                    }
                    Ok(LoadMsg::Done) => {
                        finished.push(id);
                        break;
                    }
                    Ok(LoadMsg::Error(e)) => {
                        self.status = e;
                        finished.push(id);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished.push(id);
                        break;
                    }
                }
            }
        }
        for id in finished {
            self.loaders.remove(&id);
        }
    }

    /// Browser trajectory streaming: read a batch of frames from each in-memory
    /// [`data::traj_wasm::TrajStream`] and append them, so frames flow in without
    /// blocking the UI. On the first batch, jump to the first trajectory frame so
    /// the load is visible. Finished/errored streams are removed; repaints continue
    /// while any stream is active.
    #[cfg(target_arch = "wasm32")]
    fn poll_wasm_loaders(&mut self, ctx: &egui::Context) {
        if self.wasm_loaders.is_empty() {
            return;
        }
        const BATCH: usize = 64;
        let ids: Vec<MolId> = self.wasm_loaders.keys().copied().collect();
        let mut finished: Vec<MolId> = Vec::new();
        let mut view_dirty = false;
        for id in ids {
            let Some(stream) = self.wasm_loaders.get_mut(&id) else {
                continue;
            };
            let batch = stream.next_batch(BATCH);
            let done = stream.done;
            match batch {
                Ok(frames) => {
                    if let Some(mol) = self.scene.molecules.iter_mut().find(|m| m.id == id) {
                        for st in frames {
                            mol.push_frame(st);
                        }
                        // First frames in → show the first trajectory frame.
                        if mol.trajectory.current == 0 && mol.trajectory.frames.len() > 1 {
                            mol.trajectory.current = 1;
                            mol.apply_current_frame();
                            view_dirty = true;
                        }
                        if done {
                            self.status =
                                format!("Loaded {} frame(s)", mol.trajectory.frames.len() - 1);
                        }
                    }
                    if done {
                        finished.push(id);
                    }
                }
                Err(e) => {
                    log::error!("{e}");
                    self.status = e;
                    finished.push(id);
                }
            }
        }
        for id in &finished {
            self.wasm_loaders.remove(id);
        }
        if view_dirty {
            self.view_dirty = true;
        }
        // Keep frames flowing while any stream is still active.
        if !self.wasm_loaders.is_empty() {
            ctx.request_repaint();
        }
    }

    /// Representations of the selected molecule as rich rows: a drag handle
    /// (reorder by dragging), the selection text (expands to full width while
    /// focused, collapses on Enter/blur), a drawn style-icon dropdown, and a
    /// right-justified action group (gear→params, eye, update-every-frame,
    /// duplicate, trash). An "Add" button precedes the list.
    /// The representations of molecule `mi`, nested under it: rich two-row blocks
    /// (drag handle · selection · actions / style · color · gear) with
    /// drag-reorder. The "add representation" control lives in the molecule's
    /// header row, not here.
    fn draw_reps_for(&mut self, ui: &mut egui::Ui, mi: usize) -> bool {
        let mut view_dirty = false;
        let editing = self
            .editing_rep
            .filter(|&(m, _)| m == mi)
            .map(|(_, r)| r);
        let mut new_editing = self.editing_rep;

        let mol = &mut self.scene.molecules[mi];
        let mol_id = mol.id;
        // The Periodic tab is only offered when the molecule has a box.
        let has_box = mol.system.state().pbox.is_some();

        let mut delete: Option<usize> = None;
        let mut duplicate: Option<usize> = None;
        let mut reorder: Option<(usize, usize)> = None;
        let mut zoom_rep: Option<usize> = None;

        for j in 0..mol.reps.len() {
            let sel_id = egui::Id::new(("rep_sel", mol_id, j));
            let rep = &mut mol.reps[j];
            // Whether the selection is valid but empty (0 atoms) — flags the field.
            let sel_empty = rep.sel_empty;

            // Each rep is two rows, grouped: row 1 = handle | selection | actions,
            // row 2 = style | color | gear. The whole block is the reorder target.
            // Row 2 is indented by the drag-handle width so it aligns under the
            // selection field rather than under the handle.
            let mut row2_indent = 0.0_f32;
            let block = ui
                .vertical(|ui| {
                    // Row 1: drag handle | selection | eye · update · copy · delete
                    ui.horizontal(|ui| {
                        let handle = ui
                            .dnd_drag_source(egui::Id::new(("rep_drag", mol_id, j)), j, |ui| {
                                ui.add(egui::Label::new(icon::DOTS_SIX_VERTICAL).selectable(false));
                            })
                            .response
                            .on_hover_cursor(egui::CursorIcon::Grab)
                            .on_hover_text("Drag to reorder");
                        row2_indent = handle.rect.width();

                        if editing == Some(j) {
                            // Focused: the selection field fills the whole row.
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut rep.sel_text)
                                    .id(sel_id)
                                    .desired_width(f32::INFINITY)
                                    .hint_text("selection"),
                            );
                            if sel_empty {
                                mark_empty_selection(ui, resp.rect);
                            }
                            if resp.lost_focus() {
                                rep.sel_dirty = true;
                                new_editing = None;
                            }
                        } else {
                            // Actions on the right; selection field fills the rest.
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                compact_actions(ui);
                                if icon_button(ui, icon::TRASH, "Delete").clicked() {
                                    delete = Some(j);
                                }
                                if icon_button(ui, icon::COPY, "Duplicate").clicked() {
                                    duplicate = Some(j);
                                }
                                // (Update-every-frame moved to the Settings ▸ Traj tab.)
                                // Eye: open when shown, crossed when hidden.
                                let eye = if rep.visible { icon::EYE } else { icon::EYE_SLASH };
                                if ui
                                    .selectable_label(rep.visible, eye)
                                    .on_hover_text(if rep.visible { "Hide" } else { "Show" })
                                    .clicked()
                                {
                                    rep.visible = !rep.visible;
                                    view_dirty = true;
                                }
                                // Zoom the camera to fit this selection.
                                if icon_button(ui, icon::MAGNIFYING_GLASS_PLUS, "Zoom to selection")
                                    .clicked()
                                {
                                    zoom_rep = Some(j);
                                }
                                // Selection field fills the remaining width.
                                ui.with_layout(
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        let resp = ui.add(
                                            egui::TextEdit::singleline(&mut rep.sel_text)
                                                .id(sel_id)
                                                .desired_width(ui.available_width())
                                                .hint_text("selection"),
                                        );
                                        if sel_empty {
                                            mark_empty_selection(ui, resp.rect);
                                        }
                                        if resp.gained_focus() {
                                            new_editing = Some((mi, j));
                                        }
                                        if resp.lost_focus() {
                                            rep.sel_dirty = true;
                                        }
                                    },
                                );
                            });
                        }
                    });

                    // Selection errors appear immediately below the selection field,
                    // aligned under it (indented past the drag handle).
                    if let Some(err) = &rep.sel_error {
                        ui.horizontal(|ui| {
                            ui.add_space(row2_indent);
                            ui.colored_label(egui::Color32::from_rgb(240, 120, 120), err);
                        });
                    }

                    // Row 2: [settings expander] style | color | material. The caret
                    // sits where the drag handle is in row 1 (so the style dropdown
                    // lines up under the selection field) and toggles the settings.
                    ui.horizontal(|ui| {
                        let caret = if rep.params_open {
                            icon::CARET_DOWN
                        } else {
                            icon::CARET_RIGHT
                        };
                        // Never shows the persistent "selected" (blue) highlight —
                        // the ▸/▾ glyph already signals expanded/collapsed; passing
                        // `false` keeps just the hover feedback.
                        if ui
                            .selectable_label(false, caret)
                            .on_hover_text("Representation settings")
                            .clicked()
                        {
                            rep.params_open = !rep.params_open;
                        }
                        style_picker(ui, rep);
                        color_picker(ui, rep);
                        material_picker(ui, rep);
                    });
                })
                .response;

            // Inline params panel (within the side panel), shown when the gear is on.
            if rep.params_open {
                view_dirty |= ui
                    .indent(egui::Id::new(("rep_params", mol_id, j)), |ui| {
                        draw_rep_params(ui, rep, has_box)
                    })
                    .inner;
            }

            // Reorder drop target spans the whole two-row block.
            if let (Some(ptr), Some(_)) = (
                ui.input(|i| i.pointer.interact_pos()),
                block.dnd_hover_payload::<usize>(),
            ) {
                let before = ptr.y < block.rect.center().y;
                let y = if before { block.rect.top() } else { block.rect.bottom() };
                ui.painter().hline(
                    block.rect.x_range(),
                    y,
                    egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
                );
                if let Some(src) = block.dnd_release_payload::<usize>() {
                    reorder = Some((*src, if before { j } else { j + 1 }));
                }
            }

            ui.add_space(6.0);
        }

        // The active (pending) selection — e.g. just captured by a lasso — appears
        // below the reps with a minimal interface: a non-editable "selection" label
        // plus accept (commit as a Ball-and-Stick rep) / discard buttons. No style,
        // color, or editable selection (those come once it's accepted).
        let mut accept_pending = false;
        let mut discard_pending = false;
        if mol.pending.is_some() {
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new("selection").italics())
                        .selectable(false),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    compact_actions(ui);
                    if icon_button(ui, icon::TRASH, "Discard selection").clicked() {
                        discard_pending = true;
                    }
                    let accept = ui
                        .selectable_label(
                            false,
                            egui::RichText::new(icon::CHECK)
                                .color(egui::Color32::from_rgb(120, 220, 120)),
                        )
                        .on_hover_text("Accept as a representation");
                    if accept.clicked() {
                        accept_pending = true;
                    }
                });
            });
            ui.add_space(6.0);
        }

        if let Some((from, to)) = reorder {
            if to != from && to != from + 1 {
                let item = mol.reps.remove(from);
                let target = (if from < to { to - 1 } else { to }).min(mol.reps.len());
                mol.reps.insert(target, item);
                mol.selected_rep = Some(target);
                view_dirty = true;
            }
        }
        if let Some(j) = duplicate {
            let dup = mol.reps[j].duplicate();
            mol.reps.insert(j + 1, dup);
            mol.selected_rep = Some(j + 1);
            view_dirty = true;
        }
        if let Some(j) = delete {
            mol.reps.remove(j);
            mol.selected_rep = if mol.reps.is_empty() {
                None
            } else {
                Some(j.min(mol.reps.len() - 1))
            };
            view_dirty = true;
        }
        if accept_pending {
            if let Some(p) = mol.pending.take() {
                // Commit as a normal, fully editable Ball-and-Stick representation.
                let mut rep = Representation::new(RepKind::BallAndStick);
                rep.sel_text = p.sel_text;
                mol.reps.push(rep);
                mol.selected_rep = Some(mol.reps.len() - 1);
                mol.reps_open = true;
            }
            mol.glow_dirty = true; // clear the glow geometry
            view_dirty = true;
        }
        if discard_pending {
            mol.pending = None;
            mol.glow_dirty = true; // clear the glow geometry
            view_dirty = true;
        }

        // Zoom the camera to fit a rep's selection (camera is a disjoint field
        // from the scene, so this is fine while `mol` is borrowed).
        if let Some(j) = zoom_rep {
            if let Some(sel) = mol.reps.get(j).and_then(|r| r.sel.as_ref()) {
                let (min, max) = mol.sel_bbox(sel);
                self.camera.focus_bbox(min, max);
                view_dirty = true;
            }
        }

        self.editing_rep = new_editing;
        view_dirty
    }

    /// Central panel: rebuild dirty geometry, route VMD-style mouse navigation,
    /// re-render the 3D scene only when needed, and blit it as an egui image.
    fn draw_viewport(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            let render_state = frame
                .wgpu_render_state()
                .expect("wgpu render state must exist");

            let geom_changed = self.rebuild_dirty(render_state);

            // Claim the whole central area as a draggable, scrollable surface.
            let available = ui.available_size();
            let (rect, response) =
                ui.allocate_exact_size(available, egui::Sense::click_and_drag());

            // VMD-style navigation:
            //   LMB = free 3D rotate · Shift+LMB = roll (screen-plane rotate)
            //   RMB = pan           · Shift+RMB = move along view Z (dolly)
            //   middle = pan        · wheel = scale (zoom)
            // In Lasso pick mode an LMB drag draws the selection polygon (the held
            // modifier picks the set op on release: plain = replace, Shift = add,
            // Ctrl = subtract) — except **Alt+LMB**, which orbits so the view can be
            // rotated without leaving Lasso mode.
            let lasso_mode = self.pick_mode == PickMode::Lasso;
            if !lasso_mode {
                self.lasso_path.clear();
            }
            let delta = response.drag_delta();
            let mods = ui.input(|i| i.modifiers);
            let (shift, alt) = (mods.shift, mods.alt);
            // Alt+drag in Lasso mode orbits; otherwise an LMB lasso draws the polygon.
            let lasso_draw = lasso_mode && !alt;
            if response.dragged_by(egui::PointerButton::Primary) {
                if lasso_draw {
                    if let Some(pos) = response.interact_pointer_pos() {
                        // Drop near-duplicate points (drag jitter) for a clean polygon.
                        if self.lasso_path.last().is_none_or(|&p| (p - pos).length() > 1.5) {
                            self.lasso_path.push(pos);
                        }
                    }
                } else if shift && !lasso_mode {
                    self.camera.roll(delta.x);
                } else {
                    // Non-lasso LMB, or Alt+LMB in lasso mode → free orbit.
                    self.camera.orbit(delta.x, delta.y);
                }
            } else if response.dragged_by(egui::PointerButton::Secondary) {
                if shift {
                    self.camera.zoom_drag(delta.y);
                } else {
                    self.camera.pan(delta.x, delta.y, rect.height());
                }
            } else if response.dragged_by(egui::PointerButton::Middle) {
                self.camera.pan(delta.x, delta.y, rect.height());
            }
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    self.camera.zoom_scroll(scroll);
                }
            }

            let ppp = ui.ctx().pixels_per_point();
            let size_px = [
                ((rect.width() * ppp).round() as u32).max(1),
                ((rect.height() * ppp).round() as u32).max(1),
            ];

            // An active (pending) selection glows with a gentle pulse: while one is
            // present, animate the glow's intensity multiplier and keep repainting
            // (and re-rendering each frame) so it breathes. Otherwise idle = 0 GPU.
            let pulsing = self
                .scene
                .molecules
                .iter()
                .any(|m| m.visible && m.pending.is_some());
            let glow_pulse = if pulsing {
                let t = ui.input(|i| i.time) as f32;
                0.70 + 0.30 * (t * 3.2).sin()
            } else {
                1.0
            };
            if pulsing {
                ui.ctx().request_repaint();
            }

            let cam_changed = self.last_render_camera != Some(self.camera);
            let size_changed = size_px != self.last_size;
            if geom_changed || cam_changed || size_changed || self.view_dirty || pulsing {
                let aspect = size_px[0] as f32 / size_px[1] as f32;
                let view = self.camera.view();
                let proj = self.camera.proj(aspect);
                self.renderer.render_scene(
                    render_state,
                    size_px,
                    view,
                    proj,
                    self.camera.is_perspective(),
                    self.camera.cue_uniform(),
                    self.camera.eye_depth_range(),
                    glow_pulse,
                    &self.scene,
                );
                self.last_render_camera = Some(self.camera);
                self.last_size = size_px;
            }
            self.view_dirty = false;

            let texture_id = self.renderer.texture_id();
            egui::Image::new(egui::load::SizedTexture::new(texture_id, rect.size()))
                .paint_at(ui, rect);

            // Hover-info picking: ray-cast the cursor against the atoms *as drawn*
            // (smoothed + periodic-replicated), glow the hit, and show its identity
            // + **real** (stored) coordinates. Skipped while dragging the camera.
            if self.pick_mode == PickMode::HoverInfo && !response.dragged() {
                // Normally the cursor; the debug hook forces the viewport center so
                // the overlay can be screenshot without simulating a mouse.
                let ndc = if std::env::var("MOLAR_VIS_DEBUG_PICK").is_ok() {
                    Some((0.0, 0.0))
                } else {
                    response.hover_pos().map(|p| {
                        (
                            ((p.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0,
                            1.0 - ((p.y - rect.top()) / rect.height().max(1.0)) * 2.0,
                        )
                    })
                };
                if let Some((ndc_x, ndc_y)) = ndc {
                    let aspect = size_px[0] as f32 / size_px[1] as f32;
                    let view = self.camera.view();
                    let proj = self.camera.proj(aspect);
                    if let Some(hit) = pick::pick(&self.scene, view, proj, ndc_x, ndc_y) {
                        draw_pick_overlay(ui, rect, &self.camera, aspect, &hit);
                    }
                }
            }

            // The active (pending) selection is highlighted by a GPU glow pass
            // (`render_scene` pass 4), so there's nothing to draw here.

            // Lasso selection: draw the in-progress polygon, and on release stage
            // the enclosed (style-eligible) atoms as the active selection.
            if lasso_mode && self.lasso_path.len() >= 2 {
                let painter = ui.painter_at(rect);
                let col = egui::Color32::from_rgb(130, 215, 255);
                painter.add(egui::Shape::line(
                    self.lasso_path.clone(),
                    egui::Stroke::new(1.5, col),
                ));
                // Faint segment closing the loop back to the start.
                if let (Some(&first), Some(&last)) =
                    (self.lasso_path.first(), self.lasso_path.last())
                {
                    painter.line_segment(
                        [last, first],
                        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(130, 215, 255, 110)),
                    );
                }
            }
            if lasso_mode
                && !self.lasso_path.is_empty()
                && response.drag_stopped_by(egui::PointerButton::Primary)
            {
                // The modifier held at release picks the set operation: Shift adds to
                // the active selection, Ctrl (⌘ on mac) subtracts, plain replaces.
                let m = ui.input(|i| i.modifiers);
                let op = if m.shift {
                    LassoOp::Add
                } else if m.command {
                    LassoOp::Subtract
                } else {
                    LassoOp::Replace
                };
                self.finish_lasso(rect, size_px, op);
            }

            // VMD-style orientation axes gizmo in the chosen corner (a gizmo painted
            // onto the 3D image; its on/off + corner live in the top view toolbar).
            if self.axes_on {
                draw_axes_overlay(ui, rect, &self.camera, self.axes_corner);
            }
        });
    }

    /// Finish a lasso gesture: convert the screen-space path to a clip-space
    /// polygon, collect the enclosed atoms (per `pick::lasso_select`, honoring the
    /// per-rep style logic), and combine them with each molecule's **active (pending)
    /// selection** per `op` — a glowing highlight with a minimal accept/discard UI,
    /// not yet a real representation. Staging is not undoable; accepting it (→ a
    /// Ball-and-Stick rep) is.
    fn finish_lasso(&mut self, rect: egui::Rect, size_px: [u32; 2], op: LassoOp) {
        let path = std::mem::take(&mut self.lasso_path);
        if path.len() < 3 {
            return;
        }
        // Screen px → clip-space NDC (y up), matching `pick`'s convention.
        let polygon: Vec<glam::Vec2> = path
            .iter()
            .map(|p| {
                glam::Vec2::new(
                    ((p.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0,
                    1.0 - ((p.y - rect.top()) / rect.height().max(1.0)) * 2.0,
                )
            })
            .collect();
        let aspect = size_px[0] as f32 / size_px[1] as f32;
        let view = self.camera.view();
        let proj = self.camera.proj(aspect);
        let results = pick::lasso_select(&self.scene, view, proj, &polygon);

        // Replace clears the previous active selection everywhere first; Add/Subtract
        // merge into the existing per-molecule pending set.
        if op == LassoOp::Replace {
            for mol in &mut self.scene.molecules {
                if mol.pending.take().is_some() {
                    mol.glow_dirty = true;
                }
            }
        }
        let mode = self.selection_mode;
        for res in results {
            let mol = &mut self.scene.molecules[res.mol];
            // Expand this gesture's raw hits per the selection mode (exact atoms /
            // whole residues / heavy + bonded H) before combining with the op.
            let hits = pick::expand_selection(&mol.system, &mol.bonds, &res.atoms, mode);
            // Start from the current pending atoms (sorted, unique), then apply the op.
            let mut set: std::collections::BTreeSet<usize> = mol
                .pending
                .as_ref()
                .map(|p| p.atoms.iter().copied().collect())
                .unwrap_or_default();
            match op {
                // For Replace the set was just cleared above, so this is just the new
                // atoms; Add unions them in.
                LassoOp::Replace | LassoOp::Add => set.extend(hits),
                LassoOp::Subtract => {
                    for a in &hits {
                        set.remove(a);
                    }
                }
            }
            let atoms: Vec<usize> = set.into_iter().collect();
            match pick::index_selection_string(&atoms) {
                Some(sel_text) => {
                    mol.pending = Some(scene::PendingSelection { sel_text, atoms });
                    mol.reps_open = true;
                }
                // Empty result (e.g. subtracted everything) → no active selection.
                None => {
                    mol.pending = None;
                }
            }
            mol.glow_dirty = true;
        }
        self.view_dirty = true;
    }
}

/// How a lasso gesture combines with the molecule's existing active selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LassoOp {
    /// Plain drag: the lasso becomes the new active selection.
    Replace,
    /// Shift+drag: union the lassoed atoms into the active selection.
    Add,
    /// Ctrl/⌘+drag: remove the lassoed atoms from the active selection.
    Subtract,
}
