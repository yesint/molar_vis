//! The eframe application: owns UI state, the camera, the loaded molecule, the
//! active representation, and the 3D scene renderer; lays out the VMD-style left
//! control panel plus the central 3D viewport.

use eframe::egui;

use crate::camera::{Camera, Projection};
use crate::data::{self, LoadedMolecule};
use crate::geometry::{self, RepKind, RepParams};
use crate::launch::AppLaunch;
use crate::render::SceneRenderer;

pub struct App {
    renderer: SceneRenderer,
    camera: Camera,
    molecule: Option<LoadedMolecule>,
    rep: RepKind,
    params: RepParams,
    /// Set when rep/params change; the viewport rebuilds + reuploads geometry.
    geom_dirty: bool,
    status: String,
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

        let mut renderer = SceneRenderer::new(render_state);
        let mut camera = Camera::default();
        let mut molecule = None;
        // Verification hook: VMD_RS_DEBUG_REP=licorice|ballstick|lines|vdw picks
        // the initial representation so each can be screenshotted headlessly.
        let rep = std::env::var("VMD_RS_DEBUG_REP")
            .ok()
            .and_then(|s| RepKind::from_name(&s))
            .unwrap_or(RepKind::Vdw);
        let params = RepParams::for_kind(rep);
        let mut status = "No molecule loaded.".to_string();

        // M3 loads the first file from the command line; multi-molecule is M4.
        if let Some(path) = launch.files.first() {
            if launch.files.len() > 1 {
                log::warn!(
                    "{} files given; loading only the first (multi-molecule is M4)",
                    launch.files.len()
                );
            }
            match data::load(path) {
                Ok(m) => {
                    camera = Camera::frame_bbox(m.bbox_min, m.bbox_max);
                    // Verification hook: VMD_RS_DEBUG_ORBIT=<deg> applies a startup
                    // rotation so a screenshot proves the orbit→view→GPU path.
                    if let Ok(deg) = std::env::var("VMD_RS_DEBUG_ORBIT") {
                        if let Ok(d) = deg.parse::<f32>() {
                            camera.orbit(d, d * 0.4);
                        }
                    }
                    let geom = geometry::build(&m, rep, &params);
                    renderer.set_geometry(render_state, &geom);
                    status = format!("{} — {} atoms", m.name, m.n_atoms);
                    molecule = Some(m);
                }
                Err(e) => {
                    log::error!("{e}");
                    status = e;
                }
            }
        }

        // Verification hook: VMD_RS_DEBUG_ORTHO=1 starts in orthographic mode.
        if std::env::var("VMD_RS_DEBUG_ORTHO").is_ok() {
            camera.projection = Projection::Orthographic;
        }

        Ok(Self {
            renderer,
            camera,
            molecule,
            rep,
            params,
            geom_dirty: false,
            status,
        })
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // Drive continuous repaint so navigation is smooth and the FPS readout is
        // meaningful. M4 will gate the 3D pass on a scene-dirty flag.
        ui.ctx().request_repaint();
        self.draw_left_panel(ui);
        self.draw_viewport(ui, frame);
    }
}

impl App {
    /// Left panel, top to bottom: molecules → representations → rep controls.
    fn draw_left_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("controls_panel")
            .resizable(true)
            .default_size(300.0)
            .size_range(egui::Rangef::new(220.0, 520.0))
            .show_inside(ui, |ui| {
                ui.add_space(8.0);

                egui::CollapsingHeader::new("Scene")
                    .default_open(true)
                    .show(ui, |ui| self.draw_scene_controls(ui));

                ui.separator();
                egui::CollapsingHeader::new("Molecules")
                    .default_open(true)
                    .show(ui, |ui| match &self.molecule {
                        Some(m) => {
                            ui.horizontal(|ui| {
                                ui.label(&m.name);
                                ui.weak(format!("({} atoms)", m.n_atoms));
                            });
                        }
                        None => {
                            ui.weak(&self.status);
                        }
                    });

                ui.separator();
                egui::CollapsingHeader::new("Representations")
                    .default_open(true)
                    .show(ui, |ui| self.draw_rep_selector(ui));

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
    }

    /// Global scene options. Projection now; lighting/background/etc. later.
    fn draw_scene_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Projection");
            ui.radio_value(
                &mut self.camera.projection,
                Projection::Perspective,
                "Perspective",
            );
            ui.radio_value(
                &mut self.camera.projection,
                Projection::Orthographic,
                "Orthographic",
            );
        });
    }

    fn draw_rep_selector(&mut self, ui: &mut egui::Ui) {
        if self.molecule.is_none() {
            ui.weak("Load a molecule to choose a representation.");
            return;
        }
        ui.horizontal(|ui| {
            ui.label("Style");
            egui::ComboBox::from_id_salt("rep_kind")
                .selected_text(self.rep.label())
                .show_ui(ui, |ui| {
                    for kind in RepKind::ALL {
                        if ui
                            .selectable_value(&mut self.rep, kind, kind.label())
                            .clicked()
                        {
                            self.params = RepParams::for_kind(kind);
                            self.geom_dirty = true;
                        }
                    }
                });
        });
    }

    fn draw_rep_controls(&mut self, ui: &mut egui::Ui) {
        if self.molecule.is_none() {
            ui.weak("—");
            return;
        }
        match self.rep {
            RepKind::Vdw => {
                ui.weak("Spheres at van der Waals radius.");
            }
            RepKind::Lines => {
                ui.weak("Half-bond colored lines (1 px).");
            }
            RepKind::Licorice => {
                if ui
                    .add(
                        egui::Slider::new(&mut self.params.bond_radius, 0.005..=0.10)
                            .text("bond radius (nm)"),
                    )
                    .changed()
                {
                    self.geom_dirty = true;
                }
            }
            RepKind::BallAndStick => {
                let mut changed = ui
                    .add(
                        egui::Slider::new(&mut self.params.sphere_scale, 0.05..=0.6)
                            .text("sphere scale"),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.params.bond_radius, 0.005..=0.05)
                            .text("bond radius (nm)"),
                    )
                    .changed();
                if changed {
                    self.geom_dirty = true;
                }
            }
        }
    }

    /// Central panel: route VMD-style mouse navigation, rebuild geometry if the
    /// representation changed, render the 3D scene, and blit it as an egui image.
    fn draw_viewport(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            let render_state = frame
                .wgpu_render_state()
                .expect("wgpu render state must exist");

            // Rebuild + reupload geometry when the rep/params changed.
            if self.geom_dirty {
                if let Some(mol) = self.molecule.as_ref() {
                    let geom = geometry::build(mol, self.rep, &self.params);
                    self.renderer.set_geometry(render_state, &geom);
                }
                self.geom_dirty = false;
            }

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
            let aspect = size_px[0] as f32 / size_px[1] as f32;

            let view = self.camera.view();
            let proj = self.camera.proj(aspect);
            let texture_id = self.renderer.render(
                render_state,
                size_px,
                view,
                proj,
                self.camera.is_perspective(),
            );

            egui::Image::new(egui::load::SizedTexture::new(texture_id, rect.size()))
                .paint_at(ui, rect);
        });
    }
}
