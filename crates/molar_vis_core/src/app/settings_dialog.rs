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
            ui.label("Anti-aliasing").on_hover_text("Supersampling smooths the image at a cost proportional to this factor squared. Applies immediately.");
            egui::ComboBox::from_id_salt("set_ssaa")
                .selected_text(ssaa_label(r.ssaa))
                .show_ui(ui, |ui| {
                    for n in [1u32, 2, 3, 4] {
                        ui.selectable_value(&mut r.ssaa, n, ssaa_label(n));
                    }
                });
            ui.end_row();

            ui.label("Shadow-map resolution").on_hover_text("Higher resolutions improve cast-shadow detail. Used when cast shadows are enabled; applies immediately.");
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

}

/// Shared view-style controls for current-session edits and saved defaults.
pub(super) fn settings_page_view(
    ui: &mut egui::Ui,
    v: &mut crate::settings::ViewDefaults,
    page: &mut ViewPage,
    ray_supported: bool,
    scene_tex: Option<egui::TextureId>,
) {
    tab_bar(
        ui,
        page,
        &[
            (ViewPage::Camera, "Camera"),
            (ViewPage::Lighting, "Lighting"),
            (ViewPage::Scene, "Scene"),
        ],
    );
    ui.add_space(8.0);
    match page {
        ViewPage::Camera => {
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

                    ui.label("Frame fill").on_hover_text(
                    "Fraction of the viewport occupied after framing: 0.9 means 90%. Applies on the next zoom-to or Unobstructed View action; does not change the current zoom.",
                );
                    slider_with_edit(ui, &mut v.fill, 0.5..=1.0, true);
                    ui.end_row();
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
        }
        ViewPage::Lighting => {
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

            ui.add_enabled_ui(sh_on, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Softness");
                    slider_with_edit(
                        ui,
                        &mut v.shadow.softness,
                        crate::camera::Shadow::MIN_SOFTNESS..=1.0,
                        true,
                    );
                });
            });
            ui.separator();
            ui.label("Ray tracing").on_hover_text(if ray_supported { "Press R in the viewport to ray-trace the view." } else { "Ray tracing is unavailable on this device." });
            ui.add_enabled_ui(ray_supported, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Global illumination");
                    slider_with_edit(ui, &mut v.gi, 0.0..=1.0, true);
                });
            });

        }
        ViewPage::Scene => {
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
            ui.label("Orientation axes");
            draw_axes_widget(ui, &mut v.axes_on, &mut v.axes_corner, scene_tex);
        }
    }
}

