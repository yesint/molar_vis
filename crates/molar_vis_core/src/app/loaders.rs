//! Trajectory/structure load + delete-frames + rename dialogs, loaders, file picker.
use super::*;
use super::draw::*;
use super::widgets::*;


/// State of the "Load trajectory" modal.
pub(super) struct LoadDialog {
    mol_id: MolId,
    path: Option<PathBuf>,
    from: usize,
    /// Last frame to read, as text. **Empty = read to the end of the file.**
    to_text: String,
    stride: usize,
    mode: LoadMode,
    error: Option<String>,
}

impl LoadDialog {
    pub(super) fn new(mol_id: MolId) -> Self {
        Self {
            mol_id,
            path: None,
            from: 0,
            to_text: String::new(),
            stride: 1,
            mode: LoadMode::Sync,
            error: None,
        }
    }
}

/// Outcome of drawing the load dialog this frame.
pub(super) enum DialogAction {
    Keep,
    Cancel,
    Load,
}

/// How the "Delete frames" dialog selects which frames to drop.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DeleteFramesMode {
    /// Delete the inclusive frame range `[from, to]`.
    Range,
    /// Keep every `stride`-th frame, drop the rest.
    Decimate,
}

/// State of the "Delete frames" modal (trajectory frame deletion).
pub(super) struct DeleteFramesDialog {
    mol_id: MolId,
    mode: DeleteFramesMode,
    from: usize,
    to: usize,
    stride: usize,
}

impl DeleteFramesDialog {
    pub(super) fn new(mol_id: MolId) -> Self {
        Self { mol_id, mode: DeleteFramesMode::Range, from: 0, to: 0, stride: 2 }
    }
}

/// Browser file open: create a hidden `<input type=file>` (limited to `accept`),
/// click it, and when the user picks a file read it (`Blob::array_buffer`) into a
/// `Vec<u8>` and hand `(name, bytes)` to `deliver`, then request a repaint so the
/// app processes it. Async (the dialog + the read), so this returns immediately;
/// `deliver` runs later on the main thread. Used by both the structure-open and
/// trajectory-load buttons (the latter's `deliver` tags the bytes with a molecule).
#[cfg(target_arch = "wasm32")]
pub(super) fn pick_file(accept: &str, ctx: egui::Context, deliver: impl Fn(String, Vec<u8>) + Clone + 'static) {
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast as _;

    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Ok(input) = document
        .create_element("input")
        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().map_err(|_| wasm_bindgen::JsValue::NULL.into()))
    else {
        return;
    };
    input.set_type("file");
    input.set_accept(accept);

    let input_for_cb = input.clone();
    let on_change = Closure::<dyn FnMut()>::new(move || {
        let Some(file) = input_for_cb.files().and_then(|f| f.get(0)) else {
            return;
        };
        let name = file.name();
        let deliver = deliver.clone();
        let ctx = ctx.clone();
        // Read the Blob asynchronously, then hand the bytes to the app.
        wasm_bindgen_futures::spawn_local(async move {
            match wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await {
                Ok(buf) => {
                    let bytes = js_sys::Uint8Array::new(&buf).to_vec();
                    deliver(name, bytes);
                    ctx.request_repaint();
                }
                Err(e) => log::error!("failed to read file: {e:?}"),
            }
        });
    });
    input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    // The closure must outlive this call (it fires later); leak it deliberately.
    on_change.forget();
    input.click();
}
impl App {

    /// Open molecule `i` in the drawing editor: flag it `editable` (so its structure
    /// is snapshotted for undo) and start a Draw session targeting it. Mutually
    /// exclusive with picking.
    pub(super) fn open_in_editor(&mut self, i: usize) {
        let Some(mol) = self.scene.molecules.get_mut(i) else { return };
        mol.editable = true;
        let id = mol.id;
        self.pick_mode = PickMode::Off;
        let mut session = self.draw.take().unwrap_or_default();
        session.target = Some(id);
        session.drag = DrawDrag::Idle;
        self.draw = Some(session);
    }

