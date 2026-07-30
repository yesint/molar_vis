//! Representation rows: selection field, rep params, traj/periodic tabs, traj bar.
use super::*;
use super::widgets::*;
use super::pickers::*;


/// Overlay a red border + a right-justified "⚠ 0!" on a selection field whose
/// selection is valid but matched **zero atoms** (molar's "empty" error, surfaced
/// as a non-destructive warning — the text stays editable).
pub(super) fn mark_empty_selection(ui: &egui::Ui, rect: egui::Rect) {
    let red = ui.visuals().error_fg_color;
    let painter = ui.painter();
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.5_f32, red),
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

/// What a click on the pending-selection stub asked for.
enum PendingAction {
    /// Commit it as a normal, editable representation.
    Accept,
    /// Throw the capture away.
    Discard,
}

/// The active (pending) selection stub — the tree row standing in for a lasso/click capture
/// until it is accepted as a representation — plus its accept / discard buttons.
///
/// It is **colour-coded to the selection glow and pulses with it**: `glow` is
/// [`crate::theme::glow_color`] and `pulse` is [`App::glow_pulse`], the same two values that
/// drive the highlighted geometry in the viewport. The stub is that geometry's panel-side face,
/// so it has to be recognizable as the same object — drawn as one more dim italic label among
/// the reps it was easy to miss entirely, which made a captured selection look like something
/// that couldn't be committed.
///
/// The glow colour is keyed to the **viewport backdrop**, not to the panel, so it paints the
/// plate, the border and the marquee glyph but deliberately *not* the label ink: a light cyan
/// on the light theme's mid-grey panel would be text at a fraction of the panel's contrast.
fn pending_stub(
    ui: &mut egui::Ui,
    n_atoms: usize,
    glow: egui::Color32,
    pulse: f32,
    reveal: bool,
) -> Option<PendingAction> {
    // The pulse rides the *alpha* of every glow-coloured part, so the whole stub breathes at
    // once — and at the trough it fades toward the panel rather than toward some other colour.
    // A fully suppressed glow (0.0, while a ray-traced still is held) would take the stub with
    // it; this is a control rather than part of the render, so it then just stops breathing.
    let pulse = if pulse <= 0.0 { 1.0 } else { pulse };
    let tint = |a: f32| {
        egui::Color32::from_rgba_unmultiplied(glow.r(), glow.g(), glow.b(), (a * pulse) as u8)
    };
    let mut action = None;
    let frame = egui::Frame::default()
        .fill(tint(56.0))
        .stroke(egui::Stroke::new(1.0_f32, tint(230.0)))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(5, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                ui.add(
                    egui::Label::new(egui::RichText::new(icon::SELECTION).color(tint(255.0)))
                        .selectable(false),
                );
                ui.add(egui::Label::new(bold_name(ui, "selection")).selectable(false));
                ui.add(
                    egui::Label::new(egui::RichText::new(format!("{n_atoms} atoms")).weak())
                        .selectable(false),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    compact_actions(ui);
                    if icon_button(ui, icon::TRASH, "Discard selection").clicked() {
                        action = Some(PendingAction::Discard);
                    }
                    if ui
                        .selectable_label(
                            false,
                            egui::RichText::new(icon::CHECK).color(crate::theme::ok_color(ui)),
                        )
                        .on_hover_text("Accept as a representation")
                        .clicked()
                    {
                        action = Some(PendingAction::Accept);
                    }
                });
            });
        });
    // A just-captured selection scrolls its own stub into view — the capture happened in the
    // viewport, and this row is the panel's only sign of it (see [`crate::scene::Reveal`]).
    if reveal {
        ui.scroll_to_rect(frame.response.rect, Some(egui::Align::Center));
    }
    action
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
    let red = ui.visuals().error_fg_color;
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