/// Representation preferences, applied to loaded and newly created representations.
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
            let defaults = r.clone();
            super::pickers::material_selector(ui, &mut r.material, &defaults);
            ui.end_row();

            ui.label("Selection");
            ui.add(egui::TextEdit::singleline(&mut r.selection).desired_width(220.0));
            ui.end_row();
        });
    ui.add_space(4.0);
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
    ui.label("Picking");
    egui::Grid::new("set_behavior_pick")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Pick mode");
            egui::ComboBox::from_id_salt("set_pick_mode")
                .selected_text(b.pick_mode.label())
                .show_ui(ui, |ui| {
                    for m in [PickMode::Off, PickMode::Click, PickMode::Lasso] {
                        ui.selectable_value(&mut b.pick_mode, m, m.label());
                    }
                });
            ui.end_row();
            ui.label("Selection scope");
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
    ui.label("Interaction aids");
    ui.checkbox(&mut b.hover_detail_lens, "Hover detail lens over cartoon/surface")
        .on_hover_text(
            "In pick/hover mode, reveal a faded ball-and-stick of the atoms under the cursor \
             over a Cartoon or Surface rep (hints where the atoms are). Off by default.",
        );
    ui.checkbox(&mut b.rep_zoom_unobstructed, "Rep magnifier does an unobstructed view")
        .on_hover_text(
            "The magnifier button on each representation rotates and scales the camera to show \
             that rep with the least obstruction by the other reps (default). Off → it just \
             zooms to fit the selection at the current orientation.",
        );

    ui.separator();
    ui.label("Trajectory playback");
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
    ui.label("Bond detection");
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
    ui.label("Periodic rendering (current scene)");
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
        egui::Stroke::new(1.0_f32, ui.visuals().widgets.inactive.fg_stroke.color),
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

    fn current_settings(&self) -> Settings {
        let mut current = self.settings.clone();
        current.view = crate::settings::ViewDefaults::from_camera(&self.camera);
        current.behavior.pick_mode = self.pick_mode;
        current.behavior.selection_mode = self.selection_mode;
        current
    }

    pub(super) fn open_settings(&mut self, tab: SettingsPage) {
        if let Some(dialog) = &mut self.settings_dialog {
            dialog.tab = tab;
            dialog.popup = false;
            return;
        }
        let current = self.current_settings();
        self.settings_dialog = Some(SettingsDialog::capture(current, &self.scene, tab));
    }

    /// Apply runtime settings without writing startup defaults.
    fn apply_live_settings(
        &mut self,
        next: Settings,
        ctx: &egui::Context,
        frame: &mut eframe::Frame,
        redetect: bool,
        force: bool,
    ) -> Result<(), String> {
        let previous = self.settings.clone();
        let detected = if redetect
            && (force || previous.behavior.bond_params() != next.behavior.bond_params())
        {
            Some(
                self.scene
                    .molecules
                    .iter()
                    .map(|m| m.detect_bonds(&next.behavior.bond_params()))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        } else {
            None
        };
        if let Some(detected) = detected {
            for (mol, bonds) in self.scene.molecules.iter_mut().zip(detected) {
                mol.set_detected_bonds(bonds);
            }
        }
        let reps_changed = force || previous.reps != next.reps;
        if previous.appearance != next.appearance {
            crate::theme::apply(ctx, &next.appearance);
        }
        if previous.rendering != next.rendering {
            if let Some(rs) = frame.wgpu_render_state() {
                self.renderer.reconfigure(rs, &next.rendering);
            }
        }
        next.view.seed_camera(&mut self.camera);
        self.pick_mode = next.behavior.pick_mode;
        self.selection_mode = next.behavior.selection_mode;
        for mol in &mut self.scene.molecules {
            if force || previous.behavior.traj_fps != next.behavior.traj_fps {
                mol.trajectory.speed_fps = next.behavior.traj_fps;
            }
            if force || previous.behavior.loop_mode != next.behavior.loop_mode {
                mol.trajectory.loop_mode = next.behavior.loop_mode;
            }
            for rep in &mut mol.reps {
                if force || previous.reps.kind != next.reps.kind {
                    rep.kind = next.reps.kind;
                    rep.params = next.reps.style_params(rep.kind);
                }
                if force || previous.reps.color != next.reps.color {
                    rep.color = next.reps.color;
                }
                if force || previous.reps.material != next.reps.material {
                    rep.material = next.reps.material_for(next.reps.material);
                }
                if force || previous.reps.style_params(rep.kind) != next.reps.style_params(rep.kind) {
                    rep.params = next.reps.style_params(rep.kind);
                }
                if previous.reps.materials != next.reps.materials {
                    rep.material = next.reps.material_for(rep.material.preset());
                }
                if force || previous.reps.ss_algo != next.reps.ss_algo { rep.ss_algo = next.reps.ss_algo; }
                if force || previous.reps.selection != next.reps.selection {
                    rep.sel_text = next.reps.selection.clone();
                    rep.expr = None;
                    rep.sel_dirty = true;
                }
                if reps_changed
                    || previous.behavior.dashed_pbc_bonds != next.behavior.dashed_pbc_bonds
                {
                    rep.geom_dirty = true;
                }
            }
        }
        self.settings = next;
        self.rep_defaults = Self::effective_rep_defaults(&self.settings);
        self.last_render_camera = None;
        self.view_dirty = true;
        ctx.request_repaint();
        Ok(())
    }

    pub(super) fn draw_settings_dialog(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let Some(mut dialog) = self.settings_dialog.take() else {
            return;
        };
        let mut current = self.current_settings();
        let before = current.clone();
        let popup_was_open = dialog.child_popup_open || egui::Popup::is_any_open(ctx);
        let mut close = false;
        let mut factory = false;
        let mut revert = false;
        let mut save = false;
        let screen = ctx.content_rect();
        let popup = dialog.popup;
        let mut window_open = true;
        let mut window = egui::Window::new(if popup {
            "view_settings".to_string()
        } else {
            format!("{}  Settings", icon::GEAR_SIX)
        })
        .id(egui::Id::new(if popup {
            "view_settings"
        } else {
            "settings_window"
        }))
        .collapsible(false)
        .resizable(false);
        if popup {
            window = window
                .title_bar(false)
                .movable(false)
                .pivot(egui::Align2::RIGHT_TOP)
                .fixed_pos(dialog.anchor.right_bottom() + egui::vec2(0.0, 4.0));
        } else {
            window = window
                .open(&mut window_open)
                .movable(true)
                .pivot(egui::Align2::CENTER_TOP)
                .default_pos(egui::pos2(screen.center().x, screen.top() + 48.0));
        }
        if !popup && dialog.preview.is_none() && matches!(dialog.tab, SettingsPage::Styles | SettingsPage::Materials) {
            if let Some(rs) = frame.wgpu_render_state() {
                dialog.preview = Some(SettingsPreview::new(rs.clone()));
            }
        }
        let inner = window.show(ctx, |ui| {
            if popup {
                ui.set_width(300.0);
            } else {
                ui.set_width(640.0);
            }
            if !popup {
                tab_bar(
                    ui,
                    &mut dialog.tab,
                    &[
                        (SettingsPage::Appearance, "Appearance"),
                        (SettingsPage::Rendering, "Render"),
                        (SettingsPage::View, "View"),
                        (SettingsPage::Representations, "Representations"),
                        (SettingsPage::Styles, "Styles"),
                        (SettingsPage::Materials, "Materials"),
                        (SettingsPage::Behavior, "Behavior"),
                    ],
                );
                ui.separator();
            }
            egui::ScrollArea::vertical()
                .max_height(if popup { 380.0 } else { 440.0 })
                .auto_shrink([false, true])
                .show(ui, |ui| match dialog.tab {
                    SettingsPage::Appearance => settings_page_appearance(ui, &mut current),
                    SettingsPage::Rendering => settings_page_rendering(ui, &mut current),
                    SettingsPage::View => settings_page_view(
                        ui,
                        &mut current.view,
                        &mut dialog.view_page,
                        self.renderer.raytrace_supported(),
                        Some(self.renderer.texture_id()),
                    ),
                    SettingsPage::Representations => settings_page_reps(ui, &mut current),
                    SettingsPage::Styles => settings_page_styles(ui, &mut current, &mut dialog.style_page, dialog.preview.as_mut()),
                    SettingsPage::Materials => settings_page_materials(ui, &mut current, &mut dialog.material_page, dialog.preview.as_mut()),
                    SettingsPage::Behavior => settings_page_behavior(ui, &mut current),
                });
            ui.separator();
            if let Some(error) = &dialog.save_error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.horizontal(|ui| {
                if ui
                    .button("Default")
                    .on_hover_text("Apply factory defaults to all settings")
                    .clicked()
                {
                    current = Settings::default();
                    factory = true;
                }
                if ui
                    .button("Revert")
                    .on_hover_text("Restore all settings from when this dialog opened")
                    .clicked()
                {
                    current = dialog.original.clone();
                    revert = true;
                }
                if ui
                    .button("Save")
                    .on_hover_text("Save all current settings as defaults for new sessions")
                    .clicked()
                {
                    save = true;
                }
            });

        });
        close |= !window_open;
        if popup {
            if let Some(inner) = inner {
                let hit_rect = dialog.last_rect.unwrap_or(inner.response.rect);
                let child_open = egui::Popup::is_any_open(ctx);
                if !child_open && !popup_was_open && ctx.input(|i| i.pointer.any_click()) {
                    if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                        close |= !hit_rect.contains(pos) && !dialog.anchor.contains(pos);
                    }
                }
                dialog.last_rect = Some(inner.response.rect);
                dialog.child_popup_open = child_open;
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape))
            && !popup_was_open
            && !egui::Popup::is_any_open(ctx)
        {
            close = true;
        }
        if factory || current != before {
            dialog.reps_changed |= factory || current.reps != before.reps;
            dialog.playback_changed |= factory
                || current.behavior.traj_fps != before.behavior.traj_fps
                || current.behavior.loop_mode != before.behavior.loop_mode;
            dialog.bonds_changed |=
                factory || current.behavior.bond_params() != before.behavior.bond_params();
            match self.apply_live_settings(current.clone(), ctx, frame, !revert, factory) {
                Ok(()) => dialog.save_error = None,
                Err(error) => {
                    dialog.save_error = Some(error.clone());
                    self.status = error;
                }
            }
        }
        if revert {
            dialog.restore_scene(&mut self.scene);
            self.last_render_camera = None;
            ctx.request_repaint();
        }
        if save {
            match self.current_settings().save() {
                Ok(()) => {
                    dialog.save_error = None;
                    self.status = "Defaults saved for new sessions".to_string();
                }
                Err(e) => {
                    let error = format!("Could not save defaults: {e}");
                    dialog.save_error = Some(error.clone());
                    self.status = error;
                }
            }

        }
        if !popup {
            self.last_settings_page = dialog.tab;
        }
        if !close {
            self.settings_dialog = Some(dialog);
        }
    }
}

