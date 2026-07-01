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

                // FPS pinned to the panel bottom; the molecule/group list scrolls in
                // the space above it.
                egui::Panel::bottom("controls_fps").show_inside(ui, |ui| {
                    ui.add_space(4.0);
                    let dt = ui.ctx().input(|i| i.stable_dt);
                    let fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
                    ui.weak(format!("{fps:.0} fps  ({:.1} ms/frame)", dt * 1000.0));
                });

                // The WHOLE list (molecules + groups + their reps) scrolls when it's
                // taller than the panel. Reserve the scrollbar (non-floating) so it
                // sits at the panel edge and never overlaps the right-aligned row
                // buttons/menus. Molecules are listed directly (no section headers).
                ui.style_mut().spacing.scroll.floating = false;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        view_dirty |= self.draw_molecule_list(ui);
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
                ui.label("Global illumination").on_hover_text(
                    "Path-traced GI strength — 0 = off, higher = stronger sky-dome ambient + \
                     indirect colour bleeding. Applies to the R-key ray trace and Render ▸ Save \
                     image. Heavier (extra bounces), so it converges slower.",
                );
                slider_with_edit(ui, &mut self.camera.gi, 0.0..=1.0, true);
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
            // wasm: a browser download). **Image…** opens a dialog to pick the output size +
            // format, then saves (ray-traced on a compute device — see `render/raytrace.rs`).
            menu_buttons.push(ui.menu_button("Render", |ui| {
                if ui.button(format!("{}  Image…", icon::IMAGE)).clicked() {
                    self.image_dialog = Some(ImageDialog { scale: 1 });
                    ui.close();
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
        // A group delete / single-member delete chosen from the panel — deferred to
        // after the loop because removing molecules shifts the entry indices it iterates.
        let mut delete_group: Option<GroupId> = None;
        let mut delete_member: Option<MolId> = None;

        // Top-level entries in panel order: a standalone molecule, or a group shown at
        // the position of its first member (later members are folded into the group).
        enum Entry {
            Mol(usize),
            Group(usize),
        }
        let mut seen_groups = std::collections::HashSet::new();
        let mut entries: Vec<Entry> = Vec::new();
        for (i, mol) in self.scene.molecules.iter().enumerate() {
            match mol.group {
                None => entries.push(Entry::Mol(i)),
                Some(gid) => {
                    if seen_groups.insert(gid) {
                        if let Some(gi) = self.scene.group_index(gid) {
                            entries.push(Entry::Group(gi));
                        }
                    }
                }
            }
        }

        for entry in entries {
            let i = match entry {
                Entry::Mol(i) => i,
                Entry::Group(gi) => {
                    view_dirty |=
                        self.draw_group_entry(ui, gi, &mut delete_group, &mut delete_member);
                    continue;
                }
            };
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
                let len = self.scene.molecules[i].reps.len();
                ui.indent(egui::Id::new(("reps", i)), |ui| {
                    view_dirty |= self.draw_reps_for(ui, i, 0, len, false);
                });
            }
            ui.add_space(4.0);
        }

        // Delete a whole group: move every member to the trash and the group to the
        // group trash (so undo can restore it), then drop the group.
        if let Some(gid) = delete_group {
            if let Some(gi) = self.scene.group_index(gid) {
                let g = self.scene.groups.remove(gi);
                for mid in &g.members {
                    if let Some(mmi) = self.scene.mol_index(*mid) {
                        let m = self.scene.molecules.remove(mmi);
                        self.loaders.remove(&m.id);
                        #[cfg(target_arch = "wasm32")]
                        self.wasm_loaders.remove(&m.id);
                        self.scene.trash.insert(m.id, m);
                    }
                }
                self.scene.group_trash.insert(g.id, g);
                self.scene.clamp_selection();
                view_dirty = true;
            }
        }

        // Delete a single group member: remove it (shared reps preserved on the new
        // shown member, see `Scene::remove_grouped_molecule`) and park it in the trash.
        if let Some(mid) = delete_member {
            if let Some(m) = self.scene.remove_grouped_molecule(mid) {
                self.loaders.remove(&m.id);
                #[cfg(target_arch = "wasm32")]
                self.wasm_loaders.remove(&m.id);
                self.scene.trash.insert(m.id, m);
            }
            self.scene.clamp_selection();
            view_dirty = true;
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

    /// Draw one molecular group: a header (group glyph · name · eye · zoom · add-shared
    /// · menu), a member cycle bar, the shared reps (the shown member's prefix), and —
    /// when expanded — each member with its own reps. Applies its own actions (member
    /// switch with camera re-fit, eye, expand, add shared/own rep); a "Delete group"
    /// is deferred to the caller via `delete_group` (it removes molecules, which would
    /// shift the outer loop's indices). Returns whether the view needs re-rendering.
    pub(super) fn draw_group_entry(
        &mut self,
        ui: &mut egui::Ui,
        gi: usize,
        delete_group: &mut Option<GroupId>,
        delete_member: &mut Option<MolId>,
    ) -> bool {
        let mut view_dirty = false;
        // Snapshot display data so the UI closures don't hold a scene borrow.
        let gid = self.scene.groups[gi].id;
        let gname = self.scene.groups[gi].name.clone();
        let members = self.scene.groups[gi].members.clone();
        let current = self.scene.groups[gi].current;
        let gvisible = self.scene.groups[gi].visible;
        let expanded = self.scene.groups[gi].expanded;
        let n_members = members.len();
        let member_names: Vec<String> = members
            .iter()
            .map(|&id| {
                self.scene
                    .mol_index(id)
                    .map(|mi| self.scene.molecules[mi].name.clone())
                    .unwrap_or_default()
            })
            .collect();
        let cur_mi = members.get(current).and_then(|&id| self.scene.mol_index(id));
        let cur_bbox = cur_mi.map(|mi| self.scene.molecules[mi].current_bbox());

        // Deferred (set in closures, applied at the end so closures stay borrow-free).
        let mut do_toggle_expand = false;
        let mut do_toggle_eye = false;
        let mut do_add_shared = false;
        let mut do_focus = false;
        let mut do_delete = false;
        let mut new_current: Option<usize> = None;
        let mut add_member: Option<MolId> = None;
        let mut del_member: Option<MolId> = None;

        // Header row.
        ui.horizontal(|ui| {
            let caret = if expanded { icon::CARET_DOWN } else { icon::CARET_RIGHT };
            if ui
                .selectable_label(false, caret)
                .on_hover_text("Members")
                .clicked()
            {
                do_toggle_expand = true;
            }
            ui.add(egui::Label::new(icon::STACK).selectable(false))
                .on_hover_text("Molecular group");
            ui.label(&gname)
                .on_hover_text(format!("group · {n_members} molecules"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                compact_actions(ui);
                ui.menu_button(icon::LIST, |ui| {
                    if ui.button(format!("{}  Delete group", icon::TRASH)).clicked() {
                        do_delete = true;
                        ui.close();
                    }
                })
                .response
                .on_hover_text("Group menu");
                let eye = if gvisible { icon::EYE } else { icon::EYE_SLASH };
                if ui
                    .selectable_label(gvisible, eye)
                    .on_hover_text(if gvisible { "Hide group" } else { "Show group" })
                    .clicked()
                {
                    do_toggle_eye = true;
                }
                if icon_button(ui, icon::MAGNIFYING_GLASS_PLUS, "Zoom to shown molecule").clicked() {
                    do_focus = true;
                }
                if ui
                    .button(format!("{} rep", icon::PLUS))
                    .on_hover_text("Add a shared representation (applies to every member)")
                    .clicked()
                {
                    do_add_shared = true;
                }
            });
        });

        // Member cycle bar (which molecule is shown). Tooltip shows "N/M <name>".
        ui.indent(egui::Id::new(("groupbar", gid)), |ui| {
            if let Some(nc) = draw_group_bar(ui, &member_names, current) {
                new_current = Some(nc);
            }
        });

        // Shared reps: the shown member's prefix, drawn under the group header.
        if let Some(mi) = cur_mi {
            let n_shared = self.scene.molecules[mi].n_shared;
            ui.indent(egui::Id::new(("shared", gid)), |ui| {
                view_dirty |= self.draw_reps_for(ui, mi, 0, n_shared, true);
            });
        }

        // Expanded: list members (the whole panel list scrolls — see draw_left_panel),
        // each foldable to its own (non-shared) reps. Member name is clickable → jump.
        if expanded {
            ui.indent(egui::Id::new(("members", gid)), |ui| {
                        for (pos, &mid) in members.iter().enumerate() {
                            let Some(mmi) = self.scene.mol_index(mid) else { continue };
                            let mopen;
                            {
                                let shown = pos == current;
                                // Reserve a shape slot *before* the row so the shown
                                // member's highlight can be painted behind its content.
                                let hl = ui.painter().add(egui::Shape::Noop);
                                let m = &mut self.scene.molecules[mmi];
                                let row = ui.horizontal(|ui| {
                                    let caret = if m.reps_open {
                                        icon::CARET_DOWN
                                    } else {
                                        icon::CARET_RIGHT
                                    };
                                    if ui
                                        .selectable_label(false, caret)
                                        .on_hover_text("Own representations")
                                        .clicked()
                                    {
                                        m.reps_open = !m.reps_open;
                                    }
                                    // Clickable name: underlines on hover, click shows it.
                                    let text = if shown {
                                        egui::RichText::new(m.name.as_str()).strong()
                                    } else {
                                        egui::RichText::new(m.name.as_str())
                                    };
                                    let resp = ui
                                        .add(egui::Label::new(text).sense(egui::Sense::click()))
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .on_hover_text(format!(
                                            "{} atoms — click to show{}",
                                            m.n_atoms,
                                            if shown { " (shown)" } else { "" }
                                        ));
                                    if resp.hovered() {
                                        let r = resp.rect;
                                        ui.painter().hline(
                                            r.x_range(),
                                            r.bottom(),
                                            egui::Stroke::new(1.0, ui.visuals().text_color()),
                                        );
                                    }
                                    if resp.clicked() {
                                        // Clicking a name shows it AND centers the
                                        // camera on it (partial focus — pan, no zoom).
                                        new_current = Some(pos);
                                    }
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            compact_actions(ui);
                                            ui.menu_button(icon::LIST, |ui| {
                                                if ui
                                                    .button(format!(
                                                        "{}  Delete molecule",
                                                        icon::TRASH
                                                    ))
                                                    .clicked()
                                                {
                                                    del_member = Some(mid);
                                                    ui.close();
                                                }
                                            })
                                            .response
                                            .on_hover_text("Molecule menu");
                                            if ui
                                                .button(format!("{} rep", icon::PLUS))
                                                .on_hover_text(
                                                    "Add a representation to this molecule only",
                                                )
                                                .clicked()
                                            {
                                                add_member = Some(mid);
                                            }
                                        },
                                    );
                                });
                                // Highlight the currently-shown member's row (a subtle
                                // accent bar behind its content).
                                if shown {
                                    let c = ui.visuals().selection.bg_fill;
                                    let bar = egui::Color32::from_rgba_unmultiplied(
                                        c.r(),
                                        c.g(),
                                        c.b(),
                                        60,
                                    );
                                    ui.painter().set(
                                        hl,
                                        egui::Shape::rect_filled(
                                            row.response.rect.expand2(egui::vec2(4.0, 1.0)),
                                            3.0,
                                            bar,
                                        ),
                                    );
                                }
                                mopen = m.reps_open;
                            }
                            if mopen {
                                let (start, end) = {
                                    let m = &self.scene.molecules[mmi];
                                    (m.n_shared, m.reps.len())
                                };
                                ui.indent(egui::Id::new(("ownreps", mid)), |ui| {
                                    if start < end {
                                        view_dirty |= self.draw_reps_for(ui, mmi, start, end, false);
                                    } else {
                                        ui.weak("(no own representations)");
                                    }
                                });
                            }
                        }
            });
        }
        ui.add_space(4.0);

        // --- apply deferred actions (indices into `scene.molecules` stay valid: none
        // of these add or remove molecules; "Delete group" is deferred to the caller).
        if do_toggle_expand {
            self.scene.groups[gi].expanded = !self.scene.groups[gi].expanded;
        }
        if do_toggle_eye {
            self.scene.groups[gi].visible = !self.scene.groups[gi].visible;
            self.scene.apply_group_visibility(gi);
            view_dirty = true;
        }
        if do_focus {
            // Full zoom-to-fit the shown member (the magnifier means "zoom").
            if let Some((min, max)) = cur_bbox {
                self.camera.focus_bbox(min, max);
                view_dirty = true;
            }
        }
        if let Some(nc) = new_current {
            // Partial focus: switch the shown member and CENTER the camera on it (pan
            // the target only, keeping the current zoom). Applies to both the cycle bar
            // (slider/arrows) and clicking a member name. Not gated on `switch`'s return
            // so clicking the already-shown member re-centers it.
            self.scene.switch_group_member(gi, nc);
            if let Some(&id) = self.scene.groups[gi].members.get(nc) {
                if let Some(mi) = self.scene.mol_index(id) {
                    let (min, max) = self.scene.molecules[mi].current_bbox();
                    self.camera.target = 0.5 * (min + max);
                }
            }
            view_dirty = true;
        }
        if do_add_shared {
            // Append a shared rep at the end of the shown member's shared prefix.
            let cur = self.scene.groups[gi].current;
            if let Some(&cur_id) = self.scene.groups[gi].members.get(cur) {
                if let Some(mmi) = self.scene.mol_index(cur_id) {
                    let rep = Representation::from_defaults(&self.rep_defaults);
                    let m = &mut self.scene.molecules[mmi];
                    let ns = m.n_shared.min(m.reps.len());
                    m.reps.insert(ns, rep);
                    m.n_shared = ns + 1;
                    m.selected_rep = Some(ns);
                    view_dirty = true;
                }
            }
        }
        if let Some(mid) = add_member {
            if let Some(mmi) = self.scene.mol_index(mid) {
                let rep = Representation::from_defaults(&self.rep_defaults);
                let m = &mut self.scene.molecules[mmi];
                m.reps.push(rep);
                m.selected_rep = Some(m.reps.len() - 1);
                m.reps_open = true;
                view_dirty = true;
            }
        }
        if do_delete {
            *delete_group = Some(gid);
        }
        if del_member.is_some() {
            // Deferred to the caller: removing a molecule shifts the entry indices the
            // outer loop iterates over.
            *delete_member = del_member;
        }
        view_dirty
    }
}