/// The one thing a rep row asked for this frame.
///
/// A single `Option<RepAction>` rather than the eleven independent `Option`s this used to be,
/// because four of these variants — `Reorder`, `Duplicate`, `CloneForInteractions`, `Delete` —
/// mutate the rep list **by index**. Applying two in one frame would leave the second one's
/// index pointing at the wrong rep. With separate slots that only held because at most one
/// button can be clicked per pointer per frame, an unenforced invariant whose failure mode is
/// silent (the neighbouring rep deleted). As one slot it is enforced by the type, and the
/// apply site is a single exhaustive `match` whose arm order no longer matters.
///
/// The rep-list surgery itself lives on [`Molecule`] (`reorder_rep`, `duplicate_rep`,
/// `insert_rep_above`, `delete_rep`), where it is unit-tested — see `scene::rep_surgery_tests`.
///
/// Note what is deliberately *not* here: `view_dirty`. It legitimately accumulates from many
/// widgets in one frame (a visibility toggle, a periodic-image spinner, a params edit), so it
/// stays a separate additive `bool`. That is the convention across the panel layer — an
/// outcome is `{ view_dirty: bool, action: Option<A> }`: the flag is additive, the action is
/// exclusive (see [`RepParamsOutcome`], [`GroupOutcome`](super::panels::GroupOutcome)).
pub(super) enum RepAction {
    /// Drag-reorder: move rep `from` into the gap before position `to`.
    Reorder { from: usize, to: usize },
    /// Duplicate rep `j`, inserting the copy just after it.
    Duplicate(usize),
    /// Rep `j` was switched to Interactions: re-insert `clone` (its old style, still visible)
    /// just above it, so the molecule's look isn't lost — an Interactions rep draws only
    /// contact lines. Boxed: a `Representation` is ~850 bytes and every other variant is a
    /// couple of words, so inline it would set the size of the whole enum.
    CloneForInteractions { at: usize, clone: Box<Representation> },
    Delete(usize),
    /// Commit the active (pending) selection as a Ball-and-Stick rep.
    AcceptPending,
    /// Drop the active (pending) selection.
    DiscardPending,
    /// Zoom the camera to fit rep `j`'s selection.
    ZoomTo(usize),
    /// Enter partner-pick mode for Interactions rep `j` (its [⊕ Choose…] button).
    StartPartnerPick(usize),
    /// Focus the camera on an Interactions rep's partner (its clickable label).
    FocusPartner(MoleculeSource, usize),
    /// A rep row was clicked *while* choosing a partner → assign it as the partner.
    ChoosePartner(usize),
    /// Open the per-type Interactions settings dialog for rep `j`.
    OpenInteractionSettings(usize),
    /// Run espaloma partial-charge prediction on rep `j`'s selection.
    ComputeCharges(usize),
    /// Write rep `j`'s selected atoms to a structure file. Native only — molar writes to the
    /// filesystem, so the browser never draws the button.
    #[cfg(not(target_arch = "wasm32"))]
    SaveSelection(usize),
}

/// What the rep-settings panel wants the app to do next (the panel only sees the rep, so
/// anything needing the molecule is reported back — same pattern as the Interactions
/// partner/settings buttons).
#[derive(Default)]
pub(super) struct RepParamsOutcome {
    /// Some render-only setting changed (periodic images); re-render without rebuilding.
    pub view_dirty: bool,
    /// The [Compute charges] button was pressed.
    pub compute_charges: bool,
}

