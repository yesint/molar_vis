//! Program-settings dialog: per-tab pages, apply, axes widget.
use super::*;
use super::widgets::*;


// --- Program-settings dialog: one function per tab (see `App::draw_settings_dialog`). ---

/// Appearance tab: theme mode, UI font scale, accent color.
pub(super) fn settings_page_appearance(ui: &mut egui::Ui, s: &mut Settings) {
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
pub(super) fn settings_page_rendering(ui: &mut egui::Ui, s: &mut Settings) {
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
pub(super) fn settings_page_view(ui: &mut egui::Ui, s: &mut Settings) -> bool {
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
pub(super) fn settings_page_reps(ui: &mut egui::Ui, s: &mut Settings) {
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
pub(super) fn settings_page_behavior(ui: &mut egui::Ui, s: &mut Settings) {
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
    ui.checkbox(&mut b.hover_detail_lens, "Hover detail lens over cartoon/surface")
        .on_hover_text(
            "In pick/hover mode, reveal a faded ball-and-stick of the atoms under the cursor \
             over a Cartoon or Surface rep (hints where the atoms are). Off by default.",
        );

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
pub(super) fn draw_axes_widget(
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
impl App {

    /// Effective new-rep defaults: the settings' `reps`, with the kind overridden by
    /// the `MOLAR_VIS_DEBUG_REP` env hook (headless verification). Recomputed when
    /// settings change.
    pub(super) fn effective_rep_defaults(settings: &Settings) -> RepDefaults {
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
    pub(super) fn draw_settings_dialog(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
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
    pub(super) fn apply_settings(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
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
}