impl SettingsDialog {
    fn capture(current: Settings, scene: &Scene, tab: SettingsPage) -> Self {
        let style_page = current.reps.kind;
        let material_page = current.reps.material.preset();
        Self {
            original: current,
            original_reps: scene
                .molecules
                .iter()
                .map(|m| {
                    (
                        m.id,
                        m.reps
                            .iter()
                            .map(crate::history::RepState::capture)
                            .collect(),
                        m.trajectory.speed_fps,
                        m.trajectory.loop_mode,
                    )
                })
                .collect(),
            original_bonds: scene
                .molecules
                .iter()
                .map(|m| (m.id, m.bonds.clone(), m.bond_guess_source.clone()))
                .collect(),
            bonds_changed: false,
            reps_changed: false,
            playback_changed: false,
            tab,
            view_page: ViewPage::default(),
            style_page,
            material_page,
            preview: None,
            popup: false,
            anchor: egui::Rect::NOTHING,
            last_rect: None,
            child_popup_open: false,
            save_error: None,
        }
    }

    fn restore_scene(&self, scene: &mut Scene) {
        if self.bonds_changed {
            for (id, bonds, source) in &self.original_bonds {
                if let Some(mol) = scene.molecules.iter_mut().find(|m| m.id == *id) {
                    mol.set_detected_bonds(bonds.clone());
                    mol.bond_guess_source = source.clone();
                }
            }
        }
        for (id, reps, fps, looping) in &self.original_reps {
            if let Some(mol) = scene.molecules.iter_mut().find(|m| m.id == *id) {
                if self.reps_changed {
                    for (rep, original) in mol.reps.iter_mut().zip(reps) {
                        rep.kind = original.kind;
                        rep.params = original.params;
                        rep.color = original.color;
                        rep.material = original.material;
                        rep.ss_algo = original.ss_algo;
                        rep.sel_text = original.sel_text.clone();
                        rep.expr = None;
                        rep.sel_dirty = true;
                        rep.geom_dirty = true;
                    }
                }
                if self.playback_changed {
                    mol.trajectory.speed_fps = *fps;
                    mol.trajectory.loop_mode = *looping;
                }
            }
        }
    }
}

#[cfg(test)]
mod live_settings_tests {
    use super::*;

    fn scene() -> Scene {
        let mut scene = Scene::default();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/2lao.pdb");
        scene.add(
            crate::data::load(&path).unwrap(),
            &crate::settings::RepDefaults::default(),
        );
        scene
    }

    #[test]
    fn revert_restores_distinct_rep_and_playback_values() {
        let mut scene = scene();
        let mol = &mut scene.molecules[0];
        mol.reps[0].kind = RepKind::Cartoon;
        mol.reps[0].color = ColorMethod::Chain;
        mol.reps.push(Representation::new(RepKind::Vdw));
        mol.trajectory.speed_fps = 37.0;
        mol.trajectory.loop_mode = LoopMode::Once;
        let original: Vec<_> = mol
            .reps
            .iter()
            .map(crate::history::RepState::capture)
            .collect();
        let mut dialog =
            SettingsDialog::capture(Settings::default(), &scene, SettingsPage::Appearance);
        dialog.reps_changed = true;
        dialog.playback_changed = true;
        for rep in &mut scene.molecules[0].reps {
            rep.kind = RepKind::Lines;
            rep.color = ColorMethod::Element;
        }
        scene.molecules[0].trajectory.speed_fps = 15.0;
        scene.molecules[0].trajectory.loop_mode = LoopMode::Loop;
        dialog.restore_scene(&mut scene);
        let mol = &scene.molecules[0];
        let restored: Vec<_> = mol
            .reps
            .iter()
            .map(crate::history::RepState::capture)
            .collect();
        assert!(original == restored);
        assert_eq!(mol.trajectory.speed_fps, 37.0);
        assert_eq!(mol.trajectory.loop_mode, LoopMode::Once);
    }

