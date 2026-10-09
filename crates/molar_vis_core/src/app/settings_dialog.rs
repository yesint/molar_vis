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
    ui.weak("shadow map only matters when cast shadows are on. Both apply immediately.");
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
                    "Used the next time you frame a molecule or representation. Does not change the current zoom.",
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
            ui.label("Ray tracing");
            ui.add_enabled_ui(ray_supported, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Global illumination");
                    slider_with_edit(ui, &mut v.gi, 0.0..=1.0, true);
                });
            });
            ui.weak(if ray_supported {
                "Press R in the viewport to ray-trace the view."
            } else {
                "Ray tracing is unavailable on this device."
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
                    rep.params = RepParams::for_kind(rep.kind);
                }
                if force || previous.reps.color != next.reps.color {
                    rep.color = next.reps.color;
                }
                if force || previous.reps.material != next.reps.material {
                    rep.material = next.reps.material;
                }
                if force || previous.reps.selection != next.reps.selection {
                    rep.sel_text = next.reps.selection.clone();
                    rep.expr = None;
                    rep.sel_dirty = true;
                }
                if force
                    || previous.reps.surface_quality != next.reps.surface_quality
                    || previous.reps.kind != next.reps.kind
                {
                    if let RepParams::Surface { quality, .. } = &mut rep.params {
                        *quality = next.reps.surface_quality;
                    }
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
        let inner = window.show(ctx, |ui| {
            if popup {
                ui.set_width(300.0);
            } else {
                ui.set_width(560.0);
            }
            if !popup {
                tab_bar(
                    ui,
                    &mut dialog.tab,
                    &[
                        (SettingsPage::Appearance, "Appearance"),
                        (SettingsPage::Rendering, "Render quality"),
                        (SettingsPage::View, "View"),
                        (SettingsPage::Representations, "Representations"),
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
            #[cfg(target_arch = "wasm32")]
            ui.weak("Browser defaults remain in memory for this run.");
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
            #[cfg(not(target_arch = "wasm32"))]
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
            #[cfg(target_arch = "wasm32")]
            {
                dialog.save_error = None;
                self.status = "Defaults applied for this browser run".to_string();
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