/// Parameter controls for a representation, shown inline under its row as a tidy
/// two-column table (parameter name on the left, control on the right).
/// Returns `true` if a render-only change was made (periodic-image params) so the
/// caller can flag the viewport dirty; geometry changes set `rep.geom_dirty`
/// directly. `has_box` gates the **Periodic** tab (only meaningful with a box).
pub(super) fn draw_rep_params(
    ui: &mut egui::Ui,
    rep: &mut Representation,
    has_box: bool,
    charge_status: Option<&str>,
) -> RepParamsOutcome {
    let mut out = RepParamsOutcome::default();
    // The Periodic tab only exists when the molecule has a box, and the Color tab only for
    // a color scheme that has options (just Charge today); if the active tab's condition
    // went away, fall back to Style.
    let has_color_tab = rep.color.is_charge();
    if (!has_box && rep.settings_tab == SettingsTab::Periodic)
        || (!has_color_tab && rep.settings_tab == SettingsTab::Color)
    {
        rep.settings_tab = SettingsTab::Style;
    }
    // Tab bar: [Style] [Traj] [Periodic?] [Color?] — the app's standard underline tabs.
    let mut tabs = vec![(SettingsTab::Style, "Style"), (SettingsTab::Traj, "Traj")];
    if has_box {
        tabs.push((SettingsTab::Periodic, "Periodic"));
    }
    if has_color_tab {
        tabs.push((SettingsTab::Color, "Color"));
    }
    tab_bar(ui, &mut rep.settings_tab, &tabs);
    ui.separator();
    match rep.settings_tab {
        SettingsTab::Traj => {
            draw_traj_tab(ui, rep);
            return out;
        }
        SettingsTab::Periodic => {
            out.view_dirty |= draw_periodic_tab(ui, rep);
            return out;
        }
        SettingsTab::Color => {
            out.compute_charges = draw_color_tab(ui, rep, charge_status);
            return out;
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
    out
}

impl App {
    /// Run espaloma partial-charge prediction on rep `j`'s selection of molecule `mi` and
    /// record it as one undo step. The outcome — a charge-range summary or the failure's
    /// advice — is parked in `charge_status` for that rep's **Color** tab to display.
    ///
    /// Charges are per-atom topology data, so unlike the coordinate edits this is not tied
    /// to a trajectory frame; and like them it is a `StructEdit`, so Ctrl+Z reverts it.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn compute_rep_charges(&mut self, mi: usize, j: usize) {
        // A **shared** rep of a molecular group (one of the shown member's first `n_shared`)
        // stands for the same selection on every member, so charge them all — the point of a
        // group is to treat the set as one thing, and only one member is visible at a time,
        // which would make charging "the active one" look arbitrary. A member's *own* rep,
        // and any ordinary molecule, is just itself.
        use molar::prelude::IndexSliceProvider; // for `Sel::get_index_slice`
        let targets: Vec<usize> = {
            let mol = &self.scene.molecules[mi];
            match mol.group.filter(|_| j < mol.n_shared) {
                Some(gid) => self
                    .scene
                    .group_index(gid)
                    .map(|gi| {
                        self.scene.groups[gi]
                            .members
                            .iter()
                            .filter_map(|&id| self.scene.mol_index(id))
                            .collect()
                    })
                    .unwrap_or_else(|| vec![mi]),
                None => vec![mi],
            }
        };
        // The selection is `rep.sel` on the molecule that owns the rep; on the group's other
        // members the shared rep isn't materialized, so re-evaluate its text against each.
        let sel_text = self.scene.molecules[mi].reps.get(j).map(|r| r.sel_text.clone());

        let mut edits = Vec::new();
        let mut failure: Option<String> = None;
        let mut failed = 0usize;
        for t in &targets {
            let sel = if *t == mi {
                // `Sel` isn't `Clone` and lives inside the rep we'd otherwise hold borrowed,
                // so rebuild an equivalent one from its indices.
                self.scene.molecules[mi]
                    .reps
                    .get(j)
                    .and_then(|r| r.sel.as_ref())
                    .map(|s| s.get_index_slice().to_vec())
                    .and_then(|v| molar::prelude::Sel::from_vec(v).ok())
            } else {
                sel_text
                    .as_deref()
                    .and_then(|text| self.scene.molecules[*t].data.evaluate(text).ok())
                    .map(|(_, sel)| sel)
            };
            let Some(sel) = sel else {
                failed += 1;
                failure.get_or_insert_with(|| "select some atoms first".into());
                continue;
            };
            let mol = &mut self.scene.molecules[*t];
            match crate::charges::compute_espaloma(mol, &sel) {
                Ok(edit) => {
                    mol.set_charges(&edit.atoms, &edit.after);
                    let id = mol.id;
                    edits.push((
                        id,
                        crate::history::StructEdit::Charges {
                            atoms: edit.atoms,
                            before: edit.before,
                            after: edit.after,
                        },
                    ));
                }
                Err(e) => {
                    failed += 1;
                    failure.get_or_insert(e);
                }
            }
        }

        if targets.len() > 1 {
            log::info!("charged {} of {} group molecules", edits.len(), targets.len());
        }
        if !edits.is_empty() {
            // One step for the whole gesture, so a single Ctrl+Z takes back every member.
            self.history.record_structs(edits, "compute charges".into());
            self.view_dirty = true;
        }
        // Success needs no message — it shows in the colors. A partial failure across a
        // group says how many, since the rest did succeed.
        self.charge_status = failure.map(|e| {
            let msg = if failed > 1 {
                format!("{failed} of {} molecules could not be charged.\n\n{e}", targets.len())
            } else {
                e
            };
            (j, msg)
        });
    }
}

/// The **[Color]** tab: options of the active color scheme. Shown only for `Charge`
/// (the one scheme with any) — pick which charge to paint, and compute partial charges
/// for selections that don't carry them. Returns whether [Compute charges] was pressed.
pub(super) fn draw_color_tab(
    ui: &mut egui::Ui,
    rep: &mut Representation,
    status: Option<&str>,
) -> bool {
    if !rep.color.is_charge() {
        return false;
    }
    let mut kind = rep.charge_kind;
    // Only the native branch below assigns it (the compute button is native-only).
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut compute = false;

    ui.horizontal(|ui| {
        ui.label("Charge");
        for k in ChargeKind::ALL {
            let tip = match k {
                ChargeKind::Partial => "Per-atom partial charge (from the topology, or computed)",
                ChargeKind::Formal => "Integer formal charge, where the structure records one (e.g. an SDF 'M  CHG')",
            };
            ui.radio_value(&mut kind, k, k.label()).on_hover_text(tip);
            // The compute button sits with **Partial**, since that is the charge it
            // assigns — formal charges are read from the structure, never computed.
            // Icon-only: it's one action on an already-labelled option.
            #[cfg(not(target_arch = "wasm32"))]
            if k == ChargeKind::Partial {
                compute = ui
                    .add_enabled(
                        kind == ChargeKind::Partial,
                        egui::Button::new(icon::LIGHTNING),
                    )
                    .on_hover_text("Compute Espaloma charges on selection")
                    .clicked();
            }
        }
    });
    if kind != rep.charge_kind {
        rep.charge_kind = kind;
        rep.geom_dirty = true; // colors are baked into the geometry
    }

    #[cfg(target_arch = "wasm32")]
    ui.weak("Charge computation is not available in the browser.");

    // Only failures are reported, and only until the next interaction (`App::ui` drops the
    // status on any click/keypress) — a successful assignment shows in the colors, and a
    // stale error hanging under the tab reads as if it were still true.
    if let Some(msg) = status {
        ui.add_space(4.0);
        // The advice is multi-line (see `charges::explain`), so let it wrap.
        ui.add(
            egui::Label::new(
                egui::RichText::new(msg).color(ui.visuals().error_fg_color),
            )
            .wrap(),
        );
    }
    compute
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
    //
    // `Sides::shrink_left` **measures** the two trailing buttons and gives the slider side
    // whatever is left. The alternative — subtracting a hardcoded reserve from
    // `available_width()` — has to predict the rendered width of buttons that have not been
    // added yet, from theme data (button padding, item spacing, Phosphor glyph metrics);
    // it left 2 px of slack, so any of those changing pushed the buttons out of the row.
    // The right side lays out right-to-left, so "Last" is added *before* "Step forward" to
    // keep the on-screen order `▶| ⏭`.
    //
    // `Sides::show` builds both closures before running either, so the right one cannot also
    // hold `&mut traj`: it reports its clicks back and they are applied below, in the order
    // the single-closure version evaluated them.
    let (_, (to_last, forward)) = egui::containers::Sides::new()
        .shrink_left()
        .spacing(COMPACT_SPACING)
        .show(
            ui,
            |ui| {
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
                // Zoomed: a ±25-frame window around the current frame (finer scrubbing on
                // a long trajectory); otherwise the full range.
                let (lo, hi) = if traj.slider_zoom && n > 50 {
                    (traj.current.saturating_sub(25), (traj.current + 25).min(last))
                } else {
                    (0, last)
                };
                // This Ui's max rect is already bounded by the right side's measured width,
                // so what's left after the two leading buttons is exactly the slider's.
                ui.spacing_mut().slider_width = ui.available_width().max(40.0);
                let mut cur = traj.current;
                let resp = ui.add(egui::Slider::new(&mut cur, lo..=hi).show_value(false));
                if resp.changed() {
                    traj.set_playing(false);
                    traj.set_current(cur);
                }
                if let Some(t) = traj.current_time() {
                    resp.on_hover_text(format!("frame {} — t = {:.3}", traj.current, t));
                }
            },
            |ui| {
                compact_actions(ui);
                let to_last = icon_button(ui, icon::SKIP_FORWARD, "Last frame").clicked();
                let forward = icon_button(ui, icon::CARET_RIGHT, "Step forward").clicked();
                (to_last, forward)
            },
        );
    if forward {
        traj.set_playing(false);
        traj.step(1);
    }
    if to_last {
        traj.set_playing(false);
        traj.set_current(last);
    }

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
    // `Sides::shrink_left` measures the trailing buttons; see the same call in
    // `draw_traj_bar` for why that beats a hardcoded reserve, and why the right side reports
    // its clicks instead of mutating `cur`. The right side lays out right-to-left, so "Last"
    // is added *before* "Next".
    let (_, (to_last, next)) = egui::containers::Sides::new()
        .shrink_left()
        .spacing(COMPACT_SPACING)
        .show(
            ui,
            |ui| {
                compact_actions(ui);
                if icon_button(ui, icon::SKIP_BACK, "First molecule").clicked() {
                    cur = 0;
                }
                if icon_button(ui, icon::CARET_LEFT, "Previous molecule").clicked() {
                    cur = cur.saturating_sub(1);
                }
                // The slider takes whatever the two leading buttons leave of this Ui,
                // which the right side has already bounded.
                ui.spacing_mut().slider_width = ui.available_width().max(40.0);
                let resp = ui.add(egui::Slider::new(&mut cur, 0..=last).show_value(false));
                // Tooltip anchored **under the knob** (not at the cursor), showing "N/M
                // name" for the member the knob points at — updates live while dragging.
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
                            egui::Frame::popup(ui.style()).show(ui, |ui| {
                                ui.label(format!("{}/{} {}", cur + 1, n_members, name))
                            });
                        });
                }
            },
            |ui| {
                compact_actions(ui);
                let to_last = icon_button(ui, icon::SKIP_FORWARD, "Last molecule").clicked();
                let next = icon_button(ui, icon::CARET_RIGHT, "Next molecule").clicked();
                (to_last, next)
            },
        );
    if next {
        cur = (cur + 1).min(last);
    }
    if to_last {
        cur = last;
    }
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

        // Read before the `&mut mol` borrow below, since the params panel needs it inside.
        let charge_status = self.charge_status.clone();
        // Likewise the selection-glow colour + pulse the pending stub is painted with: both are
        // read off `self` as a whole, which the `&mut mol` borrow below rules out.
        let glow = crate::theme::glow_color(ui.ctx(), &self.camera.background);
        let (glow_pulse, _) = self.glow_pulse(ui.ctx());
        // Whether one of this pass's rows was asked to be scrolled into view — see
        // [`crate::scene::Reveal`]. The request is dropped by `draw_left_panel` after the pass.
        let reveal = self.scene.reveal.filter(|r| match *r {
            Reveal::Pending(id) => id == mol_id,
            Reveal::Rep(id, j) => id == mol_id && (start..end).contains(&j),
        });
        let mol = &mut self.scene.molecules[mi];

        // The single thing this pass asked for — see [`RepAction`]. Set from inside the row
        // closures (which hold `&mut mol`), applied once they have all ended.
        let mut action: Option<RepAction> = None;

        for j in start..end {
            let sel_id = egui::Id::new(("rep_sel", mol_id, j));
            let rep = &mut mol.reps[j];
            // Whether the selection is valid but empty (0 atoms) — flags the field.
            let sel_empty = rep.sel_empty;
            // Read before the row is laid out: the two `Sides` closures can't both hold
            // `&mut rep`, so the action side works from a copy and reports its clicks back.
            let rep_visible = rep.visible;

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
                            // Selection field on the left filling the rest, actions on the
                            // right. `Sides::shrink_left` measures the action group and
                            // bounds the field's Ui with what's left — replacing the
                            // pre-`Sides` workaround (a `right_to_left` wrapping a
                            // `left_to_right` that read `available_width()`), one nesting
                            // level shallower. The right closure is right-to-left just as
                            // that wrapper was, so the action order is unchanged.
                            //
                            // Both closures are built before either runs, so only one may
                            // hold `&mut rep`: the actions report their clicks back, and the
                            // eye toggle is applied after.
                            let (_, toggle_eye) = egui::containers::Sides::new()
                                    .shrink_left()
                                    .spacing(COMPACT_SPACING)
                                    .show(
                                        ui,
                                        |ui| {
                                            // The field's max rect is now bounded, so it can
                                            // simply ask for everything.
                                            let resp = sel_text_edit(
                                                ui,
                                                &mut rep.sel_text,
                                                sel_id,
                                                f32::INFINITY,
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
                                        |ui| {
                                            compact_actions(ui);
                                            if icon_button(ui, icon::TRASH, "Delete").clicked() {
                                                action = Some(RepAction::Delete(j));
                                            }
                                            // Save just the selected atoms to a structure
                                            // file (sits left of delete). Native only.
                                            #[cfg(not(target_arch = "wasm32"))]
                                            if icon_button(
                                                ui,
                                                icon::FLOPPY_DISK,
                                                "Save selection to file",
                                            )
                                            .clicked()
                                            {
                                                action = Some(RepAction::SaveSelection(j));
                                            }
                                            if icon_button(ui, icon::COPY, "Duplicate").clicked() {
                                                action = Some(RepAction::Duplicate(j));
                                            }
                                            // (Update-every-frame moved to Settings ▸ Traj.)
                                            // Eye: open when shown, crossed when hidden.
                                            let eye = if rep_visible {
                                                icon::EYE
                                            } else {
                                                icon::EYE_SLASH
                                            };
                                            let toggle = ui
                                                .selectable_label(rep_visible, eye)
                                                .on_hover_text(match rep_visible {
                                                    true => "Hide",
                                                    false => "Show",
                                                })
                                                .clicked();
                                            // Zoom the camera to fit this selection.
                                            if icon_button(
                                                ui,
                                                icon::MAGNIFYING_GLASS_PLUS,
                                                "Zoom to selection",
                                            )
                                            .clicked()
                                            {
                                                action = Some(RepAction::ZoomTo(j));
                                            }
                                            toggle
                                        },
                                    );
                            if toggle_eye {
                                rep.visible = !rep_visible;
                                view_dirty = true;
                            }
                        }
                    });

                    // Selection errors appear immediately below the selection field,
                    // aligned under it (indented past the drag handle).
                    if let Some(err) = &rep.sel_error {
                        ui.horizontal(|ui| {
                            ui.add_space(row2_indent);
                            let red = ui.visuals().error_fg_color;
                            ui.colored_label(red, err);
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
                            action = Some(RepAction::CloneForInteractions {
                                at: j,
                                clone: Box::new(clone),
                            });
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
                                        action = rep
                                            .partner
                                            .clone()
                                            .map(|(src, pr)| RepAction::FocusPartner(src, pr));
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
                                    action = Some(RepAction::StartPartnerPick(j));
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
                                        action = Some(RepAction::OpenInteractionSettings(j));
                                    }
                                });
                            }
                            if w_changed {
                                rep.geom_dirty = true;
                            }
                            false
                        } else {
                            let status = charge_status
                                .as_ref()
                                .filter(|(sj, _)| *sj == j)
                                .map(|(_, m)| m.as_str());
                            let out = draw_rep_params(ui, rep, has_box, status);
                            if out.compute_charges {
                                action = Some(RepAction::ComputeCharges(j));
                            }
                            out.view_dirty
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
                    action = Some(RepAction::ChoosePartner(j));
                }
            }

            // The rep an accepted selection just became: bring its row on screen. It is
            // appended at the end of a fold that may well be off the bottom of the list.
            if reveal == Some(Reveal::Rep(mol_id, j)) {
                ui.scroll_to_rect(block.rect, Some(egui::Align::Center));
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
                        egui::Stroke::new(2.0_f32, ui.visuals().selection.bg_fill),
                    );
                    if let Some(src) = block.dnd_release_payload::<usize>() {
                        // Confine reorder to this drawn region (shared vs own): ignore a
                        // drag that originated in the other list (their payloads share the
                        // `usize` type), so the `n_shared` boundary can't be crossed.
                        if (start..end).contains(&*src) {
                            action = Some(RepAction::Reorder {
                                from: *src,
                                to: if before { j } else { j + 1 },
                            });
                        }
                    }
                }
            }

            ui.add_space(6.0);
        }

        // The active (pending) selection — e.g. just captured by a lasso — appears below the
        // reps as the glow-coloured [`pending_stub`]: the atom count plus accept (commit as a
        // Ball-and-Stick rep) / discard. No style, color, or editable selection (those come
        // once it's accepted).
        // The pending selection belongs to the molecule, not to the shared document, so it is
        // skipped in the shared pass — including a group member's, where it belongs with the
        // *member's own* rows: the atoms it captured are that member's, and its siblings know
        // nothing about them. Reaching those rows means unfolding the group, its "Molecules"
        // list and the member's own fold, which is [`Scene::reveal_pending`]'s job; the caller
        // must also draw this pass when the member has no own reps yet (a fresh SDF member
        // has none), or a lasso on it would glow with no way to accept it.
        let pending_atoms = mol.pending.as_ref().map(|p| p.atoms.len()).filter(|_| !is_shared);
        if let Some(n_atoms) = pending_atoms {
            let scroll = reveal == Some(Reveal::Pending(mol_id));
            match pending_stub(ui, n_atoms, glow, glow_pulse, scroll) {
                Some(PendingAction::Accept) => action = Some(RepAction::AcceptPending),
                Some(PendingAction::Discard) => action = Some(RepAction::DiscardPending),
                None => {}
            }
            ui.add_space(6.0);
        }

        // --- Apply the one action this pass asked for. -----------------------------------
        //
        // One exhaustive `match`, so the arm order carries no meaning (it used to: four
        // index-mutating blocks ran in sequence and a second one would have indexed into the
        // list the first had already changed). Each arm re-borrows only what it needs, which
        // is what lets the `&mut Molecule` arms and the `&mut self` arms share one match.
        if let Some(action) = action {
            match action {
                RepAction::Reorder { from, to } => {
                    view_dirty |= self.scene.molecules[mi].reorder_rep(from, to);
                }
                RepAction::Duplicate(j) => {
                    self.scene.molecules[mi].duplicate_rep(j, is_shared);
                    view_dirty = true;
                }
                RepAction::CloneForInteractions { at, clone } => {
                    self.scene.molecules[mi].insert_rep_above(at, *clone, is_shared);
                    view_dirty = true;
                }
                RepAction::Delete(j) => {
                    self.scene.molecules[mi].delete_rep(j, is_shared);
                    view_dirty = true;
                }
                RepAction::AcceptPending => {
                    let mol = &mut self.scene.molecules[mi];
                    let n_before = mol.reps.len();
                    mol.accept_pending_selection();
                    // The committed rep is appended last — at the end of a fold that may be
                    // off-screen, and for a group member behind three of them. Unfold and
                    // scroll to it, so the accept visibly *lands* somewhere.
                    if mol.reps.len() > n_before {
                        self.scene.reveal_rep(mi, self.scene.molecules[mi].reps.len() - 1);
                    }
                    view_dirty = true;
                }
                RepAction::DiscardPending => {
                    self.scene.molecules[mi].discard_pending_selection();
                    view_dirty = true;
                }
                RepAction::ZoomTo(j) => {
                    let mol = &self.scene.molecules[mi];
                    if let Some(sel) = mol.reps.get(j).and_then(|r| r.sel.as_ref()) {
                        let (min, max) = mol.sel_bbox(sel);
                        self.camera.focus_bbox(min, max);
                        view_dirty = true;
                    }
                }
                RepAction::StartPartnerPick(j) => self.partner_pick = Some((mol_id, j)),
                RepAction::OpenInteractionSettings(j) => {
                    self.interactions_dialog = Some(InteractionsDialog {
                        mol: mol_id,
                        rep: j,
                        tab: crate::interactions::InteractionKind::HBond,
                    });
                }
                // A rep row clicked while choosing a partner → assign it.
                RepAction::ChoosePartner(j) => self.assign_partner(mi, j),
                // Focus the camera on a clicked partner rep — it reads *another* molecule,
                // which is why this could never run inside the row closures.
                RepAction::FocusPartner(src, pr) => {
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
                RepAction::ComputeCharges(j) => {
                    #[cfg(not(target_arch = "wasm32"))]
                    self.compute_rep_charges(mi, j);
                    #[cfg(target_arch = "wasm32")]
                    let _ = j; // espaloma is native-only (the button isn't drawn on wasm)
                }
                #[cfg(not(target_arch = "wasm32"))]
                RepAction::SaveSelection(j) => self.save_rep_selection(mi, j),
            }
        }

        self.editing_rep = new_editing;
        view_dirty
    }

    /// The Interactions **Settings** dialog (a movable `egui::Window`): a tab per
    /// interaction type, each exposing that type's full parameter set (enable + all
    /// cutoffs/angles), plus a shared line-width + Defaults footer, for the rep in
    /// [`App::interactions_dialog`]. Any edit marks the rep `geom_dirty` so its contacts
    /// rebuild. Closed via the window ✕ or when the target rep vanishes.
    pub(super) fn draw_interactions_dialog(&mut self, ctx: &egui::Context) {
        use crate::interactions::InteractionKind as K;
        // Taken out for the duration, so the tab can be edited by `&mut` while the closure
        // also borrows `self.scene` — and put back below unless the dialog was closed.
        let Some(mut dialog) = self.interactions_dialog.take() else {
            return;
        };
        let (mol_id, ri) = (dialog.mol, dialog.rep);
        let Some(mi) = self.scene.mol_index(mol_id) else {
            return; // the rep's molecule is gone → the dialog closes with it
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
                    &mut dialog.tab,
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
                let tab = dialog.tab;
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
        if open {
            self.interactions_dialog = Some(dialog); // still open — keep the active tab
        }
    }
}


