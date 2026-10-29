//! Trajectory/structure load + delete-frames + rename dialogs, loaders, file picker.
use super::*;
use super::draw::*;
use super::widgets::*;


/// State of the "Load trajectory" modal.
#[derive(Clone)]
pub(super) struct LoadDialog {
    mol_id: MolId,
    path: Option<PathBuf>,
    from: usize,
    /// Last frame to read, as text. **Empty = read to the end of the file.**
    to_text: String,
    stride: usize,
    mode: LoadMode,
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
        }
    }
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
    let input = match document
        .create_element("input")
        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().map_err(wasm_bindgen::JsValue::from))
    {
        Ok(input) => input,
        Err(error) => {
            log::error!("could not create file picker: {}", crate::errors::js_error_chain(&error));
            return;
        }
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
                Err(e) => log::error!("failed to read file: {}", crate::errors::js_error_chain(&e)),
            }
        });
    });
    input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    // The closure must outlive this call (it fires later); leak it deliberately.
    on_change.forget();
    input.click();
}
impl App {

    /// Open **representation `rep_idx`** of molecule `mi` in the drawing editor: start a
    /// Draw session scoped to that rep (its selection defines the editable atoms; new atoms
    /// fold into it). Every molecule is editable + undoable now (no per-molecule flag), so
    /// this just switches into edit mode. Mutually exclusive with picking.
    pub(super) fn open_rep_in_editor(&mut self, mi: usize, rep_idx: usize) {
        let Some(mol) = self.scene.molecules.get(mi) else { return };
        let id = mol.id;
        let target_rep = (rep_idx < mol.reps.len()).then_some(rep_idx);
        self.pick_mode = PickMode::Off;
        let mut session = self.draw.take().unwrap_or_default();
        session.target = Some(id);
        session.target_rep = target_rep;
        session.drag = DrawDrag::Idle;
        // Start this molecule with a clean editing slate: a leftover DihedralRotate tool +
        // its selected axis are keyed (by MolId) to the previously-edited molecule, so a
        // handle drag would keep rotating the *old* molecule instead of this one. Reset to
        // the default Draw tool and drop the axis/handle state, as a fresh session would.
        session.tool = DrawTool::Draw;
        session.dihedral = super::dihedral::DihedralState::default();
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
                .add_filter("Structures", &["pdb", "ent", "gro", "xyz", "tpr", "sdf", "sd", "mol"])
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
                ".pdb,.ent,.gro,.xyz,.dcd,.trr,.xtc,.sdf,.sd,.mol",
                ctx.clone(),
                move |name, bytes| {
                    let _ = tx.send((name, bytes));
                },
            );
        }
    }

    /// Load a structure file from disk (native). A multi-molecule SDF/MOL (≥2 `$$$$`
    /// records) becomes a [`MolGroup`]; anything else (incl. a single-record SDF) is
    /// one ordinary molecule. Read/parse on a worker, then install on the UI thread.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn open_structure_path(&mut self, path: &std::path::Path) {
        let bonds = self.settings.behavior.bond_params();
        let path = path.to_path_buf();
        let source = path.clone();
        self.begin_loading(move || {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if matches!(ext.as_str(), "sdf" | "sd" | "mol") {
                data::load_records(&path, &bonds)
            } else {
                data::load_with(&path, &bonds).map(|raw| vec![raw])
            }
        }, move |app, mut records| {
            if records.len() > 1 {
                let name = source.file_name().unwrap_or_default().to_string_lossy().into_owned();
                app.add_group(records, MoleculeSource::File(source), name);
            } else if let Some(mut raw) = records.pop() {
                raw.source = MoleculeSource::File(source);
                app.add_loaded(raw);
            } else {
                app.status = "no molecules found in file".into();
            }
        });
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
                let (min, max) =
                    (self.scene.molecules[mi].bbox_min, self.scene.molecules[mi].bbox_max);
                self.reframe_camera(min, max);
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
                self.reframe_camera(min, max);
            }
        }
        self.status = format!("{} molecule(s) loaded", self.scene.molecules.len());
        self.view_dirty = true;
    }

    /// Render the "Delete frames" modal: pick a frame range to drop or a decimate
    /// stride (keep every Nth frame), with Delete/Cancel. Trajectory frames are
    /// view state, so this is not undoable (like loading frames).
    pub(super) fn draw_delete_frames_dialog(&mut self, ctx: &egui::Context) {
        modal_shell(
            self,
            ctx,
            |a| &mut a.delete_frames_dialog,
            ModalSpec {
                id: "del_frames_modal",
                width: 340.0,
                heading: "Delete trajectory frames",
                commit: "Delete",
            },
            |ui, dialog, _err, app| {
                let n_frames = app
                    .scene
                    .molecules
                    .iter()
                    .find(|m| m.id == dialog.mol_id)
                    .map(|m| m.trajectory.n_frames());
                let last = n_frames.unwrap_or(0).saturating_sub(1);
                match n_frames {
                    Some(nf) => {
                        ui.label(format!("{nf} frames loaded (indices 0..{last})"));
                    }
                    None => {
                        ui.colored_label(ui.visuals().error_fg_color, "molecule no longer exists");
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
                ModalBody::enabled(n_frames.is_some())
            },
            |app, dialog| {
                if let Some(mol) = app.scene.molecules.iter_mut().find(|m| m.id == dialog.mol_id) {
                    let removed = match dialog.mode {
                        DeleteFramesMode::Range => {
                            mol.trajectory.delete_range(dialog.from, dialog.to)
                        }
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
                    app.status = format!("Deleted {removed} frame(s)");
                    app.view_dirty = true;
                }
                Ok(())
            },
        );
    }

    /// Render the "Load trajectory" modal (a-la VMD): file chooser + frame range
    /// / stride + sync/async, with Load/Cancel. Driven from `ctx` (egui modals
    /// take a `Context`, not a `Ui`), so it floats above the whole window.
    pub(super) fn draw_load_dialog(&mut self, ctx: &egui::Context) {
        modal_shell(
            self,
            ctx,
            |a| &mut a.load_dialog,
            ModalSpec {
                id: "load_traj_modal",
                width: 360.0,
                heading: "Load trajectory",
                commit: "Load",
            },
            |ui, dialog, err, app| {
                match app.scene.molecules.iter().find(|m| m.id == dialog.mol_id) {
                    Some(mol) => {
                        ui.label(format!("Into “{}”  ({} atoms)", mol.name, mol.n_atoms));
                    }
                    None => {
                        ui.colored_label(ui.visuals().error_fg_color, "molecule no longer exists");
                    }
                }
                ui.separator();

                // File chooser.
                ui.horizontal(|ui| {
                    if ui
                        .button(format!("{}  Choose file…", icon::FOLDER_OPEN))
                        .clicked()
                    {
                        // A different file may well work, so drop the last failure's message.
                        *err = None;
                        #[cfg(not(target_arch = "wasm32"))]
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter(
                                "Trajectories",
                                &["xtc", "trr", "dcd", "pdb", "gro", "xyz", "nc", "ncdf"],
                            )
                            .pick_file()
                        {
                            dialog.path = Some(p);
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

                ModalBody::enabled(dialog.path.is_some())
            },
            // A failed read reopens the dialog with the reason (the shell's fourth state), so
            // the range/stride can be corrected without choosing the file again.
            |app, dialog| app.start_load(dialog),
        );
    }

    /// Modal to rename a molecule's displayed name (set from the molecule menu).
    pub(super) fn draw_rename_dialog(&mut self, ctx: &egui::Context) {
        modal_shell(
            self,
            ctx,
            |a| &mut a.rename_dialog,
            ModalSpec {
                id: "rename_mol_modal",
                width: 280.0,
                heading: "Rename molecule",
                commit: "Rename",
            },
            |ui, dialog, _err, _app| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut dialog.name)
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
                let ok = !dialog.name.trim().is_empty();
                ModalBody {
                    can_commit: ok,
                    // Enter in the field commits, so the body decides in that case.
                    action: (entered && ok).then_some(DialogAction::Commit),
                }
            },
            |app, dialog| {
                if let Some(mol) = app.scene.molecules.iter_mut().find(|m| m.id == dialog.mol) {
                    mol.name = dialog.name.trim().to_string();
                }
                Ok(())
            },
        );
    }

    /// Begin loading the dialog's file into its molecule (sync or async).
    #[cfg_attr(target_arch = "wasm32", allow(unreachable_code))]
    pub(super) fn start_load(&mut self, dialog: &LoadDialog) -> Result<(), String> {
        let path = dialog.path.clone().ok_or("no file selected")?;
        // Empty "last frame" → read to the end of the file.
        let to = match dialog.to_text.trim() {
            "" => None,
            s => Some(s.parse::<usize>().map_err(|e| format!("last frame must be a number: {}", crate::errors::error_chain(&e)))?),
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
                    let mol_id = dialog.mol_id;
                    let retry = dialog.clone();
                    self.begin_loading(move || {
                        let frames = data::traj_loader::read_frames_sync(&path, &opts, expected)
                            .and_then(|frames| {
                                if frames.is_empty() { Err("no frames matched the selected range".into()) }
                                else { Ok(frames) }
                            });
                        Ok(frames)
                    }, move |app, result: Result<Vec<State>, String>| {
                        let frames = match result {
                            Ok(frames) => frames,
                            Err(error) => {
                                app.status = error.clone();
                                app.load_dialog = Some(ModalState { inner: retry, error: Some(error) });
                                return;
                            }
                        };
                        let Some(mol) = app.scene.molecules.iter_mut().find(|m| m.id == mol_id) else {
                            app.status = "molecule no longer exists".into();
                            return;
                        };
                        let added = frames.len();
                        let first_new = mol.trajectory.frames.len();
                        mol.append_frames(frames);
                        mol.traj_loads.push(record);
                        mol.trajectory.current = first_new;
                        mol.apply_current_frame();
                        app.status = format!("Loaded {added} frame(s)");
                        app.view_dirty = true;
                    });
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
                    self.scene.loaders.insert(dialog.mol_id, rx);
                    let mut progress = super::loading::AsyncProgress::new(&opts);
                    progress.request = Some(dialog.clone());
                    self.async_load_progress.insert(dialog.mol_id, progress);
                    if let Some(mol) =
                        self.scene.molecules.iter_mut().find(|m| m.id == dialog.mol_id)
                    {
                        if let Some(progress) = self.async_load_progress.get_mut(&dialog.mol_id) {
                            progress.record_index = Some(mol.traj_loads.len());
                        }
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

    /// Exercise the actual async dialog path in hidden UI captures.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn debug_start_async_trajectory(&mut self, path: PathBuf) {
        let Some(mol) = self.scene.molecules.first() else { return };
        let mut dialog = LoadDialog::new(mol.id);
        dialog.path = Some(path);
        dialog.mode = LoadMode::Async;
        if let Err(error) = self.start_load(&dialog) { self.status = error; }
    }

    fn fail_async_load(&mut self, id: MolId, error: String) {
        self.status = error.clone();
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(progress) = self.async_load_progress.get_mut(&id) {
            // Failed loads must not be replayed as successful full-range loads
            // when the user saves and reopens the session.
            if let Some(mol) = self.scene.molecules.iter_mut().find(|mol| mol.id == id) {
                if let Some(index) = progress.record_index {
                    if progress.loaded == 0 && index < mol.traj_loads.len() {
                        mol.traj_loads.remove(index);
                    } else if let Some(record) = mol.traj_loads.get_mut(index) {
                        record.to = Some(record.from.saturating_add(
                            progress.loaded.saturating_sub(1).saturating_mul(record.stride.max(1))));
                    }
                }
                if let Some(request) = progress.request.take() {
                    self.load_dialog = Some(ModalState { inner: request, error: Some(error) });
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = id;
    }

    /// Drain background loaders, appending streamed frames to their molecules.
    /// Non-blocking (`try_recv`); finished/errored/disconnected loaders are removed.
    pub(super) fn poll_loaders(&mut self) {
        if self.scene.loaders.is_empty() {
            return;
        }
        use std::sync::mpsc::TryRecvError;
        let ids: Vec<MolId> = self.scene.loaders.keys().copied().collect();
        let mut finished: Vec<MolId> = Vec::new();
        for id in ids {
            for _ in 0..64 {
                let Some(rx) = self.scene.loaders.get(&id) else { break };
                let msg = rx.try_recv();
                match msg {
                    Ok(LoadMsg::Frame(state)) => {
                        // Append to the molecule if it still exists; else discard.
                        if let Some(mol) =
                            self.scene.molecules.iter_mut().find(|m| m.id == id)
                        {
                            mol.push_frame(state);
                            if let Some(progress) = self.async_load_progress.get_mut(&id) { progress.loaded += 1; }
                        }
                    }
                    Ok(LoadMsg::Done) => {
                        if let Some(progress) = self.async_load_progress.get(&id) {
                            if progress.loaded == 0 {
                                self.fail_async_load(id, "No frames matched the selected range".into());
                            } else {
                                self.status = format!("Loaded {} frame(s)", progress.loaded);
                            }
                        }
                        finished.push(id);
                        break;
                    }
                    Ok(LoadMsg::Error(error)) => {
                        self.fail_async_load(id, error);
                        finished.push(id);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.fail_async_load(id, "Trajectory loading stopped unexpectedly".into());
                        finished.push(id);
                        break;
                    }
                }
            }
        }
        for id in finished {
            self.scene.loaders.remove(&id);
            self.async_load_progress.remove(&id);
        }
    }

    /// Browser trajectory streaming: read a batch of frames from each in-memory
    /// [`data::traj_wasm::TrajStream`] and append them, so frames flow in without
    /// blocking the UI. On the first batch, jump to the first trajectory frame so
    /// the load is visible. Finished/errored streams are removed; repaints continue
    /// while any stream is active.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn poll_wasm_loaders(&mut self, ctx: &egui::Context) {
        if self.scene.wasm_loaders.is_empty() {
            return;
        }
        const BATCH: usize = 64;
        let ids: Vec<MolId> = self.scene.wasm_loaders.keys().copied().collect();
        let mut finished: Vec<MolId> = Vec::new();
        let mut view_dirty = false;
        for id in ids {
            let Some(stream) = self.scene.wasm_loaders.get_mut(&id) else {
                continue;
            };
            let batch = stream.next_batch(BATCH);
            let done = stream.done;
            match batch {
                Ok(frames) => {
                    if let Some(mol) = self.scene.molecules.iter_mut().find(|m| m.id == id) {
                        if let Some(progress) = self.async_load_progress.get_mut(&id) { progress.loaded += frames.len(); }
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
            self.scene.wasm_loaders.remove(id);
            self.async_load_progress.remove(id);
        }
        if view_dirty {
            self.view_dirty = true;
        }
        // Keep frames flowing while any stream is still active.
        if !self.scene.wasm_loaders.is_empty() {
            ctx.request_repaint();
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod loading_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "requires native GPU; exercises sync/async trajectory modes and delayed read errors"]
    fn trajectory_jobs_preserve_modes_ranges_and_retry_errors() {
        let ctx = egui::Context::default();
        let mut cc = eframe::CreationContext::_new_kittest(ctx.clone());
        cc.wgpu_render_state = Some(crate::render::test_gpu());
        let mut app = App::new(&cc, AppLaunch::default()).unwrap();
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/2lao.pdb");
        app.add_loaded(data::load(&file).unwrap());
        let id = app.scene.molecules[0].id;
        let mut request = LoadDialog::new(id);
        request.path = Some(file.with_file_name("missing-trajectory.xtc"));
        app.start_load(&request).unwrap();
        let wait = |app: &mut App| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while app.service_loading(&ctx) {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        wait(&mut app);
        let retry = app.load_dialog.take().expect("a failed sync read must reopen its dialog");
        assert!(retry.error.is_some());
        assert_eq!(retry.inner.mol_id, id);
        assert_eq!(app.scene.molecules[0].trajectory.frames.len(), 1);
        request.path = Some(file.clone());
        request.to_text = "0".into();
        app.start_load(&request).unwrap();
        wait(&mut app);
        let mol = &app.scene.molecules[0];
        assert_eq!(mol.trajectory.frames.len(), 2);
        assert_eq!(mol.trajectory.current, 1);
        assert_eq!(mol.traj_loads[0].to, Some(0));
        request.mode = LoadMode::Async;
        app.start_load(&request).unwrap();
        assert!(app.async_loading_active());
        assert!(!app.service_loading(&ctx), "async mode must not block the interface");
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.async_loading_active() {
            app.poll_loaders();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(app.scene.molecules[0].trajectory.frames.len(), 3);
        assert_eq!(app.scene.molecules[0].traj_loads.len(), 2);
        request.path = Some(file.with_file_name("toy_tube.pdb"));
        app.start_load(&request).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.async_loading_active() {
            app.poll_loaders();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let retry = app.load_dialog.as_ref().expect("atom mismatch must show a visible error dialog");
        let error = retry.error.as_ref().unwrap();
        assert!(error.contains("2 atoms") && error.contains("1911"), "{error}");
        assert_eq!(retry.inner.mode, LoadMode::Async);
        assert_eq!(retry.inner.path, request.path);
        assert_eq!(app.scene.molecules[0].trajectory.frames.len(), 3);
        assert_eq!(app.scene.molecules[0].traj_loads.len(), 2, "failed load must not be saved");
        let mut visible_error = false;
        for _ in 0..3 {
            let output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                ..Default::default()
            }, |_| app.draw_load_dialog(&ctx));
            visible_error |= output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.text().contains("2 atoms")));
        }
        assert!(visible_error);
    }
}
