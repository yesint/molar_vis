//! Left panel, menu bar, molecule list, top view toolbar, view-settings window.
use super::*;
use super::widgets::*;
use super::settings_dialog::*;
use super::rep_panel::*;
#[cfg(target_arch = "wasm32")]
use super::loaders::pick_file;

impl App {
    pub(super) fn draw_left_panel(&mut self, ui: &mut egui::Ui) -> bool {
        let mut view_dirty = false;
        egui::Panel::left("controls_panel")
            .resizable(true)
            .default_size(300.0)
            .size_range(egui::Rangef::new(220.0, 520.0))
            .show_inside(ui, |ui| {
                ui.add_space(8.0);

                self.draw_menu_bar(ui);
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
    /// mode), grouped by a separator. The lasso/click modifier hint (add / subtract /
    /// rotate) appears on its own line **below** the controls while a modifier is held.
    /// All buttons are the shared `overlay_button` (uniform height, framed,
    /// ink-centered glyph); dropdowns/popups hang off `egui::Popup::menu`.
    pub(super) fn draw_view_toolbar(&mut self, ui: &mut egui::Ui) {
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
                    toolbar_label(ui, "Selection mode");
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

                    // (Draw-mode toggle now lives in the left-panel Molecule menu.)
                    // (The selection modifier hint (add / subtract / rotate) is drawn
                    // as a floating overlay *on the 3D viewport* — `modifier_hint` /
                    // `draw_modifier_hint_overlay` in `draw_viewport` — so it never
                    // resizes the view.)

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
    pub(super) fn view_settings_window(&mut self, ctx: &egui::Context, anchor: egui::Rect) {
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
    pub(super) fn view_tab_camera(&mut self, ui: &mut egui::Ui) {
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
    pub(super) fn view_tab_lighting(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let ao = &mut self.camera.ao;
            ui.checkbox(&mut ao.enabled, "Ambient occlusion")
                .on_hover_text(
                    "Darken creases and cavities (true hemisphere AO in the ray-traced view, \
                     screen-space AO in the realtime view)",
                );
            ui.add_enabled_ui(ao.enabled, |ui| {
                egui::Grid::new("ao_opts")
                    .num_columns(2)
                    .spacing(egui::vec2(8.0, 4.0))
                    .show(ui, |ui| {
                        ui.label("Strength");
                        slider_with_edit(ui, &mut ao.strength, 0.0..=1.0, ao.enabled);
                        ui.end_row();
                        ui.label("Radius").on_hover_text(
                            "Occlusion reach. In the ray-traced view this scales with the \
                             molecule size (a fraction of the scene), so AO finds cavities at \
                             any scale.",
                        );
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
                        ui.label("Softness")
                            .on_hover_text("Soft shadow edges (ray-traced view only)");
                        slider_with_edit(ui, &mut sh.softness, 0.0..=1.0, sh.enabled);
                        ui.end_row();
                    });
            });
        });
        ui.add_space(6.0);
        // Ray tracing (WebGPU/native only): press R to ray-trace the current view (PyMOL-
        // `ray` style) + an optional global-illumination tier for Save image.
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let supported = self.renderer.raytrace_supported();
            if supported {
                ui.label("Ray tracing").on_hover_text(
                    "Press R in the viewport to ray-trace the current view (ambient occlusion \
                     + shadows); it holds until you move the camera. Render ▸ Save image \
                     ray-traces to a file at any resolution.",
                );
                ui.label(
                    egui::RichText::new("Press R to ray-trace the view")
                        .weak()
                        .small(),
                );
                ui.add_space(2.0);
                ui.checkbox(&mut self.camera.gi, "Global illumination")
                    .on_hover_text(
                        "Path-traced global illumination — soft sky-dome ambient + indirect \
                         colour bleeding. Applies to both the R-key ray trace and Render ▸ Save \
                         image. Heavier (more bounces), so it takes longer to converge.",
                    );
            } else {
                ui.label(
                    egui::RichText::new("Ray tracing needs WebGPU (unavailable on this device)")
                        .weak()
                        .small(),
                );
            }
        });
    }

    /// Scene tab: orientation axes + background.
    pub(super) fn view_tab_scene(&mut self, ui: &mut egui::Ui) {
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

    /// The left-panel **menu bar** — three drop-down menus that hold every global
    /// action (the old inline toolbar of buttons is gone):
    /// - **Molecule**: *Draw* (toggle the interactive sketch mode) · *Load…* (open a
    ///   structure file as a new molecule).
    /// - **Session** (native only — wasm has no filesystem to reload sources from):
    ///   *New* / *Save…* / *Load…* the whole visualization state.
    /// - **Edit**: *Undo* / *Redo* (single step, labelled with the next action; the
    ///   `▼`-dropdown cumulative undo is gone — Ctrl+Z/Ctrl+Shift+Z still repeat) ·
    ///   *Settings…*.
    pub(super) fn draw_menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            // Each top-level menu's button response is collected so we can switch the
            // open menu on **hover** (desktop menu-bar behavior): egui only opens a
            // bar menu on click, so once one is open we forward a hover over a sibling
            // button to `Popup::open_id` (which closes the others — at most one popup
            // is open per viewport). See the hover-switch block at the end.
            let mut menu_buttons: Vec<egui::Response> = Vec::new();

            // — Molecule —
            menu_buttons.push(ui.menu_button("Molecule", |ui| {
                let drawing = self.draw.is_some();
                if ui
                    .selectable_label(drawing, format!("{}  Draw", icon::PENCIL_SIMPLE))
                    .on_hover_text("Draw mode — sketch atoms and bonds by hand")
                    .clicked()
                {
                    self.toggle_draw();
                    ui.close();
                }
                if ui
                    .button(format!("{}  Load…", icon::FOLDER_OPEN))
                    .on_hover_text("Open a structure file as a new molecule")
                    .clicked()
                {
                    self.open_structure(ui.ctx());
                    ui.close();
                }
            }).response);

            // — Session — New starts an empty scene (pure in-memory, so available
            // everywhere); Save/Load persist the whole visualization state but reload
            // molecules from disk, so they're native-only.
            menu_buttons.push(ui.menu_button("Session", |ui| {
                if ui
                    .button(format!("{}  New", icon::FILE))
                    .on_hover_text("Clear all molecules and start an empty scene")
                    .clicked()
                {
                    self.new_session();
                    ui.close();
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
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
                }
            }).response);

            // — Render — output the current view to an image file (native: save dialog;
            // wasm: a browser download). Rendered at a multiple of the viewport for crisp
            // figures. Future high-quality / raytraced renders will live here too.
            menu_buttons.push(ui.menu_button("Render", |ui| {
                ui.label("Save image (PNG)");
                for (label, scale) in
                    [("Viewport (1×)", 1u32), ("2× viewport", 2), ("4× viewport", 4)]
                {
                    if ui
                        .button(format!("{}  {label}", icon::IMAGE))
                        .clicked()
                    {
                        self.export_request = Some(scale);
                        ui.close();
                    }
                }
            }).response);

            // — Edit —
            menu_buttons.push(ui.menu_button("Edit", |ui| {
                let can_undo = self.history.can_undo();
                let undo_label = if can_undo {
                    format!("{}  Undo {}", icon::ARROW_COUNTER_CLOCKWISE, self.history.undo_label(0))
                } else {
                    format!("{}  Undo", icon::ARROW_COUNTER_CLOCKWISE)
                };
                if ui
                    .add_enabled(can_undo, egui::Button::new(undo_label).shortcut_text("Ctrl+Z"))
                    .clicked()
                {
                    self.pending_undo_n = Some(1);
                    ui.close();
                }

                let can_redo = self.history.can_redo();
                let redo_label = if can_redo {
                    format!("{}  Redo {}", icon::ARROW_CLOCKWISE, self.history.redo_label(0))
                } else {
                    format!("{}  Redo", icon::ARROW_CLOCKWISE)
                };
                if ui
                    .add_enabled(can_redo, egui::Button::new(redo_label).shortcut_text("Ctrl+Shift+Z"))
                    .clicked()
                {
                    self.pending_redo_n = Some(1);
                    ui.close();
                }

                ui.separator();

                if ui
                    .button(format!("{}  Settings…", icon::GEAR_SIX))
                    .on_hover_text("Program settings")
                    .clicked()
                {
                    if self.settings_draft.is_none() {
                        self.settings_draft = Some(self.settings.clone());
                    }
                    ui.close();
                }
            }).response);

            // — View —
            menu_buttons.push(ui.menu_button("View", |ui| {
                // Checkable Console toggle (a `[x]`-style item via the leading icon).
                let mark = if self.console_open { icon::CHECK_SQUARE } else { icon::SQUARE };
                if ui
                    .button(format!("{}  Console", mark))
                    .on_hover_text("Scripting console (Rhai) — drive the viewer with commands")
                    .clicked()
                {
                    self.console_open = !self.console_open;
                    if self.console_open {
                        self.console.focus_input = true; // grab the input field on open
                    }
                    ui.close();
                }
            }).response);

            // Hover-switch: once any bar menu is open, moving the pointer onto a
            // different top-level button opens that menu (and closes the rest, since
            // only one popup is open at a time). Takes effect next frame, so request a
            // repaint to show it even if the pointer then holds still. We test the raw
            // pointer against each button rect (not `Response::hovered`, which egui can
            // gate by layer while another Area is open) — the open popup hangs *below*
            // the bar, so it never covers a sibling button.
            let ctx = ui.ctx();
            let popup_id = |r: &egui::Response| egui::Popup::default_response_id(r);
            let any_open = menu_buttons
                .iter()
                .any(|r| egui::Popup::is_id_open(ctx, popup_id(r)));
            if any_open {
                if let Some(pos) = ctx.pointer_hover_pos() {
                    for r in &menu_buttons {
                        let id = popup_id(r);
                        if r.rect.contains(pos) && !egui::Popup::is_id_open(ctx, id) {
                            egui::Popup::open_id(ctx, id);
                            ctx.request_repaint();
                        }
                    }
                }
            }
        });
    }

    /// Loaded molecules. Each is a foldable block: a header row (fold caret, name,
    /// atom count, then a right-justified action group: add-rep, eye, trash), with
    /// the molecule's representations nested below when expanded.
    pub(super) fn draw_molecule_list(&mut self, ui: &mut egui::Ui) -> bool {
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
}
