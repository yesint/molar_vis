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
    pending_undo: bool,
    pending_redo: bool,
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
            pending_undo: false,
            pending_redo: false,
        })
    }

    /// Recompile dirty selections and rebuild/reupload dirty geometry. Returns
    /// true if any geometry was uploaded (so the frame needs re-rendering).
    fn rebuild_dirty(&mut self, rs: &eframe::egui_wgpu::RenderState) -> bool {
        let mut changed = false;
        for mol in &mut self.scene.molecules {
            // Skip molecules with nothing to do — avoids binding an all-selection
            // (which allocates a full index) every frame.
            if !mol.reps.iter().any(|r| r.sel_dirty || r.geom_dirty) {
                continue;
            }
            // Bound all-selection over the System: the source of positions/atoms
            // for geometry (atom index == global index). Shares `mol.system` with
            // compile_selection below (both immutable borrows).
            let all = mol.system.select_all_bound();
            for rep in &mut mol.reps {
                if rep.sel_dirty {
                    match scene::compile_selection(&mol.system, rep.sel_text.as_str()) {
                        Ok(idx) => {
                            rep.sel_indices = idx;
                            rep.sel_error = None;
                            rep.geom_dirty = true;
                        }
                        Err(e) => rep.sel_error = Some(e),
                    }
                    rep.sel_dirty = false;
                }
                if rep.geom_dirty {
                    let geom =
                        geometry::build(&all, &mol.bonds, &rep.sel_indices, rep.kind, &rep.params);
                    rep.gpu = self.renderer.upload(rs, &geom);
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
                    self.pending_redo = true;
                } else {
                    self.pending_undo = true;
                }
            }
            if i.modifiers.command && i.key_pressed(egui::Key::Y) {
                self.pending_redo = true;
            }
        });

        let panel_dirty = self.draw_left_panel(ui);
        self.view_dirty |= panel_dirty;

        // Apply undo/redo after the panel so list indices stay stable during draw.
        let (do_undo, do_redo) = (self.pending_undo, self.pending_redo && !self.pending_undo);
        self.pending_undo = false;
        self.pending_redo = false;
        let applied = if do_undo {
            self.history.undo()
        } else if do_redo {
            self.history.redo()
        } else {
            None
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

                ui.horizontal(|ui| {
                    let can_undo = self.history.can_undo();
                    let can_redo = self.history.can_redo();
                    if ui
                        .add_enabled(can_undo, egui::Button::new(icon::ARROW_COUNTER_CLOCKWISE))
                        .on_hover_text("Undo (Ctrl+Z)")
                        .clicked()
                    {
                        self.pending_undo = true;
                    }
                    if ui
                        .add_enabled(can_redo, egui::Button::new(icon::ARROW_CLOCKWISE))
                        .on_hover_text("Redo (Ctrl+Shift+Z)")
                        .clicked()
                    {
                        self.pending_redo = true;
                    }
                });
                ui.add_space(4.0);

                egui::CollapsingHeader::new("Scene")
                    .default_open(true)
                    .show(ui, |ui| self.draw_scene_controls(ui));

                ui.separator();
                egui::CollapsingHeader::new("Molecules")
                    .default_open(true)
                    .show(ui, |ui| view_dirty |= self.draw_molecule_list(ui));

                ui.separator();
                egui::CollapsingHeader::new("Representations")
                    .default_open(true)
                    .show(ui, |ui| view_dirty |= self.draw_rep_list(ui));

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

    /// Loaded molecules: name on the left, a right-justified icon group
    /// (visibility eye, delete trash). Returns true if a re-render is needed.
    fn draw_molecule_list(&mut self, ui: &mut egui::Ui) -> bool {
        if self.scene.molecules.is_empty() {
            ui.weak(&self.status);
            return false;
        }
        let mut view_dirty = false;
        let mut new_selected = self.scene.selected_mol;
        let mut delete: Option<usize> = None;

        for i in 0..self.scene.molecules.len() {
            let selected = self.scene.selected_mol == Some(i);
            let mol = &mut self.scene.molecules[i];
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, icon::TRASH, "Delete molecule").clicked() {
                        delete = Some(i);
                    }
                    let (eye, tip) = if mol.visible {
                        (icon::EYE, "Hide")
                    } else {
                        (icon::EYE_SLASH, "Show")
                    };
                    if icon_button(ui, eye, tip).clicked() {
                        mol.visible = !mol.visible;
                        view_dirty = true;
                    }
                    // Name fills the remaining space, left-aligned.
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let label = format!("{}  ({} atoms)", mol.name, mol.n_atoms);
                        if ui.selectable_label(selected, label).clicked() {
                            new_selected = Some(i);
                        }
                    });
                });
            });
        }
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

    /// Representations of the selected molecule. An "Add" button precedes the
    /// list; each row shows the "<sel>/<style>" name with a right-justified icon
    /// group (visibility eye, duplicate, delete).
    fn draw_rep_list(&mut self, ui: &mut egui::Ui) -> bool {
        let default_rep = self.default_rep;
        let Some(mi) = self.scene.selected_mol else {
            ui.weak("Select a molecule above.");
            return false;
        };
        let mut view_dirty = false;
        let mol = &mut self.scene.molecules[mi];

        // Add button BEFORE the list.
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

        for j in 0..mol.reps.len() {
            let selected = mol.selected_rep == Some(j);
            let rep = &mut mol.reps[j];
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, icon::TRASH, "Delete").clicked() {
                        delete = Some(j);
                    }
                    if icon_button(ui, icon::COPY, "Duplicate").clicked() {
                        duplicate = Some(j);
                    }
                    let (eye, tip) = if rep.visible {
                        (icon::EYE, "Hide")
                    } else {
                        (icon::EYE_SLASH, "Show")
                    };
                    if icon_button(ui, eye, tip).clicked() {
                        rep.visible = !rep.visible;
                        view_dirty = true;
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        if ui.selectable_label(selected, rep.summary()).clicked() {
                            new_sel_rep = Some(j);
                        }
                    });
                });
            });
        }
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

    /// Controls for the selected representation: selection text, style, params.
    /// Mutates rep dirty-flags; the viewport rebuilds geometry next.
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

        ui.label("Selection");
        let resp = ui.text_edit_singleline(&mut rep.sel_text);
        if resp.lost_focus() {
            rep.sel_dirty = true;
        }
        if let Some(err) = &rep.sel_error {
            ui.colored_label(egui::Color32::from_rgb(240, 120, 120), err);
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Style");
            egui::ComboBox::from_id_salt("rep_kind")
                .selected_text(rep.kind.label())
                .show_ui(ui, |ui| {
                    for kind in RepKind::ALL {
                        if ui.selectable_value(&mut rep.kind, kind, kind.label()).clicked() {
                            rep.params = RepParams::for_kind(kind);
                            rep.geom_dirty = true;
                        }
                    }
                });
        });

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
