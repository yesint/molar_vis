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
    let prev = mol.data.set_state(displayed).map_err(|e| e.to_string())?;
    let res = (|| -> Result<(), String> {
        let mut h = FileHandler::create(path).map_err(|e| e.to_string())?;
        match rep {
            Some(j) => {
                let sel = mol.reps[j].sel.as_ref().ok_or("selection is empty")?;
                // The whole-selection write needs `SaveTopologyState` (impl'd for the
                // System-coupled `SelBound`, not `SelBoundParts`), and saving is
                // owned-only, so bind through the owned `System`.
                let sys = mol.data.system().ok_or("cannot save a shared molecule directly")?;
                let bound = sys.bind(sel);
                h.write(&bound).map_err(|e| e.to_string())
            }
            None => h.write(mol.data.system().ok_or("cannot save a shared molecule directly")?).map_err(|e| e.to_string()),
        }
    })();
    let _ = mol.data.set_state(prev); // restore the System's own state
    res
}
impl App {

    /// Tear down the current document: drop all molecules (and the trash), cancel
    /// in-flight trajectory loaders, and clear transient editing/dialog state.
    /// Shared by [`Self::new_session`] (start empty) and [`Self::apply_session`]
    /// (start empty, then reload from a file).
    pub(super) fn reset_document(&mut self) {
        self.scene.molecules.clear();
        self.scene.groups.clear();
        self.scene.trash.clear();
        self.scene.group_trash.clear();
        self.loaders.clear();
        self.editing_rep = None;
        self.load_dialog = None;
        // A new document = a fresh REPL: drop console variables so a stored handle
        // (`let m = mol(0)`) doesn't outlive the molecule it referred to.
        self.script.reset();
    }

    /// Start a new, empty visualization state: remove every molecule, reset the
    /// camera, and clear the undo history (a new document is its own baseline).
    /// Pure in-memory (no filesystem), so it's available on wasm too — unlike
    /// session save/load, which reload molecules from disk.
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

