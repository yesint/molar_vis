//! The eframe application: owns UI state, the camera, the scene (molecules and
//! their representations), and the 3D renderer. Lays out the VMD-style left
//! control panel (Scene → Molecules → Representations → Rep controls) plus the
//! central 3D viewport, and only re-renders the scene when something changed.

use eframe::egui;

use crate::camera::{Camera, Projection};
use crate::data;
use crate::geometry::{self, RepKind, RepParams};
use crate::history::{EditState, History};
use crate::launch::AppLaunch;
use crate::render::SceneRenderer;
use crate::scene::{self, Representation, Scene};

use egui_phosphor::regular as icon;

/// A compact icon button: frameless at rest, with a background highlight on
/// hover, plus a tooltip. Implemented via `selectable_label` (always unselected)
/// because the theme overrides text color, so a frameless `Button` would show no
/// hover feedback, whereas `selectable_label` highlights its background.
fn icon_button(ui: &mut egui::Ui, glyph: &str, hover: &str) -> egui::Response {
    ui.selectable_label(false, glyph).on_hover_text(hover)
}

pub struct App {
    renderer: SceneRenderer,
    camera: Camera,
    scene: Scene,
    /// Style used for the initial representation of each loaded molecule.
    default_rep: RepKind,
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
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        launch: AppLaunch,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        crate::theme::apply(&cc.egui_ctx);

        let render_state = cc
            .wgpu_render_state
            .as_ref()
            .ok_or("wgpu render state unavailable (eframe must use the wgpu backend)")?;
        let renderer = SceneRenderer::new(render_state);

        // VMD's default style for a new molecule is Lines; override for headless
        // checks with VMD_RS_DEBUG_REP=vdw|licorice|ballstick|lines.
        let default_rep = std::env::var("VMD_RS_DEBUG_REP")
            .ok()
            .and_then(|s| RepKind::from_name(&s))
            .unwrap_or(RepKind::Lines);

        let mut scene = Scene::default();
        let mut status = String::new();
        for path in &launch.files {
            match data::load(path) {
                Ok(raw) => {
                    scene.add(raw, default_rep);
                }
                Err(e) => {
                    log::error!("{e}");
                    status = e;
                }
            }
        }
        if !scene.molecules.is_empty() {
            scene.selected_mol = Some(0);
            status = format!("{} molecule(s) loaded", scene.molecules.len());
        } else if status.is_empty() {
            status = "No molecules loaded.".to_string();
        }

        // Verification hook: VMD_RS_DEBUG_SEL=<selection> overrides the initial
        // selection of every molecule's first rep (e.g. "name CA", "protein").
        if let Ok(sel) = std::env::var("VMD_RS_DEBUG_SEL") {
            for mol in &mut scene.molecules {
                if let Some(rep) = mol.reps.first_mut() {
                    rep.sel_text = sel.clone();
                    rep.sel_dirty = true;
                }
            }
        }

        let mut camera = match scene.bbox() {
            Some((min, max)) => Camera::frame_bbox(min, max),
            None => Camera::default(),
        };
        if let Ok(deg) = std::env::var("VMD_RS_DEBUG_ORBIT") {
            if let Ok(d) = deg.parse::<f32>() {
                camera.orbit(d, d * 0.4);
            }
        }
        if std::env::var("VMD_RS_DEBUG_ORTHO").is_ok() {
            camera.projection = Projection::Orthographic;
        }

        let history = History::new(EditState::capture(&scene));

