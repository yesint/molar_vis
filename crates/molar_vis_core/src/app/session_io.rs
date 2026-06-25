//! Save molecules / selections / sessions; view-state seam; load-demo; new/reset doc.
use super::*;


/// Write a molecule (whole, `rep = None`) or one representation's selection
/// (`rep = Some(j)`) to `path` via molar, at the **currently displayed** frame.
/// Trajectory frames render by reference and aren't held in the `System`, so the
/// displayed `State` is swapped in around the write and restored afterwards. The
/// file format is chosen by molar from `path`'s extension. Native only (molar's
/// `FileHandler::create` writes to the filesystem).
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn save_displayed(
    mol: &mut scene::Molecule,
    path: &std::path::Path,
    rep: Option<usize>,
) -> Result<(), String> {
    let displayed = mol.render_state().clone();
    let prev = mol.system.set_state(displayed).map_err(|e| e.to_string())?;
    let res = (|| -> Result<(), String> {
        let mut h = FileHandler::create(path).map_err(|e| e.to_string())?;
        match rep {
            Some(j) => {
                let sel = mol.reps[j].sel.as_ref().ok_or("selection is empty")?;
                let bound = mol.system.bind(sel);
                h.write(&bound).map_err(|e| e.to_string())
            }
            None => h.write(&mol.system).map_err(|e| e.to_string()),
        }
    })();
    let _ = mol.system.set_state(prev); // restore the System's own state
    res
}
impl App {

    /// Tear down the current document: drop all molecules (and the trash), cancel
    /// in-flight trajectory loaders, and clear transient editing/dialog state.
    /// Shared by [`Self::new_session`] (start empty) and [`Self::apply_session`]
    /// (start empty, then reload from a file).
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn reset_document(&mut self) {
        self.scene.molecules.clear();
        self.scene.trash.clear();
        self.loaders.clear();
        self.editing_rep = None;
        self.load_dialog = None;
    }

    /// Start a new, empty visualization state: remove every molecule, reset the
    /// camera, and clear the undo history (a new document is its own baseline).
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn new_session(&mut self) {
        self.reset_document();
        self.scene.selected_mol = None;
        self.scene.clamp_selection();
        self.camera = Camera::default();
        self.settings.view.seed_camera(&mut self.camera);
        self.last_render_camera = None;
        self.history = History::new(EditState::capture(&self.scene));
        self.view_dirty = true;
        self.status = "New session".to_string();
    }