    #[test]
    fn live_bond_detection_preserves_recorded_chemical_orders() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/ligands20.sdf");
        let raw = crate::data::load_records(&path, &crate::data::BondParams::default())
            .unwrap()
            .remove(0);
        let mut scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        let mol = &scene.molecules[0];
        let params = crate::data::BondParams {
            factor: 0.3,
            search_cutoff: 0.1,
            ..Default::default()
        };
        let bonds = mol.detect_bonds(&params).unwrap();
        assert_eq!(bonds.len(), mol.bonds.len());
        for (original, detected) in mol.bonds.iter().zip(bonds) {
            assert_eq!(
                (original.i1, original.i2, original.order),
                (detected.i1, detected.i2, detected.order)
            );
        }
    }

    #[test]
    fn bond_settings_can_tighten_and_loosen_without_losing_source_bonds() {
        let mut scene = scene();
        let mol = &mut scene.molecules[0];
        let original = mol.bonds.clone();
        let source = mol.bond_guess_source.clone();
        let tight = crate::data::BondParams {
            factor: 0.3,
            ..Default::default()
        };
        let bonds = mol.detect_bonds(&tight).unwrap();
        assert!(bonds.len() < original.len());
        for bond in &source {
            assert!(bonds
                .iter()
                .any(|b| (b.i1 == bond.i1 && b.i2 == bond.i2)
                    || (b.i1 == bond.i2 && b.i2 == bond.i1)));
        }
        mol.set_detected_bonds(bonds);
        let relaxed = mol
            .detect_bonds(&crate::data::BondParams::default())
            .unwrap();
        assert_eq!(relaxed.len(), original.len());
        let mut dialog =
            SettingsDialog::capture(Settings::default(), &scene, SettingsPage::Behavior);
        dialog.bonds_changed = true;
        scene.molecules[0].set_detected_bonds(Vec::new());
        dialog.restore_scene(&mut scene);
        assert_eq!(
            scene.molecules[0].bonds.len(),
            scene.molecules[0].detect_bonds(&tight).unwrap().len()
        );
    }
}

/// Geometry defaults for one style; choosing a style here does not switch the scene.
pub(super) fn settings_page_styles(
    ui: &mut egui::Ui,
    s: &mut Settings,
    selected: &mut RepKind,
    preview: Option<&mut SettingsPreview>,
) {
    ui.horizontal(|ui| {
        ui.label("Style").on_hover_text("Edit matching representations. Save keeps these options for new representations and sessions.");
        egui::ComboBox::from_id_salt("settings_style").selected_text(selected.label()).show_ui(ui, |ui| {
            for kind in RepKind::ALL { ui.selectable_value(selected, kind, kind.label()); }
        });
    });
    ui.separator();
    let mut params = s.reps.style_params(*selected);
    let before = params;
    let controls_width = (ui.available_width() - PREVIEW_SIZE - 16.0).max(320.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        ui.vertical(|ui| {
            ui.set_width(controls_width);
            ui.spacing_mut().slider_width = 100.0;
            super::rep_panel::draw_style_options(ui, &mut params);
            if let RepParams::Interactions { settings } = &mut params {
                settings_interaction_options(ui, settings);
            }
            if *selected == RepKind::Cartoon {
                ui.horizontal(|ui| {
                    ui.label("SS algorithm").on_hover_text(
                        "Algorithm used to assign the helix, sheet, and coil geometry.",
                    );
                    egui::ComboBox::from_id_salt("settings_ss")
                        .selected_text(match s.reps.ss_algo {
                            SsAlgorithm::Dssp => "DSSP",
                            SsAlgorithm::DsspGmx => "DSSP (gmx)",
                            SsAlgorithm::Dss => "dss (PyMOL)",
                        })
                        .show_ui(ui, |ui| {
                            for (algorithm, label) in [
                                (SsAlgorithm::Dssp, "DSSP"),
                                (SsAlgorithm::DsspGmx, "DSSP (gmx)"),
                                (SsAlgorithm::Dss, "dss (PyMOL)"),
                            ] {
                                ui.selectable_value(&mut s.reps.ss_algo, algorithm, label);
                            }
                        });
                });
            }
            if ui
                .button("Reset style")
                .on_hover_text("Restore this style's factory options")
                .clicked()
            {
                params = RepParams::for_kind(*selected);
                if *selected == RepKind::Cartoon {
                    s.reps.ss_algo = SsAlgorithm::default();
                }
            }
        });
        if let Some(preview) = preview {
            preview.show(
                ui,
                params,
                s.reps.material_for(s.reps.material),
                s.reps.ss_algo,
            );
        } else {
            preview_rectangle(ui);
        }
    });
    if params != before {
        s.reps.styles.retain(|p| p.kind() != *selected);
        s.reps.styles.push(params);
    }
}

