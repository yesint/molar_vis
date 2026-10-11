//! Mesh import/export and the molecule's mesh rows, below its selections.
use super::widgets::*;
use super::*;
use crate::mesh::{export_ply, import_ply, Mesh, MeshStyle};

impl App {
    pub(super) fn add_mesh_bytes(&mut self, id: MolId, name: String, bytes: &[u8]) {
        let Some(mi) = self.scene.mol_index(id) else {
            return;
        };
        match import_ply(bytes) {
            Ok(source) => {
                let mesh = Mesh::new(name.clone(), source);
                self.scene.molecules[mi].meshes.push(mesh);
                self.scene.molecules[mi].reps_open = true;
                self.status = format!("Loaded mesh {name}");
                self.view_dirty = true;
            }
            Err(e) => self.status = format!("Could not load mesh {name}: {e}"),
        }
    }

    pub(super) fn load_mesh(&mut self, id: MolId, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = ctx;
            let Some(path) = rfd::FileDialog::new()
                .add_filter("PLY mesh", &["ply"])
                .pick_file()
            else {
                return;
            };
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            match std::fs::read(&path) {
                Ok(bytes) => self.add_mesh_bytes(id, name, &bytes),
                Err(e) => self.status = format!("Could not read mesh: {e}"),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let tx = self.mesh_tx.clone();
            super::loaders::pick_file(".ply", ctx.clone(), move |name, bytes| {
                let _ = tx.send((id, name, bytes));
            });
        }
    }

    fn save_mesh(&mut self, mi: usize, index: usize) {
        let mesh = &self.scene.molecules[mi].meshes[index];
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("PLY mesh", &["ply"])
                .set_file_name(&mesh.state.name)
                .save_file()
            else {
                return;
            };
            match std::fs::write(&path, export_ply(&mesh.state.source)) {
                Ok(()) => self.status = format!("Saved mesh to {}", path.display()),
                Err(e) => self.status = format!("Could not save mesh: {e}"),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            super::export::trigger_download(&mesh.state.name, &export_ply(&mesh.state.source));
            self.status = format!("Downloaded {}", mesh.state.name);
        }
    }

    pub(super) fn draw_meshes_for(&mut self, ui: &mut egui::Ui, mi: usize) -> bool {
        let mut changed = false;
        let mut delete = None;
        let mut save = None;
        let mol = &mut self.scene.molecules[mi];
        for (i, mesh) in mol.meshes.iter_mut().enumerate() {
            ui.push_id(("mesh", mol.id.0, i), |ui| {
                ui.horizontal(|ui| {
                    ui.label(icon::CUBE_TRANSPARENT).on_hover_text("Mesh");
                    let spacing = ui.spacing().clone();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        compact_actions(ui);
                        ui.menu_button(icon::LIST, |ui| {
                            if ui.button(format!("{}  Save…", icon::FLOPPY_DISK)).clicked() {
                                save = Some(i);
                                ui.close();
                            }
                            if ui.button(format!("{}  Delete", icon::TRASH)).clicked() {
                                delete = Some(i);
                                ui.close();
                            }
                        })
                        .response
                        .on_hover_text("Mesh menu");
                        let eye = if mesh.state.visible {
                            icon::EYE
                        } else {
                            icon::EYE_SLASH
                        };
                        if ui
                            .selectable_label(mesh.state.visible, eye)
                            .on_hover_text(if mesh.state.visible {
                                "Hide mesh"
                            } else {
                                "Show mesh"
                            })
                            .clicked()
                        {
                            mesh.state.visible = !mesh.state.visible;
                            changed = true;
                        }
                        row_rest(ui, &spacing, |ui| {
                            truncated(ui, egui::Label::new(&mesh.state.name), 0.0).on_hover_text(
                                format!(
                                    "{}\n{} vertices / {} triangles\nCoordinates in nanometers",
                                    mesh.state.name,
                                    mesh.state.source.vertices.len(),
                                    mesh.state.source.indices.len() / 3
                                ),
                            );
                        });
                    });
                });
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    let old_style = mesh.state.style;
                    let label_width = max_label_width(ui, MeshStyle::ALL.into_iter().map(MeshStyle::label));
                    let icon_kind = match mesh.state.style {
                        MeshStyle::Points => RepKind::Vdw,
                        MeshStyle::Wireframe => RepKind::Lines,
                        MeshStyle::Surface => RepKind::Surface,
                    };
                    let color = ui.visuals().text_color();
                    let response = picker_button(ui, mesh.state.style.label(), label_width, |p, rect| {
                        super::pickers::paint_style_icon(p, rect, icon_kind, color);
                    });
                    egui::Popup::menu(&response).show(|ui| {
                        for style in MeshStyle::ALL {
                            if ui.selectable_value(&mut mesh.state.style, style, style.label()).clicked() {
                                ui.close();
                            }
                        }
                    });
                    if old_style != mesh.state.style { mesh.dirty = true; changed = true; }
                    let mut selected = mesh.state.material;
                    if super::pickers::material_selector(ui, &mut selected, &self.rep_defaults) {
                        mesh.state.material = self.rep_defaults.material_for(selected);
                        mesh.dirty = true;
                        changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(22.0);
                    let edited = match mesh.state.style {
                        MeshStyle::Points => {
                            ui.label("Point size (px)");
                            ui.add(egui::Slider::new(&mut mesh.state.point_size, 1.0..=10.0)).changed()
                        }
                        MeshStyle::Wireframe => {
                            ui.label("Line width (px)");
                            ui.add(egui::Slider::new(&mut mesh.state.line_width, 1.0..=10.0)).changed()
                        }
                        MeshStyle::Surface => {
                            ui.label("Smoothing");
                            ui.add(egui::Slider::new(&mut mesh.state.smoothing, 0..=5))
                                .on_hover_text("Smooth the surface geometry; 0 preserves the imported positions and normals")
                                .changed()
                        }
                    };
                    if edited { mesh.dirty = true; changed = true; }
                });
            });
        }
        if let Some(i) = save {
            self.save_mesh(mi, i);
        }
        if let Some(i) = delete {
            self.scene.molecules[mi].meshes.remove(i);
            changed = true;
        }
        changed
    }
}