/// Layout regression tests for the two cycle bars. Both take only a `&mut Ui` plus plain
/// data, so they run headlessly with no wgpu device — see
/// `theme::hover_does_not_resize_widgets` for the same `ctx.run_ui` pattern.
///
/// What they pin down is the thing the bars cannot check for themselves: that the row whose
/// slider **stretches** keeps its trailing buttons inside the row. That row sizes the slider
/// by subtracting a reserve from `available_width()`, and the reserve has to cover buttons
/// that have not been added yet — whose rendered width is theme data (button padding, item
/// spacing, Phosphor glyph metrics). As measured when these tests were written, the reserve
/// leaves just **2 px** of slack, so any of those changing pushes the buttons out of the row.
#[cfg(test)]
mod bar_fit_tests {
    use super::*;
    use crate::settings::{AppearanceSettings, ThemeMode};

    /// Widths to probe, in points. The floor is set by the trajectory bar's **first** row,
    /// whose widget set is fixed and cannot shrink: measured at 310.25 (≤50 frames) and
    /// 329.44 (>50 frames, where the slider-zoom toggle appears). Narrower than that and row
    /// 1 overhangs no matter what the slider row does — a separate, pre-existing limit of
    /// the left panel's minimum width, not the reserve arithmetic under test. Starting at
    /// 336 also makes this a tripwire: add another widget to row 1 and the first probe fails,
    /// so the floor has to be raised deliberately.
    const WIDTHS: [f32; 5] = [336.0, 360.0, 420.0, 512.0, 700.0];