pub(super) fn settings_page_materials(
    ui: &mut egui::Ui,
    s: &mut Settings,
    selected: &mut Material,
    preview: Option<&mut SettingsPreview>,
) {
    ui.horizontal(|ui| {
        ui.label("Material").on_hover_text(
            "Edit matching representations. Save keeps the shader options for new sessions.",
        );
        super::pickers::material_selector(ui, selected, &s.reps);
    });
    ui.separator();
    let mut options = s.reps.material_for(*selected).options();
    let before = options;
    let mut reset = false;
    let controls_width = (ui.available_width() - PREVIEW_SIZE - 16.0).max(320.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        ui.vertical(|ui| {
            ui.set_width(controls_width);
            egui::Grid::new("material_options")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Opacity")
                        .on_hover_text("0 is invisible; 1 is fully opaque.");
                    slider_with_edit(ui, &mut options.opacity, 0.0..=1.0, true);
                    ui.end_row();
                    if *selected == Material::FlatOutline {
                        ui.label("Contour width");
                        slider_with_edit(ui, &mut options.outline_width, 0.05..=1.0, true);
                        ui.end_row();
                        ui.label("Contour strength");
                        slider_with_edit(ui, &mut options.outline, 0.0..=1.0, true);
                        ui.end_row();
                    } else {
                        ui.label("Ambient")
                            .on_hover_text("Unlit fill contribution.");
                        slider_with_edit(ui, &mut options.ambient, 0.0..=1.0, true);
                        ui.end_row();
                        ui.label("Diffuse")
                            .on_hover_text("Strength of the light falling on the surface.");
                        slider_with_edit(ui, &mut options.diffuse, 0.0..=1.0, true);
                        ui.end_row();
                        ui.label("Specular")
                            .on_hover_text("Strength of surface highlights.");
                        slider_with_edit(ui, &mut options.specular, 0.0..=1.0, true);
                        ui.end_row();
                        ui.label(if *selected == Material::MolecularNodes {
                            "Roughness"
                        } else {
                            "Shininess"
                        });
                        slider_with_edit(ui, &mut options.shininess, 0.0..=1.0, true);
                        ui.end_row();
                        if *selected == Material::AoEdgy {
                            ui.label("Outline");
                            let mut outline = options.outline > 0.5;
                            if ui.checkbox(&mut outline, "").changed() {
                                options.outline = if outline { 0.7 } else { 0.0 };
                            }
                            ui.end_row();
                        }
                    }
                });
            reset = ui
                .button("Reset material")
                .on_hover_text("Restore this material's factory shader options")
                .clicked();
        });
        ui.vertical(|ui| {
            let material = if reset {
                *selected
            } else {
                selected.with_options(options)
            };
            if let Some(preview) = preview {
                preview.show(ui, RepParams::Vdw { scale: 1.0 }, material, s.reps.ss_algo);
            } else {
                preview_rectangle(ui);
            }
        });
    });
    if reset {
        s.reps.materials.retain(|m| m.preset() != *selected);
    } else if options != before {
        s.reps.materials.retain(|m| m.preset() != *selected);
        s.reps.materials.push(selected.with_options(options));
    }
}

fn settings_interaction_options(
    ui: &mut egui::Ui,
    s: &mut crate::interactions::InteractionSettings,
) {
    egui::Grid::new("style_interactions")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Line width (px)");
            slider_with_edit(ui, &mut s.line_width, 1.0..=10.0, true);
            ui.end_row();
            for (label, enabled) in [
                ("Hydrogen bonds", &mut s.hbonds),
                ("Hydrophobic", &mut s.hydrophobic),
                ("Salt bridges", &mut s.salt_bridges),
                ("π stacking", &mut s.pi_stacking),
                ("π cation", &mut s.pi_cation),
                ("Halogen bonds", &mut s.halogen),
            ] {
                ui.label(label);
                ui.checkbox(enabled, "");
                ui.end_row();
            }
            for (label, value, range) in [
                ("H-bond distance (nm)", &mut s.hbond_dist, 0.25..=0.5),
                ("H-bond with H (nm)", &mut s.hbond_dist_h, 0.25..=0.5),
                ("H-bond angle (°)", &mut s.hbond_angle, 90.0..=180.0),
                (
                    "Hydrophobic dist. (nm)",
                    &mut s.hydrophobic_dist,
                    0.3..=0.55,
                ),
                ("Salt bridge dist. (nm)", &mut s.salt_bridge_dist, 0.3..=0.6),
                ("π stacking dist. (nm)", &mut s.pi_stacking_dist, 0.3..=0.8),
                ("π stacking angle (°)", &mut s.pi_stacking_angle, 0.0..=90.0),
                (
                    "π stacking offset (nm)",
                    &mut s.pi_stacking_offset,
                    0.0..=0.5,
                ),
                ("π cation distance (nm)", &mut s.pi_cation_dist, 0.3..=0.8),
                ("π cation offset (nm)", &mut s.pi_cation_offset, 0.0..=0.5),
                ("Halogen distance (nm)", &mut s.halogen_dist, 0.25..=0.5),
                ("Halogen angle (°)", &mut s.halogen_angle, 90.0..=180.0),
            ] {
                ui.label(label);
                slider_with_edit(ui, value, range, true);
                ui.end_row();
            }
        });
}

const PREVIEW_SIZE: f32 = 232.0;

fn preview_rectangle(ui: &mut egui::Ui) -> egui::Response {
    ui.allocate_exact_size(egui::Vec2::splat(PREVIEW_SIZE), egui::Sense::drag())
        .1
}