        Ok(Self {
            renderer,
            camera,
            scene,
            default_rep,
            last_render_camera: None,
            last_size: [0, 0],
            view_dirty: true,
            status,
            history,
            pending_undo_n: None,
            pending_redo_n: None,
        })
    }

    /// Recompile dirty selections and rebuild/reupload dirty geometry. Returns
    /// true if any geometry was uploaded (so the frame needs re-rendering).
    fn rebuild_dirty(&mut self, rs: &eframe::egui_wgpu::RenderState) -> bool {
        let mut changed = false;
        for mol in &mut self.scene.molecules {
            if !mol.reps.iter().any(|r| r.sel_dirty || r.geom_dirty) {
                continue;
            }
            for rep in &mut mol.reps {
                if rep.sel_dirty {
                    // Parse + evaluate the selection. On error keep the previous
                    // selection/geometry and just surface the message.
                    match scene::evaluate(&mol.system, rep.sel_text.as_str()) {
                        Ok((expr, sel)) => {
                            rep.expr = Some(expr);
                            rep.sel = Some(sel);
                            rep.sel_error = None;
                            rep.geom_dirty = true;
                        }
                        Err(e) => rep.sel_error = Some(e),
                    }
                    rep.sel_dirty = false;
                }
                if rep.geom_dirty {
                    if let Some(sel) = &rep.sel {
                        let geom =
                            geometry::build(&mol.system, sel, &mol.bonds, rep.kind, &rep.params);
                        rep.gpu = self.renderer.upload(rs, &geom);
                    }
                    rep.geom_dirty = false;
                    changed = true;
                }
            }
        }
        changed
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // No continuous repaint: egui repaints on input (incl. active drags), and
        // we re-render the 3D scene only when it actually changed (see viewport).
        let ctx = ui.ctx().clone();

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

        let panel_dirty = self.draw_left_panel(ui);
        self.view_dirty |= panel_dirty;

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
                ui.add_space(4.0);

                egui::CollapsingHeader::new("Scene")
                    .default_open(true)
                    .show(ui, |ui| self.draw_scene_controls(ui));

                ui.separator();
                egui::CollapsingHeader::new("Molecules")
                    .default_open(true)
                    .show(ui, |ui| view_dirty |= self.draw_molecule_table(ui));

                ui.separator();
                egui::CollapsingHeader::new("Representations")
                    .default_open(true)
                    .show(ui, |ui| view_dirty |= self.draw_rep_table(ui));

                ui.separator();
                egui::CollapsingHeader::new("Representation controls")
                    .default_open(true)
                    .show(ui, |ui| self.draw_rep_controls(ui));

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(4.0);
                    let dt = ui.ctx().input(|i| i.stable_dt);
                    let fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
                    ui.weak(format!("{fps:.0} fps  ({:.1} ms/frame)", dt * 1000.0));
                });
            });
        view_dirty
    }

    /// Global scene options. Projection now; lighting/background/etc. later.
    fn draw_scene_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Projection");
            let persp = self.camera.is_perspective();
            // Mutually exclusive icon toggle buttons (tooltips on hover).
            if ui
                .selectable_label(persp, icon::PERSPECTIVE)
                .on_hover_text("Perspective")
                .clicked()
            {
                self.camera.projection = Projection::Perspective;
            }
            if ui
                .selectable_label(!persp, icon::CUBE)
                .on_hover_text("Orthographic")
                .clicked()
            {
                self.camera.projection = Projection::Orthographic;
            }
        });
    }

    /// Undo/redo buttons, each with a dropdown listing the named actions on the
    /// stack; selecting an entry undoes/redoes cumulatively up to it.
    fn draw_history_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;

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
        });
    }

    /// Loaded molecules as a table: file name | atoms | actions (eye, trash).
    fn draw_molecule_table(&mut self, ui: &mut egui::Ui) -> bool {
        if self.scene.molecules.is_empty() {
            ui.weak(&self.status);
            return false;
        }
        let mut view_dirty = false;
        let mut new_selected = self.scene.selected_mol;
        let mut delete: Option<usize> = None;

        egui::Grid::new("molecule_table")
            .num_columns(3)
            .striped(true)
            .show(ui, |ui| {
                ui.strong("File");
                ui.strong("Atoms");
                ui.label("");
                ui.end_row();

                for i in 0..self.scene.molecules.len() {
                    let selected = self.scene.selected_mol == Some(i);
                    let mol = &mut self.scene.molecules[i];

                    if ui.selectable_label(selected, mol.name.as_str()).clicked() {
                        new_selected = Some(i);
                    }
                    ui.label(format!("{}", mol.n_atoms));
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let (eye, tip) = if mol.visible {
                            (icon::EYE, "Hide")
                        } else {
                            (icon::EYE_SLASH, "Show")
                        };
                        if icon_button(ui, eye, tip).clicked() {
                            mol.visible = !mol.visible;
                            view_dirty = true;
                        }
                        if icon_button(ui, icon::TRASH, "Delete molecule").clicked() {
                            delete = Some(i);
                        }
                    });
                    ui.end_row();
                }
            });
        self.scene.selected_mol = new_selected;

        if let Some(i) = delete {
            // Park the molecule in the trash so the delete can be undone.
            let m = self.scene.molecules.remove(i);
            self.scene.trash.insert(m.id, m);
            self.scene.clamp_selection();
            view_dirty = true;
        }
        view_dirty
    }

    /// Representations of the selected molecule as a table: selection | style |
    /// actions (visibility eye, update-every-frame, duplicate, delete). An "Add"
    /// button precedes the table.
    fn draw_rep_table(&mut self, ui: &mut egui::Ui) -> bool {
        let default_rep = self.default_rep;
        let Some(mi) = self.scene.selected_mol else {
            ui.weak("Select a molecule above.");
            return false;
        };
        let mut view_dirty = false;
        let mol = &mut self.scene.molecules[mi];

        // Add button BEFORE the table.
        if ui
            .button(format!("{}  Add representation", icon::PLUS))
            .clicked()
        {
            mol.reps.push(Representation::new(default_rep));
            mol.selected_rep = Some(mol.reps.len() - 1);
            view_dirty = true;
        }
        ui.add_space(2.0);

        let mut new_sel_rep = mol.selected_rep;
        let mut delete: Option<usize> = None;
        let mut duplicate: Option<usize> = None;

        egui::Grid::new("rep_table")
            .num_columns(3)
            .striped(true)
            .show(ui, |ui| {
                ui.strong("Selection");
                ui.strong("Style");
                ui.label("");
                ui.end_row();

                for j in 0..mol.reps.len() {
                    let rep = &mut mol.reps[j];

                    // Selection text (editable). Editing it / focusing selects the row.
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut rep.sel_text).desired_width(100.0),
                    );
                    if resp.lost_focus() {
                        rep.sel_dirty = true;
                    }
                    if resp.gained_focus() {
                        new_sel_rep = Some(j);
                    }

                    // Style dropdown.
                    egui::ComboBox::from_id_salt(("rep_style", j))
                        .selected_text(rep.kind.label())
                        .width(72.0)
                        .show_ui(ui, |ui| {
                            for kind in RepKind::ALL {
                                if ui.selectable_value(&mut rep.kind, kind, kind.label()).clicked()
                                {
                                    rep.params = RepParams::for_kind(kind);
                                    rep.geom_dirty = true;
                                    new_sel_rep = Some(j);
                                }
                            }
                        });

                    // Action buttons (tight spacing).
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let (eye, tip) = if rep.visible {
                            (icon::EYE, "Hide")
                        } else {
                            (icon::EYE_SLASH, "Show")
                        };
                        if icon_button(ui, eye, tip).clicked() {
                            rep.visible = !rep.visible;
                            view_dirty = true;
                        }
                        if ui
                            .selectable_label(rep.dynamic, icon::ARROWS_CLOCKWISE)
                            .on_hover_text("Update every frame")
                            .clicked()
                        {
                            rep.dynamic = !rep.dynamic;
                        }
                        if icon_button(ui, icon::COPY, "Duplicate").clicked() {
                            duplicate = Some(j);
                        }
                        if icon_button(ui, icon::TRASH, "Delete").clicked() {
                            delete = Some(j);
                        }
                    });
                    ui.end_row();
                }
            });
        mol.selected_rep = new_sel_rep;

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
        view_dirty
    }

    /// Parameter controls for the selected representation (selection text and
    /// style live in the rep table). Surfaces selection errors.
    fn draw_rep_controls(&mut self, ui: &mut egui::Ui) {
        let Some(mi) = self.scene.selected_mol else {
            ui.weak("—");
            return;
        };
        let mol = &mut self.scene.molecules[mi];
        let Some(ri) = mol.selected_rep else {
            ui.weak("—");
            return;
        };
        let rep = &mut mol.reps[ri];

        if let Some(err) = &rep.sel_error {
            ui.colored_label(egui::Color32::from_rgb(240, 120, 120), err);
        }

        match rep.kind {
            RepKind::Vdw => {
                ui.weak("Spheres at van der Waals radius.");
            }
            RepKind::Lines => {
                ui.weak("Half-bond colored lines (1 px).");
            }
            RepKind::Licorice => {
                if ui
                    .add(egui::Slider::new(&mut rep.params.bond_radius, 0.005..=0.10).text("bond radius (nm)"))
                    .changed()
                {
                    rep.geom_dirty = true;
                }
            }
            RepKind::BallAndStick => {
                let mut changed = ui
                    .add(egui::Slider::new(&mut rep.params.sphere_scale, 0.05..=0.6).text("sphere scale"))
                    .changed();
                changed |= ui
                    .add(egui::Slider::new(&mut rep.params.bond_radius, 0.005..=0.05).text("bond radius (nm)"))
                    .changed();
                if changed {
                    rep.geom_dirty = true;
                }
            }
        }
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

            // VMD-style navigation: left = rotate, middle = pan, right = zoom.
            let delta = response.drag_delta();
            if response.dragged_by(egui::PointerButton::Primary) {
                self.camera.orbit(delta.x, delta.y);
            } else if response.dragged_by(egui::PointerButton::Middle) {
                self.camera.pan(delta.x, delta.y, rect.height());
            } else if response.dragged_by(egui::PointerButton::Secondary) {
                self.camera.zoom_drag(delta.y);
            }
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    self.camera.zoom_scroll(scroll);
                }
            }

            let ppp = ui.ctx().pixels_per_point();
            let size_px = [
                ((rect.width() * ppp).round() as u32).max(1),
                ((rect.height() * ppp).round() as u32).max(1),
            ];

            let cam_changed = self.last_render_camera != Some(self.camera);
            let size_changed = size_px != self.last_size;
            if geom_changed || cam_changed || size_changed || self.view_dirty {
                let aspect = size_px[0] as f32 / size_px[1] as f32;
                let view = self.camera.view();
                let proj = self.camera.proj(aspect);
                self.renderer.render_scene(
                    render_state,
                    size_px,
                    view,
                    proj,
                    self.camera.is_perspective(),
                    &self.scene,
                );
                self.last_render_camera = Some(self.camera);
                self.last_size = size_px;
            }
            self.view_dirty = false;

            let texture_id = self.renderer.texture_id();
            egui::Image::new(egui::load::SizedTexture::new(texture_id, rect.size()))
                .paint_at(ui, rect);
        });
    }
}