    /// Open a structure file as a new molecule. Native: a synchronous `rfd` file
    /// picker → [`data::load`]. Browser: an async `<input type=file>` whose bytes
    /// come back through `file_rx` and are loaded in [`Self::ui`] via
    /// [`data::load_from_bytes`]. Only topology+coordinate formats can seed a
    /// molecule. The add is undoable (end-of-frame history checkpoint).
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
    pub(super) fn open_structure(&mut self, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Structures", &["pdb", "ent", "gro", "xyz", "tpr", "sdf", "mol"])
                .pick_file()
            else {
                return;
            };
            self.open_structure_path(&path);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let tx = self.file_tx.clone();
            pick_file(
                ".pdb,.ent,.gro,.xyz,.dcd,.trr,.xtc,.sdf,.mol",
                ctx.clone(),
                move |name, bytes| {
                    let _ = tx.send((name, bytes));
                },
            );
        }
    }

    /// Load a structure file from disk (native). A multi-molecule SDF/MOL (≥2 `$$$$`
    /// records) becomes a [`MolGroup`]; anything else (incl. a single-record SDF) is
    /// one ordinary molecule. Shared by the Open dialog and the startup path.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn open_structure_path(&mut self, path: &std::path::Path) {
        let bonds = self.settings.behavior.bond_params();
        let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
        if matches!(ext.as_deref(), Some("sdf") | Some("mol")) {
            match data::load_records(path, &bonds) {
                Ok(records) if records.len() >= 2 => {
                    let name = path
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "group".to_string());
                    self.add_group(records, MoleculeSource::File(path.to_path_buf()), name);
                }
                Ok(mut records) => {
                    // A single-record SDF is just one molecule; reload it like any
                    // file (source = File, so the session round-trips it normally).
                    let mut raw = records.pop().unwrap();
                    raw.source = MoleculeSource::File(path.to_path_buf());
                    self.add_loaded(raw);
                }
                Err(e) => {
                    log::error!("{e}");
                    self.status = e;
                }
            }
        } else {
            match data::load_with(path, &bonds) {
                Ok(raw) => self.add_loaded(raw),
                Err(e) => {
                    log::error!("{e}");
                    self.status = e;
                }
            }
        }
    }

    /// Add a freshly loaded multi-record file as a [`MolGroup`]: each record becomes a
    /// hidden member molecule, the group gets one default **Licorice** shared rep
    /// materialized onto the first (shown) member, and the camera frames that member
    /// if the scene was empty. The add is undoable (end-of-frame checkpoint).
    pub(super) fn add_group(
        &mut self,
        records: Vec<data::RawMolecule>,
        source: MoleculeSource,
        name: String,
    ) {
        if records.is_empty() {
            return;
        }
        let was_empty = self.scene.molecules.is_empty();
        let rep_defaults = self.rep_defaults.clone();
        let n = records.len();
        let Some(first_member) = self.scene.add_group(records, source, name, &rep_defaults) else {
            return;
        };
        self.scene.selected_mol = self.scene.mol_index(first_member);
        if was_empty {
            if let Some(mi) = self.scene.mol_index(first_member) {
                let (min, max) = (self.scene.molecules[mi].bbox_min, self.scene.molecules[mi].bbox_max);
                self.camera = Camera::frame_bbox(min, max, self.settings.view.fill);
                self.settings.view.seed_camera(&mut self.camera);
            }
        }
        self.status = format!("Loaded group ({n} molecules)");
        self.view_dirty = true;
    }

    /// Add a freshly loaded structure as a new molecule: select it, frame the
    /// camera if it's the first one, and flag a re-render. Shared by the native
    /// picker and the browser byte-loader.
    pub(super) fn add_loaded(&mut self, raw: data::RawMolecule) {
        let was_empty = self.scene.molecules.is_empty();
        let rep_defaults = self.rep_defaults.clone();
        self.scene.add(raw, &rep_defaults);
        self.scene.selected_mol = Some(self.scene.molecules.len() - 1);
        if let Some(mol) = self.scene.molecules.last_mut() {
            mol.trajectory.speed_fps = self.settings.behavior.traj_fps;
            mol.trajectory.loop_mode = self.settings.behavior.loop_mode;
        }
        if was_empty {
            // First molecule into an empty scene: frame it and seed the user's
            // default view (projection / background / depth-cue / …).
            if let Some((min, max)) = self.scene.bbox() {
                self.camera = Camera::frame_bbox(min, max, self.settings.view.fill);
                self.settings.view.seed_camera(&mut self.camera);
            }
        }
        self.status = format!("{} molecule(s) loaded", self.scene.molecules.len());
        self.view_dirty = true;
    }

    /// Render the "Delete frames" modal: pick a frame range to drop or a decimate
    /// stride (keep every Nth frame), with Delete/Cancel. Trajectory frames are
    /// view state, so this is not undoable (like loading frames).
    pub(super) fn draw_delete_frames_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.delete_frames_dialog.take() else {
            return;
        };
        let n_frames = self
            .scene
            .molecules
            .iter()
            .find(|m| m.id == dialog.mol_id)
            .map(|m| m.trajectory.n_frames());
        let last = n_frames.unwrap_or(0).saturating_sub(1);
        let mut do_delete = false;
        let mut close = false;

        let modal = egui::Modal::new(egui::Id::new("del_frames_modal")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.heading("Delete trajectory frames");
            match n_frames {
                Some(nf) => {
                    ui.label(format!("{nf} frames loaded (indices 0..{last})"));
                }
                None => {
                    ui.colored_label(
                        egui::Color32::from_rgb(240, 120, 120),
                        "molecule no longer exists",
                    );
                }
            }
            ui.separator();
            tab_bar(
                ui,
                &mut dialog.mode,
                &[
                    (DeleteFramesMode::Range, "Range"),
                    (DeleteFramesMode::Decimate, "Decimate"),
                ],
            );
            ui.add_space(4.0);
            match dialog.mode {
                DeleteFramesMode::Range => {
                    egui::Grid::new("del_range_opts")
                        .num_columns(2)
                        .spacing(egui::vec2(8.0, 4.0))
                        .show(ui, |ui| {
                            ui.label("First frame");
                            ui.add(egui::DragValue::new(&mut dialog.from).range(0..=last));
                            ui.end_row();
                            ui.label("Last frame");
                            ui.add(egui::DragValue::new(&mut dialog.to).range(0..=last));
                            ui.end_row();
                        });
                    ui.weak("Deletes frames in [first, last] inclusive.");
                }
                DeleteFramesMode::Decimate => {
                    ui.horizontal(|ui| {
                        ui.label("Keep every");
                        ui.add(egui::DragValue::new(&mut dialog.stride).range(2..=100_000));
                        ui.label("-th frame");
                    });
                    ui.weak("Keeps frames 0, N, 2N, … and deletes the rest.");
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(n_frames.is_some(), egui::Button::new("Delete"))
                    .clicked()
                {
                    do_delete = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }

        if do_delete {
            if let Some(mol) = self.scene.molecules.iter_mut().find(|m| m.id == dialog.mol_id) {
                let removed = match dialog.mode {
                    DeleteFramesMode::Range => mol.trajectory.delete_range(dialog.from, dialog.to),
                    DeleteFramesMode::Decimate => mol.trajectory.decimate(dialog.stride),
                };
                // Re-render at the (clamped) current frame, or the static structure
                // if every frame was removed.
                mol.box_dirty = true;
                if mol.trajectory.frames.is_empty() {
                    for rep in &mut mol.reps {
                        rep.coords_dirty = true;
                    }
                    if mol.pending.is_some() {
                        mol.glow_dirty = true;
                    }
                } else {
                    mol.apply_current_frame();
                }
                self.status = format!("Deleted {removed} frame(s)");
                self.view_dirty = true;
            }
        } else if !close {
            self.delete_frames_dialog = Some(dialog); // keep open
        }
    }

    /// Render the "Load trajectory" modal (a-la VMD): file chooser + frame range
    /// / stride + sync/async, with Load/Cancel. Driven from `ctx` (egui modals
    /// take a `Context`, not a `Ui`), so it floats above the whole window.
    pub(super) fn draw_load_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.load_dialog.take() else {
            return;
        };
        let mut action = DialogAction::Keep;

        let modal = egui::Modal::new(egui::Id::new("load_traj_modal")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Load trajectory");
            match self.scene.molecules.iter().find(|m| m.id == dialog.mol_id) {
                Some(mol) => {
                    ui.label(format!("Into “{}”  ({} atoms)", mol.name, mol.n_atoms));
                }
                None => {
                    ui.colored_label(
                        egui::Color32::from_rgb(240, 120, 120),
                        "molecule no longer exists",
                    );
                }
            }
            ui.separator();

            // File chooser.
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{}  Choose file…", icon::FOLDER_OPEN))
                    .clicked()
                {
                    #[cfg(not(target_arch = "wasm32"))]
                    if let Some(p) = rfd::FileDialog::new()
                        .add_filter(
                            "Trajectories",
                            &["xtc", "trr", "dcd", "pdb", "gro", "xyz", "nc", "ncdf"],
                        )
                        .pick_file()
                    {
                        dialog.path = Some(p);
                        dialog.error = None;
                    }
                }
                match &dialog.path {
                    Some(p) => {
                        ui.monospace(
                            p.file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        );
                    }
                    None => {
                        ui.weak("no file selected");
                    }
                }
            });

            // Frame range + stride.
            egui::Grid::new("traj_load_opts")
                .num_columns(2)
                .spacing(egui::vec2(8.0, 4.0))
                .show(ui, |ui| {
                    ui.label("First frame");
                    ui.add(egui::DragValue::new(&mut dialog.from));
                    ui.end_row();

                    ui.label("Last frame");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut dialog.to_text)
                                .desired_width(60.0)
                                .hint_text("end"),
                        );
                        ui.weak("(empty = to end of file)");
                    });
                    ui.end_row();

                    ui.label("Stride");
                    ui.add(egui::DragValue::new(&mut dialog.stride).range(1..=usize::MAX))
                        .on_hover_text("Keep every Nth frame");
                    ui.end_row();
                });

            ui.horizontal(|ui| {
                ui.label("Reading:");
                ui.radio_value(&mut dialog.mode, LoadMode::Sync, "Sync")
                    .on_hover_text("Read all frames now (UI blocks until done)");
                ui.radio_value(&mut dialog.mode, LoadMode::Async, "Async")
                    .on_hover_text("Read in the background; frames appear as they load");
            });

            if let Some(err) = &dialog.error {
                ui.colored_label(egui::Color32::from_rgb(240, 120, 120), err);
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(dialog.path.is_some(), egui::Button::new("Load"))
                    .clicked()
                {
                    action = DialogAction::Load;
                }
                if ui.button("Cancel").clicked() {
                    action = DialogAction::Cancel;
                }
            });
        });

        if modal.should_close() {
            action = DialogAction::Cancel;
        }

        match action {
            DialogAction::Keep => self.load_dialog = Some(dialog),
            DialogAction::Cancel => {}
            DialogAction::Load => {
                if let Err(e) = self.start_load(&dialog) {
                    dialog.error = Some(e);
                    self.load_dialog = Some(dialog); // reopen, showing the error
                }
            }
        }
    }

    /// Modal to rename a molecule's displayed name (set from the molecule menu).
    pub(super) fn draw_rename_dialog(&mut self, ctx: &egui::Context) {
        let Some((id, mut name)) = self.rename_mol.take() else {
            return;
        };
        let mut commit = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("rename_mol_modal")).show(ctx, |ui| {
            ui.set_width(280.0);
            ui.heading("Rename molecule");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut name)
                    .desired_width(f32::INFINITY)
                    .hint_text("name"),
            );
            // Detect Enter *before* re-requesting focus: `request_focus()` re-grabs
            // focus the same frame, which would mask the Enter-induced `lost_focus()`
            // (so Enter never committed). Only keep the field focused when not entered.
            let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if !entered {
                resp.request_focus();
            }
            ui.separator();
            ui.horizontal(|ui| {
                let ok = ui
                    .add_enabled(!name.trim().is_empty(), egui::Button::new("Rename"))
                    .clicked();
                commit = ok || (entered && !name.trim().is_empty());
                cancel = ui.button("Cancel").clicked();
            });
        });
        if commit && !name.trim().is_empty() {
            if let Some(mol) = self.scene.molecules.iter_mut().find(|m| m.id == id) {
                mol.name = name.trim().to_string();
            }
        } else if !cancel && !modal.should_close() {
            self.rename_mol = Some((id, name)); // still open — keep the edit buffer
        }
    }

    /// Begin loading the dialog's file into its molecule (sync or async).
    pub(super) fn start_load(&mut self, dialog: &LoadDialog) -> Result<(), String> {
        let path = dialog.path.clone().ok_or("no file selected")?;
        // Empty "last frame" → read to the end of the file.
        let to = match dialog.to_text.trim() {
            "" => None,
            s => Some(s.parse::<usize>().map_err(|_| "last frame must be a number".to_string())?),
        };
        let opts = LoadOptions { from: dialog.from, to, stride: dialog.stride.max(1) };
        if let Some(to) = opts.to {
            if to < opts.from {
                return Err("last frame is before first frame".to_string());
            }
        }

        // Seed frame 0 with the structure coords (idempotent) and learn the count.
        let expected = {
            let mol = self
                .scene
                .molecules
                .iter_mut()
                .find(|m| m.id == dialog.mol_id)
                .ok_or("molecule no longer exists")?;
            mol.seed_frame0();
            mol.n_atoms
        };

        // Record the load so a saved session can replay it (native paths only).
        #[cfg(not(target_arch = "wasm32"))]
        let record = TrajLoad {
            path: path.clone(),
            from: opts.from,
            to: opts.to,
            stride: opts.stride,
        };

        match dialog.mode {
            LoadMode::Sync => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let frames = data::traj_loader::read_frames_sync(&path, &opts, expected)?;
                    if frames.is_empty() {
                        return Err("no frames matched the selected range".to_string());
                    }
                    let added = frames.len();
                    let mol = self
                        .scene
                        .molecules
                        .iter_mut()
                        .find(|m| m.id == dialog.mol_id)
                        .ok_or("molecule no longer exists")?;
                    let first_new = mol.trajectory.frames.len();
                    mol.append_frames(frames);
                    mol.traj_loads.push(record);
                    mol.trajectory.current = first_new; // jump to first loaded frame
                    mol.apply_current_frame();
                    self.status = format!("Loaded {added} frame(s)");
                    self.view_dirty = true;
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = (&path, &opts, expected);
                    return Err("trajectory loading is not yet supported on the web".to_string());
                }
            }
            LoadMode::Async => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let rx = data::traj_loader::spawn_async(path, opts, expected);
                    self.loaders.insert(dialog.mol_id, rx);
                    if let Some(mol) =
                        self.scene.molecules.iter_mut().find(|m| m.id == dialog.mol_id)
                    {
                        mol.traj_loads.push(record);
                    }
                    self.status = "Loading trajectory…".to_string();
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = (&path, &opts, expected);
                    return Err("trajectory loading is not yet supported on the web".to_string());
                }
            }
        }
        Ok(())
    }

    /// Drain background loaders, appending streamed frames to their molecules.
    /// Non-blocking (`try_recv`); finished/errored/disconnected loaders are removed.
    pub(super) fn poll_loaders(&mut self) {
        if self.loaders.is_empty() {
            return;
        }
        use std::sync::mpsc::TryRecvError;
        let ids: Vec<MolId> = self.loaders.keys().copied().collect();
        let mut finished: Vec<MolId> = Vec::new();
        for id in ids {
            while let Some(rx) = self.loaders.get(&id) {
                let msg = rx.try_recv();
                match msg {
                    Ok(LoadMsg::Frame(state)) => {
                        // Append to the molecule if it still exists; else discard.
                        if let Some(mol) =
                            self.scene.molecules.iter_mut().find(|m| m.id == id)
                        {
                            mol.push_frame(state);
                        }
                    }
                    Ok(LoadMsg::Done) => {
                        finished.push(id);
                        break;
                    }
                    Ok(LoadMsg::Error(e)) => {
                        self.status = e;
                        finished.push(id);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished.push(id);
                        break;
                    }
                }
            }
        }
        for id in finished {
            self.loaders.remove(&id);
        }
    }

    /// Browser trajectory streaming: read a batch of frames from each in-memory
    /// [`data::traj_wasm::TrajStream`] and append them, so frames flow in without
    /// blocking the UI. On the first batch, jump to the first trajectory frame so
    /// the load is visible. Finished/errored streams are removed; repaints continue
    /// while any stream is active.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn poll_wasm_loaders(&mut self, ctx: &egui::Context) {
        if self.wasm_loaders.is_empty() {
            return;
        }
        const BATCH: usize = 64;
        let ids: Vec<MolId> = self.wasm_loaders.keys().copied().collect();
        let mut finished: Vec<MolId> = Vec::new();
        let mut view_dirty = false;
        for id in ids {
            let Some(stream) = self.wasm_loaders.get_mut(&id) else {
                continue;
            };
            let batch = stream.next_batch(BATCH);
            let done = stream.done;
            match batch {
                Ok(frames) => {
                    if let Some(mol) = self.scene.molecules.iter_mut().find(|m| m.id == id) {
                        for st in frames {
                            mol.push_frame(st);
                        }
                        // First frames in → show the first trajectory frame.
                        if mol.trajectory.current == 0 && mol.trajectory.frames.len() > 1 {
                            mol.trajectory.current = 1;
                            mol.apply_current_frame();
                            view_dirty = true;
                        }
                        if done {
                            self.status =
                                format!("Loaded {} frame(s)", mol.trajectory.frames.len() - 1);
                        }
                    }
                    if done {
                        finished.push(id);
                    }
                }
                Err(e) => {
                    log::error!("{e}");
                    self.status = e;
                    finished.push(id);
                }
            }
        }
        for id in &finished {
            self.wasm_loaders.remove(id);
        }
        if view_dirty {
            self.view_dirty = true;
        }
        // Keep frames flowing while any stream is still active.
        if !self.wasm_loaders.is_empty() {
            ctx.request_repaint();
        }
    }
}