    /// Save every member of group `gi` to a single file (rfd save dialog), each at its
    /// displayed frame. A `.sdf`/`.sd` writes a `$$$$`-delimited multi-record file (how a
    /// group is normally loaded); `.mol`/`.pdb`/… write the members concatenated per that
    /// format. Mirrors [`save_displayed`]'s frame-swap for each member.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn save_group(&mut self, gi: usize) {
        let (name, member_ids) = match self.scene.groups.get(gi) {
            Some(g) => (g.name.clone(), g.members.clone()),
            None => return,
        };
        // Default to the group's own file name (usually "<something>.sdf").
        let default = if name.is_empty() { "group.sdf".to_string() } else { name };
        let _ = member_ids;
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Multi-molecule", &["sdf", "sd", "mol", "pdb"])
            .set_file_name(&default)
            .save_file()
        else {
            return;
        };
        self.status = match self.save_group_to(gi, &path) {
            Ok(n) => format!("Saved {n} molecule(s) to {}", path.display()),
            Err(e) => {
                log::error!("save group: {e}");
                format!("Save failed: {e}")
            }
        };
    }

    /// Write every member of group `gi` to `path` as one multi-record file (the dialog-
    /// free core of [`save_group`], also driven by the `MOLAR_VIS_DEBUG_SAVE_GROUP` hook).
    /// Returns the number of members written.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn save_group_to(&mut self, gi: usize, path: &std::path::Path) -> Result<usize, String> {
        let member_ids = match self.scene.groups.get(gi) {
            Some(g) => g.members.clone(),
            None => return Err("no such group".to_string()),
        };
        let mut h = FileHandler::create(path).map_err(|e| e.to_string())?;
        let mut n = 0usize;
        for id in member_ids {
            let Some(mi) = self.scene.mol_index(id) else { continue };
            let mol = &mut self.scene.molecules[mi];
            // Swap the displayed frame into the System around the write, restore after
            // (frames render by reference, not held in the System) — as save_displayed.
            let displayed = mol.render_state().clone();
            let prev = mol.data.set_state(displayed).map_err(|e| e.to_string())?;
            let w = mol
                .data
                .system()
                .ok_or_else(|| "cannot save a shared molecule".to_string())
                .and_then(|sys| h.write(sys).map_err(|e| e.to_string()));
            let _ = mol.data.set_state(prev);
            w?;
            n += 1;
        }
        Ok(n)
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
    ///
    /// A session references molecules by source **path**. A File-source molecule that
    /// was **structurally edited** (dihedral twist, draw/erase, cleanup — detected via
    /// `structure_version`) is first written out as `<stem>.edited.<ext>` next to the
    /// session file, and the session is pointed at that copy, so the edits reload (the
    /// original file is left untouched). A molecule with no reloadable file source
    /// (hand-drawn / in-memory bytes) still can't be restored — the user is warned.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn save_session_to(&mut self, path: &std::path::Path) {
        // Where the `*.edited.*` copies go: next to the session (falls back to the
        // original's own directory if the session has no parent).
        let session_dir = path.parent().map(|p| p.to_path_buf());

        // Collect the edited File-source standalone molecules + their target paths
        // (immutable pass), then write each (mutable pass). Group members reload from
        // their multi-record SDF by index, so per-member edited copies aren't wired up.
        let targets: Vec<(usize, MolId, std::path::PathBuf)> = self
            .scene
            .molecules
            .iter()
            .enumerate()
            .filter_map(|(i, m)| {
                if m.group.is_some() || m.structure_version == 0 {
                    return None; // grouped, or never structurally edited → original reloads it
                }
                let MoleculeSource::File(orig) = &m.source else {
                    return None; // no reloadable file source
                };
                let dir = session_dir
                    .clone()
                    .or_else(|| orig.parent().map(|p| p.to_path_buf()))?;
                let raw_stem = orig.file_stem()?.to_string_lossy().into_owned();
                // Strip a prior ".edited" so re-saving doesn't pile up ".edited.edited".
                let stem = raw_stem.strip_suffix(".edited").unwrap_or(&raw_stem);
                let ext = orig
                    .extension()
                    .map(|e| e.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "pdb".to_string());
                Some((i, m.id, dir.join(format!("{stem}.edited.{ext}"))))
            })
            .collect();

        let mut edited: std::collections::HashMap<MolId, std::path::PathBuf> = Default::default();
        let mut edited_failed = 0usize;
        for (i, id, ep) in targets {
            match save_displayed(&mut self.scene.molecules[i], &ep, None) {
                Ok(()) => {
                    edited.insert(id, ep);
                }
                Err(e) => {
                    log::error!("save edited molecule: {e}");
                    edited_failed += 1;
                }
            }
        }

        let mut session = Session::capture(&self.scene, self.view_state());
        // Rewire each saved edited molecule to its `*.edited.*` copy. `session.molecules`
        // is captured in the same order as the non-grouped molecules, so zip by that.
        let ids: Vec<MolId> = self
            .scene
            .molecules
            .iter()
            .filter(|m| m.group.is_none())
            .map(|m| m.id)
            .collect();
        for (ms, id) in session.molecules.iter_mut().zip(ids) {
            if let Some(ep) = edited.get(&id) {
                ms.source = MoleculeSource::File(ep.clone());
            }
        }

        // Molecules that still can't be restored: no file source, or an edited write
        // that failed (those revert to the original on reload).
        let unreloadable = self
            .scene
            .molecules
            .iter()
            .filter(|m| m.group.is_none() && !matches!(m.source, MoleculeSource::File(_)))
            .count();

        let result = session
            .to_json()
            .and_then(|json| std::fs::write(path, json).map_err(|e| e.to_string()));
        match result {
            Ok(()) => {
                let mut msg = format!("Saved session to {}", path.display());
                if !edited.is_empty() {
                    msg.push_str(&format!(
                        " — {} edited molecule(s) saved as *.edited.*",
                        edited.len()
                    ));
                }
                if unreloadable > 0 {
                    msg.push_str(&format!(
                        "; {unreloadable} molecule(s) won't reload (use Save molecule… to export them)"
                    ));
                }
                if edited_failed > 0 {
                    msg.push_str(&format!(
                        "; {edited_failed} edited molecule(s) couldn't be written (revert to original)"
                    ));
                }
                self.status = msg;
            }
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

        // Reconstruct molecular groups: re-open each group's source file, pull out the
        // recorded members by record index, restore their own reps + the shared reps.
        for gs in &session.groups {
            let MoleculeSource::File(path) = &gs.source else {
                errors.push(format!("group “{}” was loaded from memory — cannot reload", gs.name));
                continue;
            };
            let records = match data::load_records(path, &self.settings.behavior.bond_params()) {
                Ok(r) => r,
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };
            // Take records out by index (RawMolecule isn't Clone, members are distinct).
            let mut records: Vec<Option<data::RawMolecule>> = records.into_iter().map(Some).collect();
            let gid = self.scene.alloc_group_id();
            let mut members = Vec::with_capacity(gs.members.len());
            for ms in &gs.members {
                let Some(mut raw) = records.get_mut(ms.record_index).and_then(|o| o.take()) else {
                    errors.push(format!(
                        "group “{}”: record {} missing in {}",
                        gs.name,
                        ms.record_index,
                        path.display()
                    ));
                    continue;
                };
                raw.source = MoleculeSource::SdfRecord { path: path.clone(), index: ms.record_index };
                let id = self.scene.add(raw, &self.rep_defaults);
                let mi = self.scene.mol_index(id).unwrap();
                let mol = &mut self.scene.molecules[mi];
                if !ms.name.is_empty() {
                    mol.name = ms.name.clone();
                }
                mol.group = Some(gid);
                mol.visible = false;
                mol.n_shared = 0;
                mol.reps = ms.reps.iter().map(|r| r.to_representation()).collect();
                mol.selected_rep = (!mol.reps.is_empty()).then_some(0);
                members.push(id);
                loaded += 1;
            }
            if members.is_empty() {
                continue;
            }
            let current = gs.current.min(members.len() - 1);
            // Materialize the shared reps onto the shown member.
            if let Some(mi) = self.scene.mol_index(members[current]) {
                let live: Vec<Representation> =
                    gs.shared_reps.iter().map(|r| r.to_representation()).collect();
                let n = live.len();
                let mol = &mut self.scene.molecules[mi];
                mol.reps.splice(0..0, live);
                mol.n_shared = n;
                mol.selected_rep = Some(0);
            }
            let name = if gs.name.is_empty() {
                path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
            } else {
                gs.name.clone()
            };
            self.scene.groups.push(MolGroup {
                id: gid,
                name,
                source: gs.source.clone(),
                members,
                current,
                visible: gs.visible,
                expanded: false,
                members_expanded: false,
            });
            let gi = self.scene.groups.len() - 1;
            self.scene.apply_group_visibility(gi);
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