    /// Save molecule `i` to a structure file (rfd save dialog), at the currently
    /// displayed frame. Coordinates + topology of the whole molecule.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn save_molecule(&mut self, i: usize) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Structure", &["pdb", "gro", "xyz", "ent"])
            .set_file_name("molecule.pdb")
            .save_file()
        else {
            return;
        };
        self.status = match save_displayed(&mut self.scene.molecules[i], &path, None) {
            Ok(()) => format!("Saved molecule to {}", path.display()),
            Err(e) => {
                log::error!("save molecule: {e}");
                format!("Save failed: {e}")
            }
        };
    }

    /// Save representation `j` of molecule `mi`'s selection (just the selected
    /// atoms) to a structure file (rfd save dialog), at the displayed frame.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn save_rep_selection(&mut self, mi: usize, j: usize) {
        if self.scene.molecules[mi].reps[j].sel.is_none() {
            self.status = "Selection is empty — nothing to save".to_string();
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Structure", &["pdb", "gro", "xyz", "ent"])
            .set_file_name("selection.pdb")
            .save_file()
        else {
            return;
        };
        self.status = match save_displayed(&mut self.scene.molecules[mi], &path, Some(j)) {
            Ok(()) => format!("Saved selection to {}", path.display()),
            Err(e) => {
                log::error!("save selection: {e}");
                format!("Save failed: {e}")
            }
        };
    }

    /// The persistable global view state (camera + view-toolbar toggles). This and
    /// [`Self::apply_view_state`] are the **only** manual plumbing the save/load
    /// framework needs: a new persisted global setting is added to
    /// [`ViewState`](crate::session::ViewState) and read/written in these two
    /// functions. (Per-rep state needs no plumbing — it rides
    /// [`RepState`](crate::history::RepState).)
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn view_state(&self) -> ViewState {
        ViewState {
            camera: Some(self.camera),
            pick_mode: self.pick_mode,
            selection_mode: self.selection_mode,
            axes_on: self.axes_on,
            axes_corner: self.axes_corner,
        }
    }

    /// Restore the global view state captured by [`Self::view_state`].
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn apply_view_state(&mut self, view: ViewState) {
        if let Some(cam) = view.camera {
            self.camera = cam;
        }
        self.pick_mode = view.pick_mode;
        self.selection_mode = view.selection_mode;
        self.axes_on = view.axes_on;
        self.axes_corner = view.axes_corner;
    }

    /// Save the current visualization state to a JSON session file (rfd picker).
    /// Records molecule sources + the full rep document + global view state;
    /// molecule coordinates are *not* embedded (they are reloaded from disk).
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn save_session(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("molar_vis session", &["mvs", "json"])
            .set_file_name("session.mvs")
            .save_file()
        else {
            return;
        };
        self.save_session_to(&path);
    }

    /// Write the current state to `path` (the file half of [`Self::save_session`],
    /// also driven by the `MOLAR_VIS_DEBUG_SAVE_SESSION` verification hook).
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn save_session_to(&mut self, path: &std::path::Path) {
        let session = Session::capture(&self.scene, self.view_state());
        // Drawn molecules have no file source to reload from, so a session can't
        // restore them (it references molecules by source path). Warn so the user
        // knows to export them via "Save molecule…" instead.
        let drawn = self.scene.molecules.iter().filter(|m| m.editable).count();
        let result = session
            .to_json()
            .and_then(|json| std::fs::write(path, json).map_err(|e| e.to_string()));
        match result {
            Ok(()) if drawn > 0 => {
                self.status = format!(
                    "Saved session to {} — {drawn} drawn molecule(s) won't reload (use Save molecule… to export them)",
                    path.display()
                );
            }
            Ok(()) => self.status = format!("Saved session to {}", path.display()),
            Err(e) => {
                log::error!("save session: {e}");
                self.status = format!("Save failed: {e}");
            }
        }
    }

    /// Load a visualization state from a JSON session file (rfd picker), replacing
    /// the current scene (open-document semantics).
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn load_session(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("molar_vis session", &["mvs", "json"])
            .pick_file()
        else {
            return;
        };
        self.load_session_from(&path);
    }

    /// Read and apply a session file at `path` (the file half of
    /// [`Self::load_session`], also driven by `MOLAR_VIS_DEBUG_LOAD_SESSION`).
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn load_session_from(&mut self, path: &std::path::Path) {
        let json = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                self.status = format!("can't read {}: {e}", path.display());
                return;
            }
        };
        match Session::from_json(&json) {
            Ok(session) => self.apply_session(session),
            Err(e) => {
                log::error!("{e}");
                self.status = e;
            }
        }
    }

    /// Rebuild the scene from a parsed [`Session`]: reload each molecule from its
    /// source file, restore its representations / visibility / box / trajectory,
    /// then apply the global view state. Reloading a session is treated as opening
    /// a new document — the undo history is reset to the loaded state.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn apply_session(&mut self, session: Session) {
        // Replace the whole document.
        self.reset_document();

        let mut errors: Vec<String> = Vec::new();
        let mut loaded = 0usize;
        for ms in &session.molecules {
            let MoleculeSource::File(path) = &ms.source else {
                errors.push(format!(
                    "“{}” was loaded from memory (no file) — cannot reload",
                    ms.name
                ));
                continue;
            };
            let raw = match data::load_with(path, &self.settings.behavior.bond_params()) {
                Ok(r) => r,
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };
            self.scene.add(raw, &self.rep_defaults);
            let mol = self.scene.molecules.last_mut().unwrap();
            if !ms.name.is_empty() {
                mol.name = ms.name.clone(); // restore a custom (renamed) display name
            }
            mol.visible = ms.visible;
            mol.show_box = ms.show_box;
            mol.box_dirty = true;
            mol.reps = ms.build_reps(self.rep_defaults.kind);
            mol.selected_rep = (!mol.reps.is_empty()).then_some(0);

            // Replay trajectory loads (synchronous: a session load is a discrete
            // action and the frames are needed before the first render).
            if !ms.traj_loads.is_empty() {
                mol.seed_frame0();
                for tl in &ms.traj_loads {
                    let opts = LoadOptions {
                        from: tl.from,
                        to: tl.to,
                        stride: tl.stride.max(1),
                    };
                    match data::traj_loader::read_frames_sync(&tl.path, &opts, mol.n_atoms) {
                        Ok(frames) if !frames.is_empty() => {
                            mol.append_frames(frames);
                            mol.traj_loads.push(tl.clone());
                        }
                        Ok(_) => {} // recorded load now yields no frames — skip silently
                        Err(e) => errors.push(format!("trajectory {}: {e}", tl.path.display())),
                    }
                }
                mol.trajectory.set_current(ms.current_frame);
                mol.apply_current_frame();
            }
            loaded += 1;
        }

        self.scene.clamp_selection();
        self.scene.selected_mol = (!self.scene.molecules.is_empty()).then_some(0);
        self.apply_view_state(session.view);

        // Opening a document is a new baseline, not an undo step.
        self.history = History::new(EditState::capture(&self.scene));
        self.view_dirty = true;
        self.last_render_camera = None;

        self.status = if errors.is_empty() {
            format!("Loaded session: {loaded} molecule(s)")
        } else {
            for e in &errors {
                log::warn!("load session: {e}");
            }
            format!("Loaded {loaded} molecule(s); {} issue(s) — see log", errors.len())
        };
    }

    /// Load the small bundled structure (2lao) so the web/GitHub-Pages demo opens
    /// to a molecule instead of an empty viewport. Wasm only (embeds the file in
    /// the binary); the native app starts empty and loads via the Open button.
    #[cfg(target_arch = "wasm32")]
    pub fn load_demo(&mut self) {
        const DEMO_PDB: &[u8] = include_bytes!("../../../../tests/2lao.pdb");
        match data::load_from_bytes(
            "2lao.pdb",
            DEMO_PDB.to_vec(),
            &self.settings.behavior.bond_params(),
        ) {
            Ok(raw) => self.add_loaded(raw),
            Err(e) => log::error!("demo load failed: {e}"),
        }
    }
}
