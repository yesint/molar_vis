//! Mesh import/export and the molecule's mesh rows, below its selections.
use super::widgets::*;
use super::*;
use crate::mesh::{Mesh, MeshStyle, export_ply, import_ply};

impl App {
    pub fn mesh_ids(&self, mol: usize) -> Result<Vec<u64>, String> {
        Ok(self
            .scene
            .molecules
            .get(mol)
            .ok_or_else(|| format!("no molecule {mol}"))?
            .meshes
            .iter()
            .map(|mesh| mesh.id)
            .collect())
    }

    pub fn mesh_state(&self, mol: usize, id: u64) -> Result<crate::mesh::MeshState, String> {
        self.scene
            .molecules
            .iter()
            .flat_map(|m| &m.meshes)
            .find(|m| m.id == id)
            .map(|m| m.state.clone())
            .ok_or_else(|| format!("no mesh {id} on molecule {mol}"))
    }

    pub fn mesh_details_json(&self, mol: usize, id: u64) -> Result<String, String> {
        let s = self.mesh_state(mol, id)?;
        Ok(serde_json::json!({
            "name": s.name, "visible": s.visible,
            "material": s.material.label(),
            "style": s.style.label().to_ascii_lowercase(),
            "point_size": s.point_size, "line_width": s.line_width,
            "smoothing": s.smoothing,
            "vertices": s.source.vertices.len(), "triangles": s.source.indices.len() / 3
        })
        .to_string())
    }
    /// Import a PLY mesh into a molecule. `id` is reserved by the external host so
    /// handles remain valid when other meshes are removed.
    pub fn add_mesh_ply(
        &mut self,
        mol: usize,
        id: u64,
        name: String,
        bytes: &[u8],
    ) -> Result<(), String> {
        let source = import_ply(bytes)?;
        if self
            .scene
            .molecules
            .iter()
            .any(|m| m.meshes.iter().any(|mesh| mesh.id == id))
        {
            return Err(format!("mesh ID {id} already exists"));
        }
        let m = self
            .scene
            .molecules
            .get_mut(mol)
            .ok_or_else(|| format!("no molecule {mol}"))?;
        let mut mesh = Mesh::new(name, source);
        mesh.id = id;
        m.meshes.push(mesh);
        m.reps_open = true;
        self.view_dirty = true;
        Ok(())
    }

    fn external_mesh(&mut self, mol: usize, id: u64) -> Result<&mut Mesh, String> {
        self.scene
            .molecules
            .iter_mut()
            .flat_map(|m| &mut m.meshes)
            .find(|m| m.id == id)
            .ok_or_else(|| format!("no mesh {id} on molecule {mol}"))
    }

    pub fn set_mesh_visible(&mut self, mol: usize, id: u64, visible: bool) -> Result<(), String> {
        self.external_mesh(mol, id)?.state.visible = visible;
        self.view_dirty = true;
        Ok(())
    }

    pub fn set_mesh_material(&mut self, mol: usize, id: u64, name: &str) -> Result<(), String> {
        let preset = crate::script::command::parse_material(name)
            .ok_or_else(|| format!("unknown material: {name}"))?;
        let material = self.rep_defaults.material_for(preset);
        let mesh = self.external_mesh(mol, id)?;
        mesh.state.material = material;
        mesh.dirty = true;
        self.view_dirty = true;
        Ok(())
    }

    pub fn set_mesh_style(&mut self, mol: usize, id: u64, name: &str) -> Result<(), String> {
        let style = match name.to_ascii_lowercase().as_str() {
            "points" => MeshStyle::Points,
            "wireframe" => MeshStyle::Wireframe,
            "surface" => MeshStyle::Surface,
            _ => return Err(format!("unknown mesh style: {name}")),
        };
        let mesh = self.external_mesh(mol, id)?;
        mesh.state.style = style;
        mesh.dirty = true;
        self.view_dirty = true;
        Ok(())
    }

    pub fn set_mesh_point_size(&mut self, mol: usize, id: u64, size: f32) -> Result<(), String> {
        if !size.is_finite() || !(1.0..=10.0).contains(&size) {
            return Err("point size must be 1..=10 pixels".into());
        }
        let mesh = self.external_mesh(mol, id)?;
        mesh.state.point_size = size;
        mesh.dirty = true;
        self.view_dirty = true;
        Ok(())
    }

    pub fn set_mesh_line_width(&mut self, mol: usize, id: u64, width: f32) -> Result<(), String> {
        if !width.is_finite() || !(1.0..=10.0).contains(&width) {
            return Err("line width must be 1..=10 pixels".into());
        }
        let mesh = self.external_mesh(mol, id)?;
        mesh.state.line_width = width;
        mesh.dirty = true;
        self.view_dirty = true;
        Ok(())
    }

    pub fn set_mesh_smoothing(&mut self, mol: usize, id: u64, steps: u32) -> Result<(), String> {
        if steps > 5 {
            return Err("smoothing must be 0..=5".into());
        }
        let mesh = self.external_mesh(mol, id)?;
        mesh.state.smoothing = steps;
        mesh.dirty = true;
        self.view_dirty = true;
        Ok(())
    }

    pub fn mesh_ply(&mut self, mol: usize, id: u64) -> Result<Vec<u8>, String> {
        Ok(export_ply(&self.external_mesh(mol, id)?.state.source))
    }

    pub fn delete_mesh(&mut self, mol: usize, id: u64) -> Result<(), String> {
        let meshes = &mut self
            .scene
            .molecules
            .iter_mut()
            .find(|m| m.meshes.iter().any(|mesh| mesh.id == id))
            .ok_or_else(|| format!("no mesh {id} on molecule {mol}"))?
            .meshes;
        let index = meshes.iter().position(|m| m.id == id).unwrap();
        meshes.remove(index);
        self.view_dirty = true;
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
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
            self.begin_loading(move || {
                let bytes = std::fs::read(&path).map_err(|e| format!("Could not read mesh: {e}", e = crate::errors::error_chain(&e)))?;
                import_ply(&bytes)
            }, move |app, source| {
                if let Some(mi) = app.scene.mol_index(id) {
                    app.scene.molecules[mi].meshes.push(Mesh::new(name.clone(), source));
                    app.scene.molecules[mi].reps_open = true;
                    app.status = format!("Loaded mesh {name}");
                    app.view_dirty = true;
                }
            });
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
                Err(e) => self.status = format!("Could not save mesh: {}", crate::errors::error_chain(&e)),
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
                    let icon_kind = match mesh.state.style {
                        MeshStyle::Points => RepKind::Vdw,
                        MeshStyle::Wireframe => RepKind::Lines,
                        MeshStyle::Surface => RepKind::Surface,
                    };
                    let color = ui.visuals().text_color();
                    let label_width = max_label_width(ui, MeshStyle::ALL.into_iter().map(MeshStyle::label));
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
