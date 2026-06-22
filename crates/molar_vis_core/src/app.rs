//! The eframe application: owns UI state, the camera, the scene (molecules and
//! their representations), and the 3D renderer. Lays out the VMD-style left
//! control panel (Scene → Molecules → Representations → Rep controls) plus the
//! central 3D viewport, and only re-renders the scene when something changed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use eframe::egui;
use molar::prelude::{AtomProvider, Measure, ParticleIterProvider, SsAlgorithm, State};
#[cfg(not(target_arch = "wasm32"))]
use molar::prelude::FileHandler;

use crate::camera::{BgKind, Camera, CueMode, Projection};
use crate::color::ColorMethod;
use crate::data;
use crate::geometry::{self, RepKind, RepParams};
use crate::history::{EditState, History};
use crate::launch::AppLaunch;
use crate::material::{Material, MaterialParams};
use crate::minimize::{Bond, BondOrderExt};
use crate::pick::{self, PickMode, SelectionMode};
use crate::render::{SceneRenderer, SphereInstance};
use crate::scene::{self, MolId, Representation, Scene, SettingsTab};
use crate::secstruct::SsMap;
#[cfg(not(target_arch = "wasm32"))]
use crate::session::{Session, ViewState};
use crate::settings::{RepDefaults, Settings, ThemeMode};
#[cfg(not(target_arch = "wasm32"))]
use crate::scene::{MoleculeSource, TrajLoad};
use crate::suggest::SelHints;
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

/// Drop a rep's stale selection feedback (error message, in-field red highlight,
/// and the empty-match warning) — called while the user is editing the text, so
/// the old evaluation's markers don't linger over text they no longer match. The
/// feedback is recomputed when the edit is committed (`sel_dirty` → `rebuild_dirty`).
fn clear_sel_feedback(rep: &mut Representation) {
    rep.sel_error = None;
    rep.sel_error_caret = None;
    rep.sel_empty = false;
}

/// Write a molecule (whole, `rep = None`) or one representation's selection
/// (`rep = Some(j)`) to `path` via molar, at the **currently displayed** frame.
/// Trajectory frames render by reference and aren't held in the `System`, so the
/// displayed `State` is swapped in around the write and restored afterwards. The
/// file format is chosen by molar from `path`'s extension. Native only (molar's
/// `FileHandler::create` writes to the filesystem).
#[cfg(not(target_arch = "wasm32"))]
fn save_displayed(
    mol: &mut scene::Molecule,
    path: &std::path::Path,
    rep: Option<usize>,
) -> Result<(), String> {
    let displayed = mol.render_state().clone();
    let prev = mol.system.set_state(displayed).map_err(|e| e.to_string())?;
    let res = (|| -> Result<(), String> {
        let mut h = FileHandler::create(path).map_err(|e| e.to_string())?;
        match rep {
            Some(j) => {
                let sel = mol.reps[j].sel.as_ref().ok_or("selection is empty")?;
                let bound = mol.system.bind(sel);
                h.write(&bound).map_err(|e| e.to_string())
            }
            None => h.write(&mol.system).map_err(|e| e.to_string()),
        }
    })();
    let _ = mol.system.set_state(prev); // restore the System's own state
    res
}

/// Draw the rep selection `TextEdit`. When `error_caret` is `Some(off)`, the text
/// from character `off` to the end is painted **red** (via a custom layouter),
/// highlighting the part of the selection where molar reported a parse error
/// (the caret position in its message). Returns the field's `Response`.
fn sel_text_edit(
    ui: &mut egui::Ui,
    text: &mut String,
    id: egui::Id,
    width: f32,
    error_caret: Option<usize>,
) -> egui::Response {
    let red = egui::Color32::from_rgb(240, 120, 120);
    let fmt = |font_id: egui::FontId, color: egui::Color32| egui::text::TextFormat {
        font_id,
        color,
        ..Default::default()
    };
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
        let s = buf.as_str();
        let font_id = egui::TextStyle::Body.resolve(ui.style());
        let base = ui.visuals().text_color();
        let mut job = egui::text::LayoutJob::default();
        match error_caret.filter(|_| !s.is_empty()) {
            Some(off) => {
                let nchars = s.chars().count();
                // If the caret sits at/after the end (an "expected more" error),
                // highlight the last character so there's always a visible mark.
                let off = off.min(nchars.saturating_sub(1));
                let split = s.char_indices().nth(off).map(|(b, _)| b).unwrap_or(s.len());
                job.append(&s[..split], 0.0, fmt(font_id.clone(), base));
                job.append(&s[split..], 0.0, fmt(font_id, red));
            }
            None => job.append(s, 0.0, fmt(font_id, base)),
        }
        ui.fonts_mut(|f| f.layout_job(job))
    };
    ui.add(
        egui::TextEdit::singleline(text)
            .id(id)
            .desired_width(width)
            .hint_text("selection")
            .layouter(&mut layouter),
    )
}

/// Workaround for a winit/egui IME bug seen on recent Wayland compositors: while a
/// text field is focused the compositor streams `Ime(Disabled)` events and delivers
/// every typed character as `Ime(Commit(..))` *without* a preceding `Ime(Enabled)` or
/// `Ime(Preedit)`. egui's `TextEdit` only honors a commit when its (preedit-derived)
/// IME cursor matches the live cursor, and that IME cursor is only updated by
/// `Enabled`/`Preedit` — so it stays at the post-focus position and only the **first**
/// keystroke is accepted; every later one (and any edit of pre-existing text) is
/// silently dropped, though paste and backspace still work. Rewriting each
/// `Ime(Commit(s))` into a plain `Text(s)` event routes it through egui's ungated
/// insertion path, and dropping the stray `Ime` events stops them from confusing the
/// state machine. Selection/name fields are ASCII, so IME composition isn't needed.
///
/// Linux-only: X11 emits no `Commit` events (characters arrive as `Text`), so this is
/// a no-op there, and macOS/Windows IME (which works) is left untouched.
#[cfg(target_os = "linux")]
fn defuse_broken_ime(ctx: &egui::Context) {
    ctx.input_mut(|i| {
        if !i.events.iter().any(|e| matches!(e, egui::Event::Ime(_))) {
            return;
        }
        for ev in &mut i.events {
            if let egui::Event::Ime(egui::ImeEvent::Commit(s)) = ev {
                let s = std::mem::take(s);
                *ev = egui::Event::Text(s);
            }
        }
        i.events.retain(|e| !matches!(e, egui::Event::Ime(_)));
    });
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

/// Above this atom count, draw-mode edits skip the automatic whole-molecule
/// perception + relax (relax is O(N²) per step). Editor-scale fragments relax/
/// aromatize live; a large loaded structure is edited without those.
const DRAW_AUTO_MAX_ATOMS: usize = 300;

/// Length (nm) of a freshly drawn bond before relaxation (a generic single bond).
const DRAW_BOND_LEN: f32 = 0.15;

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
    draw_info_box(
        &painter,
        rect,
        &[
            format!("{} {} {}", hit.name, hit.resname, hit.resid),
            format!("{:.3}, {:.3}, {:.3}", hit.real.x, hit.real.y, hit.real.z),
        ],
    );
}

/// Lower-left info box for the **Residues** hover mode: reports the hovered residue
/// (no ring — the steady GPU glow shows which atoms are highlighted).
fn draw_residue_info_overlay(ui: &egui::Ui, rect: egui::Rect, hit: &crate::pick::PickHit, n: usize) {
    draw_info_box(
        &ui.painter_at(rect),
        rect,
        &[
            format!("{} {}", hit.resname, hit.resid),
            format!("residue · {n} atom{}", if n == 1 { "" } else { "s" }),
        ],
    );
}

/// Draw a framed lower-left info box with `lines` of monospace text.
fn draw_info_box(painter: &egui::Painter, rect: egui::Rect, lines: &[String]) {
    let font = egui::FontId::monospace(13.0);
    let tc = egui::Color32::from_gray(240);
    let galleys: Vec<_> = lines
        .iter()
        .map(|l| painter.layout_no_wrap(l.clone(), font.clone(), tc))
        .collect();
    let pad = 6.0;
    let w = galleys.iter().map(|g| g.size().x).fold(0.0_f32, f32::max) + pad * 2.0;
    let line_h = galleys.first().map(|g| g.size().y).unwrap_or(0.0);
    let h = line_h * galleys.len() as f32 + pad * 2.0;
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
    for (i, g) in galleys.into_iter().enumerate() {
        painter.galley(egui::pos2(x + pad, y + pad + line_h * i as f32), g, tc);
    }
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

/// A plain text label vertically centered the same way `overlay_button` centers its
/// glyph (by ink bounds, at the toolbar button height) — so a label sitting next to
/// `overlay_button` dropdowns lines up with them instead of riding high/low.
fn toolbar_label(ui: &mut egui::Ui, text: &str) {
    const H: f32 = 26.0;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let col = ui.visuals().text_color();
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, col);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(galley.size().x, H), egui::Sense::hover());
    let ink = galley.mesh_bounds;
    ui.painter()
        .galley(rect.center() - ink.center().to_vec2(), galley, col);
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
/// Blinn-Phong shade of a surface point (screen-space unit normal `n`, +y down) for
/// a material preview — mirrors the lit shaders' model: `base·(amb + dif·N·L) +
/// spec·(N·H)^exp`, white highlight, plus the silhouette `outline` (grazing-rim
/// darkening) and the material `opacity` as alpha.
fn preview_shade(
    n: glam::Vec3,
    base: glam::Vec3,
    p: &MaterialParams,
    light: glam::Vec3,
    half: glam::Vec3,
    exp: f32,
    alpha: f32,
) -> egui::Color32 {
    let ndl = n.dot(light).max(0.0);
    let ndh = n.dot(half).max(0.0);
    let diff = p.ambient + p.diffuse * ndl;
    let spec = p.specular * ndh.powf(exp);
    let rim = (1.0 - n.z.max(0.0)).powi(2);
    let col = (base * diff + glam::Vec3::splat(spec)) * (1.0 - p.outline * 0.9 * rim);
    let col = col.clamp(glam::Vec3::ZERO, glam::Vec3::ONE);
    egui::Color32::from_rgba_unmultiplied(
        (col.x * 255.0) as u8,
        (col.y * 255.0) as u8,
        (col.z * 255.0) as u8,
        (alpha * 255.0) as u8,
    )
}

/// Append a shaded sphere (an `egui::Mesh` of a polar grid, each vertex shaded by
/// its surface normal) to `mesh`, for the material preview.
#[allow(clippy::too_many_arguments)]
fn push_preview_sphere(
    mesh: &mut egui::Mesh,
    center: egui::Pos2,
    radius: f32,
    base: glam::Vec3,
    p: &MaterialParams,
    light: glam::Vec3,
    half: glam::Vec3,
    exp: f32,
    alpha: f32,
) {
    const RINGS: usize = 6;
    const SEG: usize = 24;
    let start = mesh.vertices.len() as u32;
    mesh.colored_vertex(
        center,
        preview_shade(glam::Vec3::Z, base, p, light, half, exp, alpha),
    );
    for i in 1..=RINGS {
        let rr = i as f32 / RINGS as f32;
        for s in 0..SEG {
            let a = s as f32 / SEG as f32 * std::f32::consts::TAU;
            let (nx, ny) = (rr * a.cos(), rr * a.sin());
            let nz = (1.0 - rr * rr).max(0.0).sqrt();
            mesh.colored_vertex(
                center + egui::vec2(nx * radius, ny * radius),
                preview_shade(glam::Vec3::new(nx, ny, nz), base, p, light, half, exp, alpha),
            );
        }
    }
    let ring = |i: usize| start + 1 + ((i - 1) * SEG) as u32;
    for s in 0..SEG {
        mesh.add_triangle(start, ring(1) + s as u32, ring(1) + ((s + 1) % SEG) as u32);
    }
    for i in 1..RINGS {
        for s in 0..SEG {
            let (s1, n) = (s as u32, ((s + 1) % SEG) as u32);
            let (a0, a1) = (ring(i) + s1, ring(i) + n);
            let (b0, b1) = (ring(i + 1) + s1, ring(i + 1) + n);
            mesh.add_triangle(a0, b0, b1);
            mesh.add_triangle(a0, b1, a1);
        }
    }
}

/// Append a shaded bond (a side-on cylinder: a quad strip shaded across its width by
/// the cross-section normal) between `a` and `b` to `mesh`.
#[allow(clippy::too_many_arguments)]
fn push_preview_bond(
    mesh: &mut egui::Mesh,
    a: egui::Pos2,
    b: egui::Pos2,
    half_w: f32,
    base: glam::Vec3,
    p: &MaterialParams,
    light: glam::Vec3,
    half: glam::Vec3,
    exp: f32,
    alpha: f32,
) {
    const N: usize = 9;
    let dir = (b - a).normalized();
    let perp = egui::vec2(-dir.y, dir.x);
    let start = mesh.vertices.len() as u32;
    for end in [a, b] {
        for j in 0..N {
            let t = -1.0 + 2.0 * j as f32 / (N - 1) as f32;
            let nz = (1.0 - t * t).max(0.0).sqrt();
            let n = glam::Vec3::new(perp.x * t, perp.y * t, nz);
            mesh.colored_vertex(
                end + perp * (t * half_w),
                preview_shade(n, base, p, light, half, exp, alpha),
            );
        }
    }
    for j in 0..(N as u32 - 1) {
        let (a0, a1) = (start + j, start + j + 1);
        let (b0, b1) = (start + N as u32 + j, start + N as u32 + j + 1);
        mesh.add_triangle(a0, b0, b1);
        mesh.add_triangle(a0, b1, a1);
    }
}

/// Paint a material preview — two spheres joined by a bond, shaded with `material`'s
/// lighting (so Glossy/Metal/Diffuse/Glass/AO… read distinctly) — into `rect`.
fn paint_material_preview(painter: &egui::Painter, rect: egui::Rect, material: Material) {
    let p = material.params();
    let base = glam::Vec3::new(0.60, 0.62, 0.68);
    let light = glam::Vec3::new(-0.4, -0.5, 0.78).normalize();
    let half = (light + glam::Vec3::Z).normalize();
    let exp = 2.0 + p.shininess * 128.0;
    let alpha = (p.opacity * 0.85 + 0.15).clamp(0.0, 1.0); // floor so faint mats show
    let r = (rect.height() * 0.34).min(rect.width() * 0.22);
    let cy = rect.center().y;
    let a = egui::pos2(rect.center().x - r * 1.45, cy);
    let b = egui::pos2(rect.center().x + r * 1.45, cy);
    let mut mesh = egui::Mesh::default();
    // Bond first, then spheres on top (so the spheres cap the bond ends).
    push_preview_bond(&mut mesh, a, b, r * 0.5, base, &p, light, half, exp, alpha);
    push_preview_sphere(&mut mesh, a, r, base, &p, light, half, exp, alpha);
    push_preview_sphere(&mut mesh, b, r, base, &p, light, half, exp, alpha);
    painter.add(egui::Shape::mesh(mesh));
}

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

/// A grid cell in the material picker: a two-sphere-and-bond **preview** rendered
/// with the material's lighting, plus its label. Returns true if clicked.
fn material_cell(ui: &mut egui::Ui, material: Material, selected: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(82.0, 70.0), egui::Sense::click());
    if selected || resp.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    if selected {
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.5, ui.visuals().selection.bg_fill),
            egui::StrokeKind::Inside,
        );
    }
    let preview = egui::Rect::from_min_size(
        rect.left_top() + egui::vec2(4.0, 4.0),
        egui::vec2(rect.width() - 8.0, 44.0),
    );
    paint_material_preview(ui.painter(), preview, material);
    ui.painter().text(
        egui::pos2(rect.center().x, rect.bottom() - 11.0),
        egui::Align2::CENTER_CENTER,
        material.label(),
        egui::FontId::proportional(11.5),
        ui.visuals().text_color(),
    );
    resp.clicked()
}

/// A drawn material icon + label button that opens a **grid** of material previews
/// (each a two-sphere-and-bond fragment shaded with that material). A material
/// change forces a geometry rebuild (opacity/lighting are baked per geometry element).
fn material_picker(ui: &mut egui::Ui, rep: &mut Representation) {
    let material = rep.material;
    let resp = picker_button(ui, material.label(), |p, r| paint_material_icon(p, r, material));

    egui::Popup::menu(&resp).show(|ui| {
        egui::Grid::new("material_grid")
            .spacing(egui::vec2(4.0, 4.0))
            .show(ui, |ui| {
                for (i, material) in Material::ALL.into_iter().enumerate() {
                    if material_cell(ui, material, material == rep.material) {
                        rep.material = material;
                        rep.geom_dirty = true;
                        ui.close();
                    }
                    if (i + 1) % 3 == 0 {
                        ui.end_row();
                    }
                }
            });
    });
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
fn tab_bar<T: Copy + PartialEq>(ui: &mut egui::Ui, current: &mut T, tabs: &[(T, &str)]) -> bool {
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
fn slider_with_edit(
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
fn color_submenu(ui: &mut egui::Ui, _id: &str, c: &mut [f32; 4]) {
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

// --- Program-settings dialog: one function per tab (see `App::draw_settings_dialog`). ---

/// Appearance tab: theme mode, UI font scale, accent color.
fn settings_page_appearance(ui: &mut egui::Ui, s: &mut Settings) {
    let a = &mut s.appearance;
    egui::Grid::new("set_appearance")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Theme");
            egui::ComboBox::from_id_salt("set_theme")
                .selected_text(a.theme.label())
                .show_ui(ui, |ui| {
                    for t in ThemeMode::ALL {
                        ui.selectable_value(&mut a.theme, t, t.label());
                    }
                });
            ui.end_row();

            ui.label("UI font scale");
            slider_with_edit(ui, &mut a.font_scale, 0.7..=1.6, true);
            ui.end_row();

            ui.label("Accent color");
            color_submenu(ui, "set_accent", &mut a.accent);
            ui.end_row();
        });
    ui.add_space(4.0);
    ui.weak("Theme and font scale apply immediately when you press Save.");
}

/// Rendering tab: anti-aliasing (SSAA) and shadow-map resolution.
fn settings_page_rendering(ui: &mut egui::Ui, s: &mut Settings) {
    let r = &mut s.rendering;
    let ssaa_label = |n: u32| match n {
        1 => "Off (1×)",
        2 => "2× (default)",
        3 => "3×",
        4 => "4×",
        _ => "?",
    };
    egui::Grid::new("set_rendering")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Anti-aliasing");
            egui::ComboBox::from_id_salt("set_ssaa")
                .selected_text(ssaa_label(r.ssaa))
                .show_ui(ui, |ui| {
                    for n in [1u32, 2, 3, 4] {
                        ui.selectable_value(&mut r.ssaa, n, ssaa_label(n));
                    }
                });
            ui.end_row();

            ui.label("Shadow-map resolution");
            egui::ComboBox::from_id_salt("set_shadow_res")
                .selected_text(format!("{}²", r.shadow_res))
                .show_ui(ui, |ui| {
                    for n in [1024u32, 2048, 4096] {
                        ui.selectable_value(&mut r.shadow_res, n, format!("{n}²"));
                    }
                });
            ui.end_row();
        });
    ui.add_space(4.0);
    ui.weak("Supersampling smooths everything but costs ~ssaa² more fragments. The");
    ui.weak("shadow map only matters when cast shadows are on. Both apply on Save.");
}

/// View tab: defaults seeded onto a **new** scene's camera (projection, background,
/// depth cue, AO, shadows). Returns true if "Apply to current view" was clicked.
fn settings_page_view(ui: &mut egui::Ui, s: &mut Settings) -> bool {
    let v = &mut s.view;
    let proj_label = |p: Projection| match p {
        Projection::Perspective => "Perspective",
        Projection::Orthographic => "Orthographic",
    };
    egui::Grid::new("set_view")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Projection");
            egui::ComboBox::from_id_salt("set_proj")
                .selected_text(proj_label(v.projection))
                .show_ui(ui, |ui| {
                    for p in [Projection::Orthographic, Projection::Perspective] {
                        ui.selectable_value(&mut v.projection, p, proj_label(p));
                    }
                });
            ui.end_row();

            ui.label("Frame fill");
            slider_with_edit(ui, &mut v.fill, 0.5..=1.0, true);
            ui.end_row();
        });

    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Background");
        ui.radio_value(&mut v.background.kind, BgKind::Solid, "Solid");
        ui.radio_value(&mut v.background.kind, BgKind::Gradient, "Gradient");
    });
    ui.horizontal(|ui| match v.background.kind {
        BgKind::Solid => {
            ui.label("Color");
            color_submenu(ui, "set_bg", &mut v.background.color);
        }
        BgKind::Gradient => {
            ui.label("Top");
            color_submenu(ui, "set_bg_top", &mut v.background.top);
            ui.label("Bottom");
            color_submenu(ui, "set_bg_bot", &mut v.background.bottom);
        }
    });

    ui.separator();
    ui.checkbox(&mut v.depth_cue.enabled, "Depth cue (fog)");
    let cue_on = v.depth_cue.enabled;
    egui::Grid::new("set_cue")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.add_enabled(cue_on, egui::Label::new("Falloff"));
            ui.add_enabled_ui(cue_on, |ui| {
                egui::ComboBox::from_id_salt("set_cue_mode")
                    .selected_text(v.depth_cue.mode.label())
                    .show_ui(ui, |ui| {
                        for m in CueMode::ALL {
                            ui.selectable_value(&mut v.depth_cue.mode, m, m.label());
                        }
                    });
            });
            ui.end_row();
            ui.add_enabled(cue_on, egui::Label::new("Strength"));
            slider_with_edit(ui, &mut v.depth_cue.strength, 0.0..=1.0, cue_on);
            ui.end_row();
            ui.add_enabled(cue_on, egui::Label::new("Start"));
            slider_with_edit(ui, &mut v.depth_cue.start, 0.0..=1.0, cue_on);
            ui.end_row();
        });

    ui.separator();
    ui.checkbox(&mut v.ao.enabled, "Ambient occlusion");
    let ao_on = v.ao.enabled;
    egui::Grid::new("set_ao")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.add_enabled(ao_on, egui::Label::new("Strength"));
            slider_with_edit(ui, &mut v.ao.strength, 0.0..=1.0, ao_on);
            ui.end_row();
            ui.add_enabled(ao_on, egui::Label::new("Radius (nm)"));
            slider_with_edit(ui, &mut v.ao.radius, 0.05..=1.5, ao_on);
            ui.end_row();
        });

    ui.separator();
    ui.checkbox(&mut v.shadow.enabled, "Cast shadows");
    let sh_on = v.shadow.enabled;
    egui::Grid::new("set_shadow")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.add_enabled(sh_on, egui::Label::new("Strength"));
            slider_with_edit(ui, &mut v.shadow.strength, 0.0..=1.0, sh_on);
            ui.end_row();
        });

    ui.separator();
    let apply = ui
        .button("Apply to current view")
        .on_hover_text("Push these view defaults onto the open scene now (without saving)")
        .clicked();
    ui.weak("These seed new scenes; a loaded session keeps its own saved view.");
    apply
}