    /// Draw `build` in a fresh context at `width` under `theme`, and return
    /// `(available width, width actually used)`. Two frames, because egui reads a widget's
    /// state from the previous frame's response.
    fn measure(width: f32, theme: ThemeMode, mut build: impl FnMut(&mut egui::Ui)) -> (f32, f32) {
        use std::cell::Cell;
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &AppearanceSettings { theme, ..Default::default() });
        let out = Cell::new((0.0, 0.0));
        let mut frame = || {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 300.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                let avail = ui.available_width();
                let used = ui.scope(|ui| build(ui)).response.rect.width();
                out.set((avail, used));
            });
        };
        frame();
        frame();
        out.get()
    }

    fn traj(n: usize) -> Trajectory {
        let mut t = Trajectory::default();
        t.frames = (0..n).map(|_| molar::prelude::State::default()).collect();
        t
    }

    /// The trajectory bar must fit the panel in both themes at every width — and at both
    /// trajectory lengths, since >50 frames enables the slider-zoom toggle (one more widget
    /// competing for row 1's space, which raises the whole bar's floor).
    #[test]
    fn traj_bar_fits_its_row() {
        for theme in [ThemeMode::Dark, ThemeMode::Light] {
            for n in [10usize, 200] {
                for w in WIDTHS {
                    let mut t = traj(n);
                    let (avail, used) = measure(w, theme, |ui| {
                        draw_traj_bar(ui, &mut t);
                    });
                    assert!(
                        used <= avail + 0.5,
                        "{theme:?}: traj bar ({n} frames) overhangs a {w}-wide panel: \
                         used {used} of {avail}"
                    );
                }
            }
        }
    }

    /// Same for the group cycle bar, which has only the slider row. Its own floor is much
    /// lower (~148), so the shared [`WIDTHS`] are all comfortably above it. Long member names
    /// must not widen it either — the name only ever appears in a tooltip, its own `Area`.
    #[test]
    fn group_bar_fits_its_row() {
        let names: Vec<String> = (0..12)
            .map(|i| format!("a-rather-long-ligand-name-{i}"))
            .collect();
        for theme in [ThemeMode::Dark, ThemeMode::Light] {
            for w in WIDTHS {
                let (avail, used) = measure(w, theme, |ui| {
                    draw_group_bar(ui, &names, 3);
                });
                assert!(
                    used <= avail + 0.5,
                    "{theme:?}: group bar overhangs a {w}-wide panel: used {used} of {avail}"
                );
            }
        }
    }
}


