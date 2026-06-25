//! Viewport overlays: pick info, residue info, modifier hint, axes gizmo, glow ring.
use super::*;


/// The blue glow ring shared by hover-picking and the active-selection highlight:
/// a faint thick halo fading inward to a bright thin core, centered at `center`
/// with core pixel radius `rpx`.
pub(super) fn draw_glow_ring(painter: &egui::Painter, center: egui::Pos2, rpx: f32) {
    let glow = |a: u8| egui::Color32::from_rgba_unmultiplied(130, 215, 255, a);
    painter.circle_stroke(center, rpx + 4.0, egui::Stroke::new(6.0, glow(35)));
    painter.circle_stroke(center, rpx + 1.5, egui::Stroke::new(3.0, glow(95)));
    painter.circle_stroke(center, rpx, egui::Stroke::new(1.8, glow(235)));
}

/// Draw the hover-pick highlight over the viewport: a glowing outline ring at the
/// hovered atom's **displayed** position (sized to the rep's sphere radius) plus a
/// lower-left info box with the atom's identity and **real** coordinates (nm).
pub(super) fn draw_pick_overlay(
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
pub(super) fn draw_residue_info_overlay(ui: &egui::Ui, rect: egui::Rect, hit: &crate::pick::PickHit, n: usize) {
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
pub(super) fn draw_info_box(painter: &egui::Painter, rect: egui::Rect, lines: &[String]) {
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

/// Floating overlay on the 3D viewport showing the active selection-modifier action
/// (add / subtract / rotate) while a modifier is held in Click/Lasso mode. Frameless
/// glyph + label, in the action's `color`, centered at the **very top** of `rect` —
/// painted over the 3D image, so it never resizes the viewport (unlike a toolbar row).
pub(super) fn draw_modifier_hint_overlay(
    ui: &egui::Ui,
    rect: egui::Rect,
    glyph: &str,
    text: &str,
    color: egui::Color32,
) {
    let painter = ui.painter_at(rect);
    let g_icon = painter.layout_no_wrap(glyph.to_string(), egui::FontId::proportional(16.0), color);
    let g_text = painter.layout_no_wrap(text.to_string(), egui::FontId::proportional(14.0), color);
    let gap = 6.0;
    let w = g_icon.size().x + gap + g_text.size().x;
    let h = g_icon.size().y.max(g_text.size().y);
    let x = rect.center().x - w * 0.5;
    let y = rect.top() + 3.0;
    let icon_w = g_icon.size().x;
    painter.galley(egui::pos2(x, y + (h - g_icon.size().y) * 0.5), g_icon, color);
    painter.galley(egui::pos2(x + icon_w + gap, y + (h - g_text.size().y) * 0.5), g_text, color);
}

/// Draw a VMD-style orientation-axes gizmo into the chosen corner of `rect`.
/// The three world axes (X red, Y green, Z blue) rotate with the camera; only the
/// positive directions are drawn, labelled, and depth-sorted so nearer axes sit on top.
pub(super) fn draw_axes_overlay(ui: &egui::Ui, rect: egui::Rect, camera: &Camera, corner: Corner) {
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