/// Representations tab: the defaults for each newly created representation.
fn settings_page_reps(ui: &mut egui::Ui, s: &mut Settings) {
    let r = &mut s.reps;
    egui::Grid::new("set_reps")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Style");
            egui::ComboBox::from_id_salt("set_rep_kind")
                .selected_text(r.kind.label())
                .show_ui(ui, |ui| {
                    for k in RepKind::ALL {
                        ui.selectable_value(&mut r.kind, k, k.label());
                    }
                });
            ui.end_row();

            ui.label("Color");
            egui::ComboBox::from_id_salt("set_rep_color")
                .selected_text(r.color.label())
                .show_ui(ui, |ui| {
                    for c in ColorMethod::ALL {
                        ui.selectable_value(&mut r.color, c, c.label());
                    }
                });
            ui.end_row();

            ui.label("Material");
            egui::ComboBox::from_id_salt("set_rep_material")
                .selected_text(r.material.label())
                .show_ui(ui, |ui| {
                    for m in Material::ALL {
                        ui.selectable_value(&mut r.material, m, m.label());
                    }
                });
            ui.end_row();

            ui.label("Surface quality");
            ui.add(egui::DragValue::new(&mut r.surface_quality).range(0..=4));
            ui.end_row();

            ui.label("Default selection");
            ui.add(egui::TextEdit::singleline(&mut r.selection).desired_width(220.0));
            ui.end_row();
        });
    ui.add_space(4.0);
    ui.weak("Used for the first representation of a newly loaded molecule and the");
    ui.weak("“+ rep” button.");
}

/// Behavior tab: mouse sensitivity, default pick/selection modes, trajectory
/// playback, and bond-guessing thresholds.
fn settings_page_behavior(ui: &mut egui::Ui, s: &mut Settings) {
    let b = &mut s.behavior;
    egui::Grid::new("set_behavior_mouse")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Orbit sensitivity");
            slider_with_edit(ui, &mut b.orbit_sensitivity, 0.2..=3.0, true);
            ui.end_row();
            ui.label("Roll sensitivity");
            slider_with_edit(ui, &mut b.roll_sensitivity, 0.2..=3.0, true);
            ui.end_row();
        });

    ui.separator();
    egui::Grid::new("set_behavior_pick")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Default pick mode");
            egui::ComboBox::from_id_salt("set_pick_mode")
                .selected_text(b.pick_mode.label())
                .show_ui(ui, |ui| {
                    for m in [PickMode::Off, PickMode::Click, PickMode::Lasso] {
                        ui.selectable_value(&mut b.pick_mode, m, m.label());
                    }
                });
            ui.end_row();
            ui.label("Default selection scope");
            egui::ComboBox::from_id_salt("set_sel_mode")
                .selected_text(b.selection_mode.label())
                .show_ui(ui, |ui| {
                    for m in [
                        SelectionMode::Atoms,
                        SelectionMode::Residues,
                        SelectionMode::BoundH,
                    ] {
                        ui.selectable_value(&mut b.selection_mode, m, m.label());
                    }
                });
            ui.end_row();
        });

    ui.separator();
    egui::Grid::new("set_behavior_traj")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Trajectory FPS");
            slider_with_edit(ui, &mut b.traj_fps, 1.0..=60.0, true);
            ui.end_row();
            ui.label("Loop playback");
            let mut looping = b.loop_mode == LoopMode::Loop;
            if ui.checkbox(&mut looping, "").changed() {
                b.loop_mode = if looping { LoopMode::Loop } else { LoopMode::Once };
            }
            ui.end_row();
        });

    ui.separator();
    ui.label("Bond detection (next structure loaded)");
    egui::Grid::new("set_behavior_bonds")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("VDW factor");
            slider_with_edit(ui, &mut b.bond_factor, 0.3..=1.0, true);
            ui.end_row();
            ui.label("Search cutoff (nm)");
            slider_with_edit(ui, &mut b.bond_search_cutoff, 0.1..=0.5, true);
            ui.end_row();
            ui.label("Min distance (nm)");
            slider_with_edit(ui, &mut b.bond_min_dist, 0.0..=0.1, true);
            ui.end_row();
        });
    ui.checkbox(&mut b.bond_search_periodic, "Periodic search (bonds across box faces)")
        .on_hover_text(
            "Minimum-image bond search: also finds covalent bonds that cross a box face in a \
             wrapped structure. Off (default) is much faster for large structures.",
        );

    ui.separator();
    ui.label("Periodic rendering");
    ui.checkbox(&mut b.dashed_pbc_bonds, "Dashed wrap-around bonds")
        .on_hover_text(
            "Draw bonds that span a box face as dashed minimum-image half-bonds (and split \
             cartoon ribbons at the boundary). Off draws them as plain solid bonds. Applies to \
             the current scene.",
        );
}

/// The orientation-axes "screen" widget: a monitor-like rectangle showing a mini
/// downsampled render of the scene, an on/off checkbox in its center, and a corner
/// radio **outside** each of the four corners (where the gizmo is anchored):
/// ```text
///   (o)          (o)
///      +--------+
///      |  [v]   |
///      +--------+
///   (o)          (o)
/// ```
fn draw_axes_widget(
    ui: &mut egui::Ui,
    on: &mut bool,
    corner: &mut Corner,
    scene_tex: Option<egui::TextureId>,
) {
    let radio = 18.0;
    let margin = 22.0;
    let screen = egui::vec2(128.0, 82.0);
    let total = egui::vec2(screen.x + 2.0 * margin, screen.y + 2.0 * margin);
    let (rect, _) = ui.allocate_exact_size(total, egui::Sense::hover());
    let screen_rect = egui::Rect::from_center_size(rect.center(), screen);

    // The "screen": a mini downsampled render of the scene (last frame), or a dark
    // fill if no texture is available yet.
    let painter = ui.painter();
    painter.rect_filled(screen_rect, 4.0, ui.visuals().extreme_bg_color);
    if let Some(tex) = scene_tex {
        painter.image(
            tex,
            screen_rect.shrink(2.0),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
    painter.rect_stroke(
        screen_rect,
        4.0,
        egui::Stroke::new(1.0, ui.visuals().widgets.inactive.fg_stroke.color),
        egui::StrokeKind::Inside,
    );

    // Corner radios at the widget's four outer corners (outside the screen rect).
    let h = radio * 0.5;
    let spots = [
        (Corner::TopLeft, egui::pos2(rect.left() + h, rect.top() + h)),
        (Corner::TopRight, egui::pos2(rect.right() - h, rect.top() + h)),
        (Corner::BottomLeft, egui::pos2(rect.left() + h, rect.bottom() - h)),
        (Corner::BottomRight, egui::pos2(rect.right() - h, rect.bottom() - h)),
    ];
    for (c, pos) in spots {
        let r = egui::Rect::from_center_size(pos, egui::vec2(radio, radio));
        if ui.put(r, egui::RadioButton::new(*corner == c, "")).clicked() {
            *corner = c;
        }
    }

    // On/off checkbox in the center of the screen, on a translucent backing so it
    // stays legible over the mini render.
    let cb = egui::Rect::from_center_size(screen_rect.center(), egui::vec2(26.0, 24.0));
    ui.painter()
        .rect_filled(cb.expand(3.0), 5.0, egui::Color32::from_black_alpha(170));
    ui.put(cb, egui::Checkbox::new(on, ""))
        .on_hover_text("Show orientation axes");
}

fn draw_rep_params(ui: &mut egui::Ui, rep: &mut Representation, has_box: bool) -> bool {
    let mut view_dirty = false;
    // The Periodic tab only exists when the molecule has a box; if it was the
    // active tab and the box went away, fall back to Style.
    if !has_box && rep.settings_tab == SettingsTab::Periodic {
        rep.settings_tab = SettingsTab::Style;
    }
    // Tab bar: [Style] [Traj] [Periodic?] — the app's standard underline tabs.
    let mut tabs = vec![(SettingsTab::Style, "Style"), (SettingsTab::Traj, "Traj")];
    if has_box {
        tabs.push((SettingsTab::Periodic, "Periodic"));
    }
    tab_bar(ui, &mut rep.settings_tab, &tabs);
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

    // Row 1: play · frame/total · fps · loop · zoom · step.
    ui.horizontal(|ui| {
        compact_actions(ui);

        let play_glyph = if traj.playing { icon::PAUSE } else { icon::PLAY };
        if ui
            .selectable_label(traj.playing, play_glyph)
            .on_hover_text(if traj.playing { "Pause" } else { "Play" })
            .clicked()
        {
            traj.set_playing(!traj.playing);
        }

        ui.separator();
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

        ui.separator();
        // Playback speed (frames per second).
        ui.add(
            egui::DragValue::new(&mut traj.speed_fps)
                .range(1.0..=120.0)
                .suffix(" fps")
                .fixed_decimals(0),
        )
        .on_hover_text("Playback speed");

        ui.separator();
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

        ui.separator();
        // Slider zoom toggle — only useful (and enabled) for long trajectories; it
        // narrows the scrub slider to a ±25-frame window around the current frame.
        let can_zoom = n > 50;
        if !can_zoom {
            traj.slider_zoom = false;
        }
        ui.add_enabled_ui(can_zoom, |ui| {
            if ui
                .selectable_label(traj.slider_zoom, icon::MAGNIFYING_GLASS_PLUS)
                .on_hover_text("Zoom the scrub slider to ±25 frames around the current frame")
                .clicked()
            {
                traj.slider_zoom = !traj.slider_zoom;
            }
        });

        ui.separator();
        // Playback step (skip frames while playing).
        ui.label("step");
        let mut step = traj.play_step.max(1);
        if ui
            .add(egui::DragValue::new(&mut step).range(1..=last.max(1)))
            .on_hover_text("Frames to advance per playback step")
            .changed()
        {
            traj.play_step = step.max(1);
        }
    });

    // Row 2: first · back · [full-width scrub slider] · forward · last.
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

        // The slider stretches across the row between the flanking step buttons.
        // Zoomed: a ±25-frame window around the current frame (finer scrubbing on a
        // long trajectory); otherwise the full range.
        let (lo, hi) = if traj.slider_zoom && n > 50 {
            (traj.current.saturating_sub(25), (traj.current + 25).min(last))
        } else {
            (0, last)
        };
        // Reserve room for the two trailing buttons (forward, last) + spacing.
        let reserve = 52.0;
        ui.spacing_mut().slider_width = (ui.available_width() - reserve).max(40.0);
        let mut cur = traj.current;
        let resp = ui.add(egui::Slider::new(&mut cur, lo..=hi).show_value(false));
        if resp.changed() {
            traj.set_playing(false);
            traj.set_current(cur);
        }
        if let Some(t) = traj.current_time() {
            resp.on_hover_text(format!("frame {} — t = {:.3}", traj.current, t));
        }

        if icon_button(ui, icon::CARET_RIGHT, "Step forward").clicked() {
            traj.set_playing(false);
            traj.step(1);
        }
        if icon_button(ui, icon::SKIP_FORWARD, "Last frame").clicked() {
            traj.set_playing(false);
            traj.set_current(last);
        }
    });

    traj.current != before
}

pub struct App {
    renderer: SceneRenderer,
    camera: Camera,
    scene: Scene,
    /// Persisted program settings (theme, render quality, new-document defaults,
    /// behavior). Loaded on launch from the platform config dir; edited via the
    /// settings dialog (see `settings_draft`).
    settings: Settings,
    /// Effective defaults for a new representation = `settings.reps`, with the kind
    /// overridden by the `MOLAR_VIS_DEBUG_REP` env hook. Recomputed when settings
    /// change. Used for the initial rep of each loaded molecule + the add-rep button.
    rep_defaults: RepDefaults,
    /// Working copy of the settings while the settings dialog is open (edit-then-
    /// apply); `None` when the dialog is closed.
    settings_draft: Option<Settings>,
    /// Active tab in the settings dialog.
    settings_tab: SettingsPage,
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
    /// Cached per-molecule selection hints (distinct chains/resnames/names +
    /// numeric ranges), shown under the selection field while editing. Computed
    /// lazily on first edit of a molecule (topology is static); keyed by [`MolId`].
    sel_hints: HashMap<MolId, SelHints>,
    /// Open trajectory-load dialog, if any (one at a time).
    load_dialog: Option<LoadDialog>,
    /// Open "delete trajectory frames" dialog, if any.
    delete_frames_dialog: Option<DeleteFramesDialog>,
    /// Open "rename molecule" dialog: the target molecule + the edit buffer.
    rename_mol: Option<(MolId, String)>,
    /// In-flight background trajectory loaders, keyed by molecule (so they
    /// survive reorder/delete/undo). Drained each frame via `try_recv`.
    loaders: HashMap<MolId, Receiver<LoadMsg>>,
    /// Picking mode (top view-toolbar dropdown). `Click` shows the hovered atom's
    /// identity + glow and selects it on click; `Lasso` drags a freehand selection
    /// polygon.
    pick_mode: PickMode,
    /// How a lasso expands its hit atoms (viewport-overlay dropdown): exact atoms,
    /// whole residues, or heavy atoms + their bonded hydrogens.
    selection_mode: SelectionMode,
    /// In-progress lasso polygon (viewport pixel coords), accumulated while
    /// dragging in `PickMode::Lasso`. Empty when not lassoing. Transient view state.
    lasso_path: Vec<egui::Pos2>,
    /// Last cursor NDC the hover detail lens was rebuilt at, so it only rebuilds as
    /// the cursor actually moves (the fade follows the ray, so any move rebuilds).
    last_lens_ndc: Option<(f32, f32)>,
    /// Last completed GPU hover pick `(mol, rep, atom)` (native only). The async
    /// id-buffer readback lags a frame or two, so the hit is cached here and the
    /// `PickHit` is rebuilt from it each frame. `None` = nothing hovered.
    #[cfg(not(target_arch = "wasm32"))]
    hover_pick: Option<(usize, usize, usize)>,
    /// Pick-target pixel of the last requested GPU pick (native). A new pick is only
    /// requested when the cursor moves or the view changes, so a stationary hover
    /// stays idle (0 GPU) instead of re-picking every frame.
    #[cfg(not(target_arch = "wasm32"))]
    last_pick_px: Option<(u32, u32)>,
    /// Whether the VMD-style orientation axes gizmo is shown in the viewport.
    axes_on: bool,
    /// Which viewport corner the axes gizmo is anchored to.
    axes_corner: Corner,
    /// Active tab in the top-bar "view settings" (hamburger) menu.
    view_tab: ViewTab,
    /// Whether the view-settings (hamburger) window is open. A real `Window` rather
    /// than a `Popup` so nested click-to-open dropdowns / color pickers work; closed
    /// manually on a click outside it (see `view_settings_window`).
    view_menu_open: bool,
    /// The view-settings window's rect **as drawn last frame** — the geometry the user
    /// actually clicked on. The close-on-click-outside test must use this, not the
    /// current frame's rect: switching tabs re-lays-out the (right-pivoted) window in
    /// the *same* frame, so the freshly-narrowed rect no longer covers the leftmost
    /// tab the click landed on (see `view_settings_window`).
    view_menu_rect: Option<egui::Rect>,
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
    /// Active interactive-drawing session (Draw mode), or `None` when off. Mutually
    /// exclusive with the pick modes (`pick_mode`): turning Draw on forces `pick_mode
    /// = Off`, and choosing any pick mode clears `draw`. See the Draw-mode types at
    /// the bottom of this file.
    draw: Option<DrawSession>,
}

/// Tabs in the top-bar "view settings" (hamburger) menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum ViewTab {
    #[default]
    Camera,
    Lighting,
    Scene,
}

/// Tabs in the program-settings dialog (the cogwheel modal).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum SettingsPage {
    #[default]
    Appearance,
    Rendering,
    View,
    Representations,
    Behavior,
}

/// A viewport corner, for anchoring the axes gizmo.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

/// State of the "Load trajectory" modal.
struct LoadDialog {
    mol_id: MolId,
    path: Option<PathBuf>,
    from: usize,
    /// Last frame to read, as text. **Empty = read to the end of the file.**
    to_text: String,
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
            to_text: String::new(),
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

/// How the "Delete frames" dialog selects which frames to drop.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DeleteFramesMode {
    /// Delete the inclusive frame range `[from, to]`.
    Range,
    /// Keep every `stride`-th frame, drop the rest.
    Decimate,
}

/// State of the "Delete frames" modal (trajectory frame deletion).
struct DeleteFramesDialog {
    mol_id: MolId,
    mode: DeleteFramesMode,
    from: usize,
    to: usize,
    stride: usize,
}