/// Widget-layer tests for the rep-settings panel and the three pickers. Same headless
/// `ctx.run_ui` pattern as [`bar_fit_tests`] — these take only a `&mut Ui` plus a
/// `Representation`, which constructs fine because `RepGpu` derives `Default`.
///
/// They pin invariants nothing else checks: which tabs exist for which rep, and that a
/// picker button's width does not depend on the option currently selected.
#[cfg(test)]
mod rep_panel_tests {
    use super::*;
    use crate::material::Material;
    use crate::settings::{AppearanceSettings, ThemeMode};

    /// Run `build` in a fresh themed context, twice (egui reads a widget's state from the
    /// previous frame's response).
    fn run(theme: ThemeMode, mut build: impl FnMut(&mut egui::Ui)) {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &AppearanceSettings { theme, ..Default::default() });
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 600.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| build(ui));
        }
    }

    /// Draw the params panel once for `rep` with `has_box`, and report the tab it ended on.
    fn params_tab(rep: &mut Representation, has_box: bool) -> SettingsTab {
        run(ThemeMode::Dark, |ui| {
            draw_rep_params(ui, rep, has_box, None);
        });
        rep.settings_tab
    }

    /// The **Periodic** tab exists only with a box and the **Color** tab only for a colour
    /// scheme that has options — and if the tab you were *on* loses its condition, the panel
    /// must fall back to Style rather than leave a tab selected that is no longer in the bar.
    /// Both ways of losing it are ways the app really does: the molecule's box goes away
    /// (session load, a frame without one), and the colour scheme is switched off Charge.
    #[test]
    fn conditional_tabs_fall_back_to_style_when_their_condition_goes_away() {
        // Periodic: reachable with a box, abandoned when the box goes.
        let mut rep = Representation::new(RepKind::Lines);
        rep.settings_tab = SettingsTab::Periodic;
        assert_eq!(
            params_tab(&mut rep, true),
            SettingsTab::Periodic,
            "with a box, the Periodic tab must stay selected"
        );
        assert_eq!(
            params_tab(&mut rep, false),
            SettingsTab::Style,
            "without a box there is no Periodic tab, so it must fall back to Style"
        );

        // Color: reachable only for the Charge scheme, abandoned when it changes.
        let mut rep = Representation::new(RepKind::Lines);
        rep.color = ColorMethod::Charge;
        rep.settings_tab = SettingsTab::Color;
        assert_eq!(
            params_tab(&mut rep, false),
            SettingsTab::Color,
            "the Charge scheme has options, so its Color tab must stay selected"
        );
        rep.color = ColorMethod::Element;
        assert_eq!(
            params_tab(&mut rep, false),
            SettingsTab::Style,
            "Element has no options, so the Color tab must fall back to Style"
        );

        // Style and Traj are unconditional — neither may ever be taken away.
        for tab in [SettingsTab::Style, SettingsTab::Traj] {
            let mut rep = Representation::new(RepKind::Lines);
            rep.settings_tab = tab;
            assert_eq!(params_tab(&mut rep, false), tab, "{tab:?} must always be available");
        }
    }

    /// Every style must draw in the params panel on every tab it offers, in both themes. The
    /// Style tab dispatches on [`RepParams`], so a style added without its arm would otherwise
    /// surface only at runtime.
    #[test]
    fn every_style_draws_on_every_available_tab() {
        for theme in [ThemeMode::Dark, ThemeMode::Light] {
            for kind in RepKind::ALL {
                for tab in [SettingsTab::Style, SettingsTab::Traj, SettingsTab::Periodic] {
                    let mut rep = Representation::new(kind);
                    rep.settings_tab = tab;
                    run(theme, |ui| {
                        draw_rep_params(ui, &mut rep, true, None);
                    });
                }
                // The Color tab, for the one scheme that has options — with a charge-status
                // message, since a leading `!` marks it an error and takes a different branch.
                let mut rep = Representation::new(kind);
                rep.color = ColorMethod::Charge;
                rep.settings_tab = SettingsTab::Color;
                run(theme, |ui| {
                    draw_rep_params(ui, &mut rep, false, Some("!load an SDF/MOL"));
                });
            }
        }
    }

    /// A picker button must be **the same width whatever is selected**.
    ///
    /// That is the entire point of [`max_label_width`]: the button reserves the *widest*
    /// option's label, so choosing "Ball and stick" after "Lines" cannot grow the button, shove
    /// its neighbours along and reflow the panel. It is also why an egui `sizing_pass` could
    /// not replace it — a sizing pass measures the current content, not the max over options.
    ///
    /// This exercises the same two lines each of the three pickers opens with
    /// (`max_label_width` over the enum's `ALL`, then `picker_button`), because the pickers
    /// return their selection rather than their button's rect. Checked in both themes, since
    /// the reservation is measured in the themed font.
    #[test]
    fn picker_button_width_does_not_depend_on_the_selection() {
        use std::cell::RefCell;
        // (group, label) pairs covering all three pickers' option lists.
        let groups: [(&str, Vec<&str>); 3] = [
            ("style", RepKind::ALL.iter().map(|k| k.label()).collect()),
            ("color", ColorMethod::ALL.iter().map(|m| m.label()).collect()),
            ("material", Material::ALL.iter().map(|m| m.label()).collect()),
        ];
        for theme in [ThemeMode::Dark, ThemeMode::Light] {
            let seen: RefCell<Vec<(&str, &str, f32)>> = RefCell::new(Vec::new());
            run(theme, |ui| {
                seen.borrow_mut().clear();
                for (group, labels) in &groups {
                    let lw = max_label_width(ui, labels.iter().copied());
                    for label in labels {
                        let w = picker_button(ui, label, lw, |_, _| {}).rect.width();
                        seen.borrow_mut().push((group, label, w));
                    }
                }
            });
            for (group, _) in &groups {
                let ws: Vec<(&str, f32)> = seen
                    .borrow()
                    .iter()
                    .filter(|(g, _, _)| g == group)
                    .map(|(_, l, w)| (*l, *w))
                    .collect();
                let (_, first) = ws[0];
                for (label, w) in &ws {
                    assert!(
                        (w - first).abs() < 0.01,
                        "{theme:?}: the {group} picker is {w} wide for {label:?} but {first} \
                         for {:?} — its width must not follow the selection",
                        ws[0].0
                    );
                }
            }
        }
    }
}