/// Neutral glycine, including explicit hydrogens and its carbonyl double bond.
fn small_molecule_preview_scene() -> Scene {
    use super::draw::Element;
    use crate::minimize::BondOrder;
    use glam::vec3;
    let atoms = [
        (Element::N, vec3(-0.145, 0.03, 0.0)),
        (Element::C, vec3(0.0, 0.0, 0.0)),
        (Element::C, vec3(0.13, 0.065, 0.0)),
        (Element::O, vec3(0.15, 0.185, 0.0)),
        (Element::O, vec3(0.225, -0.025, 0.0)),
        (Element::H, vec3(-0.19, 0.015, 0.085)),
        (Element::H, vec3(-0.185, 0.045, -0.087)),
        (Element::H, vec3(0.005, -0.055, 0.093)),
        (Element::H, vec3(-0.025, -0.067, -0.08)),
        (Element::H, vec3(0.31, 0.015, 0.0)),
    ];
    let raw = data::RawMolecule::single_atom("glycine", atoms[0].0.make_atom(), atoms[0].1)
        .expect("valid preview molecule");
    let mut scene = Scene::default();
    scene.add(raw, &crate::settings::RepDefaults::default());
    let mol = &mut scene.molecules[0];
    for &(element, pos) in &atoms[1..] {
        mol.add_atom(&element.make_atom(), pos);
    }
    for (a, b) in [
        (0, 1),
        (1, 2),
        (2, 4),
        (0, 5),
        (0, 6),
        (1, 7),
        (1, 8),
        (4, 9),
    ] {
        mol.add_bond(a, b, BondOrder::Single);
    }
    mol.add_bond(2, 3, BondOrder::Double);
    mol.refresh_bbox();
    mol.reps[0].color = ColorMethod::Element;
    mol.reps[0].sel = Some(mol.data.select_all());
    scene
}

/// Bounds include actual sphere/cap radii, multiple-bond offsets and mesh vertices.
fn preview_bounds(geom: &geometry::GeometryData) -> Vec<(glam::Vec3, f32)> {
    use glam::Vec3;
    let mut bounds = Vec::new();
    bounds.extend(
        geom.spheres
            .iter()
            .map(|s| (Vec3::from_array(s.center), s.radius)),
    );
    for c in &geom.cylinders {
        let a = Vec3::from_array(c.p0);
        let b = Vec3::from_array(c.p1);
        let r0 = c.profile[0].hypot(c.profile[1]);
        let r1 = (a.distance(b) - c.profile[2]).hypot(c.profile[3]);
        let flare = if c.profile[1] > 0.0 {
            r0.max(r1) + c.smoothing * r0.min(r1) / 3.0
        } else {
            0.0
        };
        let radius = c.radius.max(flare) + (c.offset[0] * c.offset[1]).abs();
        bounds.extend([(a, radius), (b, radius)]);
    }
    bounds.extend(
        geom.mesh
            .vertices
            .iter()
            .map(|v| (Vec3::from_array(v.pos), 0.0)),
    );
    bounds.extend(geom.lines.iter().map(|v| (Vec3::from_array(v.pos), 0.0)));
    bounds
}

fn fit_preview_camera(
    bounds: &[(glam::Vec3, f32)],
    orientation: glam::Quat,
) -> crate::camera::Camera {
    use glam::Vec3;
    let (mut min, mut max) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for &(center, radius) in bounds {
        min = min.min(center - Vec3::splat(radius));
        max = max.max(center + Vec3::splat(radius));
    }
    if bounds.is_empty() {
        min = Vec3::splat(-0.3);
        max = -min;
    }
    let mut camera = crate::camera::Camera::frame_bbox(min, max, 0.9);
    camera.orientation = orientation;
    let mut radius = 0.01_f32;
    for &(center, bead_radius) in bounds {
        radius = radius.max(center.distance(camera.target) + bead_radius);
    }
    // Fit a rotation-invariant envelope so dragging never changes zoom or clips caps.
    camera.scene_radius = radius;
    camera.distance = radius / (0.9 * (camera.fov_y * 0.5).tan());
    camera.background.color = [1.0; 4];
    camera
}

/// A separate offscreen viewport keeps Settings previews on the production shaders.
pub(super) struct SettingsPreview {
    rs: eframe::egui_wgpu::RenderState,
    renderer: SceneRenderer,
    scene: Scene,
    protein: Scene,
    last: Option<(RepParams, Material, SsAlgorithm)>,
    camera: crate::camera::Camera,
    bounds: Vec<(glam::Vec3, f32)>,
    render_size: [u32; 2],
}

impl SettingsPreview {
    fn new(rs: eframe::egui_wgpu::RenderState) -> Self {
        let scene = small_molecule_preview_scene();
        let renderer = SceneRenderer::new(
            &rs,
            &crate::settings::RenderingSettings {
                ssaa: 2,
                shadow_res: 256,
            },
        );
        let raw = data::load_from_bytes(
            "cartoon-preview.pdb",
            include_bytes!("../../assets/cartoon-preview.pdb").to_vec(),
            &crate::data::BondParams::default(),
        )
        .expect("valid bundled crambin protein");
        let mut protein = Scene::default();
        protein.add(raw, &crate::settings::RepDefaults::default());
        protein.molecules[0].reps[0].color = ColorMethod::SecStruct;
        protein.molecules[0].reps[0].sel = Some(protein.molecules[0].data.select_all());
        let mut camera = crate::camera::Camera::default();
        camera.orientation = glam::Quat::from_rotation_x(0.45) * glam::Quat::from_rotation_y(-0.3);
        Self {
            rs,
            renderer,
            scene,
            protein,
            last: None,
            camera,
            bounds: Vec::new(),
            render_size: [0; 2],
        }
    }