impl DeleteFramesDialog {
    fn new(mol_id: MolId) -> Self {
        Self { mol_id, mode: DeleteFramesMode::Range, from: 0, to: 0, stride: 2 }
    }
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        launch: AppLaunch,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Program settings: load from the platform config dir (created with defaults
        // on first launch). `MOLAR_VIS_DEBUG_DEFAULTS=1` forces built-in defaults
        // (no file IO) so headless verification is reproducible and never depends on
        // the dev's saved config. WASM has no filesystem, so it always uses defaults.
        let settings = {
            #[cfg(not(target_arch = "wasm32"))]
            {
                if std::env::var("MOLAR_VIS_DEBUG_DEFAULTS").is_ok() {
                    Settings::default()
                } else {
                    Settings::load_or_create()
                }
            }
            #[cfg(target_arch = "wasm32")]
            {
                Settings::default()
            }
        };

        crate::theme::apply(&cc.egui_ctx, &settings.appearance);

        let render_state = cc
            .wgpu_render_state
            .as_ref()
            .ok_or("wgpu render state unavailable (eframe must use the wgpu backend)")?;
        let renderer = SceneRenderer::new(render_state, &settings.rendering);

        // New-representation defaults come from the settings; VMD's default style is
        // Lines. MOLAR_VIS_DEBUG_REP=vdw|licorice|… still overrides the kind for
        // headless checks.
        let rep_defaults = Self::effective_rep_defaults(&settings);
        let bond_params = settings.behavior.bond_params();

        let mut scene = Scene::default();
        let mut status = String::new();
        // VMD-style command-line grouping: each `launch.files` entry is one molecule's
        // file list — the first file is the structure (topology + frame 0) and any
        // following files load as appended trajectory states. `-m` on the command line
        // starts a new molecule (see `launch::parse_file_args`).
        for group in &launch.files {
            let (structure, extra) = match group.split_first() {
                Some(parts) => parts,
                None => continue,
            };
            let raw = match data::load_with(structure, &bond_params) {
                Ok(raw) => raw,
                Err(e) => {
                    log::error!("{e}");
                    status = e;
                    continue;
                }
            };
            scene.add(raw, &rep_defaults);
            let mol = scene.molecules.last_mut().unwrap();
            mol.trajectory.speed_fps = settings.behavior.traj_fps;
            mol.trajectory.loop_mode = settings.behavior.loop_mode;
            // Build the trajectory from ALL frames in the group, VMD-style: the **first
            // file's frames beyond frame 0** (frame 0 is the structure just loaded — a
            // multi-MODEL/trajectory structure file thus contributes all its frames),
            // then every extra file's frames. `seed_frame0` (idempotent) makes frame 0
            // the structure; only files that actually yield frames are recorded as
            // trajectory loads, so a plain single-frame structure stays static.
            #[cfg(not(target_arch = "wasm32"))]
            {
                let n = mol.n_atoms;
                let mut seeded = false;
                // (path, from): the first file is read from frame 1 (frame 0 is the
                // structure); extra files from frame 0.
                let sources = std::iter::once((structure, 1usize))
                    .chain(extra.iter().map(|p| (p, 0usize)));
                for (path, from) in sources {
                    let opts = LoadOptions { from, to: None, stride: 1 };
                    match data::traj_loader::read_frames_sync(path, &opts, n) {
                        Ok(frames) if !frames.is_empty() => {
                            if !seeded {
                                mol.seed_frame0();
                                seeded = true;
                            }
                            mol.append_frames(frames);
                            mol.traj_loads.push(crate::scene::TrajLoad {
                                path: path.clone(),
                                from,
                                to: None,
                                stride: 1,
                            });
                        }
                        Ok(_) => {} // no frames in this file (e.g. single-MODEL structure)
                        Err(e) => {
                            log::error!("trajectory {}: {e}", path.display());
                            status = e;
                        }
                    }
                }
                if seeded {
                    mol.apply_current_frame();
                }
            }
            #[cfg(target_arch = "wasm32")]
            let _ = extra;
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
                        mol.traj_loads.push(crate::scene::TrajLoad {
                            path: std::path::PathBuf::from(&traj_path),
                            from: opts.from,
                            to: opts.to,
                            stride: opts.stride,
                        });
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
            Some((min, max)) => Camera::frame_bbox(min, max, settings.view.fill),
            None => Camera::default(),
        };
        // Seed the fresh camera with the user's default view (projection, depth-cue,
        // AO, shadows, background). The debug hooks below override specific fields.
        settings.view.seed_camera(&mut camera);
        if let Ok(deg) = std::env::var("MOLAR_VIS_DEBUG_ORBIT") {
            if let Ok(d) = deg.parse::<f32>() {
                camera.orbit(d, d * 0.4, 1.0);
            }
        }
        if std::env::var("MOLAR_VIS_DEBUG_ORTHO").is_ok() {
            camera.projection = Projection::Orthographic;
        }
        if std::env::var("MOLAR_VIS_DEBUG_PERSP").is_ok() {
            camera.projection = Projection::Perspective;
        }
        // Verification hook: MOLAR_VIS_DEBUG_ZOOM=<factor> dollies out (factor > 1).
        if let Ok(f) = std::env::var("MOLAR_VIS_DEBUG_ZOOM") {
            if let Ok(f) = f.parse::<f32>() {
                camera.distance *= f.max(0.05);
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_CUEMODE=linear|exp|exp2 sets the depth-
        // cue falloff curve (and bumps strength so it's visible in a screenshot).
        if let Ok(m) = std::env::var("MOLAR_VIS_DEBUG_CUEMODE") {
            camera.depth_cue.mode = match m.to_ascii_lowercase().as_str() {
                "exp" => CueMode::Exp,
                "exp2" => CueMode::Exp2,
                _ => CueMode::Linear,
            };
            camera.depth_cue.enabled = true;
            camera.depth_cue.strength = 0.9;
            camera.depth_cue.start = 0.0;
        }
        // Verification hook: MOLAR_VIS_DEBUG_AO[=strength] enables ambient occlusion.
        if let Ok(v) = std::env::var("MOLAR_VIS_DEBUG_AO") {
            camera.ao.enabled = true;
            if let Ok(s) = v.trim().parse::<f32>() {
                camera.ao.strength = s.clamp(0.0, 1.0);
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_SHADOW[=strength] enables cast shadows.
        if let Ok(v) = std::env::var("MOLAR_VIS_DEBUG_SHADOW") {
            camera.shadow.enabled = true;
            if let Ok(s) = v.trim().parse::<f32>() {
                camera.shadow.strength = s.clamp(0.0, 1.0);
            }
        }
        // Verification hook: MOLAR_VIS_DEBUG_BG=gradient|white sets the background.
        if let Ok(v) = std::env::var("MOLAR_VIS_DEBUG_BG") {
            match v.trim().to_ascii_lowercase().as_str() {
                "gradient" => camera.background.kind = crate::camera::BgKind::Gradient,
                "white" => camera.background.color = [0.95, 0.95, 0.95, 1.0],
                _ => {}
            }
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

        // Compute these before the struct init moves `settings` in.
        let pick_mode = if std::env::var("MOLAR_VIS_DEBUG_PICK").is_ok() {
            PickMode::Click
        } else {
            settings.behavior.pick_mode
        };
        let selection_mode = match std::env::var("MOLAR_VIS_DEBUG_SELMODE").as_deref() {
            Ok("residues") => SelectionMode::Residues,
            Ok("boundh") => SelectionMode::BoundH,
            _ => settings.behavior.selection_mode,
        };

        #[allow(unused_mut)]
        let mut app = Self {
            renderer,
            camera,
            scene,
            settings,
            rep_defaults,
            settings_draft: None,
            settings_tab: SettingsPage::default(),
            last_render_camera: None,
            last_size: [0, 0],
            view_dirty: true,
            status,
            history,
            pending_undo_n: None,
            pending_redo_n: None,
            editing_rep: None,
            sel_hints: HashMap::new(),
            load_dialog: None,
            delete_frames_dialog: None,
            rename_mol: None,
            loaders: HashMap::new(),
            pick_mode,
            selection_mode,
            lasso_path: Vec::new(),
            last_lens_ndc: None,
            #[cfg(not(target_arch = "wasm32"))]
            hover_pick: None,
            #[cfg(not(target_arch = "wasm32"))]
            last_pick_px: None,
            axes_on: std::env::var("MOLAR_VIS_DEBUG_AXES").is_ok(),
            axes_corner: Corner::BottomRight,
            view_tab: ViewTab::default(),
            view_menu_open: std::env::var("MOLAR_VIS_DEBUG_VIEWMENU").is_ok(),
            view_menu_rect: None,
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
            draw: None,
        };

        // Verification hooks (native): exercise the session save/load round-trip
        // headlessly, since the rfd file dialogs can't be driven in a headless run.
        // MOLAR_VIS_DEBUG_LOAD_SESSION=<path> replaces the scene from a session
        // file; MOLAR_VIS_DEBUG_SAVE_SESSION=<path> writes the current state out.
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Ok(path) = std::env::var("MOLAR_VIS_DEBUG_LOAD_SESSION") {
                app.load_session_from(std::path::Path::new(&path));
            }
            if let Ok(path) = std::env::var("MOLAR_VIS_DEBUG_SAVE_SESSION") {
                app.save_session_to(std::path::Path::new(&path));
            }
            // MOLAR_VIS_DEBUG_SAVE_MOL=<path> writes mol 0 to a structure file
            // (exercises the molar FileHandler write + displayed-frame swap path).
            if let Ok(path) = std::env::var("MOLAR_VIS_DEBUG_SAVE_MOL") {
                if let Some(mol) = app.scene.molecules.first_mut() {
                    match save_displayed(mol, std::path::Path::new(&path), None) {
                        Ok(()) => log::info!("debug: saved molecule to {path}"),
                        Err(e) => log::error!("debug save molecule failed: {e}"),
                    }
                }
            }
        }

        // Verification hook: MOLAR_VIS_DEBUG_SETTINGS=1 (or =appearance|rendering|
        // view|reps|behavior) opens the program-settings modal at the given tab at
        // startup (it can't be driven by a mouse in a headless run), so each tab can
        // be screenshot. Pair with MOLAR_VIS_DEBUG_DEFAULTS=1 to keep the shown values
        // reproducible regardless of the saved config.
        if let Ok(tab) = std::env::var("MOLAR_VIS_DEBUG_SETTINGS") {
            app.settings_draft = Some(app.settings.clone());
            app.settings_tab = match tab.to_ascii_lowercase().as_str() {
                "rendering" => SettingsPage::Rendering,
                "view" => SettingsPage::View,
                "reps" | "representations" => SettingsPage::Representations,
                "behavior" => SettingsPage::Behavior,
                _ => SettingsPage::Appearance,
            };
        }

        // Verification hook: MOLAR_VIS_DEBUG_DELFRAMES=1 opens the delete-frames
        // dialog for mol 0 (pair with MOLAR_VIS_DEBUG_TRAJ to have frames).
        if std::env::var("MOLAR_VIS_DEBUG_DELFRAMES").is_ok() {
            if let Some(mol) = app.scene.molecules.first() {
                app.delete_frames_dialog = Some(DeleteFramesDialog::new(mol.id));
            }
        }

        // Verification hook: MOLAR_VIS_DEBUG_EDIT_REP=1 opens mol 0's first rep
        // selection field in edit mode, so the contextual suggestion hint (and a
        // selection error's in-field highlight) can be screenshot headlessly.
        if std::env::var("MOLAR_VIS_DEBUG_EDIT_REP").is_ok()
            && app.scene.molecules.first().is_some_and(|m| !m.reps.is_empty())
        {
            app.editing_rep = Some((0, 0));
        }

        // Verification hook: MOLAR_VIS_DEBUG_DRAW=<preset> builds a small molecule
        // through the *same* `Molecule` edit helpers the Draw-mode UI uses (single_atom
        // → add_atom/add_bond with rough coords), turns Draw mode on, gives it a
        // Ball-and-Stick rep, frames the camera, and relaxes it once — so the drawing
        // path can be exercised without a mouse on this headless (Wayland) box. Pair
        // with `RUST_LOG=molar_vis_core=info` to see the atom/bond counts + a bond
        // length after relaxation. Presets: methane, ethane, water, benzene.
        if let Ok(preset) = std::env::var("MOLAR_VIS_DEBUG_DRAW") {
            app.debug_draw_preset(&preset.to_ascii_lowercase());
        }

        Ok(app)
    }

    /// Headless verification: build a known small molecule via the Draw-mode edit
    /// helpers and relax it. Enters Draw mode (so the toolbar/viewport state is
    /// realistic). Logs the result at `info`. Native + wasm safe (pure CPU + molar).
    fn debug_draw_preset(&mut self, preset: &str) {
        use crate::minimize::{BondOrder, RelaxKind};
        // (element, x, y, z) seed atoms + (i, j, order) bonds — rough/strained coords
        // (a hand-drawn sketch), all in nm. The minimizer is what makes them sensible.
        let atoms: Vec<(Element, glam::Vec3)>;
        let bonds: Vec<(usize, usize, BondOrder)>;
        match preset {
            "water" => {
                atoms = vec![
                    (Element::O, glam::vec3(0.0, 0.0, 0.0)),
                    (Element::H, glam::vec3(0.10, 0.0, 0.0)),
                    (Element::H, glam::vec3(0.0, 0.10, 0.0)),
                ];
                bonds = vec![(0, 1, BondOrder::Single), (0, 2, BondOrder::Single)];
            }
            "ethane" => {
                atoms = vec![
                    (Element::C, glam::vec3(0.0, 0.0, 0.0)),
                    (Element::C, glam::vec3(0.16, 0.0, 0.0)),
                    (Element::H, glam::vec3(-0.05, 0.09, 0.0)),
                    (Element::H, glam::vec3(-0.05, -0.09, 0.0)),
                    (Element::H, glam::vec3(-0.05, 0.0, 0.09)),
                    (Element::H, glam::vec3(0.21, 0.09, 0.0)),
                    (Element::H, glam::vec3(0.21, -0.09, 0.0)),
                    (Element::H, glam::vec3(0.21, 0.0, -0.09)),
                ];
                bonds = vec![
                    (0, 1, BondOrder::Single),
                    (0, 2, BondOrder::Single),
                    (0, 3, BondOrder::Single),
                    (0, 4, BondOrder::Single),
                    (1, 5, BondOrder::Single),
                    (1, 6, BondOrder::Single),
                    (1, 7, BondOrder::Single),
                ];
            }
            "ethene" => {
                // H2C=CH2 — a double bond to exercise multi-order rendering.
                atoms = vec![
                    (Element::C, glam::vec3(0.0, 0.0, 0.0)),
                    (Element::C, glam::vec3(0.14, 0.0, 0.0)),
                    (Element::H, glam::vec3(-0.06, 0.08, 0.0)),
                    (Element::H, glam::vec3(-0.06, -0.08, 0.0)),
                    (Element::H, glam::vec3(0.20, 0.08, 0.0)),
                    (Element::H, glam::vec3(0.20, -0.08, 0.0)),
                ];
                bonds = vec![
                    (0, 1, BondOrder::Double),
                    (0, 2, BondOrder::Single),
                    (0, 3, BondOrder::Single),
                    (1, 4, BondOrder::Single),
                    (1, 5, BondOrder::Single),
                ];
            }
            "acetylene" => {
                // HC≡CH — a triple bond.
                atoms = vec![
                    (Element::C, glam::vec3(0.0, 0.0, 0.0)),
                    (Element::C, glam::vec3(0.13, 0.0, 0.0)),
                    (Element::H, glam::vec3(-0.10, 0.0, 0.0)),
                    (Element::H, glam::vec3(0.23, 0.0, 0.0)),
                ];
                bonds = vec![
                    (0, 1, BondOrder::Triple),
                    (0, 2, BondOrder::Single),
                    (1, 3, BondOrder::Single),
                ];
            }
            "benzene" => {
                // Six carbons on a rough hexagon (radius ~0.14 nm) — deliberately a bit
                // off so the relax has something to do — with aromatic ring bonds.
                let mut a = Vec::new();
                let mut b = Vec::new();
                let r = 0.135_f32;
                for k in 0..6 {
                    let th = std::f32::consts::TAU * (k as f32) / 6.0;
                    // jitter so it isn't already perfect
                    let rr = r * if k % 2 == 0 { 1.08 } else { 0.92 };
                    a.push((Element::C, glam::vec3(rr * th.cos(), rr * th.sin(), 0.0)));
                }
                for k in 0..6 {
                    b.push((k, (k + 1) % 6, BondOrder::Aromatic));
                }
                atoms = a;
                bonds = b;
            }
            // "methane" and anything unrecognized → methane.
            _ => {
                atoms = vec![
                    (Element::C, glam::vec3(0.0, 0.0, 0.0)),
                    (Element::H, glam::vec3(0.08, 0.08, 0.08)),
                    (Element::H, glam::vec3(-0.08, -0.08, 0.08)),
                    (Element::H, glam::vec3(-0.08, 0.08, -0.08)),
                    (Element::H, glam::vec3(0.08, -0.08, -0.08)),
                ];
                bonds = vec![
                    (0, 1, BondOrder::Single),
                    (0, 2, BondOrder::Single),
                    (0, 3, BondOrder::Single),
                    (0, 4, BondOrder::Single),
                ];
            }
        }
        let Some((first_el, first_pos)) = atoms.first().copied() else {
            return;
        };
        // Create the molecule from the first atom (a drawn molecule is never empty),
        // exactly like the first click of the Atom tool.
        let raw = match data::RawMolecule::single_atom("drawn", first_el.make_atom(), first_pos) {
            Ok(raw) => raw,
            Err(e) => {
                log::error!("debug draw: {e}");
                return;
            }
        };
        let mut session = DrawSession { element: first_el, ..DrawSession::default() };
        self.start_drawn_molecule(raw, &mut session);
        let Some(target) = session.target else { return };
        let Some(mi) = self.scene.molecules.iter().position(|m| m.id == target) else {
            return;
        };
        // Append the rest via the same helpers, then relax to convergence.
        {
            let mol = &mut self.scene.molecules[mi];
            for &(el, pos) in atoms.iter().skip(1) {
                mol.add_atom(&el.make_atom(), pos);
            }
            for &(i, j, order) in &bonds {
                mol.add_bond(i, j, order);
            }
            mol.refresh_bbox();
            mol.perceive_aromaticity(); // detect rings/aromaticity (drives the ring-circle overlay)
            let res = crate::minimize::relax_in_system(
                &mut mol.system,
                &mol.bonds,
                RelaxKind::Cleanup,
            );
            // A representative bond length after relaxation (the first bond).
            let len0 = mol.bonds.first().map(|bond| {
                let (a, b) = (bond.i1, bond.i2);
                let st = mol.system.state();
                match (st.coords.get(a), st.coords.get(b)) {
                    (Some(pa), Some(pb)) => {
                        glam::vec3(pa.x, pa.y, pa.z).distance(glam::vec3(pb.x, pb.y, pb.z))
                    }
                    _ => f32::NAN,
                }
            });
            log::info!(
                "debug draw '{preset}': {} atoms, {} bonds, relax {} steps (converged={}, fmax={:.4}), bond0 len = {:.4} nm",
                mol.n_atoms,
                mol.bonds.len(),
                res.steps,
                res.converged,
                res.final_force_norm,
                len0.unwrap_or(f32::NAN),
            );
            if let Some(rep) = mol.reps.first_mut() {
                rep.geom_dirty = true;
            }
        }
        // Frame the camera on the drawn molecule and keep the session active.
        let (min, max) = self.scene.molecules[mi].current_bbox();
        self.camera = Camera::frame_bbox(min, max, self.settings.view.fill);
        self.settings.view.seed_camera(&mut self.camera);
        self.last_render_camera = None;
        self.view_dirty = true;
        self.pick_mode = PickMode::Off;
        self.draw = Some(session);
        // A fresh drawn molecule is its own baseline for undo.
        self.history = History::new(EditState::capture(&self.scene));
    }

    /// Recompile dirty selections and rebuild/reupload dirty geometry. Returns
    /// true if any geometry was uploaded (so the frame needs re-rendering).
    fn rebuild_dirty(&mut self, rs: &eframe::egui_wgpu::RenderState) -> bool {
        let mut changed = false;
        // Whether wrapping bonds are drawn as dashed minimum-image half-bonds (read
        // once: the molecule loop below borrows `self.scene` mutably).
        let dashed = self.settings.behavior.dashed_pbc_bonds;
        // A structural change (molecule add/remove/reorder/visibility) shifts molecule
        // indices, so the GPU pick geometry's baked `mol+1` ids must be rebuilt.
        #[cfg(not(target_arch = "wasm32"))]
        let structure_changed = self.view_dirty;
        for (_mi, mol) in self.scene.molecules.iter_mut().enumerate() {
            #[cfg(not(target_arch = "wasm32"))]
            if structure_changed {
                mol.pick_dirty = true;
            }
            #[cfg(not(target_arch = "wasm32"))]
            let pick_pending = mol.pick_dirty;
            #[cfg(target_arch = "wasm32")]
            let pick_pending = false;
            let any_rep_dirty = mol
                .reps
                .iter()
                .any(|r| r.sel_dirty || r.geom_dirty || r.coords_dirty);
            if !any_rep_dirty
                && !(mol.show_box && mol.box_dirty)
                && !mol.aromatic_dirty
                && !mol.glow_dirty
                && !mol.hover_dirty
                && !mol.hover_detail_dirty
                && !pick_pending
            {
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
                            rep.sel_error_caret = None;
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
                            rep.sel_error_caret = None;
                            rep.sel_empty = true;
                            rep.gpu = Default::default();
                            changed = true;
                        }
                        Err(scene::EvalError::Invalid(e)) => {
                            let (msg, caret) = crate::suggest::parse_sel_error(&e);
                            // molar trims the input before parsing, so shift the
                            // caret past any leading whitespace to align it with
                            // the field's text.
                            let lead =
                                rep.sel_text.chars().take_while(|c| c.is_whitespace()).count();
                            rep.sel_error = Some(msg);
                            rep.sel_error_caret = caret.map(|c| c + lead);
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
                            ss.as_ref(), dashed,
                        );
                        (geom, ss)
                    };
                    rep.ss_cache = fresh_ss;
                    rep.gpu = self.renderer.upload(rs, &geom);
                    // Cache the cartoon ribbon CPU mesh (with residue tags) for the
                    // selection glow to extract sub-ribbons from; clear for other styles.
                    rep.cartoon_cache = if matches!(rep.kind, RepKind::Cartoon) {
                        Some(geom.mesh)
                    } else {
                        None
                    };
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
                            rep.ss_cache.as_ref(), dashed,
                        )
                    };
                    self.renderer.update(rs, &mut rep.gpu, &geom);
                    if matches!(rep.kind, RepKind::Cartoon) {
                        rep.cartoon_cache = Some(geom.mesh); // keep the glow's cache fresh
                    }
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

            // Aromatic-ring circles: depth-tested 3-D line geometry (built from the
            // perceived rings at the displayed coords), so they occlude correctly.
            if mol.aromatic_dirty || (rep_geom_changed && !mol.aromatic_rings.is_empty()) {
                let lines = geometry::aromatic_circles(&mol.aromatic_rings, &render_state.coords);
                let geom = geometry::GeometryData { lines, ..Default::default() };
                mol.aromatic_gpu = self.renderer.upload(rs, &geom);
                mol.aromatic_dirty = false;
                changed = true;
            }

            // If any rep's geometry was rebuilt (style/selection/coords changed) and
            // there's a pending/hover highlight, rebuild its glow so it follows.
            if rep_geom_changed && mol.pending.is_some() {
                mol.glow_dirty = true;
            }
            if rep_geom_changed && mol.hover.is_some() {
                mol.hover_dirty = true;
            }
            if rep_geom_changed && mol.hover_detail.is_some() {
                mol.hover_detail_dirty = true;
            }
            if rep_geom_changed {
                mol.hover_grid = None; // its filtered atom set depends on the reps/coords
            }

            // Active-selection glow: rebuild the pending atoms in each rep's own
            // style (so the highlight glows in the current style), or clear it. Runs
            // after the rep loop so Cartoon reps' `ss_cache` is already populated.
            if mol.glow_dirty {
                let geom = match &mol.pending {
                    Some(pending) => build_glow(
                        &mol.system, &mol.bonds, &mol.reps, &pending.atoms, render_state, n_atoms,
                        dashed,
                    ),
                    None => geometry::GeometryData::default(),
                };
                mol.glow_gpu = self.renderer.upload(rs, &geom);
                mol.glow_dirty = false;
                changed = true;
            }
            // Hover highlight: same builder, the hovered residue's atoms (steady glow).
            if mol.hover_dirty {
                let geom = match &mol.hover {
                    Some(atoms) => build_glow(
                        &mol.system, &mol.bonds, &mol.reps, atoms, render_state, n_atoms, dashed,
                    ),
                    None => geometry::GeometryData::default(),
                };
                mol.hover_gpu = self.renderer.upload(rs, &geom);
                mol.hover_dirty = false;
                changed = true;
            }
            // Hover detail lens: faded CPK ball-and-stick of the atoms near the
            // cursor view-line (built from `hover_detail`), over a Cartoon/Surface rep.
            if mol.hover_detail_dirty {
                let geom = match &mol.hover_detail {
                    Some(d) => {
                        build_hover_detail(&mol.system, &mol.bonds, d, render_state, n_atoms, dashed)
                    }
                    None => geometry::GeometryData::default(),
                };
                mol.hover_detail_gpu = self.renderer.upload(rs, &geom);
                mol.hover_detail_dirty = false;
                changed = true;
            }
            // GPU pick geometry (native): rebuild when the molecule's geometry/coords
            // changed (rep_geom_changed covers both) or it was flagged dirty (init /
            // structure change). Mirrors the atoms CPU `pick` would ray-cast.
            #[cfg(not(target_arch = "wasm32"))]
            if rep_geom_changed || mol.pick_dirty {
                let geom = build_pick(mol, _mi, render_state);
                mol.pick_gpu = self.renderer.upload(rs, &geom);
                mol.pick_dirty = false;
                // No `changed = true`: pick geometry isn't drawn in render_scene, so
                // it doesn't require a scene re-render on its own.
            }
        }
        changed
    }
}

/// Build the hover detail "lens": a distance-faded CPK ball-and-stick of the atoms
/// near the cursor view-line (`detail.atoms`, found by the spatial grid). Rendered
/// over a Cartoon/Surface rep to reveal local atomic detail the abstraction hides.
fn build_hover_detail(
    system: &molar::prelude::System,
    bonds: &[Bond],
    detail: &crate::scene::HoverDetail,
    state: &molar::prelude::State,
    n_atoms: usize,
    dashed_pbc: bool,
) -> geometry::GeometryData {
    let Some(index_str) = pick::index_selection_string(&detail.atoms) else {
        return geometry::GeometryData::default();
    };
    let Ok((_, sel)) = scene::evaluate(system, &index_str) else {
        return geometry::GeometryData::default();
    };
    let bound = system.bind_with_state(&sel, state);
    let params = RepParams::BallAndStick { sphere_scale: 0.25, bond_radius: 0.04 };
    let mut geom = geometry::build(
        &bound,
        n_atoms,
        bonds,
        &params,
        ColorMethod::Element,
        crate::material::Material::Opaque,
        None,
        dashed_pbc,
    );
    fade_by_ray(&mut geom, detail.ray_o, detail.ray_d, detail.radius);
    geom
}

/// Set each element's alpha by its perpendicular distance from the ray `o + t·d`:
/// opaque on-axis, fading to 0 at `radius` — so the lens dissolves softly into the
/// ribbon. The alpha is the color's top byte (matching the geometry packing).
fn fade_by_ray(geom: &mut geometry::GeometryData, o: glam::Vec3, d: glam::Vec3, radius: f32) {
    const MAX_A: f32 = 235.0;
    let d = d.normalize_or_zero();
    let radius = radius.max(1e-3);
    let alpha_of = |p: [f32; 3]| -> u32 {
        let w = glam::Vec3::from(p) - o;
        let perp = (w - d * w.dot(d)).length();
        let f = (1.0 - perp / radius).clamp(0.0, 1.0);
        (f * MAX_A) as u32
    };
    let set = |c: u32, a: u32| (c & 0x00ff_ffff) | (a << 24);
    for s in &mut geom.spheres {
        s.color = set(s.color, alpha_of(s.center));
    }
    for c in &mut geom.cylinders {
        let mid = [
            (c.p0[0] + c.p1[0]) * 0.5,
            (c.p0[1] + c.p1[1]) * 0.5,
            (c.p0[2] + c.p1[2]) * 0.5,
        ];
        c.color = set(c.color, alpha_of(mid));
    }
}

/// Build the selection glow geometry for one molecule: for each visible rep, the
/// rep's selection intersected with the highlighted `atoms`, built in that rep's
/// own style/params, merged into one geometry. Used for both the pending (lasso)
/// selection and the hover highlight. The element colors/materials are irrelevant
/// (the glow shaders emit a fixed cyan Fresnel rim), so the rep's own values are
/// reused. Cartoon/SecStruct reps are skipped until their SS cache exists (it's
/// filled by the same `rebuild_dirty` pass, just before this).
fn build_glow(
    system: &molar::prelude::System,
    bonds: &[Bond],
    reps: &[Representation],
    atoms: &[usize],
    state: &State,
    n_atoms: usize,
    dashed_pbc: bool,
) -> geometry::GeometryData {
    let Some(index_str) = pick::index_selection_string(atoms) else {
        return geometry::GeometryData::default();
    };
    // Highlighted residues (resindex), for extracting the Cartoon sub-ribbon.
    let topo = system.topology();
    let res_set: std::collections::HashSet<u32> = atoms
        .iter()
        .filter_map(|&a| topo.get_atom(a).map(|at| at.resindex as u32))
        .collect();
    let mut out = geometry::GeometryData::default();
    for rep in reps {
        if !rep.visible {
            continue;
        }
        // Cartoon: don't rebuild a (degenerate, divergent) subset ribbon — extract the
        // chosen residues' triangles straight from the parent's *exact* cached mesh.
        // Coincident geometry passes the glow pass's `≤` depth test cleanly (no z-fight,
        // no inflation) and a single residue still yields its ribbon segment.
        if matches!(rep.kind, RepKind::Cartoon) {
            if let Some(cache) = &rep.cartoon_cache {
                out.append(cartoon_submesh(cache, &res_set));
            }
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
            rep.ss_cache.as_ref(), dashed_pbc,
        );
        // Surface re-builds the glow over the *subset* of selected atoms, so its mesh
        // nearly — but not exactly — coincides with the parent's (the grid isosurface
        // shifts at the subset boundary). Two near-coplanar surfaces z-fight, so push
        // the glow mesh a hair *outward* along its normals into a thin shell just in
        // front of the parent. (The glow pass writes no depth, so the shell's back
        // still fails the depth test and stays hidden.) Impostor glows coincide
        // exactly and need no offset; Cartoon reuses the parent mesh (handled above).
        inflate_mesh(&mut geom.mesh, GLOW_INFLATE);
        out.append(geom);
    }
    out
}

/// Extract the sub-ribbon of a cached Cartoon mesh for the residues in `res_set`:
/// keep a triangle when a majority (≥2) of its vertices belong to chosen residues
/// (a clean cut at residue boundaries), compacting the referenced vertices. The
/// result shares the parent's exact vertex positions, so the glow is coincident.
fn cartoon_submesh(
    mesh: &geometry::MeshData,
    res_set: &std::collections::HashSet<u32>,
) -> geometry::GeometryData {
    let mut vertices: Vec<crate::render::MeshVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut remap: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    for tri in mesh.indices.chunks_exact(3) {
        let chosen = tri
            .iter()
            .filter(|&&v| res_set.contains(&mesh.vert_res[v as usize]))
            .count();
        if chosen < 2 {
            continue;
        }
        for &v in tri {
            let nv = *remap.entry(v).or_insert_with(|| {
                vertices.push(mesh.vertices[v as usize]);
                (vertices.len() - 1) as u32
            });
            indices.push(nv);
        }
    }
    geometry::GeometryData {
        mesh: geometry::MeshData { vertices, indices, vert_res: Vec::new() },
        ..Default::default()
    }
}

/// Atom-index bits in a pick id's y channel (the rest hold the rep index). 21 bits
/// → up to ~2M atoms/molecule and 2048 reps; ample for interactive systems.
#[cfg(not(target_arch = "wasm32"))]
const PICK_ATOM_BITS: u32 = 21;

/// Build the GPU **pick** geometry for one molecule (index `mi`): an id-stamped
/// sphere per *pickable* atom — exactly the atoms CPU `pick` ray-casts (eligible
/// atoms of each visible rep, at their displayed position and effective radius). The
/// id packs `[mi+1, rep<<21 | atom]` so the readback decodes back to (mol, rep, atom).
/// **Periodic images are baked in**: a rep with periodic display emits one sphere per
/// atom per drawn image (shifted by the lattice offset), so the single-camera pick
/// pass covers every image — matching what CPU `pick` tests. The id is the same for
/// all images, so a hit on any image still reports the (central) atom.
#[cfg(not(target_arch = "wasm32"))]
fn build_pick(mol: &scene::Molecule, mi: usize, state: &State) -> geometry::GeometryData {
    // Box lattice vectors (columns of the box matrix), for periodic image offsets.
    let box_vecs = state.pbox.as_ref().map(|pb| {
        let m = pb.get_matrix();
        [
            glam::Vec3::new(m[(0, 0)], m[(1, 0)], m[(2, 0)]),
            glam::Vec3::new(m[(0, 1)], m[(1, 1)], m[(2, 1)]),
            glam::Vec3::new(m[(0, 2)], m[(1, 2)], m[(2, 2)]),
        ]
    });
    let mut spheres: Vec<SphereInstance> = Vec::new();
    for (rj, rep) in mol.reps.iter().enumerate() {
        if !rep.visible {
            continue;
        }
        let Some(sel) = &rep.sel else { continue };
        let smoothed = (rep.smooth_window > 1)
            .then(|| mol.trajectory.smoothed_state(rep.smooth_window))
            .flatten();
        let disp_state: &State = smoothed.as_ref().unwrap_or(state);
        let offsets = match box_vecs {
            Some([a, b, c]) => rep.periodic.offsets(a, b, c),
            None => vec![glam::Vec3::ZERO],
        };
        let bound = mol.system.bind_with_state(sel, disp_state);
        let pick_x = mi as u32 + 1;
        let pick_rep = (rj as u32) << PICK_ATOM_BITS;
        for p in bound.iter_particle() {
            if !pick::atom_in_rep(rep.kind, p.atom.name.as_str()) {
                continue;
            }
            let base = glam::Vec3::new(p.pos.x, p.pos.y, p.pos.z);
            let radius = pick::effective_radius(&rep.params, p.atom);
            let id = [pick_x, pick_rep | (p.id as u32)];
            for off in &offsets {
                let c = base + *off;
                spheres.push(SphereInstance {
                    center: [c.x, c.y, c.z],
                    radius,
                    color: 0,
                    mat: 0,
                    pick: id,
                });
            }
        }
    }
    geometry::GeometryData { spheres, ..Default::default() }
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

        // Work around a winit/egui Wayland IME bug that otherwise breaks all text
        // entry (only the first char of a field is accepted). See `defuse_broken_ime`.
        #[cfg(target_os = "linux")]
        defuse_broken_ime(&ctx);

        // Browser file picker results: load each (filename, bytes) the async picker
        // delivered (see `pick_file`) as a new molecule.
        #[cfg(target_arch = "wasm32")]
        while let Ok((name, bytes)) = self.file_rx.try_recv() {
            match data::load_from_bytes(&name, bytes, &self.settings.behavior.bond_params()) {
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

        // The "Load trajectory" / "Delete frames" modals float above everything.
        self.draw_load_dialog(&ctx);
        self.draw_delete_frames_dialog(&ctx);
        self.draw_rename_dialog(&ctx);
        self.draw_settings_dialog(&ctx, frame);

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
        // Vertical drawing-tools palette on the right (only while Draw mode is on);
        // a panel, so it reserves its strip before the viewport fills the rest.
        self.draw_tools_panel(ui);
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

                    // — Selection controls (left) —
                    // Pick/selection-mode dropdown (label + caret). Off by default.
                    toolbar_label(ui, "Sel. mode");
                    let pick_label = format!("{}  {}", self.pick_mode.label(), icon::CARET_DOWN);
                    let resp =
                        overlay_button(ui, &pick_label, false).on_hover_text("Selection mode");
                    let mut pick_changed = false;
                    egui::Popup::menu(&resp).show(|ui| {
                        for m in [PickMode::Off, PickMode::Click, PickMode::Lasso] {
                            if ui.selectable_value(&mut self.pick_mode, m, m.label()).clicked() {
                                pick_changed = true;
                            }
                        }
                    });
                    // Mutually exclusive with Draw: choosing any pick mode leaves Draw.
                    if pick_changed && self.pick_mode != PickMode::Off {
                        self.draw = None;
                    }
                    // Scope dropdown (how a hit expands: Atoms / Residues / Bound H).
                    // Only relevant when picking is on, so hidden while pick mode is Off.
                    // `Bound H` is meaningless for single-atom click picking, so hidden in
                    // Click mode (and a stale value snaps back to Atoms).
                    if self.pick_mode != PickMode::Off {
                        let single = self.pick_mode == PickMode::Click;
                        if single && self.selection_mode == SelectionMode::BoundH {
                            self.selection_mode = SelectionMode::Atoms;
                        }
                        let modes: &[SelectionMode] = if single {
                            &[SelectionMode::Atoms, SelectionMode::Residues]
                        } else {
                            &[SelectionMode::Atoms, SelectionMode::Residues, SelectionMode::BoundH]
                        };
                        toolbar_label(ui, "Scope");
                        let sel_label =
                            format!("{}  {}", self.selection_mode.label(), icon::CARET_DOWN);
                        let resp = overlay_button(ui, &sel_label, false).on_hover_text(
                            "Scope — how a hit expands:\n\
                             Atoms (exact) · Residues (whole) · Bound H (heavy + bonded H, lasso only)",
                        );
                        egui::Popup::menu(&resp).show(|ui| {
                            for &m in modes {
                                ui.selectable_value(&mut self.selection_mode, m, m.label());
                            }
                        });
                    }

                    // Draw-mode toggle (after the pick controls). Mutually exclusive
                    // with picking: turning Draw on forces the pick mode Off.
                    {
                        let active = self.draw.is_some();
                        if overlay_button(
                            ui,
                            &format!("{}  Draw", icon::PENCIL_SIMPLE),
                            active,
                        )
                        .on_hover_text("Draw mode — sketch atoms and bonds by hand")
                        .clicked()
                        {
                            self.toggle_draw();
                        }
                    }

                    // In Click/Lasso mode, a held modifier changes the set operation (or,
                    // for Alt in Lasso, orbits the view) — trail the hint (matches
                    // `finish_lasso`'s `LassoOp` and the click-to-select path).
                    if matches!(self.pick_mode, PickMode::Click | PickMode::Lasso) {
                        let m = ui.input(|i| i.modifiers);
                        let hint = if m.alt && self.pick_mode == PickMode::Lasso {
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

                    // — View-settings hamburger (right-aligned) — toggles a tabbed
                    // Window (Camera / Lighting / Scene). A Window (not a Popup) so
                    // the nested click-to-open dropdowns / color pickers work; it
                    // closes on a click outside it (see `view_settings_window`).
                    let anchor = ui
                        .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let resp = overlay_button(ui, icon::LIST, self.view_menu_open)
                                .on_hover_text("View settings (camera, lighting, scene)");
                            if resp.clicked() {
                                self.view_menu_open = !self.view_menu_open;
                            }
                            resp.rect
                        })
                        .inner;
                    self.view_settings_window(ui.ctx(), anchor);
                });
            });
    }

    /// The view-settings window (Camera / Lighting / Scene tabs), opened from the
    /// toolbar hamburger. Hosted in a `Window` so nested click-downward dropdowns
    /// and color pickers behave; closed on a click outside it — but **not** while a
    /// child popup (a dropdown / color picker) is open, nor when the click is on the
    /// hamburger button itself (`anchor`).
    fn view_settings_window(&mut self, ctx: &egui::Context, anchor: egui::Rect) {
        if !self.view_menu_open {
            self.view_menu_rect = None;
            return;
        }
        let inner = egui::Window::new("view_settings")
            .title_bar(false)
            .resizable(false)
            .movable(false)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(anchor.right_bottom() + egui::vec2(0.0, 4.0))
            .show(ctx, |ui| {
                // A non-resizable window sizes to its content's `min_rect`, so it stays
                // snug — *provided the content has no width-filling widget*. (A bare
                // `ui.separator()` fills the available width, which becomes the content
                // size and made the menu balloon to the screen edge; the tabs use
                // `Frame::group`s instead, which size to content.) `min_width` is just a
                // sensible floor so the menu isn't too narrow / jittery across tabs.
                ui.set_min_width(248.0);
                tab_bar(
                    ui,
                    &mut self.view_tab,
                    &[
                        (ViewTab::Camera, "Camera"),
                        (ViewTab::Lighting, "Lighting"),
                        (ViewTab::Scene, "Scene"),
                    ],
                );
                ui.add_space(6.0);
                match self.view_tab {
                    ViewTab::Camera => self.view_tab_camera(ui),
                    ViewTab::Lighting => self.view_tab_lighting(ui),
                    ViewTab::Scene => self.view_tab_scene(ui),
                }
            });
        // Close on a click outside the window. **Test against the rect drawn _last_
        // frame** (`view_menu_rect`), not this frame's: clicking a tab switches
        // `view_tab` and `Window::show` immediately re-lays-out the (right-pivoted)
        // window for the new tab — a narrower tab moves the left edge right, so the
        // freshly-updated rect (and `layer_id_at`, which reads the same just-updated
        // area state) no longer covers the leftmost tab the click actually landed on,
        // and the menu wrongly closed. The previous frame's rect is the geometry the
        // user saw and clicked. A child popup (dropdown / color picker) keeps it open
        // via `Popup::is_any_open`; clicks on the hamburger (`anchor`) are its toggle.
        if let Some(inner) = inner {
            let hit_rect = self.view_menu_rect.unwrap_or(inner.response.rect);
            let clicked = ctx.input(|i| i.pointer.any_click());
            if clicked && !egui::Popup::is_any_open(ctx) {
                if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
                    if !hit_rect.contains(p) && !anchor.contains(p) {
                        self.view_menu_open = false;
                    }
                }
            }
            self.view_menu_rect = Some(inner.response.rect);
        }
    }

    /// Camera tab of the view-settings menu: projection + depth cue.
    fn view_tab_camera(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Projection").strong());
        ui.horizontal(|ui| {
            let persp = self.camera.is_perspective();
            if ui
                .selectable_label(persp, egui::RichText::new(icon::PERSPECTIVE).size(18.0))
                .on_hover_text("Perspective")
                .clicked()
            {
                self.camera.projection = Projection::Perspective;
            }
            if ui
                .selectable_label(!persp, egui::RichText::new(icon::CUBE).size(18.0))
                .on_hover_text("Orthographic")
                .clicked()
            {
                self.camera.projection = Projection::Orthographic;
            }
        });
        ui.add_space(6.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(egui::RichText::new("Depth cue").strong());
            let cue = &mut self.camera.depth_cue;
            egui::Grid::new("cue_grid")
                .num_columns(2)
                .spacing(egui::vec2(8.0, 6.0))
                .show(ui, |ui| {
                    ui.label("Type");
                    // Click-to-open dropdown (opens downward), as a nested menu so it
                    // stays within the parent CloseOnClickOutside menu's hierarchy.
                    let cur = if cue.enabled { cue.mode.label() } else { "None" };
                    let header = ui.button(format!("{}  {}", cur, icon::CARET_DOWN));
                    egui::Popup::menu(&header).show(|ui| {
                        if ui.selectable_label(!cue.enabled, "None").clicked() {
                            cue.enabled = false;
                            ui.close();
                        }
                        for m in CueMode::ALL {
                            let sel = cue.enabled && cue.mode == m;
                            if ui.selectable_label(sel, m.label()).clicked() {
                                cue.enabled = true;
                                cue.mode = m;
                                ui.close();
                            }
                        }
                    });
                    ui.end_row();

                    ui.label("Strength");
                    slider_with_edit(ui, &mut cue.strength, 0.0..=1.0, cue.enabled);
                    ui.end_row();

                    ui.label("Start");
                    slider_with_edit(ui, &mut cue.start, 0.0..=1.0, cue.enabled);
                    ui.end_row();
                });
        });
    }

    /// Lighting tab: ambient occlusion + cast shadows (both screen-space darkening).
    /// Each is a `Frame::group` (like the Camera/Scene tabs) — content-sized, so the
    /// window stays snug; a width-filling `ui.separator()` between them would balloon it.
    fn view_tab_lighting(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let ao = &mut self.camera.ao;
            ui.checkbox(&mut ao.enabled, "Ambient occlusion")
                .on_hover_text("Darken creases and contact points (screen-space AO)");
            ui.add_enabled_ui(ao.enabled, |ui| {
                egui::Grid::new("ao_opts")
                    .num_columns(2)
                    .spacing(egui::vec2(8.0, 4.0))
                    .show(ui, |ui| {
                        ui.label("Strength");
                        slider_with_edit(ui, &mut ao.strength, 0.0..=1.0, ao.enabled);
                        ui.end_row();
                        ui.label("Radius");
                        slider_with_edit(ui, &mut ao.radius, 0.1..=1.0, ao.enabled);
                        ui.end_row();
                    });
            });
        });
        ui.add_space(6.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let sh = &mut self.camera.shadow;
            ui.checkbox(&mut sh.enabled, "Cast shadows")
                .on_hover_text("Real-time directional shadows from a key light (shadow map)");
            ui.add_enabled_ui(sh.enabled, |ui| {
                egui::Grid::new("shadow_opts")
                    .num_columns(2)
                    .spacing(egui::vec2(8.0, 4.0))
                    .show(ui, |ui| {
                        ui.label("Strength");
                        slider_with_edit(ui, &mut sh.strength, 0.0..=1.0, sh.enabled);
                        ui.end_row();
                    });
            });
        });
    }

    /// Scene tab: orientation axes + background.
    fn view_tab_scene(&mut self, ui: &mut egui::Ui) {
        let scene_tex = self.renderer.texture_id();
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(egui::RichText::new("Axes").strong());
            draw_axes_widget(ui, &mut self.axes_on, &mut self.axes_corner, Some(scene_tex));
        });
        ui.add_space(6.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(egui::RichText::new("Background").strong());
            let bg = &mut self.camera.background;
            ui.horizontal(|ui| {
                ui.radio_value(&mut bg.kind, BgKind::Solid, "Solid color");
                color_submenu(ui, "bg_solid", &mut bg.color);
            });
            ui.radio_value(&mut bg.kind, BgKind::Gradient, "Gradient");
            let grad = bg.kind == BgKind::Gradient;
            ui.add_enabled_ui(grad, |ui| {
                egui::Grid::new("bg_grad")
                    .num_columns(2)
                    .spacing(egui::vec2(8.0, 4.0))
                    .show(ui, |ui| {
                        ui.label("Top");
                        color_submenu(ui, "bg_top", &mut bg.top);
                        ui.end_row();
                        ui.label("Bottom");
                        color_submenu(ui, "bg_bottom", &mut bg.bottom);
                        ui.end_row();
                    });
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
            // Session menu: New (empty scene) / Save / Load the whole visualization
            // state. Native only: the wasm build has no filesystem to reload
            // molecule sources from.
            #[cfg(not(target_arch = "wasm32"))]
            ui.menu_button(format!("{}  Session  {}", icon::STACK, icon::CARET_DOWN), |ui| {
                if ui
                    .button(format!("{}  New", icon::FILE))
                    .on_hover_text("Clear all molecules and start an empty scene")
                    .clicked()
                {
                    self.new_session();
                    ui.close();
                }
                if ui
                    .button(format!("{}  Save…", icon::FLOPPY_DISK))
                    .on_hover_text("Save the visualization state (molecules, representations, camera)")
                    .clicked()
                {
                    self.save_session();
                    ui.close();
                }
                if ui
                    .button(format!("{}  Load…", icon::ARCHIVE_BOX))
                    .on_hover_text("Load a saved visualization state, replacing the current scene")
                    .clicked()
                {
                    self.load_session();
                    ui.close();
                }
            });
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

            ui.add_space(8.0);
            ui.separator();
            // Program settings (cogwheel): open the modal editing a working copy.
            if ui
                .button(icon::GEAR_SIX)
                .on_hover_text("Program settings")
                .clicked()
                && self.settings_draft.is_none()
            {
                self.settings_draft = Some(self.settings.clone());
            }
        });
    }

    /// Effective new-rep defaults: the settings' `reps`, with the kind overridden by
    /// the `MOLAR_VIS_DEBUG_REP` env hook (headless verification). Recomputed when
    /// settings change.
    fn effective_rep_defaults(settings: &Settings) -> RepDefaults {
        let mut d = settings.reps.clone();
        if let Some(kind) = std::env::var("MOLAR_VIS_DEBUG_REP")
            .ok()
            .and_then(|s| RepKind::from_name(&s))
        {
            d.kind = kind;
        }
        d
    }

    /// The program-settings dialog (opened by the toolbar cogwheel). A **free,
    /// movable `Window`** rather than a centered `Modal`: a Modal re-centers itself
    /// every frame, so its top edge jumps up/down as the per-tab content height
    /// changes; a Window keeps its position, and with a **fixed width** it can only
    /// grow/shrink at the **bottom** (top stays put). Edits a working copy
    /// (`settings_draft`); **Save** commits + applies + persists, **Cancel** / Escape
    /// discards. The View tab can push its defaults onto the current camera ("Apply to
    /// current view") without saving.
    fn draw_settings_dialog(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let Some(mut draft) = self.settings_draft.take() else {
            return;
        };
        let mut tab = self.settings_tab;
        let mut save = false;
        let mut cancel = false;
        let mut apply_view = false;

        // Open centered horizontally near the top the first time, then keep position
        // (egui stores it by id). Pivot CENTER_TOP + fixed width ⇒ the top edge is
        // anchored and tab switches only resize the bottom.
        let screen = ctx.content_rect();
        egui::Window::new(format!("{}  Settings", icon::GEAR_SIX))
            .id(egui::Id::new("settings_window"))
            .collapsible(false)
            .resizable(false)
            .movable(true)
            .pivot(egui::Align2::CENTER_TOP)
            .default_pos(egui::pos2(screen.center().x, screen.top() + 48.0))
            .show(ctx, |ui| {
                ui.set_width(540.0);
                tab_bar(
                    ui,
                    &mut tab,
                    &[
                        (SettingsPage::Appearance, "Appearance"),
                        (SettingsPage::Rendering, "Rendering"),
                        (SettingsPage::View, "View"),
                        (SettingsPage::Representations, "Representations"),
                        (SettingsPage::Behavior, "Behavior"),
                    ],
                );
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(440.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| match tab {
                        SettingsPage::Appearance => settings_page_appearance(ui, &mut draft),
                        SettingsPage::Rendering => settings_page_rendering(ui, &mut draft),
                        SettingsPage::View => apply_view = settings_page_view(ui, &mut draft),
                        SettingsPage::Representations => settings_page_reps(ui, &mut draft),
                        SettingsPage::Behavior => settings_page_behavior(ui, &mut draft),
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .button("Restore defaults")
                        .on_hover_text("Reset every setting to its built-in default (not saved until you press Save)")
                        .clicked()
                    {
                        draft = Settings::default();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Save").clicked() {
                            save = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            });

        self.settings_tab = tab;
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }

        if save {
            // The dashed-PBC-bonds setting changes geometry, so rebuild every rep to
            // apply it live (other defaults only affect new scenes / the next load).
            let dashed_changed = self.settings.behavior.dashed_pbc_bonds
                != draft.behavior.dashed_pbc_bonds;
            self.settings = draft;
            self.apply_settings(ctx, frame);
            if dashed_changed {
                for mol in &mut self.scene.molecules {
                    for rep in &mut mol.reps {
                        rep.geom_dirty = true;
                    }
                }
            }
        } else if cancel {
            // Discard the working copy.
        } else {
            if apply_view {
                draft.view.seed_camera(&mut self.camera);
                self.last_render_camera = None;
                ctx.request_repaint();
            }
            self.settings_draft = Some(draft);
        }
    }

    /// Commit `self.settings` to the running app: re-theme, reconfigure the renderer
    /// (SSAA / shadow map), refresh the new-rep defaults, force a re-render, and
    /// persist to disk (native). View / representation / behavior defaults are read
    /// when the next scene/molecule is created, so they need no live action here.
    fn apply_settings(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        crate::theme::apply(ctx, &self.settings.appearance);
        if let Some(rs) = frame.wgpu_render_state() {
            self.renderer.reconfigure(rs, &self.settings.rendering);
        }
        self.rep_defaults = Self::effective_rep_defaults(&self.settings);
        self.last_render_camera = None; // force a re-render at the new quality
        self.view_dirty = true;

        #[cfg(not(target_arch = "wasm32"))]
        match self.settings.save() {
            Ok(()) => self.status = "Settings saved".to_string(),
            Err(e) => {
                log::warn!("couldn't save settings: {e}");
                self.status = format!("Settings applied; save failed: {e}");
            }
        }
        ctx.request_repaint();
    }

    /// Loaded molecules. Each is a foldable block: a header row (fold caret, name,
    /// atom count, then a right-justified action group: add-rep, eye, trash), with
    /// the molecule's representations nested below when expanded.
    fn draw_molecule_list(&mut self, ui: &mut egui::Ui) -> bool {
        if self.scene.molecules.is_empty() {
            ui.weak(&self.status);
            return false;
        }
        let rep_defaults = self.rep_defaults.clone();
        let mut view_dirty = false;
        let mut delete: Option<usize> = None;
        let mut open_load: Option<MolId> = None;
        // Deferred actions from the per-molecule menu, applied after the loop so
        // they don't conflict with the `&mut` molecule borrow.
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut save_mol: Option<usize> = None;
        let mut open_del_frames: Option<MolId> = None;
        let mut rename: Option<(MolId, String)> = None;
        // A camera "zoom to fit" request (whole-molecule bbox), applied after the
        // loop so it doesn't conflict with the `&mut` molecule borrow.
        let mut focus: Option<(glam::Vec3, glam::Vec3)> = None;
        // "Open this molecule in the editor" request (the row's edit button).
        let mut edit_mol: Option<usize> = None;
        // The molecule currently being edited (for the edit-button highlight),
        // captured before the `&mut` molecule loop.
        let editing_target = self.draw.as_ref().and_then(|d| d.target);

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
                    // Name only; the atom/frame counts move to a hover tooltip.
                    let frames = mol.trajectory.n_frames().max(1);
                    ui.label(mol.name.as_str()).on_hover_text(format!(
                        "{} atoms / {} frame{}",
                        mol.n_atoms,
                        frames,
                        if frames == 1 { "" } else { "s" }
                    ));
                    // Load a trajectory into this molecule (left-aligned, by the name).
                    if icon_button(ui, icon::FOLDER_OPEN, "Load trajectory").clicked() {
                        open_load = Some(mol.id);
                    }
                    // Right-justified action group: add-rep · zoom · eye · menu.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        compact_actions(ui);
                        // Per-molecule menu (replaces the lone delete button): save,
                        // periodic-box toggle, delete frames, delete molecule.
                        ui.menu_button(icon::LIST, |ui| {
                            #[cfg(not(target_arch = "wasm32"))]
                            if ui
                                .button(format!("{}  Save molecule…", icon::FLOPPY_DISK))
                                .clicked()
                            {
                                save_mol = Some(i);
                                ui.close();
                            }
                            if ui.button(format!("{}  Rename…", icon::PENCIL_SIMPLE)).clicked() {
                                rename = Some((mol.id, mol.name.clone()));
                                ui.close();
                            }
                            if ui
                                .checkbox(&mut mol.show_box, "Show periodic box")
                                .changed()
                            {
                                mol.box_dirty = true;
                                view_dirty = true;
                            }
                            ui.separator();
                            if ui
                                .add_enabled(
                                    mol.trajectory.has_playback(),
                                    egui::Button::new(format!("{}  Delete frames…", icon::SCISSORS)),
                                )
                                .clicked()
                            {
                                open_del_frames = Some(mol.id);
                                ui.close();
                            }
                            if ui
                                .button(format!("{}  Delete molecule", icon::TRASH))
                                .clicked()
                            {
                                delete = Some(i);
                                ui.close();
                            }
                        })
                        .response
                        .on_hover_text("Molecule menu");
                        let eye = if mol.visible { icon::EYE } else { icon::EYE_SLASH };
                        if ui
                            .selectable_label(mol.visible, eye)
                            .on_hover_text(if mol.visible { "Hide" } else { "Show" })
                            .clicked()
                        {
                            mol.visible = !mol.visible;
                            view_dirty = true;
                        }
                        // Edit: open this molecule in the drawing editor.
                        let editing = editing_target == Some(mol.id);
                        if ui
                            .selectable_label(editing, icon::PENCIL_SIMPLE)
                            .on_hover_text("Edit molecule (draw mode)")
                            .clicked()
                        {
                            edit_mol = Some(i);
                        }
                        if icon_button(ui, icon::MAGNIFYING_GLASS_PLUS, "Zoom to molecule")
                            .clicked()
                        {
                            focus = Some(mol.current_bbox());
                        }
                        if ui
                            .button(format!("{} rep", icon::PLUS))
                            .on_hover_text("Add representation")
                            .clicked()
                        {
                            mol.reps.push(Representation::from_defaults(&rep_defaults));
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
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(i) = save_mol {
            self.save_molecule(i);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = save_mol;
        if let Some(id) = open_del_frames {
            self.delete_frames_dialog = Some(DeleteFramesDialog::new(id));
        }
        if rename.is_some() {
            self.rename_mol = rename;
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
        if let Some(i) = edit_mol {
            self.open_in_editor(i);
            view_dirty = true;
        }
        view_dirty
    }

    /// Open molecule `i` in the drawing editor: flag it `editable` (so its structure
    /// is snapshotted for undo) and start a Draw session targeting it. Mutually
    /// exclusive with picking.
    fn open_in_editor(&mut self, i: usize) {
        let Some(mol) = self.scene.molecules.get_mut(i) else { return };
        mol.editable = true;
        let id = mol.id;
        self.pick_mode = PickMode::Off;
        let mut session = self.draw.take().unwrap_or_default();
        session.target = Some(id);
        session.drag = DrawDrag::Idle;
        self.draw = Some(session);
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
            match data::load_with(&path, &self.settings.behavior.bond_params()) {
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
        let rep_defaults = self.rep_defaults.clone();
        self.scene.add(raw, &rep_defaults);
        self.scene.selected_mol = Some(self.scene.molecules.len() - 1);
        if let Some(mol) = self.scene.molecules.last_mut() {
            mol.trajectory.speed_fps = self.settings.behavior.traj_fps;
            mol.trajectory.loop_mode = self.settings.behavior.loop_mode;
        }
        if was_empty {
            // First molecule into an empty scene: frame it and seed the user's
            // default view (projection / background / depth-cue / …).
            if let Some((min, max)) = self.scene.bbox() {
                self.camera = Camera::frame_bbox(min, max, self.settings.view.fill);
                self.settings.view.seed_camera(&mut self.camera);
            }
        }
        self.status = format!("{} molecule(s) loaded", self.scene.molecules.len());
        self.view_dirty = true;
    }

    /// Tear down the current document: drop all molecules (and the trash), cancel
    /// in-flight trajectory loaders, and clear transient editing/dialog state.
    /// Shared by [`Self::new_session`] (start empty) and [`Self::apply_session`]
    /// (start empty, then reload from a file).
    #[cfg(not(target_arch = "wasm32"))]
    fn reset_document(&mut self) {
        self.scene.molecules.clear();
        self.scene.trash.clear();
        self.loaders.clear();
        self.editing_rep = None;
        self.load_dialog = None;
        self.sel_hints.clear();
    }

    /// Start a new, empty visualization state: remove every molecule, reset the
    /// camera, and clear the undo history (a new document is its own baseline).
    #[cfg(not(target_arch = "wasm32"))]
    fn new_session(&mut self) {
        self.reset_document();
        self.scene.selected_mol = None;
        self.scene.clamp_selection();
        self.camera = Camera::default();
        self.settings.view.seed_camera(&mut self.camera);
        self.last_render_camera = None;
        self.history = History::new(EditState::capture(&self.scene));
        self.view_dirty = true;
        self.status = "New session".to_string();
    }

    /// Save molecule `i` to a structure file (rfd save dialog), at the currently
    /// displayed frame. Coordinates + topology of the whole molecule.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_molecule(&mut self, i: usize) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Structure", &["pdb", "gro", "xyz", "ent"])
            .set_file_name("molecule.pdb")
            .save_file()
        else {
            return;
        };
        self.status = match save_displayed(&mut self.scene.molecules[i], &path, None) {
            Ok(()) => format!("Saved molecule to {}", path.display()),
            Err(e) => {
                log::error!("save molecule: {e}");
                format!("Save failed: {e}")
            }
        };
    }

    /// Save representation `j` of molecule `mi`'s selection (just the selected
    /// atoms) to a structure file (rfd save dialog), at the displayed frame.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_rep_selection(&mut self, mi: usize, j: usize) {
        if self.scene.molecules[mi].reps[j].sel.is_none() {
            self.status = "Selection is empty — nothing to save".to_string();
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Structure", &["pdb", "gro", "xyz", "ent"])
            .set_file_name("selection.pdb")
            .save_file()
        else {
            return;
        };
        self.status = match save_displayed(&mut self.scene.molecules[mi], &path, Some(j)) {
            Ok(()) => format!("Saved selection to {}", path.display()),
            Err(e) => {
                log::error!("save selection: {e}");
                format!("Save failed: {e}")
            }
        };
    }

    /// The persistable global view state (camera + view-toolbar toggles). This and
    /// [`Self::apply_view_state`] are the **only** manual plumbing the save/load
    /// framework needs: a new persisted global setting is added to
    /// [`ViewState`](crate::session::ViewState) and read/written in these two
    /// functions. (Per-rep state needs no plumbing — it rides
    /// [`RepState`](crate::history::RepState).)
    #[cfg(not(target_arch = "wasm32"))]
    fn view_state(&self) -> ViewState {
        ViewState {
            camera: Some(self.camera),
            pick_mode: self.pick_mode,
            selection_mode: self.selection_mode,
            axes_on: self.axes_on,
            axes_corner: self.axes_corner,
        }
    }

    /// Restore the global view state captured by [`Self::view_state`].
    #[cfg(not(target_arch = "wasm32"))]
    fn apply_view_state(&mut self, view: ViewState) {
        if let Some(cam) = view.camera {
            self.camera = cam;
        }
        self.pick_mode = view.pick_mode;
        self.selection_mode = view.selection_mode;
        self.axes_on = view.axes_on;
        self.axes_corner = view.axes_corner;
    }

    /// Save the current visualization state to a JSON session file (rfd picker).
    /// Records molecule sources + the full rep document + global view state;
    /// molecule coordinates are *not* embedded (they are reloaded from disk).
    #[cfg(not(target_arch = "wasm32"))]
    fn save_session(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("molar_vis session", &["mvs", "json"])
            .set_file_name("session.mvs")
            .save_file()
        else {
            return;
        };
        self.save_session_to(&path);
    }

    /// Write the current state to `path` (the file half of [`Self::save_session`],
    /// also driven by the `MOLAR_VIS_DEBUG_SAVE_SESSION` verification hook).
    #[cfg(not(target_arch = "wasm32"))]
    fn save_session_to(&mut self, path: &std::path::Path) {
        let session = Session::capture(&self.scene, self.view_state());
        // Drawn molecules have no file source to reload from, so a session can't
        // restore them (it references molecules by source path). Warn so the user
        // knows to export them via "Save molecule…" instead.
        let drawn = self.scene.molecules.iter().filter(|m| m.editable).count();
        let result = session
            .to_json()
            .and_then(|json| std::fs::write(path, json).map_err(|e| e.to_string()));
        match result {
            Ok(()) if drawn > 0 => {
                self.status = format!(
                    "Saved session to {} — {drawn} drawn molecule(s) won't reload (use Save molecule… to export them)",
                    path.display()
                );
            }
            Ok(()) => self.status = format!("Saved session to {}", path.display()),
            Err(e) => {
                log::error!("save session: {e}");
                self.status = format!("Save failed: {e}");
            }
        }
    }

    /// Load a visualization state from a JSON session file (rfd picker), replacing
    /// the current scene (open-document semantics).
    #[cfg(not(target_arch = "wasm32"))]
    fn load_session(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("molar_vis session", &["mvs", "json"])
            .pick_file()
        else {
            return;
        };
        self.load_session_from(&path);
    }

    /// Read and apply a session file at `path` (the file half of
    /// [`Self::load_session`], also driven by `MOLAR_VIS_DEBUG_LOAD_SESSION`).
    #[cfg(not(target_arch = "wasm32"))]
    fn load_session_from(&mut self, path: &std::path::Path) {
        let json = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                self.status = format!("can't read {}: {e}", path.display());
                return;
            }
        };
        match Session::from_json(&json) {
            Ok(session) => self.apply_session(session),
            Err(e) => {
                log::error!("{e}");
                self.status = e;
            }
        }
    }

    /// Rebuild the scene from a parsed [`Session`]: reload each molecule from its
    /// source file, restore its representations / visibility / box / trajectory,
    /// then apply the global view state. Reloading a session is treated as opening
    /// a new document — the undo history is reset to the loaded state.
    #[cfg(not(target_arch = "wasm32"))]
    fn apply_session(&mut self, session: Session) {
        // Replace the whole document.
        self.reset_document();

        let mut errors: Vec<String> = Vec::new();
        let mut loaded = 0usize;
        for ms in &session.molecules {
            let MoleculeSource::File(path) = &ms.source else {
                errors.push(format!(
                    "“{}” was loaded from memory (no file) — cannot reload",
                    ms.name
                ));
                continue;
            };
            let raw = match data::load_with(path, &self.settings.behavior.bond_params()) {
                Ok(r) => r,
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };
            self.scene.add(raw, &self.rep_defaults);
            let mol = self.scene.molecules.last_mut().unwrap();
            if !ms.name.is_empty() {
                mol.name = ms.name.clone(); // restore a custom (renamed) display name
            }
            mol.visible = ms.visible;
            mol.show_box = ms.show_box;
            mol.box_dirty = true;
            mol.reps = ms.build_reps(self.rep_defaults.kind);
            mol.selected_rep = (!mol.reps.is_empty()).then_some(0);

            // Replay trajectory loads (synchronous: a session load is a discrete
            // action and the frames are needed before the first render).
            if !ms.traj_loads.is_empty() {
                mol.seed_frame0();
                for tl in &ms.traj_loads {
                    let opts = LoadOptions {
                        from: tl.from,
                        to: tl.to,
                        stride: tl.stride.max(1),
                    };
                    match data::traj_loader::read_frames_sync(&tl.path, &opts, mol.n_atoms) {
                        Ok(frames) => {
                            mol.append_frames(frames);
                            mol.traj_loads.push(tl.clone());
                        }
                        Err(e) => errors.push(format!("trajectory {}: {e}", tl.path.display())),
                    }
                }
                mol.trajectory.set_current(ms.current_frame);
                mol.apply_current_frame();
            }
            loaded += 1;
        }

        self.scene.clamp_selection();
        self.scene.selected_mol = (!self.scene.molecules.is_empty()).then_some(0);
        self.apply_view_state(session.view);

        // Opening a document is a new baseline, not an undo step.
        self.history = History::new(EditState::capture(&self.scene));
        self.view_dirty = true;
        self.last_render_camera = None;

        self.status = if errors.is_empty() {
            format!("Loaded session: {loaded} molecule(s)")
        } else {
            for e in &errors {
                log::warn!("load session: {e}");
            }
            format!("Loaded {loaded} molecule(s); {} issue(s) — see log", errors.len())
        };
    }

    /// Load the small bundled structure (2lao) so the web/GitHub-Pages demo opens
    /// to a molecule instead of an empty viewport. Wasm only (embeds the file in
    /// the binary); the native app starts empty and loads via the Open button.
    #[cfg(target_arch = "wasm32")]
    pub fn load_demo(&mut self) {
        const DEMO_PDB: &[u8] = include_bytes!("../../../tests/2lao.pdb");
        match data::load_from_bytes(
            "2lao.pdb",
            DEMO_PDB.to_vec(),
            &self.settings.behavior.bond_params(),
        ) {
            Ok(raw) => self.add_loaded(raw),
            Err(e) => log::error!("demo load failed: {e}"),
        }
    }

    /// Render the "Delete frames" modal: pick a frame range to drop or a decimate
    /// stride (keep every Nth frame), with Delete/Cancel. Trajectory frames are
    /// view state, so this is not undoable (like loading frames).
    fn draw_delete_frames_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.delete_frames_dialog.take() else {
            return;
        };
        let n_frames = self
            .scene
            .molecules
            .iter()
            .find(|m| m.id == dialog.mol_id)
            .map(|m| m.trajectory.n_frames());
        let last = n_frames.unwrap_or(0).saturating_sub(1);
        let mut do_delete = false;
        let mut close = false;

        let modal = egui::Modal::new(egui::Id::new("del_frames_modal")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.heading("Delete trajectory frames");
            match n_frames {
                Some(nf) => {
                    ui.label(format!("{nf} frames loaded (indices 0..{last})"));
                }
                None => {
                    ui.colored_label(
                        egui::Color32::from_rgb(240, 120, 120),
                        "molecule no longer exists",
                    );
                }
            }
            ui.separator();
            tab_bar(
                ui,
                &mut dialog.mode,
                &[
                    (DeleteFramesMode::Range, "Range"),
                    (DeleteFramesMode::Decimate, "Decimate"),
                ],
            );
            ui.add_space(4.0);
            match dialog.mode {
                DeleteFramesMode::Range => {
                    egui::Grid::new("del_range_opts")
                        .num_columns(2)
                        .spacing(egui::vec2(8.0, 4.0))
                        .show(ui, |ui| {
                            ui.label("First frame");
                            ui.add(egui::DragValue::new(&mut dialog.from).range(0..=last));
                            ui.end_row();
                            ui.label("Last frame");
                            ui.add(egui::DragValue::new(&mut dialog.to).range(0..=last));
                            ui.end_row();
                        });
                    ui.weak("Deletes frames in [first, last] inclusive.");
                }
                DeleteFramesMode::Decimate => {
                    ui.horizontal(|ui| {
                        ui.label("Keep every");
                        ui.add(egui::DragValue::new(&mut dialog.stride).range(2..=100_000));
                        ui.label("-th frame");
                    });
                    ui.weak("Keeps frames 0, N, 2N, … and deletes the rest.");
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(n_frames.is_some(), egui::Button::new("Delete"))
                    .clicked()
                {
                    do_delete = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }

        if do_delete {
            if let Some(mol) = self.scene.molecules.iter_mut().find(|m| m.id == dialog.mol_id) {
                let removed = match dialog.mode {
                    DeleteFramesMode::Range => mol.trajectory.delete_range(dialog.from, dialog.to),
                    DeleteFramesMode::Decimate => mol.trajectory.decimate(dialog.stride),
                };
                // Re-render at the (clamped) current frame, or the static structure
                // if every frame was removed.
                mol.box_dirty = true;
                if mol.trajectory.frames.is_empty() {
                    for rep in &mut mol.reps {
                        rep.coords_dirty = true;
                    }
                    if mol.pending.is_some() {
                        mol.glow_dirty = true;
                    }
                } else {
                    mol.apply_current_frame();
                }
                self.status = format!("Deleted {removed} frame(s)");
                self.view_dirty = true;
            }
        } else if !close {
            self.delete_frames_dialog = Some(dialog); // keep open
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
                        ui.add(
                            egui::TextEdit::singleline(&mut dialog.to_text)
                                .desired_width(60.0)
                                .hint_text("end"),
                        );
                        ui.weak("(empty = to end of file)");
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

    /// Modal to rename a molecule's displayed name (set from the molecule menu).
    fn draw_rename_dialog(&mut self, ctx: &egui::Context) {
        let Some((id, mut name)) = self.rename_mol.take() else {
            return;
        };
        let mut commit = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("rename_mol_modal")).show(ctx, |ui| {
            ui.set_width(280.0);
            ui.heading("Rename molecule");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut name)
                    .desired_width(f32::INFINITY)
                    .hint_text("name"),
            );
            resp.request_focus();
            let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.separator();
            ui.horizontal(|ui| {
                let ok = ui
                    .add_enabled(!name.trim().is_empty(), egui::Button::new("Rename"))
                    .clicked();
                commit = ok || (entered && !name.trim().is_empty());
                cancel = ui.button("Cancel").clicked();
            });
        });
        if commit && !name.trim().is_empty() {
            if let Some(mol) = self.scene.molecules.iter_mut().find(|m| m.id == id) {
                mol.name = name.trim().to_string();
            }
        } else if !cancel && !modal.should_close() {
            self.rename_mol = Some((id, name)); // still open — keep the edit buffer
        }
    }

    /// Begin loading the dialog's file into its molecule (sync or async).
    fn start_load(&mut self, dialog: &LoadDialog) -> Result<(), String> {
        let path = dialog.path.clone().ok_or("no file selected")?;
        // Empty "last frame" → read to the end of the file.
        let to = match dialog.to_text.trim() {
            "" => None,
            s => Some(s.parse::<usize>().map_err(|_| "last frame must be a number".to_string())?),
        };
        let opts = LoadOptions { from: dialog.from, to, stride: dialog.stride.max(1) };
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

        // Record the load so a saved session can replay it (native paths only).
        #[cfg(not(target_arch = "wasm32"))]
        let record = TrajLoad {
            path: path.clone(),
            from: opts.from,
            to: opts.to,
            stride: opts.stride,
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
                    mol.traj_loads.push(record);
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
                    if let Some(mol) =
                        self.scene.molecules.iter_mut().find(|m| m.id == dialog.mol_id)
                    {
                        mol.traj_loads.push(record);
                    }
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

        // Contextual suggestion for the rep being edited here: compute (and cache,
        // lazily) this molecule's distinct values, then derive the hint for the
        // last keyword in the edited rep's current selection text. Done up front,
        // before the `&mut` borrow of the molecule, so it can read both the system
        // (to build hints) and the sel text via shared borrows.
        let active_hint: Option<String> = editing.and_then(|r| {
            let mol_id = self.scene.molecules[mi].id;
            self.sel_hints
                .entry(mol_id)
                .or_insert_with(|| SelHints::compute(&self.scene.molecules[mi].system));
            let rep = self.scene.molecules[mi].reps.get(r)?;
            self.sel_hints[&mol_id].hint_for(&rep.sel_text)
        });

        let mol = &mut self.scene.molecules[mi];
        let mol_id = mol.id;
        // The Periodic tab is only offered when the molecule has a box.
        let has_box = mol.system.state().pbox.is_some();

        let mut delete: Option<usize> = None;
        let mut duplicate: Option<usize> = None;
        let mut reorder: Option<(usize, usize)> = None;
        let mut zoom_rep: Option<usize> = None;
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut save_rep: Option<usize> = None;

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
                            let resp = sel_text_edit(
                                ui,
                                &mut rep.sel_text,
                                sel_id,
                                f32::INFINITY,
                                rep.sel_error_caret,
                            );
                            // Editing invalidates the last evaluation: drop the
                            // stale error message / red highlight / empty flag
                            // until the new text is committed (re-evaluated).
                            if resp.changed() {
                                clear_sel_feedback(rep);
                            }
                            if sel_empty && !resp.changed() {
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
                                // Save just the selected atoms to a structure file
                                // (sits left of delete). Native only.
                                #[cfg(not(target_arch = "wasm32"))]
                                if icon_button(ui, icon::FLOPPY_DISK, "Save selection to file")
                                    .clicked()
                                {
                                    save_rep = Some(j);
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
                                        let width = ui.available_width();
                                        let resp = sel_text_edit(
                                            ui,
                                            &mut rep.sel_text,
                                            sel_id,
                                            width,
                                            rep.sel_error_caret,
                                        );
                                        if resp.changed() {
                                            clear_sel_feedback(rep);
                                        }
                                        if sel_empty && !resp.changed() {
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

                    // Contextual suggestion for the keyword being typed (e.g.
                    // `chains: A B C R`, `resid: 2..120`), shown only on the rep
                    // currently being edited.
                    if editing == Some(j) {
                        if let Some(hint) = &active_hint {
                            ui.horizontal(|ui| {
                                ui.add_space(row2_indent);
                                // Truncate to the panel width with an ellipsis so a
                                // long value list (many resnames/names) stays on one
                                // line instead of wrapping.
                                ui.add(
                                    egui::Label::new(egui::RichText::new(hint).small().weak())
                                        .truncate(),
                                );
                            });
                        }
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

        // Save a rep's selection to a file (after the `mol` borrow above ends).
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(j) = save_rep {
            self.save_rep_selection(mi, j);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = save_rep;

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
            // Draw mode: plain LMB is the active drawing tool (handled in `draw_input`);
            // Alt+LMB orbits, so the view can be rotated without leaving Draw. RMB/MMB/
            // wheel navigate as usual.
            let draw_mode = self.draw.is_some();
            let delta = response.drag_delta();
            let mods = ui.input(|i| i.modifiers);
            let (shift, alt) = (mods.shift, mods.alt);
            // Alt+drag in Lasso/Draw mode orbits; otherwise an LMB lasso draws the
            // polygon (lasso), or the drawing tool acts (draw — handled separately).
            let lasso_draw = lasso_mode && !alt;
            // A plain LMB in Draw mode does not orbit (the tool handles it).
            let draw_tool_drag = draw_mode && !alt;
            if response.dragged_by(egui::PointerButton::Primary) {
                if lasso_draw {
                    if let Some(pos) = response.interact_pointer_pos() {
                        // Drop near-duplicate points (drag jitter) for a clean polygon.
                        if self.lasso_path.last().is_none_or(|&p| (p - pos).length() > 1.5) {
                            self.lasso_path.push(pos);
                        }
                    }
                } else if draw_tool_drag {
                    // Drawing tool drag (e.g. Bond rubber-band) — see `draw_input`; no
                    // camera motion here.
                } else if shift && !lasso_mode && !draw_mode {
                    self.camera.roll(delta.x, self.settings.behavior.roll_sensitivity);
                } else {
                    // Non-lasso/non-draw LMB, or Alt+LMB in lasso/draw mode → free orbit.
                    self.camera
                        .orbit(delta.x, delta.y, self.settings.behavior.orbit_sensitivity);
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
                    // Zoom toward the cursor: pass its NDC position (y up) + aspect.
                    let ndc = response
                        .hover_pos()
                        .map(|p| {
                            glam::vec2(
                                ((p.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0,
                                1.0 - ((p.y - rect.top()) / rect.height().max(1.0)) * 2.0,
                            )
                        })
                        .unwrap_or(glam::Vec2::ZERO);
                    let aspect = rect.width() / rect.height().max(1.0);
                    self.camera.zoom_scroll(scroll, ndc, aspect);
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
                    self.camera.ao_uniform(),
                    self.camera.shadow_uniform(),
                    self.camera.background,
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

            // Draw mode: handle the active tool (atom/bond/erase) for this frame's
            // pointer events, draw the bond rubber-band, and drive the debounced
            // minimizer. Plain LMB acts as the tool; Alt+LMB orbits (handled above).
            if self.draw.is_some() {
                self.draw_input(ui, &response, rect, size_px);
            }

            // Hover-info picking. On native the hit comes from the **async GPU
            // id-buffer** (collect a finished readback, request the next); on wasm it's
            // the CPU ray-cast. **Atoms** mode → ring + info box on the hit atom;
            // **Residues** mode → the whole residue staged as a steady glow
            // (`Molecule::hover`) + a residue box. Skipped while dragging the camera.
            //
            // Drain a finished async pick first — every frame, even when not hovering —
            // so the readback frees up (else `request_pick` would stay blocked). When
            // a result lands it updates the cached `hover_pick` ids.
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(ids) = self.renderer.poll_pick(render_state) {
                if std::env::var("MOLAR_VIS_DEBUG_PICK").is_ok() {
                    // DEBUG forces a center pick; compare to the CPU ray-cast there.
                    let aspect = size_px[0] as f32 / size_px[1] as f32;
                    let (view, proj) = (self.camera.view(), self.camera.proj(aspect));
                    let c = pick::pick(&self.scene, view, proj, 0.0, 0.0).map(|h| (h.mol, h.id));
                    let g = ids.map(|(m, _, a)| (m, a));
                    if g == c {
                        log::info!("pick ok: gpu == cpu == {g:?}");
                    } else {
                        log::warn!("pick mismatch: gpu {g:?} != cpu {c:?}");
                    }
                }
                self.hover_pick = ids;
            }

            let hovering = self.pick_mode == PickMode::Click && !response.dragged();
            let mut residue_hit = false;
            let mut lens_shown = false;
            if hovering {
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
                    // GPU id-buffer pick on native (async O(1) readback, scales to huge
                    // systems); the CPU ray-cast stays the path on wasm (WebGL2 can't
                    // render/read back the integer id target).
                    #[cfg(not(target_arch = "wasm32"))]
                    let hit = {
                        // Cursor → pick-target pixel (the pick buffer is 1× = size_px).
                        let px = (((ndc_x * 0.5 + 0.5) * size_px[0] as f32) as i32)
                            .clamp(0, size_px[0] as i32 - 1) as u32;
                        let py = (((1.0 - (ndc_y * 0.5 + 0.5)) * size_px[1] as f32) as i32)
                            .clamp(0, size_px[1] as i32 - 1) as u32;
                        // Re-pick only when the cursor moved or the view changed (else a
                        // stationary hover would spin the GPU every frame).
                        let view_moved = geom_changed || cam_changed || size_changed;
                        if view_moved || self.last_pick_px != Some((px, py)) {
                            self.renderer.request_pick(render_state, &self.scene, px, py, size_px);
                            self.last_pick_px = Some((px, py));
                        }
                        if self.renderer.pick_in_flight() {
                            ui.ctx().request_repaint(); // keep polling until it lands
                        }
                        // Rebuild the PickHit from the cached ids each frame (keeps the
                        // displayed position current as coords change). Lags 1–2 frames.
                        self.hover_pick
                            .and_then(|(m, r, a)| pick::hit_for_atom(&self.scene, m, r, a))
                    };
                    #[cfg(target_arch = "wasm32")]
                    let hit = {
                        let view = self.camera.view();
                        let proj = self.camera.proj(aspect);
                        pick::pick(&self.scene, view, proj, ndc_x, ndc_y)
                    };
                    if let Some(hit) = hit {
                        // Click selects the hovered atom/residue: merge it into the
                        // active (pending) selection (Shift = add, Ctrl/⌘ = subtract,
                        // plain = replace), expanded per the scope mode. A plain LMB
                        // *drag* still orbits (handled above), so only a real click here.
                        if response.clicked() {
                            let m = ui.input(|i| i.modifiers);
                            let op = if m.shift {
                                LassoOp::Add
                            } else if m.command {
                                LassoOp::Subtract
                            } else {
                                LassoOp::Replace
                            };
                            if op == LassoOp::Replace {
                                for mol in &mut self.scene.molecules {
                                    if mol.pending.take().is_some() {
                                        mol.glow_dirty = true;
                                    }
                                }
                            }
                            let mode = self.effective_selection_mode();
                            let hits = {
                                let mol = &self.scene.molecules[hit.mol];
                                pick::expand_selection(&mol.system, &mol.bonds, &[hit.id], mode)
                            };
                            self.merge_into_pending(hit.mol, hits, op);
                            self.view_dirty = true;
                            ui.ctx().request_repaint();
                        }
                        if self.effective_selection_mode() == SelectionMode::Residues {
                            // Expand the hit to its whole residue → steady glow.
                            let atoms = {
                                let mol = &self.scene.molecules[hit.mol];
                                pick::expand_selection(
                                    &mol.system,
                                    &mol.bonds,
                                    &[hit.id],
                                    SelectionMode::Residues,
                                )
                            };
                            draw_residue_info_overlay(ui, rect, &hit, atoms.len());
                            if self.set_hover(hit.mol, atoms) {
                                ui.ctx().request_repaint();
                            }
                            residue_hit = true;
                        } else {
                            // Atoms mode: egui ring + atom info box.
                            draw_pick_overlay(ui, rect, &self.camera, aspect, &hit);
                        }
                    }
                    // Hover detail lens: **independent of any atom hit** — wherever the
                    // cursor view-line passes near a Cartoon/Surface molecule's atoms
                    // (including *between* atoms / in ribbon gaps, which is the whole
                    // point: hint where the atoms are), reveal a faded ball-and-stick of
                    // those atoms. Rebuilt as the cursor moves (grid query is cheap).
                    {
                        let moved = self.last_lens_ndc.map_or(true, |(lx, ly)| {
                            (lx - ndc_x).abs() > 0.004 || (ly - ndc_y).abs() > 0.004
                        });
                        if moved {
                            const R: f32 = 0.35; // lens radius (nm)
                            let view = self.camera.view();
                            let proj = self.camera.proj(aspect);
                            let (o, d) = pick::cursor_ray(view, proj, ndc_x, ndc_y);
                            let t_max = 2.0 * (self.camera.distance + self.camera.scene_radius);
                            // The Cartoon/Surface molecule with the most atoms in the
                            // view-line tube (one lens at a time).
                            let mut best: Option<(usize, Vec<usize>)> = None;
                            for mi in 0..self.scene.molecules.len() {
                                let mol = &mut self.scene.molecules[mi];
                                let wants = mol.visible
                                    && mol.reps.iter().any(|r| {
                                        r.visible
                                            && matches!(r.kind, RepKind::Cartoon | RepKind::Surface)
                                    });
                                if !wants {
                                    continue;
                                }
                                if mol.hover_grid.is_none() {
                                    // The grid holds the lens **seed** atoms — which
                                    // residues the view line passes near: for Cartoon the
                                    // **backbone** (what the ribbon traces); for Surface the
                                    // **solvent-exposed** atoms (per-atom SASA > 0), not
                                    // deep-buried ones. The query then keeps the near,
                                    // camera-facing seeds and expands them to whole residues.
                                    let has_cartoon = mol.reps.iter().any(|r| {
                                        r.visible && matches!(r.kind, RepKind::Cartoon)
                                    });
                                    let has_surface = mol.reps.iter().any(|r| {
                                        r.visible && matches!(r.kind, RepKind::Surface)
                                    });
                                    let grid = {
                                        let st = mol.render_state();
                                        let all = mol.system.select_all();
                                        let b = mol.system.bind_with_state(&all, st);
                                        let sasa = if has_surface {
                                            b.sasa().ok().map(|s| s.areas().to_vec())
                                        } else {
                                            None
                                        };
                                        let pts = b.iter_particle().filter_map(|p| {
                                            // Cartoon → the N–CA–C chain trace only (no
                                            // carbonyl / terminal backbone oxygens).
                                            let keep = (has_cartoon
                                                && matches!(p.atom.name.as_str(), "N" | "CA" | "C"))
                                                || (has_surface
                                                    && sasa.as_ref().is_some_and(|a| {
                                                        a.get(p.id).copied().unwrap_or(0.0) > 0.01
                                                    }));
                                            keep.then_some((
                                                p.id as u32,
                                                glam::Vec3::new(p.pos.x, p.pos.y, p.pos.z),
                                            ))
                                        });
                                        crate::spatial::AtomGrid::build(
                                            pts,
                                            mol.bbox_min,
                                            mol.bbox_max,
                                            R,
                                        )
                                    };
                                    mol.hover_grid = Some(grid);
                                }
                                // Show the **front-facing residues** under the view line
                                // (both Cartoon and Surface). The grid seeds mark which
                                // residues the line passes near — the ribbon backbone for
                                // Cartoon, the solvent-exposed atoms for Surface; keep only
                                // the seeds on the near (camera-facing) half along the ray
                                // (so the far side no longer bleeds through the
                                // cleared-depth overlay), then expand each to its whole
                                // residue so complete residues poke through.
                                let grid = mol.hover_grid.as_ref().unwrap();
                                let cand = grid.atoms_near_ray_t(o, d, R, 0.0, t_max);
                                let atoms: Vec<usize> = if cand.is_empty() {
                                    Vec::new()
                                } else {
                                    let (mut t_near, mut t_far) = (f32::INFINITY, f32::NEG_INFINITY);
                                    for &(_, t) in &cand {
                                        t_near = t_near.min(t);
                                        t_far = t_far.max(t);
                                    }
                                    let mid = 0.5 * (t_near + t_far);
                                    let seeds: Vec<usize> = cand
                                        .iter()
                                        .filter(|&&(_, t)| t <= mid)
                                        .map(|&(id, _)| id as usize)
                                        .collect();
                                    pick::expand_selection(
                                        &mol.system,
                                        &mol.bonds,
                                        &seeds,
                                        SelectionMode::Residues,
                                    )
                                };
                                if !atoms.is_empty()
                                    && best.as_ref().map_or(true, |(_, a)| atoms.len() > a.len())
                                {
                                    best = Some((mi, atoms));
                                }
                            }
                            if let Some((mi, atoms)) = best {
                                // The lens now shows whole residues (≈0.8 nm) for both
                                // Cartoon and Surface, so widen the perpendicular fade past
                                // the R-tube selection radius or residue side chains would
                                // fade out.
                                self.set_hover_detail(mi, atoms, o, d, R * 1.8);
                                lens_shown = true;
                            }
                            self.last_lens_ndc = Some((ndc_x, ndc_y));
                            ui.ctx().request_repaint();
                        } else {
                            // Cursor barely moved: keep the current lens (if any).
                            lens_shown =
                                self.scene.molecules.iter().any(|m| m.hover_detail.is_some());
                        }
                    }
                } else {
                    // Cursor left the viewport → drop the cached GPU hit.
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        self.hover_pick = None;
                        self.last_pick_px = None;
                    }
                }
            } else {
                // Not in hover mode (or dragging) → drop the cached GPU hit.
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.hover_pick = None;
                    self.last_pick_px = None;
                }
            }
            // Drop any stale hover highlight (left a residue, no hit, or not hovering).
            if !residue_hit && self.clear_hover() {
                ui.ctx().request_repaint();
            }
            // Drop the detail lens when the view-line isn't near a Cartoon/Surface
            // molecule's atoms (or not hovering).
            if !lens_shown && self.clear_hover_detail() {
                ui.ctx().request_repaint();
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

    /// The selection mode in effect for the current pick mode. `Bound H` is
    /// meaningless for single-atom hover picking, so it falls back to `Atoms` there
    /// (and the toolbar hides it); it stays available for the lasso.
    fn effective_selection_mode(&self) -> SelectionMode {
        if self.pick_mode == PickMode::Click && self.selection_mode == SelectionMode::BoundH {
            SelectionMode::Atoms
        } else {
            self.selection_mode
        }
    }

    /// Set molecule `mi`'s steady hover highlight to `atoms`, clearing every other
    /// molecule's. Returns whether anything changed (so the caller can request a
    /// repaint to rebuild the glow).
    fn set_hover(&mut self, mi: usize, atoms: Vec<usize>) -> bool {
        let mut changed = false;
        for (i, mol) in self.scene.molecules.iter_mut().enumerate() {
            if i != mi && mol.hover.take().is_some() {
                mol.hover_dirty = true;
                changed = true;
            }
        }
        if let Some(mol) = self.scene.molecules.get_mut(mi) {
            if mol.hover.as_deref() != Some(atoms.as_slice()) {
                mol.hover = Some(atoms);
                mol.hover_dirty = true;
                changed = true;
            }
        }
        changed
    }

    /// Clear every molecule's hover highlight. Returns whether anything changed.
    fn clear_hover(&mut self) -> bool {
        let mut changed = false;
        for mol in &mut self.scene.molecules {
            if mol.hover.take().is_some() {
                mol.hover_dirty = true;
                changed = true;
            }
        }
        changed
    }

    /// Stage molecule `mi`'s hover detail lens (always rebuilt — the fade tracks the
    /// ray, so any cursor move changes it). Clears the lens on other molecules.
    fn set_hover_detail(
        &mut self,
        mi: usize,
        atoms: Vec<usize>,
        ray_o: glam::Vec3,
        ray_d: glam::Vec3,
        radius: f32,
    ) {
        for (i, mol) in self.scene.molecules.iter_mut().enumerate() {
            if i != mi && mol.hover_detail.take().is_some() {
                mol.hover_detail_dirty = true;
            }
        }
        if let Some(mol) = self.scene.molecules.get_mut(mi) {
            mol.hover_detail = Some(crate::scene::HoverDetail { atoms, ray_o, ray_d, radius });
            mol.hover_detail_dirty = true;
        }
    }

    /// Clear every molecule's hover detail lens. Returns whether anything changed.
    fn clear_hover_detail(&mut self) -> bool {
        let mut changed = false;
        for mol in &mut self.scene.molecules {
            if mol.hover_detail.take().is_some() {
                mol.hover_detail_dirty = true;
                changed = true;
            }
        }
        self.last_lens_ndc = None;
        changed
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
            // Expand this gesture's raw hits per the selection mode (exact atoms /
            // whole residues / heavy + bonded H), then merge into the pending set.
            let hits = {
                let mol = &self.scene.molecules[res.mol];
                pick::expand_selection(&mol.system, &mol.bonds, &res.atoms, mode)
            };
            self.merge_into_pending(res.mol, hits, op);
        }
        self.view_dirty = true;
    }

    /// Combine `hits` into molecule `mi`'s **active (pending) selection** per `op`
    /// (Replace/Add union the atoms in, Subtract removes them) and flag its glow.
    /// Shared by the lasso and click-to-select paths; clearing *other* molecules'
    /// pending sets for a `Replace` is the caller's job.
    fn merge_into_pending(&mut self, mi: usize, hits: Vec<usize>, op: LassoOp) {
        let mol = &mut self.scene.molecules[mi];
        let mut set: std::collections::BTreeSet<usize> = mol
            .pending
            .as_ref()
            .map(|p| p.atoms.iter().copied().collect())
            .unwrap_or_default();
        match op {
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
            None => mol.pending = None,
        }
        mol.glow_dirty = true;
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

// ===========================================================================
// Draw mode — interactive molecule sketching
// ===========================================================================
//
// Draw mode lets the user build a small molecule by hand: place atoms in the
// viewport, draw bonds between them, cycle bond orders, and erase, with the
// force-field cleanup minimizer (`crate::minimize`) relaxing the strained
// hand-drawn geometry into sensible bond lengths/angles. State lives in an
// `App::draw: Option<DrawSession>`; it's mutually exclusive with the pick modes
// (turning one on clears the other). Cross-platform (no native-only deps in the
// pointer/UI path), so it works in the browser too.

/// An element on the drawing palette. Atoms are built via
/// `Atom::new().with_name(symbol).guess()`, which fills element/mass/vdw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Element {
    H,
    C,
    N,
    O,
    S,
    P,
    F,
    Cl,
    Br,
    I,
}

impl Element {
    /// The full palette, in display order (Carbon-first, the common organics, then
    /// the heteroatoms and halogens).
    const ALL: [Element; 10] = [
        Element::C,
        Element::H,
        Element::N,
        Element::O,
        Element::S,
        Element::P,
        Element::F,
        Element::Cl,
        Element::Br,
        Element::I,
    ];

    /// The atomic symbol — the name fed to molar's `Atom::with_name(..).guess()`,
    /// and the palette button label.
    fn symbol(self) -> &'static str {
        match self {
            Element::H => "H",
            Element::C => "C",
            Element::N => "N",
            Element::O => "O",
            Element::S => "S",
            Element::P => "P",
            Element::F => "F",
            Element::Cl => "Cl",
            Element::Br => "Br",
            Element::I => "I",
        }
    }

    /// Atomic number (for the CPK palette color).
    fn atomic_number(self) -> u8 {
        match self {
            Element::H => 1,
            Element::C => 6,
            Element::N => 7,
            Element::O => 8,
            Element::F => 9,
            Element::P => 15,
            Element::S => 16,
            Element::Cl => 17,
            Element::Br => 35,
            Element::I => 53,
        }
    }

    /// Build a fresh molar `Atom` of this element (element/mass/vdw guessed from the
    /// name). The residue is a generic `DRG` so a drawn molecule reads as one ligand.
    fn make_atom(self) -> molar::prelude::Atom {
        molar::prelude::Atom::new()
            .with_name(self.symbol())
            .with_resname("DRG")
            .guess()
    }
}

/// Which drawing tool the pointer acts as. There is no separate atom/bond mode —
/// the single **Draw** tool infers the action from the gesture: click empty space →
/// place an atom; drag from an existing atom → grow a bond; drag from empty space →
/// two bonded atoms; click a bond → cycle its order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DrawTool {
    /// Click/drag to add atoms and bonds (the default; see the type doc).
    Draw,
    /// Click an atom to delete it (+ its bonds), or a bond to delete the bond.
    Erase,
}

/// What the cursor is over in the drawing editor (used by hover-highlight + click).
#[derive(Clone, Copy)]
enum HitTarget {
    Atom(usize),
    Bond(usize),
}

/// In-progress pointer gesture for the Draw tool. `current` is the live cursor
/// position (viewport pixels) for the rubber-band line.
enum DrawDrag {
    /// No drag in progress.
    Idle,
    /// Dragging a bond out of an existing atom `from` (global index).
    FromAtom { from: usize, current: egui::Pos2 },
    /// Dragging from empty space (`start`) → will create two bonded atoms on release.
    FromEmpty { start: egui::Pos2, current: egui::Pos2 },
}

/// An in-flight relaxation. The minimizer is cheap for a few-atom molecule, so a job
/// is a one-shot — run a single `relax_in_system` call next eligible frame, then
/// clear. `to_convergence` picks the profile (Cleanup vs Quick).
struct RelaxJob {
    /// Steps budget left (unused in the one-shot model, kept for the job shape /
    /// future incremental stepping per the Draw-mode contract). 0 ⇒ done.
    #[allow(dead_code)]
    remaining: u32,
    /// Whether to run the full Cleanup profile (the "Clean up" button) vs Quick.
    to_convergence: bool,
}

/// State of an active drawing session.
struct DrawSession {
    /// The molecule edits land in. `None` until the first atom is placed (which
    /// *creates* the molecule, since molar can't append to a 0-atom system).
    target: Option<MolId>,
    /// The active tool.
    tool: DrawTool,
    /// The element placed by the Atom tool / a bonded end atom.
    element: Element,
    /// Default order for newly drawn bonds.
    bond_order: crate::minimize::BondOrder,
    /// The Bond tool's in-progress drag (rubber band).
    drag: DrawDrag,
    /// A committed edit happened; relax once the pointer settles (debounced).
    minimize_pending: bool,
    /// Active relaxation job, if any (drives the per-frame `relax_in_system` call).
    relax: Option<RelaxJob>,
    /// A world point the drawing plane passes through (updated to the last-touched
    /// atom). `None` ⇒ use the camera target.
    plane_depth: Option<glam::Vec3>,
}

impl Default for DrawSession {
    fn default() -> Self {
        Self {
            target: None,
            tool: DrawTool::Draw,
            element: Element::C,
            bond_order: crate::minimize::BondOrder::Single,
            drag: DrawDrag::Idle,
            minimize_pending: false,
            relax: None,
            plane_depth: None,
        }
    }
}

impl App {
    /// Turn Draw mode on (a fresh session) or off. Mutually exclusive with picking:
    /// entering Draw forces `pick_mode = Off` (and clears any in-progress lasso).
    fn toggle_draw(&mut self) {
        if self.draw.is_some() {
            self.draw = None;
        } else {
            self.draw = Some(DrawSession::default());
            self.pick_mode = PickMode::Off;
            self.lasso_path.clear();
        }
    }

    /// Create a new editable molecule from a freshly seeded single-atom `RawMolecule`,
    /// give it a Ball-and-Stick rep (Element color), mark it `editable`, select it,
    /// and record its `MolId` on `session.target`. Shared by the first Atom-tool click
    /// and the headless preset hook.
    fn start_drawn_molecule(&mut self, raw: data::RawMolecule, session: &mut DrawSession) {
        let rep_defaults = self.rep_defaults.clone();
        let id = self.scene.add(raw, &rep_defaults);
        let mol = self.scene.molecules.last_mut().unwrap();
        mol.editable = true;
        mol.trajectory.speed_fps = self.settings.behavior.traj_fps;
        mol.trajectory.loop_mode = self.settings.behavior.loop_mode;
        // A drawn molecule reads best as Ball-and-Stick / Element color.
        if let Some(rep) = mol.reps.first_mut() {
            rep.kind = RepKind::BallAndStick;
            rep.params = RepParams::for_kind(RepKind::BallAndStick);
            rep.color = ColorMethod::Element;
            rep.sel_text = "all".to_string();
            rep.sel_dirty = true;
            rep.geom_dirty = true;
        }
        self.scene.selected_mol = Some(self.scene.molecules.len() - 1);
        session.target = Some(id);
        self.view_dirty = true;
    }

    /// The Draw-mode toggle button + (when active) a second toolbar row with the
    /// tool selector, element palette, bond-order selector, and the Clean up / New /
    /// Finish actions. Drawn inside `draw_view_toolbar`'s panel. Returns nothing; all
    /// state lives in `self.draw`.
    /// The vertical drawing-tools palette, shown as a narrow right-side panel only
    /// while Draw mode is active. Icon-only buttons (labels in hover tooltips): tool
    /// selector, CPK-colored element chips, bond-order line icons, then Clean-up /
    /// New / Finish actions. Scrolls if the window is short.
    fn draw_tools_panel(&mut self, ui: &mut egui::Ui) {
        if self.draw.is_none() {
            return;
        }
        egui::Panel::right("draw_tools_panel")
            .resizable(false)
            .default_size(54.0)
            .show_inside(ui, |ui| {
                ui.add_space(6.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(4.0, 5.0);

                        // — Tools (Draw / Erase) —
                        let cur_tool = self.draw.as_ref().map(|d| d.tool);
                        for tool in [DrawTool::Draw, DrawTool::Erase] {
                            let glyph = match tool {
                                DrawTool::Draw => icon::PENCIL_SIMPLE,
                                DrawTool::Erase => icon::ERASER,
                            };
                            let tip = match tool {
                                DrawTool::Draw => "Draw — click to add an atom, drag to add a bond; click a bond to cycle its order",
                                DrawTool::Erase => "Erase — click an atom or bond to delete it",
                            };
                            if overlay_button(ui, glyph, cur_tool == Some(tool))
                                .on_hover_text(tip)
                                .clicked()
                            {
                                if let Some(d) = self.draw.as_mut() {
                                    d.tool = tool;
                                    d.drag = DrawDrag::Idle;
                                }
                            }
                        }

                        ui.separator();

                        // — Element palette — CPK-colored chips, two per row;
                        // picking one also switches to the Draw tool.
                        let cur_el = self.draw.as_ref().map(|d| d.element);
                        let mut pick_el = None;
                        for pair in Element::ALL.chunks(2) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
                                for &el in pair {
                                    let rgba = crate::color::element_color(el.atomic_number());
                                    if Self::element_chip(ui, el.symbol(), rgba, cur_el == Some(el))
                                        .on_hover_text(format!("Draw {} atoms", el.symbol()))
                                        .clicked()
                                    {
                                        pick_el = Some(el);
                                    }
                                }
                            });
                        }
                        if let (Some(el), Some(d)) = (pick_el, self.draw.as_mut()) {
                            d.element = el;
                            d.tool = DrawTool::Draw;
                        }

                        ui.separator();

                        // — Bond order (1 / 2 / 3 lines) for new bonds.
                        let cur_ord = self.draw.as_ref().map(|d| d.bond_order);
                        for (order, n) in [
                            (crate::minimize::BondOrder::Single, 1u8),
                            (crate::minimize::BondOrder::Double, 2),
                            (crate::minimize::BondOrder::Triple, 3),
                        ] {
                            if Self::bond_order_icon(ui, n, cur_ord == Some(order))
                                .on_hover_text(format!("{} bond", order.label()))
                                .clicked()
                            {
                                if let Some(d) = self.draw.as_mut() {
                                    d.bond_order = order;
                                }
                            }
                        }

                        ui.separator();

                        // — Clean up (relax to convergence) — disabled until a molecule exists.
                        let has_target = self.draw.as_ref().and_then(|d| d.target).is_some();
                        let cleanup = ui
                            .add_enabled_ui(has_target, |ui| {
                                overlay_button(ui, icon::SPARKLE, false)
                                    .on_hover_text("Clean up — relax the geometry to convergence")
                                    .clicked()
                            })
                            .inner;
                        if cleanup {
                            if let Some(d) = self.draw.as_mut() {
                                d.relax = Some(RelaxJob { remaining: 1, to_convergence: true });
                            }
                        }
                        // — H+ : add explicit hydrogens to satisfy valence, or remove
                        // them if the molecule already has any (toggle).
                        let toggle_h = ui
                            .add_enabled_ui(has_target, |ui| {
                                overlay_button(ui, "H+", false)
                                    .on_hover_text("Add/remove explicit hydrogens")
                                    .clicked()
                            })
                            .inner;
                        if toggle_h {
                            if let Some(mi) =
                                self.draw.as_ref().and_then(|d| d.target).and_then(|id| {
                                    self.scene.molecules.iter().position(|m| m.id == id)
                                })
                            {
                                self.toggle_hydrogens(mi);
                            }
                        }
                        // — New: the next click starts a fresh molecule.
                        if overlay_button(ui, icon::FILE_PLUS, false)
                            .on_hover_text("New — start a fresh molecule on the next click")
                            .clicked()
                        {
                            if let Some(d) = self.draw.as_mut() {
                                d.target = None;
                                d.drag = DrawDrag::Idle;
                            }
                        }
                        // — Finish: leave Draw mode.
                        if overlay_button(ui, icon::CHECK, false)
                            .on_hover_text("Finish — leave Draw mode")
                            .clicked()
                        {
                            self.draw = None;
                        }

                        // Alt-to-rotate hint (icon only; tooltip explains).
                        if ui.input(|i| i.modifiers.alt) {
                            ui.separator();
                            ui.colored_label(
                                egui::Color32::from_rgb(150, 190, 230),
                                egui::RichText::new(icon::ARROWS_CLOCKWISE).size(16.0),
                            )
                            .on_hover_text("Alt: rotate the view");
                        }
                    });
                });
            });
    }

    /// A small CPK-colored element chip (icon-only palette button): a rounded square
    /// filled with the element's color, the symbol drawn in contrasting ink, a ring
    /// when active/hovered.
    fn element_chip(ui: &mut egui::Ui, symbol: &str, rgba: [u8; 4], active: bool) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
        let fill = egui::Color32::from_rgb(rgba[0], rgba[1], rgba[2]);
        let chip = rect.shrink(1.0);
        ui.painter().rect_filled(chip, 4.0, fill);
        if active {
            ui.painter().rect_stroke(
                chip,
                4.0,
                egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Inside,
            );
        } else if resp.hovered() {
            ui.painter().rect_stroke(
                chip,
                4.0,
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(180)),
                egui::StrokeKind::Inside,
            );
        }
        // Contrasting label by luminance.
        let lum = 0.299 * rgba[0] as f32 + 0.587 * rgba[1] as f32 + 0.114 * rgba[2] as f32;
        let txt = if lum > 140.0 { egui::Color32::BLACK } else { egui::Color32::WHITE };
        let font = egui::TextStyle::Button.resolve(ui.style());
        let galley = ui.painter().layout_no_wrap(symbol.to_owned(), font, txt);
        let ink = galley.mesh_bounds;
        ui.painter().galley(rect.center() - ink.center().to_vec2(), galley, txt);
        resp
    }

    /// A bond-order icon button: `n` stacked horizontal lines on the toolbar-button
    /// frame (1 = single, 2 = double, 3 = triple).
    fn bond_order_icon(ui: &mut egui::Ui, n: u8, active: bool) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 26.0), egui::Sense::click());
        let vis = ui.style().interact_selectable(&resp, active);
        let fill = if active {
            ui.visuals().selection.bg_fill
        } else {
            vis.weak_bg_fill
        };
        ui.painter().rect_filled(rect, 4.0, fill);
        let col = ui.visuals().text_color();
        let c = rect.center();
        let half_w = 7.0;
        let spacing = 4.0;
        let total = (n as f32 - 1.0) * spacing;
        for i in 0..n {
            let y = c.y - total / 2.0 + i as f32 * spacing;
            ui.painter().line_segment(
                [egui::pos2(c.x - half_w, y), egui::pos2(c.x + half_w, y)],
                egui::Stroke::new(1.8, col),
            );
        }
        resp
    }

    /// Project a viewport pixel onto the active drawing plane → a world point (nm).
    /// The plane passes through `plane_depth` (the last-touched atom, else the camera
    /// target) with the camera view direction as its normal, so freshly placed atoms
    /// land on the focal plane the user is looking at. `None` if the ray is parallel
    /// to the plane (degenerate).
    fn drawing_plane_point(&self, px: egui::Pos2, rect: egui::Rect, size_px: [u32; 2]) -> Option<glam::Vec3> {
        let ndc_x = ((px.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0;
        let ndc_y = 1.0 - ((px.y - rect.top()) / rect.height().max(1.0)) * 2.0;
        let aspect = size_px[0] as f32 / size_px[1] as f32;
        let view = self.camera.view();
        let proj = self.camera.proj(aspect);
        let (ro, rd) = pick::cursor_ray(view, proj, ndc_x, ndc_y);
        // Plane normal = the view direction toward the eye (camera-facing).
        let n = (self.camera.eye() - self.camera.target).normalize_or_zero();
        let p0 = self
            .draw
            .as_ref()
            .and_then(|d| d.plane_depth)
            .unwrap_or(self.camera.target);
        let denom = rd.dot(n);
        if denom.abs() < 1.0e-5 {
            return None;
        }
        let t = (p0 - ro).dot(n) / denom;
        if !t.is_finite() {
            return None;
        }
        Some(ro + rd * t)
    }

    /// Pixel → world ray on the current camera (for snap-to-atom hit tests).
    fn cursor_world_ray(&self, px: egui::Pos2, rect: egui::Rect, size_px: [u32; 2]) -> (glam::Vec3, glam::Vec3) {
        let ndc_x = ((px.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0;
        let ndc_y = 1.0 - ((px.y - rect.top()) / rect.height().max(1.0)) * 2.0;
        let aspect = size_px[0] as f32 / size_px[1] as f32;
        pick::cursor_ray(self.camera.view(), self.camera.proj(aspect), ndc_x, ndc_y)
    }

    /// Project a world point (nm) to a viewport pixel (for the rubber-band's start).
    fn world_to_pixel(&self, world: glam::Vec3, rect: egui::Rect, size_px: [u32; 2]) -> Option<egui::Pos2> {
        let aspect = size_px[0] as f32 / size_px[1] as f32;
        let mvp = self.camera.proj(aspect) * self.camera.view();
        let clip = mvp * world.extend(1.0);
        if clip.w.abs() < 1.0e-6 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some(egui::pos2(
            rect.left() + (ndc.x * 0.5 + 0.5) * rect.width(),
            rect.top() + (1.0 - (ndc.y * 0.5 + 0.5)) * rect.height(),
        ))
    }

    /// World position (nm) of atom `i` of molecule `mi` at the displayed frame.
    fn atom_world(&self, mi: usize, i: usize) -> Option<glam::Vec3> {
        let mol = self.scene.molecules.get(mi)?;
        let p = mol.render_state().coords.get(i)?;
        Some(glam::vec3(p.x, p.y, p.z))
    }

    /// Pointer handling for the active drawing tool (atom/bond/erase) + the bond
    /// rubber-band + the debounced minimizer. Called from `draw_viewport` each frame
    /// while Draw mode is on. Alt+LMB orbits (handled by the navigation block), so the
    /// tool only acts when Alt is *not* held.
    fn draw_input(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        rect: egui::Rect,
        size_px: [u32; 2],
    ) {
        let alt = ui.input(|i| i.modifiers.alt);

        // Keyboard shortcuts (unless a text field is focused): element + bond order.
        if !ui.ctx().egui_wants_keyboard_input() {
            use egui::Key;
            let el = ui.input(|i| {
                if i.key_pressed(Key::C) { Some(Element::C) }
                else if i.key_pressed(Key::N) { Some(Element::N) }
                else if i.key_pressed(Key::O) { Some(Element::O) }
                else if i.key_pressed(Key::H) { Some(Element::H) }
                else if i.key_pressed(Key::P) { Some(Element::P) }
                else if i.key_pressed(Key::S) { Some(Element::S) }
                else { None }
            });
            let ord = ui.input(|i| {
                if i.key_pressed(Key::Num1) { Some(crate::minimize::BondOrder::Single) }
                else if i.key_pressed(Key::Num2) { Some(crate::minimize::BondOrder::Double) }
                else if i.key_pressed(Key::Num3) { Some(crate::minimize::BondOrder::Triple) }
                else { None }
            });
            if let Some(d) = self.draw.as_mut() {
                if let Some(el) = el {
                    d.element = el;
                    d.tool = DrawTool::Draw;
                }
                if let Some(o) = ord {
                    d.bond_order = o;
                }
            }
        }

        // Snapshot the session's tool params (avoids holding a borrow on `self.draw`
        // while we mutate `self.scene`).
        let (tool, element, bond_order, target) = match self.draw.as_ref() {
            Some(d) => (d.tool, d.element, d.bond_order, d.target),
            None => return,
        };

        // Resolve the target molecule's index (it may have been deleted/reordered).
        let target_mi = target.and_then(|id| self.scene.molecules.iter().position(|m| m.id == id));

        // --- Draw-gesture rubber-band (drawn regardless of which event fired) --
        // Update the live cursor + paint the line from the gesture's start (an atom
        // or an empty-space point) to the cursor.
        if matches!(tool, DrawTool::Draw) && !alt {
            if let Some(pos) = response.interact_pointer_pos().or_else(|| response.hover_pos()) {
                if let Some(d) = self.draw.as_mut() {
                    match &mut d.drag {
                        DrawDrag::FromAtom { current, .. }
                        | DrawDrag::FromEmpty { current, .. } => *current = pos,
                        DrawDrag::Idle => {}
                    }
                }
            }
            // Start point of the rubber band: the source atom's screen position, or
            // the empty-space start pixel.
            let band: Option<(egui::Pos2, egui::Pos2)> = match self.draw.as_ref().map(|d| &d.drag) {
                Some(DrawDrag::FromAtom { from, current }) => target_mi
                    .and_then(|mi| self.atom_world(mi, *from))
                    .and_then(|w| self.world_to_pixel(w, rect, size_px))
                    .map(|s| (s, *current)),
                Some(DrawDrag::FromEmpty { start, current }) => Some((*start, *current)),
                _ => None,
            };
            if let Some((start, current)) = band {
                let painter = ui.painter_at(rect);
                painter.line_segment(
                    [start, current],
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(130, 215, 255)),
                );
                ui.ctx().request_repaint();
            }
        }

        // --- Tool events (suppressed while Alt orbits) -------------------------
        if !alt {
            match tool {
                DrawTool::Draw => self.draw_input_draw(response, rect, size_px, element, bond_order, target_mi),
                DrawTool::Erase => self.draw_input_erase(response, rect, size_px, target_mi),
            }
            if response.clicked() || response.drag_stopped_by(egui::PointerButton::Primary) {
                ui.ctx().request_repaint();
            }
        }

        // --- Debounced minimizer + active relax job ----------------------------
        self.drive_minimize(ui);

        // --- Overlays: hover highlight, aromatic ring circles, "?" on unspecified ---
        self.draw_draw_overlays(ui, rect, size_px);
    }

    /// Unit world direction from `src` toward the cursor, in the screen-parallel
    /// drawing plane (so a new bonded atom grows in the dragged direction). Falls back
    /// to camera-right when degenerate.
    fn drag_dir(&self, src: glam::Vec3, cursor: egui::Pos2, rect: egui::Rect, size_px: [u32; 2]) -> glam::Vec3 {
        let aim = self.drawing_plane_point(cursor, rect, size_px).unwrap_or(src);
        let d = (aim - src).normalize_or_zero();
        if d == glam::Vec3::ZERO {
            self.camera.right()
        } else {
            d
        }
    }

    /// The projected screen radius (px) of atom `i` as drawn (its rep's sphere).
    fn atom_screen_radius(&self, mi: usize, i: usize, rect: egui::Rect, size_px: [u32; 2]) -> f32 {
        let mol = &self.scene.molecules[mi];
        let (Some(w), Some(atom)) = (self.atom_world(mi, i), mol.system.topology().get_atom(i)) else {
            return 4.0;
        };
        let r_world = mol
            .reps
            .iter()
            .find(|r| r.visible)
            .map_or(0.05, |r| pick::effective_radius(&r.params, atom));
        let right = self.camera.orientation * glam::Vec3::X;
        match (self.world_to_pixel(w, rect, size_px), self.world_to_pixel(w + right * r_world, rect, size_px)) {
            (Some(c), Some(e)) => (e - c).length().max(3.0),
            _ => 4.0,
        }
    }

    /// What the cursor is over: an atom **only when the cursor is inside that atom's
    /// drawn sphere** (front-most), otherwise the nearest bond *outside* the spheres.
    /// Shared by hover-highlight and click so they agree.
    fn draw_hit_test(&self, mi: usize, rect: egui::Rect, size_px: [u32; 2], cursor: egui::Pos2) -> Option<HitTarget> {
        let mol = &self.scene.molecules[mi];
        // Front-most atom whose drawn sphere the cursor is inside.
        let (ro, rd) = self.cursor_world_ray(cursor, rect, size_px);
        if let Some((i, _)) = pick::nearest_atom(mol, ro, rd, 0.04) {
            if let Some(c) = self.atom_world(mi, i).and_then(|w| self.world_to_pixel(w, rect, size_px)) {
                if (cursor - c).length() <= self.atom_screen_radius(mi, i, rect, size_px) {
                    return Some(HitTarget::Atom(i));
                }
            }
        }
        // Outside every sphere → the nearest bond, if the cursor is close to its line.
        let aspect = size_px[0] as f32 / size_px[1].max(1) as f32;
        let (view, proj) = (self.camera.view(), self.camera.proj(aspect));
        let ndc = glam::vec2(
            ((cursor.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0,
            1.0 - ((cursor.y - rect.top()) / rect.height().max(1.0)) * 2.0,
        );
        pick::nearest_bond(mol, view, proj, ndc, 0.02).map(HitTarget::Bond)
    }

    /// Painter overlays for the drawing editor: a hover highlight on the atom/bond
    /// under the cursor (shown during a drag too), a gray circle in the plane of each
    /// aromatic ring, and a "?" over each unspecified-order bond that's front-facing
    /// and on-screen (capped, so a big loaded molecule isn't flooded).
    fn draw_draw_overlays(&self, ui: &egui::Ui, rect: egui::Rect, size_px: [u32; 2]) {
        let Some(d) = self.draw.as_ref() else { return };
        let Some(mi) = d
            .target
            .and_then(|id| self.scene.molecules.iter().position(|m| m.id == id))
        else {
            return;
        };
        let mol = &self.scene.molecules[mi];
        let painter = ui.painter_at(rect);
        let view = self.camera.view();
        let px_of = |w: glam::Vec3| self.world_to_pixel(w, rect, size_px);
        let eye_z = |w: glam::Vec3| (view * w.extend(1.0)).z; // larger (less negative) = nearer

        // "?" over unspecified bonds — only the front-facing, on-screen ones, capped.
        let center = (mol.bbox_min + mol.bbox_max) * 0.5;
        let center_z = eye_z(center);
        let font = egui::FontId::proportional(13.0);
        let mut shown = 0usize;
        const Q_MAX: usize = 150;
        for b in &mol.bonds {
            if shown >= Q_MAX {
                break;
            }
            if b.order != crate::minimize::BondOrder::Unspecified {
                continue;
            }
            let (Some(p), Some(q)) = (self.atom_world(mi, b.i1), self.atom_world(mi, b.i2)) else {
                continue;
            };
            let mid = (p + q) * 0.5;
            if eye_z(mid) < center_z {
                continue; // back half of the molecule
            }
            if let Some(m) = px_of(mid).filter(|m| rect.contains(*m)) {
                painter.text(m, egui::Align2::CENTER_CENTER, "?", font.clone(), egui::Color32::from_rgb(255, 200, 90));
                shown += 1;
            }
        }

        // (The aromatic-ring circle is drawn as depth-tested 3-D line geometry in the
        // scene — `Molecule::aromatic_gpu` — not as a flat overlay here, so it occludes.)

        // Hover highlight (also during a drag, so you see the atom you'd bond to —
        // `pointer_latest_pos` stays valid while the button is held, unlike hover_pos).
        if let Some(px) = ui.ctx().pointer_latest_pos().filter(|p| rect.contains(*p)) {
            match self.draw_hit_test(mi, rect, size_px, px) {
                Some(HitTarget::Atom(i)) => {
                    if let Some(c) = self.atom_world(mi, i).and_then(px_of) {
                        // Ring sized to the atom's drawn sphere (tracks zoom).
                        let rpx = self.atom_screen_radius(mi, i, rect, size_px);
                        draw_glow_ring(&painter, c, rpx);
                    }
                }
                Some(HitTarget::Bond(k)) => {
                    let b = mol.bonds[k];
                    if let (Some(p), Some(q)) =
                        (self.atom_world(mi, b.i1).and_then(px_of), self.atom_world(mi, b.i2).and_then(px_of))
                    {
                        painter.line_segment(
                            [p, q],
                            egui::Stroke::new(5.0, egui::Color32::from_rgba_unmultiplied(120, 220, 255, 160)),
                        );
                    }
                }
                None => {}
            }
        }
    }

    /// Place an atom of `element` at world `pos` (nm), creating the drawn molecule
    /// from this first atom if none exists yet (molar can't append to a 0-atom
    /// system). Returns `(molecule index, new atom's global index)`.
    fn place_atom(&mut self, element: Element, pos: glam::Vec3) -> Option<(usize, usize)> {
        let target = self.draw.as_ref().and_then(|d| d.target);
        let mi = target.and_then(|id| self.scene.molecules.iter().position(|m| m.id == id));
        match mi {
            Some(mi) => {
                let idx = self.scene.molecules[mi].add_atom(&element.make_atom(), pos)?;
                Some((mi, idx))
            }
            None => {
                let raw = match data::RawMolecule::single_atom("drawn", element.make_atom(), pos) {
                    Ok(raw) => raw,
                    Err(e) => {
                        log::error!("draw: {e}");
                        self.status = e;
                        return None;
                    }
                };
                let mut session = self.draw.take().unwrap_or_default();
                session.element = element;
                self.start_drawn_molecule(raw, &mut session);
                let mi = session
                    .target
                    .and_then(|id| self.scene.molecules.iter().position(|m| m.id == id));
                self.draw = Some(session);
                mi.map(|mi| (mi, 0)) // the seed atom is index 0
            }
        }
    }

    /// The unified Draw tool. The gesture decides the action (no separate atom/bond
    /// mode): click empty → place an atom (the first creates the molecule); drag from
    /// an atom → grow a bond (to another atom, or to a new atom on empty space); drag
    /// from empty → two bonded atoms; click a bond → cycle its order; click an atom →
    /// no-op (reserved for element-change).
    fn draw_input_draw(
        &mut self,
        response: &egui::Response,
        rect: egui::Rect,
        size_px: [u32; 2],
        element: Element,
        bond_order: crate::minimize::BondOrder,
        target_mi: Option<usize>,
    ) {
        // --- Begin a drag: from an existing atom, or from empty space. ---------
        if response.drag_started_by(egui::PointerButton::Primary) {
            let Some(px) = response.interact_pointer_pos() else { return };
            let from_atom = target_mi.and_then(|mi| {
                let (ro, rd) = self.cursor_world_ray(px, rect, size_px);
                pick::nearest_atom(&self.scene.molecules[mi], ro, rd, 0.08).map(|(i, _)| i)
            });
            // Drag from an existing atom → set the drawing plane to that atom's depth, so
            // a new bonded atom on release lands next to it (not on the far camera-target
            // plane, which produced a stray long bond on big molecules).
            let from_depth = from_atom
                .zip(target_mi)
                .and_then(|(from, mi)| self.atom_world(mi, from));
            if let Some(d) = self.draw.as_mut() {
                if let Some(w) = from_depth {
                    d.plane_depth = Some(w);
                }
                d.drag = match from_atom {
                    Some(from) => DrawDrag::FromAtom { from, current: px },
                    None => DrawDrag::FromEmpty { start: px, current: px },
                };
            }
            return;
        }

        // --- Finish a drag → add a bond / two bonded atoms. --------------------
        if response.drag_stopped_by(egui::PointerButton::Primary) {
            let drag = match self.draw.as_mut() {
                Some(d) => std::mem::replace(&mut d.drag, DrawDrag::Idle),
                None => return,
            };
            let Some(px) = response.interact_pointer_pos() else { return };
            match drag {
                DrawDrag::FromAtom { from, .. } => {
                    let mi = match target_mi {
                        Some(mi) => mi,
                        None => return,
                    };
                    let (ro, rd) = self.cursor_world_ray(px, rect, size_px);
                    let dest =
                        pick::nearest_atom(&self.scene.molecules[mi], ro, rd, 0.08).map(|(i, _)| i);
                    match dest {
                        // Onto another atom → add a bond, or override an existing
                        // bond's order if one already joins them.
                        Some(to) if to != from => {
                            if self.scene.molecules[mi].set_or_add_bond(from, to, bond_order) {
                                self.after_draw_edit(mi, self.atom_world(mi, to));
                            }
                        }
                        Some(_) => {} // same atom → no-op
                        // Onto empty space → a new atom one bond-length from the source
                        // in the dragged direction (fixed length, not wherever the cursor
                        // landed — robust + Marvin-style; the relax refines small molecules).
                        None => {
                            let src = self.atom_world(mi, from).unwrap_or(self.camera.target);
                            let new_pos = src + self.drag_dir(src, px, rect, size_px) * DRAW_BOND_LEN;
                            if let Some((mi, new_idx)) = self.place_atom(element, new_pos) {
                                self.scene.molecules[mi].add_bond(from, new_idx, bond_order);
                                self.after_draw_edit(mi, Some(new_pos));
                            }
                        }
                    }
                }
                DrawDrag::FromEmpty { start, .. } => {
                    // Drag from empty space → two bonded atoms: the first at the start
                    // point, the second one bond-length away in the drag direction. A
                    // negligible drag falls back to a single atom.
                    let Some(p_start) = self.drawing_plane_point(start, rect, size_px) else { return };
                    let Some((mi, a)) = self.place_atom(element, p_start) else { return };
                    if (px - start).length() < 6.0 {
                        self.after_draw_edit(mi, Some(p_start)); // too short → one atom
                        return;
                    }
                    let p_end = p_start + self.drag_dir(p_start, px, rect, size_px) * DRAW_BOND_LEN;
                    if let Some((_, b)) = self.place_atom(element, p_end) {
                        self.scene.molecules[mi].add_bond(a, b, bond_order);
                        self.after_draw_edit(mi, Some(p_end));
                    }
                }
                DrawDrag::Idle => {}
            }
            return;
        }

        // --- A plain click (no drag). ------------------------------------------
        if response.clicked() {
            let Some(px) = response.interact_pointer_pos() else { return };
            // On an atom → replace its element (keeping bonds). On a bond → cycle its
            // order. Else place a new atom. Uses the same closer-wins hit-test as the
            // hover highlight so the click matches what was highlighted.
            if let Some(mi) = target_mi {
                match self.draw_hit_test(mi, rect, size_px, px) {
                    Some(HitTarget::Atom(i)) => {
                        if self.scene.molecules[mi].system.topology().get_atom(i).map(|a| a.atomic_number)
                            != Some(element.atomic_number())
                        {
                            let src = element.make_atom();
                            self.scene.molecules[mi].set_atom_element(i, &src);
                            self.after_draw_edit(mi, self.atom_world(mi, i));
                        }
                        return;
                    }
                    Some(HitTarget::Bond(k)) => {
                        self.scene.molecules[mi].cycle_bond_order(k);
                        self.after_draw_edit(mi, None);
                        return;
                    }
                    None => {}
                }
            }
            // Empty space → place an atom (creating the molecule if needed).
            let pos = self
                .drawing_plane_point(px, rect, size_px)
                .unwrap_or(self.camera.target);
            if let Some((mi, _)) = self.place_atom(element, pos) {
                self.after_draw_edit(mi, Some(pos));
            }
        }
    }

    /// Common post-edit bookkeeping for a Draw-tool change: refresh the molecule's
    /// bbox, flag the rebuild, advance the drawing-plane depth to the last-touched
    /// point, and arm the debounced relax (only worth it once there's something to
    /// relax — more than a lone atom).
    fn after_draw_edit(&mut self, mi: usize, plane: Option<glam::Vec3>) {
        let worth_relaxing = {
            let mol = &mut self.scene.molecules[mi];
            mol.refresh_bbox();
            // Auto-perception + auto-relax only for editor-scale molecules: ring
            // perception is cheap but relaxing the whole molecule is O(N²) per step, so
            // editing a large loaded structure (e.g. a protein) must NOT relax/aromatize
            // the whole thing on every click. Above the cap, edits apply instantly; the
            // user can still hand-build a small fragment.
            if mol.n_atoms <= DRAW_AUTO_MAX_ATOMS {
                // Perceive rings + aromaticity: a freshly closed ring with alternating
                // orders becomes aromatic (bonds → Aromatic, rings cached for the overlay).
                mol.perceive_aromaticity();
                mol.n_atoms > 1 || !mol.bonds.is_empty()
            } else {
                false // skip the whole-molecule relax on large structures
            }
        };
        self.flag_edit(mi);
        if let Some(d) = self.draw.as_mut() {
            if let Some(p) = plane {
                d.plane_depth = Some(p);
            }
            if worth_relaxing {
                d.minimize_pending = true;
            }
        }
    }

    /// Toggle explicit hydrogens on the drawn molecule: if it already has any H, remove
    /// them all; otherwise add the implicit-hydrogen count to every heavy atom (placed at
    /// rough offsets and then relaxed). Undoable + perceived like any draw edit.
    fn toggle_hydrogens(&mut self, mi: usize) {
        let has_h = {
            let mol = &self.scene.molecules[mi];
            let topo = mol.system.topology();
            (0..mol.n_atoms).any(|i| topo.get_atom(i).is_some_and(|a| a.atomic_number == 1))
        };
        if has_h {
            let mol = &mut self.scene.molecules[mi];
            let topo = mol.system.topology();
            let h_idx: Vec<usize> = (0..mol.n_atoms)
                .filter(|&i| topo.get_atom(i).is_some_and(|a| a.atomic_number == 1))
                .collect();
            // Don't strip an all-hydrogen molecule down to nothing.
            if !h_idx.is_empty() && h_idx.len() < mol.n_atoms {
                mol.remove_atoms(&h_idx);
            }
        } else {
            // Offset directions for placed H (FIRE relaxes them into real geometry); the
            // per-H nudge breaks symmetry so e.g. methane doesn't start in a planar saddle.
            const DIRS: [glam::Vec3; 6] = [
                glam::Vec3::X, glam::Vec3::NEG_X, glam::Vec3::Y,
                glam::Vec3::NEG_Y, glam::Vec3::Z, glam::Vec3::NEG_Z,
            ];
            let mol = &mut self.scene.molecules[mi];
            let counts = mol.implicit_hydrogens();
            let parents: Vec<(usize, glam::Vec3, u8)> = (0..mol.n_atoms)
                .filter_map(|i| {
                    let c = *counts.get(i)?;
                    let p = mol.system.state().coords.get(i)?;
                    (c > 0).then_some((i, glam::vec3(p.x, p.y, p.z), c))
                })
                .collect();
            let h_atom = Element::H.make_atom();
            for (i, p, c) in parents {
                for k in 0..c as usize {
                    let k = k as f32;
                    let dir = (DIRS[(c as usize - 1 + k as usize) % 6]
                        + glam::vec3(0.01 * k, 0.013 * (k + 1.0), 0.017 * k))
                    .normalize_or_zero();
                    if let Some(hi) = mol.add_atom(&h_atom, p + dir * 0.11) {
                        mol.add_bond(i, hi, crate::minimize::BondOrder::Single);
                    }
                }
            }
        }
        self.after_draw_edit(mi, None);
    }

    /// Erase tool: a click deletes the nearest atom (and its bonds; deleting the
    /// molecule if it becomes empty), else the nearest bond.
    fn draw_input_erase(
        &mut self,
        response: &egui::Response,
        rect: egui::Rect,
        size_px: [u32; 2],
        target_mi: Option<usize>,
    ) {
        if !response.clicked() {
            return;
        }
        let Some(mi) = target_mi else { return };
        let Some(px) = response.interact_pointer_pos() else { return };
        let (ro, rd) = self.cursor_world_ray(px, rect, size_px);
        if let Some((i, _)) = pick::nearest_atom(&self.scene.molecules[mi], ro, rd, 0.08) {
            let empty = self.scene.molecules[mi].remove_atom(i);
            if empty {
                // The molecule is now empty → delete it (park in trash, undoable).
                let m = self.scene.molecules.remove(mi);
                self.loaders.remove(&m.id);
                #[cfg(target_arch = "wasm32")]
                self.wasm_loaders.remove(&m.id);
                self.scene.trash.insert(m.id, m);
                self.scene.clamp_selection();
                if let Some(d) = self.draw.as_mut() {
                    d.target = None;
                    d.drag = DrawDrag::Idle;
                    d.minimize_pending = false;
                }
                self.view_dirty = true;
            } else {
                self.flag_edit(mi);
                if let Some(d) = self.draw.as_mut() {
                    d.minimize_pending = true;
                }
            }
            return;
        }
        // No atom hit → try a bond.
        let aspect = size_px[0] as f32 / size_px[1] as f32;
        let (view, proj) = (self.camera.view(), self.camera.proj(aspect));
        let ndc = glam::vec2(
            ((px.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0,
            1.0 - ((px.y - rect.top()) / rect.height().max(1.0)) * 2.0,
        );
        if let Some(k) = pick::nearest_bond(&self.scene.molecules[mi], view, proj, ndc, 0.02) {
            self.scene.molecules[mi].remove_bond_at(k);
            self.flag_edit(mi);
            if let Some(d) = self.draw.as_mut() {
                d.minimize_pending = true;
            }
        }
    }

    /// Mark molecule `mi`'s reps dirty after a structural edit (rebuild geometry +
    /// the GPU pick buffer) and flag a re-render.
    fn flag_edit(&mut self, mi: usize) {
        if let Some(mol) = self.scene.molecules.get_mut(mi) {
            for rep in &mut mol.reps {
                rep.sel_dirty = true; // the selection set ("all") grows/shrinks
                rep.geom_dirty = true;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                mol.pick_dirty = true;
            }
        }
        self.view_dirty = true;
    }

    /// Debounced minimizer: when an edit is pending and the pointer has settled, run a
    /// Quick relax; an active relax job (Quick or Cleanup) runs one `relax_in_system`
    /// call then clears. Repaints only while a relax job is active (idle = 0 GPU).
    fn drive_minimize(&mut self, ui: &mut egui::Ui) {
        // Promote a pending edit to a relax job once the pointer has settled — the same
        // predicate the history checkpoint uses, so we never relax mid-drag.
        let settled = !ui.ctx().egui_is_using_pointer();
        let (start_quick, target) = match self.draw.as_ref() {
            Some(d) => (
                d.minimize_pending && settled && d.relax.is_none(),
                d.target,
            ),
            None => (false, None),
        };
        if start_quick {
            if let Some(d) = self.draw.as_mut() {
                d.relax = Some(RelaxJob { remaining: 1, to_convergence: false });
                d.minimize_pending = false;
            }
        }

        // Run an active relax job (one-shot: a single relax call, then done).
        let kind = match self.draw.as_ref().and_then(|d| d.relax.as_ref()) {
            Some(job) if job.to_convergence => Some(crate::minimize::RelaxKind::Cleanup),
            Some(_) => Some(crate::minimize::RelaxKind::Quick),
            None => None,
        };
        if let Some(kind) = kind {
            if let Some(mi) = target.and_then(|id| self.scene.molecules.iter().position(|m| m.id == id)) {
                let mol = &mut self.scene.molecules[mi];
                // Relaxing is O(N²) per step (all-pairs vdW) — never auto-relax a large
                // loaded structure; clear the job and leave coordinates as drawn.
                if mol.n_atoms > DRAW_AUTO_MAX_ATOMS {
                    if let Some(d) = self.draw.as_mut() {
                        d.relax = None;
                    }
                    return;
                }
                let _ = crate::minimize::relax_in_system(
                    &mut mol.system,
                    &mol.bonds,
                    kind,
                );
                mol.refresh_bbox();
                // Coordinates moved → in-place GPU coord update (no rebuild) + glow +
                // aromatic circles follow.
                for rep in &mut mol.reps {
                    rep.coords_dirty = true;
                }
                if !mol.aromatic_rings.is_empty() {
                    mol.aromatic_dirty = true;
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    mol.pick_dirty = true;
                }
                if mol.pending.is_some() {
                    mol.glow_dirty = true;
                }
                self.view_dirty = true;
            }
            // One-shot: clear the job.
            if let Some(d) = self.draw.as_mut() {
                d.relax = None;
            }
            // Keep repainting while relaxing (a Cleanup ran in one frame, but request
            // one more paint so the moved coords show immediately).
            ui.ctx().request_repaint();
        }
    }
}

// Regression test for the Wayland IME workaround (`defuse_broken_ime`). Reproduces the
// broken event stream a recent Wayland/winit combo emits — a flood of `Ime(Disabled)`
// plus one `Ime(Commit)` per keystroke, with no `Enabled`/`Preedit` — which egui's
// `TextEdit` otherwise drops after the first character. Linux-only (the workaround and
// the bug are Linux/Wayland-specific); CI runs on Linux.
#[cfg(all(test, target_os = "linux"))]
mod ime_workaround_tests {
    use super::*;

    fn raw(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(400.0, 400.0),
            )),
            events,
            ..Default::default()
        }
    }

    fn run(ctx: &egui::Context, text: &mut String, id: egui::Id, events: Vec<egui::Event>) {
        let _ = ctx.run(raw(events), |ctx| {
            defuse_broken_ime(ctx);
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add(egui::TextEdit::singleline(text).id(id));
            });
        });
    }

    /// Typing `a`,`b`,`c` arrives as `Ime(Commit)` amid `Ime(Disabled)` noise; with the
    /// workaround every character is inserted (without it, egui keeps only the first).
    #[test]
    fn ime_commit_stream_accumulates_into_empty_field() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("f");
        let mut text = String::new();
        ctx.memory_mut(|m| m.request_focus(id));
        run(&ctx, &mut text, id, vec![egui::Event::Ime(egui::ImeEvent::Disabled)]);
        for ch in ["a", "b", "c"] {
            run(
                &ctx,
                &mut text,
                id,
                vec![
                    egui::Event::Ime(egui::ImeEvent::Disabled),
                    egui::Event::Ime(egui::ImeEvent::Commit(ch.into())),
                    egui::Event::Ime(egui::ImeEvent::Disabled),
                ],
            );
        }
        assert_eq!(text, "abc");
    }

    /// The same stream must also append to *pre-existing* text (the cursor starts > 0,
    /// which is the case egui's commit gate rejects outright).
    #[test]
    fn ime_commit_stream_appends_to_existing_text() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("f");
        let mut text = String::from("all");
        ctx.memory_mut(|m| m.request_focus(id));
        // One frame to place the cursor at the end of the existing text.
        run(&ctx, &mut text, id, vec![]);
        for ch in ["X", "Y"] {
            run(
                &ctx,
                &mut text,
                id,
                vec![egui::Event::Ime(egui::ImeEvent::Commit(ch.into()))],
            );
        }
        assert_eq!(text, "allXY");
    }
}
