//! Representation rows: selection field, rep params, traj/periodic tabs, traj bar.
use super::*;
use super::widgets::*;
use super::pickers::*;


/// Overlay a red border + a right-justified "⚠ 0!" on a selection field whose
/// selection is valid but matched **zero atoms** (molar's "empty" error, surfaced
/// as a non-destructive warning — the text stays editable).
pub(super) fn mark_empty_selection(ui: &egui::Ui, rect: egui::Rect) {
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
pub(super) fn clear_sel_feedback(rep: &mut Representation) {
    rep.sel_error = None;
    rep.sel_error_span = None;
    rep.sel_empty = false;
}

/// Draw the rep selection `TextEdit`. When `error_span` is `Some(range)`, that byte
/// range of the text — molar's offending-word span — is painted **red** via a custom
/// layouter, marking the whole bad word in place. Returns the field's `Response`.
pub(super) fn sel_text_edit(
    ui: &mut egui::Ui,
    text: &mut String,
    id: egui::Id,
    width: f32,
    error_span: Option<std::ops::Range<usize>>,
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
        // Highlight the span only if it's in-bounds and on char boundaries (it may be
        // momentarily stale while the text is being edited).
        let valid = error_span.as_ref().filter(|r| {
            r.start < r.end && r.end <= s.len() && s.is_char_boundary(r.start) && s.is_char_boundary(r.end)
        });
        match valid {
            Some(r) => {
                job.append(&s[..r.start], 0.0, fmt(font_id.clone(), base));
                job.append(&s[r.start..r.end], 0.0, fmt(font_id.clone(), red));
                job.append(&s[r.end..], 0.0, fmt(font_id, base));
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

pub(super) fn draw_rep_params(ui: &mut egui::Ui, rep: &mut Representation, has_box: bool) -> bool {
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
            // Interactions params are edited in a separate dialog (Partner row +
            // Settings button drawn by the caller), so nothing inline here.
            RepParams::Interactions { .. } => {}
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
pub(super) fn draw_periodic_tab(ui: &mut egui::Ui, rep: &mut Representation) -> bool {
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
    // One row per axis: [− n +] −x  [− n +] +x  (counts of images along ±a, ±b, ±c).
    // Each count is a spinbox (drag/edit the value, or click the ∓ step buttons).
    egui::Grid::new("periodic_images")
        .num_columns(4)
        .spacing(egui::vec2(6.0, 4.0))
        .show(ui, |ui| {
            for (axis, name) in [(0usize, "x"), (1, "y"), (2, "z")] {
                changed |= spin_u32(ui, &mut p.neg[axis], 0..=8);
                ui.label(format!("−{name}"));
                changed |= spin_u32(ui, &mut p.pos[axis], 0..=8);
                ui.label(format!("+{name}"));
                ui.end_row();
            }
        });
    changed
}

/// A compact `u32` spinbox: a `DragValue` flanked by `−`/`+` step buttons that
/// decrement/increment by one (clamped to `range`). The value can still be dragged
/// or typed directly in the middle field. Returns true if it changed this frame.
pub(super) fn spin_u32(ui: &mut egui::Ui, value: &mut u32, range: std::ops::RangeInclusive<u32>) -> bool {
    let (min, max) = (*range.start(), *range.end());
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let step = egui::vec2(20.0, 0.0);
        if ui
            .add_enabled(*value > min, egui::Button::new("−").min_size(step))
            .clicked()
        {
            *value -= 1;
            changed = true;
        }
        changed |= ui
            .add(egui::DragValue::new(value).range(range.clone()))
            .changed();
        if ui
            .add_enabled(*value < max, egui::Button::new("+").min_size(step))
            .clicked()
        {
            *value += 1;
            changed = true;
        }
    });
    changed
}

/// [Traj] tab of the representation settings: per-frame behavior.
pub(super) fn draw_traj_tab(ui: &mut egui::Ui, rep: &mut Representation) {
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
pub(super) fn draw_traj_bar(ui: &mut egui::Ui, traj: &mut Trajectory) -> bool {
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

/// A molecular-**group** cycle bar: first · prev · [member slider] · next · last.
/// Modeled on the trajectory bar's second row but for choosing which group member is
/// shown — deliberately **no** play/pause, fps, loop, or step (a group is a set of
/// distinct molecules, not an animation). Returns the newly chosen member index when
/// it changes. Caller ensures the group has ≥2 members (disabled otherwise).
pub(super) fn draw_group_bar(ui: &mut egui::Ui, names: &[String], current: usize) -> Option<usize> {
    let n_members = names.len();
    if n_members < 2 {
        return None;
    }
    let last = n_members - 1;
    let mut cur = current.min(last);
    ui.horizontal(|ui| {
        compact_actions(ui);
        if icon_button(ui, icon::SKIP_BACK, "First molecule").clicked() {
            cur = 0;
        }
        if icon_button(ui, icon::CARET_LEFT, "Previous molecule").clicked() {
            cur = cur.saturating_sub(1);
        }
        // The slider stretches between the flanking step buttons (room reserved for
        // the trailing next/last buttons + spacing).
        let reserve = 52.0;
        ui.spacing_mut().slider_width = (ui.available_width() - reserve).max(40.0);
        let resp = ui.add(egui::Slider::new(&mut cur, 0..=last).show_value(false));
        // Tooltip anchored **under the knob** (not at the cursor), showing "N/M name"
        // for the member the knob points at — updates live while dragging.
        if resp.hovered() || resp.dragged() {
            let name = names.get(cur).map(|s| s.as_str()).unwrap_or("");
            let frac = if last > 0 { cur as f32 / last as f32 } else { 0.0 };
            let knob_x = resp.rect.left() + frac * resp.rect.width();
            let pos = egui::pos2(knob_x, resp.rect.bottom() + 4.0);
            egui::Area::new(egui::Id::new("group_cycle_tooltip"))
                .order(egui::Order::Tooltip)
                .fixed_pos(pos)
                .pivot(egui::Align2::CENTER_TOP)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style())
                        .show(ui, |ui| ui.label(format!("{}/{} {}", cur + 1, n_members, name)));
                });
        }
        if icon_button(ui, icon::CARET_RIGHT, "Next molecule").clicked() {
            cur = (cur + 1).min(last);
        }
        if icon_button(ui, icon::SKIP_FORWARD, "Last molecule").clicked() {
            cur = last;
        }
    });
    (cur != current).then_some(cur)
}
impl App {

    /// Representations of the selected molecule as rich rows: a drag handle
    /// (reorder by dragging), the selection text (expands to full width while
    /// focused, collapses on Enter/blur), a drawn style-icon dropdown, and a
    /// right-justified action group (gear→params, eye, update-every-frame,
    /// duplicate, trash). An "Add" button precedes the list.
    /// The representations of molecule `mi`, nested under it: rich two-row blocks
    /// (drag handle · selection · actions / style · color · gear) with
    /// drag-reorder. The "add representation" control lives in the molecule's
    /// header row, not here.
    ///
    /// Draws only reps in `[start, end)`. For an ordinary molecule that's the whole
    /// list (`0..len`, `is_shared = false`). For a [`MolGroup`] the prefix
    /// `0..n_shared` of the shown member is drawn as the **shared** reps under the
    /// group header (`is_shared = true`), and each member's own reps `n_shared..len`
    /// below it. `is_shared` keeps the member's `n_shared` boundary correct as shared
    /// reps are added/removed/reordered, and suppresses the pending-selection UI
    /// (which belongs to a member, not the shared document).
    pub(super) fn draw_reps_for(
        &mut self,
        ui: &mut egui::Ui,
        mi: usize,
        start: usize,
        end: usize,
        is_shared: bool,
    ) -> bool {
        let mut view_dirty = false;
        let editing = self
            .editing_rep
            .filter(|&(m, _)| m == mi)
            .map(|(_, r)| r);
        let mut new_editing = self.editing_rep;

        // Read this molecule's basics + clamp the range with an *immutable* borrow, so
        // the partner-label precompute below (which reads *other* molecules) doesn't
        // conflict with the `&mut mol` borrow taken afterward.
        let (mol_id, has_box, start, end) = {
            let mol = &self.scene.molecules[mi];
            (
                mol.id,
                mol.data.state().pbox.is_some(),
                start.min(mol.reps.len()),
                end.min(mol.reps.len()),
            )
        };
        // For each Interactions rep, resolve its partner into a display label
        // ("Mol N: Rep M" / "(none)" / "(partner lost)") + whether it's a live,
        // clickable (focusable) reference. Done here because it reads other molecules.
        let partner_info: Vec<Option<(String, bool)>> = (start..end)
            .map(|j| {
                let rep = &self.scene.molecules[mi].reps[j];
                if !matches!(rep.kind, RepKind::Interactions) {
                    return None;
                }
                Some(match rep.partner {
                    None => ("(none)".to_string(), false),
                    // Group-aware resolution (follows the group's shown member), so the
                    // label matches what's actually detected against.
                    Some(_) => match super::build::partner_index(&self.scene, rep) {
                        Some((pmi, pr)) => (format!("Mol {}: Rep {}", pmi + 1, pr + 1), true),
                        None => ("(partner lost)".to_string(), false),
                    },
                })
            })
            .collect();

        // Whether we're currently choosing an interaction partner (then each rep row
        // becomes a click target for selecting it as the partner).
        let picking_partner = self.partner_pick.is_some();

        let mol = &mut self.scene.molecules[mi];

        let mut delete: Option<usize> = None;
        let mut duplicate: Option<usize> = None;
        let mut reorder: Option<(usize, usize)> = None;
        let mut zoom_rep: Option<usize> = None;
        // Interactions partner controls: which rep opened partner-pick, which partner
        // (mol id + rep) to focus, and — while picking — which rep row was clicked as
        // the partner. Applied after the `mol` borrow ends.
        let mut start_partner_pick: Option<usize> = None;
        let mut focus_partner: Option<(MoleculeSource, usize)> = None;
        let mut chosen_partner: Option<usize> = None;
        let mut open_settings: Option<usize> = None;
        // When a rep is switched to Interactions, the pre-switch clone (old style) to
        // re-insert so the molecule's look is preserved: (rep index, cloned rep).
        let mut clone_rep: Option<(usize, Representation)> = None;
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut save_rep: Option<usize> = None;

        for j in start..end {
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
                                rep.sel_error_span.clone(),
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
                                            rep.sel_error_span.clone(),
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
                        if let Some(clone) = style_picker(ui, rep) {
                            // Switched to Interactions → keep the old-style rep (below).
                            clone_rep = Some((j, clone));
                        }
                        if matches!(rep.kind, RepKind::Interactions) {
                            // An Interactions rep colors lines by contact type and draws
                            // unlit dashes, so color/material are inert; instead show the
                            // Partner link + Choose button inline on the style row.
                            if let Some(Some((label, valid))) = partner_info.get(j - start) {
                                if *valid {
                                    let link = ui
                                        .add(
                                            egui::Label::new(
                                                egui::RichText::new(label.as_str())
                                                    .color(ui.visuals().hyperlink_color),
                                            )
                                            .sense(egui::Sense::click()),
                                        )
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .on_hover_text("Focus this partner rep");
                                    if link.clicked() {
                                        focus_partner = rep.partner.clone();
                                    }
                                } else {
                                    ui.weak(label.as_str());
                                }
                                if ui
                                    .button(format!("{}  Choose…", icon::CROSSHAIR_SIMPLE))
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .on_hover_text(
                                        "Pick the partner rep: click one in the viewport or the list",
                                    )
                                    .clicked()
                                {
                                    start_partner_pick = Some(j);
                                }
                            }
                        } else {
                            color_picker(ui, rep);
                            material_picker(ui, rep);
                        }
                    });
                })
                .response;

            // Inline params panel (within the side panel), shown when the gear is on.
            if rep.params_open {
                view_dirty |= ui
                    .indent(egui::Id::new(("rep_params", mol_id, j)), |ui| {
                        // Interactions reps: a single line-width slider (applies to all
                        // types) + the button opening the per-type settings dialog. Every
                        // other style uses the normal params grid. (The Partner picker is
                        // on the style row above.)
                        if matches!(rep.kind, RepKind::Interactions) {
                            let mut w_changed = false;
                            if let RepParams::Interactions { settings } = &mut rep.params {
                                ui.horizontal(|ui| {
                                    ui.label("Line width");
                                    w_changed = ui
                                        .add(egui::Slider::new(&mut settings.line_width, 1.0..=6.0))
                                        .changed();
                                    if ui
                                        .button(format!("{}  Settings…", icon::GEAR_SIX))
                                        .on_hover_text(
                                            "Choose which interaction types to show + tune their cutoffs",
                                        )
                                        .clicked()
                                    {
                                        open_settings = Some(j);
                                    }
                                });
                            }
                            if w_changed {
                                rep.geom_dirty = true;
                            }
                            false
                        } else {
                            draw_rep_params(ui, rep, has_box)
                        }
                    })
                    .inner;
            }

            // While choosing an interaction partner, the whole rep block is a click
            // target: hovering tints it, clicking selects it as the partner. (An
            // overlay interact registered after the row, so it wins the click over the
            // row's own buttons.)
            if picking_partner {
                let pick = ui.interact(
                    block.rect,
                    egui::Id::new(("partner_target", mol_id, j)),
                    egui::Sense::click(),
                );
                if pick.hovered() {
                    ui.painter().rect_filled(
                        block.rect,
                        4.0,
                        ui.visuals().selection.bg_fill.linear_multiply(0.35),
                    );
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if pick.clicked() {
                    chosen_partner = Some(j);
                }
            }

            // Reorder drop target spans the whole two-row block (disabled while
            // choosing a partner, where the block is a partner-pick target instead).
            if !picking_partner {
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
                        // Confine reorder to this drawn region (shared vs own): ignore a
                        // drag that originated in the other list (their payloads share the
                        // `usize` type), so the `n_shared` boundary can't be crossed.
                        if (start..end).contains(&*src) {
                            reorder = Some((*src, if before { j } else { j + 1 }));
                        }
                    }
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
        // The pending selection belongs to the molecule, not the shared document —
        // only show it in the own-reps pass.
        if !is_shared && mol.pending.is_some() {
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
            // A duplicated shared rep stays inside the shared prefix.
            if is_shared {
                mol.n_shared += 1;
            }
            view_dirty = true;
        }
        // A rep switched to Interactions: re-insert its old-style clone just above it
        // (kept visible) so the molecule's previous look isn't lost.
        if let Some((j, clone)) = clone_rep {
            mol.reps.insert(j, clone);
            mol.selected_rep = Some(j + 1); // the Interactions rep (now shifted down)
            if is_shared {
                mol.n_shared += 1;
            }
            view_dirty = true;
        }
        if let Some(j) = delete {
            mol.reps.remove(j);
            if is_shared {
                mol.n_shared = mol.n_shared.saturating_sub(1);
            }
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

        // Enter partner-pick mode for an Interactions rep (the [Choose…] button).
        if let Some(j) = start_partner_pick {
            self.partner_pick = Some((mol_id, j));
        }
        // Open the interaction-settings dialog for this rep.
        if let Some(j) = open_settings {
            self.interactions_dialog = Some((mol_id, j));
        }
        // A rep row was clicked while choosing a partner → assign it.
        if let Some(j) = chosen_partner {
            self.assign_partner(mi, j);
        }
        // Focus the camera on a clicked partner rep (reads the partner molecule, so it
        // runs after the `mol` borrow above ends).
        if let Some((src, pr)) = focus_partner {
            let bbox = self
                .scene
                .molecules
                .iter()
                .find(|m| m.source == src)
                .and_then(|pmol| Some(pmol.sel_bbox(pmol.reps.get(pr)?.sel.as_ref()?)));
            if let Some((min, max)) = bbox {
                self.camera.focus_bbox(min, max);
                view_dirty = true;
            }
        }

        self.editing_rep = new_editing;
        view_dirty
    }

    /// The Interactions **Settings** dialog (a movable `egui::Window`): a tab per
    /// interaction type, each exposing that type's full parameter set (enable + all
    /// cutoffs/angles), plus a shared line-width + Defaults footer, for the rep in
    /// `self.interactions_dialog`. Any edit marks the rep `geom_dirty` so its contacts
    /// rebuild. Closed via the window ✕ or when the target rep vanishes.
    pub(super) fn draw_interactions_dialog(&mut self, ctx: &egui::Context) {
        use crate::interactions::InteractionKind as K;
        let Some((mol_id, ri)) = self.interactions_dialog else {
            return;
        };
        let Some(mi) = self.scene.mol_index(mol_id) else {
            self.interactions_dialog = None;
            return;
        };
        let mut open = true;
        egui::Window::new("Interaction settings")
            .collapsible(false)
            .resizable(false)
            .default_width(320.0)
            .open(&mut open)
            .show(ctx, |ui| {
                // A tab per interaction type (a colored dot marks each type's line color).
                tab_bar(
                    ui,
                    &mut self.interactions_tab,
                    &[
                        (K::HBond, "H-bonds"),
                        (K::Hydrophobic, "Hydrophobic"),
                        (K::SaltBridge, "Salt bridges"),
                        (K::PiStacking, "π-stack"),
                        (K::PiCation, "π-cation"),
                        (K::Halogen, "Halogen"),
                    ],
                );
                ui.separator();
                let tab = self.interactions_tab;
                let Some(rep) = self.scene.molecules.get_mut(mi).and_then(|m| m.reps.get_mut(ri))
                else {
                    return;
                };
                let mut changed = false;
                if let RepParams::Interactions { settings: s } = &mut rep.params {
                    // A legend swatch + the enable checkbox for the active type.
                    let swatch = |ui: &mut egui::Ui, kind: K| {
                        let c = geometry::interaction_color(kind);
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                        ui.painter()
                            .rect_filled(rect, 3.0, egui::Color32::from_rgb(c[0], c[1], c[2]));
                    };
                    let slider = |ui: &mut egui::Ui,
                                  on: bool,
                                  label: &str,
                                  v: &mut f32,
                                  range: std::ops::RangeInclusive<f32>,
                                  suffix: &str|
                     -> bool {
                        ui.label(label);
                        let c = ui
                            .add_enabled(on, egui::Slider::new(v, range).suffix(suffix.to_string()))
                            .changed();
                        ui.end_row();
                        c
                    };
                    match tab {
                        K::HBond => {
                            ui.horizontal(|ui| {
                                swatch(ui, K::HBond);
                                changed |= ui.checkbox(&mut s.hbonds, "Detect hydrogen bonds").changed();
                            });
                            let on = s.hbonds;
                            egui::Grid::new("hb_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                                changed |= slider(ui, on, "Donor–acceptor distance", &mut s.hbond_dist, 0.25..=0.50, " nm");
                                changed |= slider(ui, on, "D–A distance (with H)", &mut s.hbond_dist_h, 0.25..=0.50, " nm");
                                changed |= slider(ui, on, "Min D–H···A angle", &mut s.hbond_angle, 90.0..=180.0, "°");
                            });
                            ui.add_space(4.0);
                            ui.small("With an explicit hydrogen on the donor the distance-with-H + angle test is used; otherwise the heavy-atom donor–acceptor distance.");
                        }
                        K::Hydrophobic => {
                            ui.horizontal(|ui| {
                                swatch(ui, K::Hydrophobic);
                                changed |= ui.checkbox(&mut s.hydrophobic, "Detect hydrophobic contacts").changed();
                            });
                            let on = s.hydrophobic;
                            egui::Grid::new("hy_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                                changed |= slider(ui, on, "Max C···C distance", &mut s.hydrophobic_dist, 0.30..=0.55, " nm");
                            });
                            ui.add_space(4.0);
                            ui.small("Carbons whose only neighbours are C/H. One contact is kept per residue pair.");
                        }
                        K::SaltBridge => {
                            ui.horizontal(|ui| {
                                swatch(ui, K::SaltBridge);
                                changed |= ui.checkbox(&mut s.salt_bridges, "Detect salt bridges").changed();
                            });
                            let on = s.salt_bridges;
                            egui::Grid::new("sb_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                                changed |= slider(ui, on, "Max charge-centre distance", &mut s.salt_bridge_dist, 0.40..=0.70, " nm");
                            });
                            ui.add_space(4.0);
                            ui.small("Charged groups: Arg/Lys/His (+), Asp/Glu (−), and ligand carboxylate / guanidinium / phosphate.");
                        }
                        K::PiStacking => {
                            ui.horizontal(|ui| {
                                swatch(ui, K::PiStacking);
                                changed |= ui.checkbox(&mut s.pi_stacking, "Detect π-stacking").changed();
                            });
                            let on = s.pi_stacking;
                            egui::Grid::new("ps_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                                changed |= slider(ui, on, "Max ring-centre distance", &mut s.pi_stacking_dist, 0.40..=0.70, " nm");
                                changed |= slider(ui, on, "Plane angle tolerance", &mut s.pi_stacking_angle, 10.0..=45.0, "°");
                                changed |= slider(ui, on, "Max offset (parallel)", &mut s.pi_stacking_offset, 0.0..=0.40, " nm");
                            });
                            ui.add_space(4.0);
                            ui.small("Aromatic rings that are near-parallel (within the tolerance, and overlapping) or T-shaped.");
                        }
                        K::PiCation => {
                            ui.horizontal(|ui| {
                                swatch(ui, K::PiCation);
                                changed |= ui.checkbox(&mut s.pi_cation, "Detect π-cation").changed();
                            });
                            let on = s.pi_cation;
                            egui::Grid::new("pc_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                                changed |= slider(ui, on, "Max ring–cation distance", &mut s.pi_cation_dist, 0.40..=0.75, " nm");
                                changed |= slider(ui, on, "Max offset from ring axis", &mut s.pi_cation_offset, 0.0..=0.40, " nm");
                            });
                            ui.add_space(4.0);
                            ui.small("A cationic group sitting over an aromatic ring face.");
                        }
                        K::Halogen => {
                            ui.horizontal(|ui| {
                                swatch(ui, K::Halogen);
                                changed |= ui.checkbox(&mut s.halogen, "Detect halogen bonds").changed();
                            });
                            let on = s.halogen;
                            egui::Grid::new("hx_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                                changed |= slider(ui, on, "Max X···acceptor distance", &mut s.halogen_dist, 0.30..=0.50, " nm");
                                changed |= slider(ui, on, "Min C–X···A angle", &mut s.halogen_angle, 120.0..=180.0, "°");
                            });
                            ui.add_space(4.0);
                            ui.small("Cl/Br/I bonded to carbon donating to an O/N/S acceptor along the σ-hole.");
                        }
                    }
                    ui.separator();
                    // (Line width is a rep-level control, shown inline in the rep panel —
                    // it applies to all interaction types — so it's not in this dialog.)
                    if ui
                        .button(format!("{}  Reset all to defaults", icon::ARROW_COUNTER_CLOCKWISE))
                        .clicked()
                    {
                        let lw = s.line_width; // keep the (inline-edited) line width
                        *s = crate::interactions::InteractionSettings::default();
                        s.line_width = lw;
                        changed = true;
                    }
                }
                if changed {
                    rep.geom_dirty = true;
                }
            });
        if !open {
            self.interactions_dialog = None;
        }
    }
}