    fn show(
        &mut self,
        ui: &mut egui::Ui,
        params: RepParams,
        material: Material,
        ss_algo: SsAlgorithm,
    ) -> egui::Response {
        let response = preview_rectangle(ui)
            .on_hover_text("Drag to rotate")
            .on_hover_cursor(egui::CursorIcon::Grab);
        let rotating = response.dragged_by(egui::PointerButton::Primary);
        if rotating {
            let delta = response.drag_delta();
            self.camera.orbit(delta.x, delta.y, 1.0);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        let size = (PREVIEW_SIZE * ui.ctx().pixels_per_point())
            .round()
            .max(1.0) as u32;
        let render_size = [size; 2];
        let key = (params, material, ss_algo);
        let changed = self.last != Some(key);
        let geometry_changed = self
            .last
            .is_none_or(|(old_params, _, old_ss)| old_params != params || old_ss != ss_algo);
        if changed {
            let scene = if matches!(params.kind(), RepKind::Cartoon | RepKind::Surface) {
                &mut self.protein
            } else {
                &mut self.scene
            };
            let mol = &mut scene.molecules[0];
            mol.reps[0].color = if params.kind() == RepKind::Cartoon {
                ColorMethod::SecStruct
            } else {
                ColorMethod::Element
            };
            let geom = {
                let rep = &mol.reps[0];
                let bound = mol
                    .data
                    .bind_with_state(rep.sel.as_ref().unwrap(), mol.render_state());
                let ss =
                    geometry::needs_ss(&params, rep.color).then(|| SsMap::compute(&bound, ss_algo));
                geometry::build(
                    &bound,
                    mol.n_atoms,
                    &mol.bonds,
                    &params,
                    rep.color_spec(),
                    material,
                    ss.as_ref(),
                    false,
                )
            };
            self.bounds = preview_bounds(&geom);
            let rep = &mut mol.reps[0];
            rep.kind = params.kind();
            rep.params = params;
            rep.material = material;
            rep.gpu = self.renderer.upload(&self.rs, &geom);
            self.last = Some(key);
        }
        if geometry_changed {
            self.camera = fit_preview_camera(&self.bounds, self.camera.orientation);
        }
        if changed || rotating || self.render_size != render_size {
            let camera = &self.camera;
            let scene = if matches!(params.kind(), RepKind::Cartoon | RepKind::Surface) {
                &self.protein
            } else {
                &self.scene
            };
            self.renderer.render_scene(
                &self.rs,
                render_size,
                camera.view(),
                camera.proj(1.0),
                false,
                camera.cue_uniform(),
                camera.ao_uniform(),
                camera.shadow_uniform(),
                camera.background,
                camera.eye_depth_range(),
                0.0,
                scene,
            );
            self.render_size = render_size;
        }
        ui.painter().image(
            self.renderer.texture_id(),
            response.rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        response
    }
}

impl Drop for SettingsPreview {
    fn drop(&mut self) {
        self.rs
            .renderer
            .write()
            .free_texture(&self.renderer.texture_id());
    }
}

#[cfg(test)]
mod material_preview_tests {
    use super::*;

    #[test]
    fn small_molecule_contains_all_four_elements_and_fits_after_rotation() {
        let scene = small_molecule_preview_scene();
        let mol = &scene.molecules[0];
        let bound = mol
            .data
            .bind_with_state(mol.reps[0].sel.as_ref().unwrap(), mol.render_state());
        let elements: std::collections::BTreeSet<_> =
            bound.iter_atoms().map(|a| a.get_atomic_number()).collect();
        assert_eq!(elements, [1, 6, 7, 8].into_iter().collect());
        for kind in [
            RepKind::Vdw,
            RepKind::Licorice,
            RepKind::BallAndStick,
            RepKind::Lines,
            RepKind::Surface,
        ] {
            let geom = geometry::build(
                &bound,
                mol.n_atoms,
                &mol.bonds,
                &RepParams::for_kind(kind),
                mol.reps[0].color_spec(),
                Material::Opaque,
                None,
                false,
            );
            let bounds = preview_bounds(&geom);
            assert!(!bounds.is_empty(), "empty {kind:?} preview");
            let initial = fit_preview_camera(&bounds, glam::Quat::IDENTITY);
            for angle in [0.0, 0.8, 1.6, 2.4] {
                let camera = fit_preview_camera(&bounds, glam::Quat::from_rotation_y(angle));
                assert_eq!(
                    camera.distance, initial.distance,
                    "rotation changes preview zoom"
                );
                let half = camera.distance * (camera.fov_y * 0.5).tan();
                let mut extent = 0.0_f32;
                for &(center, radius) in &bounds {
                    let p = camera.orientation.conjugate() * (center - camera.target);
                    extent = extent.max(p.x.abs().max(p.y.abs()) + radius);
                    assert!(
                        p.x.abs() + radius < half && p.y.abs() + radius < half,
                        "{kind:?} clipped"
                    );
                }
                assert!(
                    angle != 0.0 || extent / half > 0.7,
                    "{kind:?} preview does not fill the image"
                );
            }
        }
    }

    #[test]
    #[ignore = "requires GPU; verifies stable preview layout and mouse rotation with actual rendering"]
    fn preview_layout_stays_fixed_and_mouse_drag_changes_render() {
        let rs = crate::render::test_gpu();
        let mut preview = SettingsPreview::new(rs);
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let mut settings = Settings::default();
        let mut selected = RepKind::Vdw;
        let mut elapsed_frames = 0;
        for height in [480.0, 600.0, 900.0] {
            let mut stable = None;
            for frame in 0..24 {
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(960.0, height),
                        )),
                        time: Some(elapsed_frames as f64 / 60.0),
                        ..Default::default()
                    },
                    |ui| {
                        let ctx = ui.ctx();
                        let window =
                            egui::Window::new("Settings")
                                .resizable(false)
                                .show(ctx, |ui| {
                                    ui.set_width(640.0);
                                    egui::ScrollArea::vertical()
                                        .max_height(440.0)
                                        .auto_shrink([false, true])
                                        .show(ui, |ui| {
                                            settings_page_styles(
                                                ui,
                                                &mut settings,
                                                &mut selected,
                                                Some(&mut preview),
                                            );
                                        });
                                });
                        let Some(window) = window else {
                            return;
                        };
                        if frame >= 8 {
                            let size = window.response.rect.size();
                            if let Some(old) = stable {
                                assert_eq!(size, old, "VDW preview layout oscillates");
                            }
                            stable = Some(size);
                        }
                    },
                );
                elapsed_frames += 1;
            }
            assert!(stable.is_some(), "Settings window never became visible");
        }
        let ctx = egui::Context::default();
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 480.0),
            )),
            events,
            ..Default::default()
        };
        let params = RepParams::Vdw { scale: 1.0 };
        let mut rect = egui::Rect::NOTHING;
        for _ in 0..3 {
            let _ = ctx.run_ui(input(vec![]), |ui| {
                rect = preview
                    .show(ui, params, Material::Opaque, SsAlgorithm::default())
                    .rect;
            });
        }
        let capture = |preview: &mut SettingsPreview| {
            let camera = &preview.camera;
            let cap = preview.renderer.capture_begin(
                &preview.rs,
                232,
                232,
                camera.view(),
                camera.proj(1.0),
                false,
                camera.cue_uniform(),
                camera.ao_uniform(),
                camera.shadow_uniform(),
                camera.background,
                camera.eye_depth_range(),
                &preview.scene,
            );
            preview
                .rs
                .device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
            cap.read()
        };
        let before = capture(&mut preview);
        let orientation = preview.camera.orientation;
        let distance = preview.camera.distance;
        let target = preview.camera.target;
        let start = rect.center();
        let _ = ctx.run_ui(
            input(vec![
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ui| {
                preview.show(ui, params, Material::Opaque, SsAlgorithm::default());
            },
        );
        let finish = start + egui::vec2(65.0, 30.0);
        let _ = ctx.run_ui(input(vec![egui::Event::PointerMoved(finish)]), |ui| {
            let response = preview.show(ui, params, Material::Opaque, SsAlgorithm::default());
            assert!(response.dragged_by(egui::PointerButton::Primary));
            assert_eq!(response.rect.size(), egui::Vec2::splat(PREVIEW_SIZE));
        });
        assert_ne!(preview.camera.orientation, orientation);
        assert_eq!(preview.camera.distance, distance, "drag changed zoom");
        assert_eq!(
            preview.camera.target, target,
            "drag moved the preview centre"
        );
        let after = capture(&mut preview);
        let changed = before
            .pixels()
            .zip(after.pixels())
            .filter(|(a, b)| a != b)
            .count();
        assert!(changed > 1000, "drag did not change the rendered molecule");
        before.save("/tmp/settings-glycine-preview.png").unwrap();
        after
            .save("/tmp/settings-glycine-preview-rotated.png")
            .unwrap();
        let rotated = preview.camera.orientation;
        let _ = ctx.run_ui(
            input(vec![egui::Event::PointerButton {
                pos: finish,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]),
            |ui| {
                preview.show(ui, params, Material::Transparent, SsAlgorithm::default());
            },
        );
        assert_eq!(
            preview.camera.orientation, rotated,
            "slider edits reset rotation"
        );
        assert_eq!(
            preview.camera.distance, distance,
            "material edits changed zoom"
        );
        let _ = ctx.run_ui(input(vec![]), |ui| {
            preview.show(
                ui,
                RepParams::for_kind(RepKind::Surface),
                Material::Opaque,
                SsAlgorithm::default(),
            );
        });
        assert_eq!(preview.protein.molecules[0].reps[0].kind, RepKind::Surface);
        assert!(preview.bounds.len() > 1000, "protein surface mesh is empty");
        let camera = &preview.camera;
        let cap = preview.renderer.capture_begin(
            &preview.rs,
            232,
            232,
            camera.view(),
            camera.proj(1.0),
            false,
            camera.cue_uniform(),
            camera.ao_uniform(),
            camera.shadow_uniform(),
            camera.background,
            camera.eye_depth_range(),
            &preview.protein,
        );
        preview
            .rs
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        cap.read()
            .save("/tmp/settings-protein-surface-preview.png")
            .unwrap();
    }

    #[test]
    fn bundled_protein_has_cartoon_geometry_for_each_ss_algorithm() {
        let raw = data::load_from_bytes(
            "cartoon-preview.pdb",
            include_bytes!("../../assets/cartoon-preview.pdb").to_vec(),
            &crate::data::BondParams::default(),
        )
        .unwrap();
        let mut scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        let mol = &scene.molecules[0];
        let sel = mol.data.select_all();
        let bound = mol.data.bind_with_state(&sel, mol.render_state());
        for algorithm in [SsAlgorithm::Dssp, SsAlgorithm::DsspGmx, SsAlgorithm::Dss] {
            let ss = SsMap::compute(&bound, algorithm);
            let geom = geometry::build(
                &bound,
                mol.n_atoms,
                &mol.bonds,
                &RepParams::for_kind(RepKind::Cartoon),
                mol.reps[0].color_spec(),
                Material::Opaque,
                Some(&ss),
                false,
            );
            assert!(
                geom.mesh.vertices.len() > 100,
                "empty protein preview for {algorithm:?}"
            );
        }
    }

    #[test]
    fn every_settings_style_and_material_page_fits_the_dialog() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &crate::settings::AppearanceSettings::default());
        let mut settings = Settings::default();
        for kind in RepKind::ALL {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let mut selected = kind;
                    settings_page_styles(ui, &mut settings, &mut selected, None);
                    assert!(
                        ui.min_rect().width() <= 640.5,
                        "style page overhangs: {kind:?}, width {}",
                        ui.min_rect().width()
                    );
                },
            );
        }
        for material in Material::ALL {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let mut selected = material;
                    settings_page_materials(ui, &mut settings, &mut selected, None);
                    assert!(
                        ui.min_rect().width() <= 640.5,
                        "material page overhangs: {material:?}"
                    );
                },
            );
        }
    }
}
