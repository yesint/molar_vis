//! Style / color / material pickers and their icon + preview painters.
use super::*;
use super::widgets::*;


/// Draw a small vector icon depicting a representation style into `rect`.
pub(super) fn paint_style_icon(painter: &egui::Painter, rect: egui::Rect, kind: RepKind, color: egui::Color32) {
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
pub(super) fn style_option(ui: &mut egui::Ui, kind: RepKind, selected: bool) -> bool {
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

/// A drawn style-icon + label button that opens a dropdown of style options.
pub(super) fn style_picker(ui: &mut egui::Ui, rep: &mut Representation) {
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
pub(super) fn paint_color_icon(painter: &egui::Painter, rect: egui::Rect, method: ColorMethod) {
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
pub(super) fn color_option(ui: &mut egui::Ui, method: ColorMethod, selected: bool, arrow: bool) -> egui::Response {
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
pub(super) const SOLID_SWATCHES: [[u8; 4]; 18] = [
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
pub(super) fn swatch_button(ui: &mut egui::Ui, c: [u8; 4], selected: bool) -> bool {
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
pub(super) fn current_solid(method: ColorMethod) -> [u8; 4] {
    match method {
        ColorMethod::Solid(c) => c,
        _ => crate::color::DEFAULT_SOLID,
    }
}

/// A drawn color-scheme icon + label button that opens a dropdown of options.
/// The built-in schemes pick-and-close; **`Solid` opens a submenu** with a grid of
/// preset swatches and a full color picker (changes are undoable like any other
/// coloring change).
pub(super) fn color_picker(ui: &mut egui::Ui, rep: &mut Representation) {
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
pub(super) fn preview_shade(
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
pub(super) fn push_preview_sphere(
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
pub(super) fn push_preview_bond(
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
pub(super) fn paint_material_preview(painter: &egui::Painter, rect: egui::Rect, material: Material) {
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

pub(super) fn paint_material_icon(painter: &egui::Painter, rect: egui::Rect, material: Material) {
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
pub(super) fn material_cell(ui: &mut egui::Ui, material: Material, selected: bool) -> bool {
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
pub(super) fn material_picker(ui: &mut egui::Ui, rep: &mut Representation) {
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
