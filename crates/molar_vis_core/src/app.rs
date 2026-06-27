//! The eframe application: owns UI state, the camera, the scene (molecules and
//! their representations), and the 3D renderer. Lays out the VMD-style left
//! control panel (Scene → Molecules → Representations → Rep controls) plus the
//! central 3D viewport, and only re-renders the scene when something changed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use eframe::egui;
use molar::prelude::{AtomProvider, Measure, ParticleIterProvider, SsAlgorithm, State};
#[cfg(not(target_arch = "wasm32"))]
use molar::prelude::FileHandler;

use crate::camera::{BgKind, Camera, CueMode, Projection};
use crate::color::ColorMethod;
use crate::data;
use crate::geometry::{self, RepKind, RepParams};
use crate::history::{EditState, History};
use crate::launch::AppLaunch;
use crate::material::{Material, MaterialParams};
use crate::minimize::{Bond, BondOrderExt};
use crate::pick::{self, PickMode, SelectionMode};
use crate::render::SceneRenderer;
#[cfg(not(target_arch = "wasm32"))]
use crate::render::SphereInstance;
use crate::scene::{self, MolId, Representation, Scene, SettingsTab};
use crate::secstruct::SsMap;
#[cfg(not(target_arch = "wasm32"))]
use crate::session::{Session, ViewState};
use crate::settings::{RepDefaults, Settings, ThemeMode};
#[cfg(not(target_arch = "wasm32"))]
use crate::scene::{MoleculeSource, TrajLoad};
use crate::trajectory::{LoadMode, LoadMsg, LoadOptions, LoopMode, Trajectory};

use egui_phosphor::regular as icon;

mod build;
mod console;
mod draw;
mod draw_input;
mod init;
mod loaders;
mod overlay;
mod panels;
mod pickers;
mod rep_panel;
mod session_io;
mod settings_dialog;
mod viewport;
mod widgets;

use build::*;
use draw::DrawSession;
use loaders::{DeleteFramesDialog, LoadDialog};


/// Workaround for a winit/egui IME bug seen on recent Wayland compositors: while a
/// text field is focused the compositor streams `Ime(Disabled)` events and delivers
/// every typed character as `Ime(Commit(..))` *without* a preceding `Ime(Enabled)` or
/// `Ime(Preedit)`. egui's `TextEdit` only honors a commit when its (preedit-derived)
/// IME cursor matches the live cursor, and that IME cursor is only updated by
/// `Enabled`/`Preedit` — so it stays at the post-focus position and only the **first**
/// keystroke is accepted; every later one (and any edit of pre-existing text) is
/// silently dropped, though paste and backspace still work. Rewriting each
/// `Ime(Commit(s))` into a plain `Text(s)` event routes it through egui's ungated
/// insertion path, and dropping the stray `Ime` events stops them from confusing the
/// state machine. Selection/name fields are ASCII, so IME composition isn't needed.
///
/// Linux-only: X11 emits no `Commit` events (characters arrive as `Text`), so this is
/// a no-op there, and macOS/Windows IME (which works) is left untouched.
#[cfg(target_os = "linux")]
fn defuse_broken_ime(ctx: &egui::Context) {
    ctx.input_mut(|i| {
        if !i.events.iter().any(|e| matches!(e, egui::Event::Ime(_))) {
            return;
        }
        for ev in &mut i.events {
            if let egui::Event::Ime(egui::ImeEvent::Commit(s)) = ev {
                let s = std::mem::take(s);
                *ev = egui::Event::Text(s);
            }
        }
        i.events.retain(|e| !matches!(e, egui::Event::Ime(_)));
    });
}


pub struct App {
    renderer: SceneRenderer,
    camera: Camera,
    scene: Scene,
    /// Persisted program settings (theme, render quality, new-document defaults,
    /// behavior). Loaded on launch from the platform config dir; edited via the
    /// settings dialog (see `settings_draft`).
    settings: Settings,
    /// Effective defaults for a new representation = `settings.reps`, with the kind
    /// overridden by the `MOLAR_VIS_DEBUG_REP` env hook. Recomputed when settings
    /// change. Used for the initial rep of each loaded molecule + the add-rep button.
    rep_defaults: RepDefaults,
    /// Working copy of the settings while the settings dialog is open (edit-then-
    /// apply); `None` when the dialog is closed.
    settings_draft: Option<Settings>,
    /// Active tab in the settings dialog.
    settings_tab: SettingsPage,
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
    /// `(molecule index, rep index)` whose selection field is focused/expanded.
    editing_rep: Option<(usize, usize)>,
    /// Open trajectory-load dialog, if any (one at a time).
    load_dialog: Option<LoadDialog>,
    /// Open "delete trajectory frames" dialog, if any.
    delete_frames_dialog: Option<DeleteFramesDialog>,
    /// Open "rename molecule" dialog: the target molecule + the edit buffer.
    rename_mol: Option<(MolId, String)>,
    /// In-flight background trajectory loaders, keyed by molecule (so they
    /// survive reorder/delete/undo). Drained each frame via `try_recv`.
    loaders: HashMap<MolId, Receiver<LoadMsg>>,
    /// Picking mode (top view-toolbar dropdown). `Click` shows the hovered atom's
    /// identity + glow and selects it on click; `Lasso` drags a freehand selection
    /// polygon.
    pick_mode: PickMode,
    /// How a lasso expands its hit atoms (viewport-overlay dropdown): exact atoms,
    /// whole residues, or heavy atoms + their bonded hydrogens.
    selection_mode: SelectionMode,
    /// In-progress lasso polygon (viewport pixel coords), accumulated while
    /// dragging in `PickMode::Lasso`. Empty when not lassoing. Transient view state.
    lasso_path: Vec<egui::Pos2>,
    /// Last cursor NDC the hover detail lens was rebuilt at, so it only rebuilds as
    /// the cursor actually moves (the fade follows the ray, so any move rebuilds).
    last_lens_ndc: Option<(f32, f32)>,
    /// Last completed GPU hover pick `(mol, rep, atom)` (native only). The async
    /// id-buffer readback lags a frame or two, so the hit is cached here and the
    /// `PickHit` is rebuilt from it each frame. `None` = nothing hovered.
    #[cfg(not(target_arch = "wasm32"))]
    hover_pick: Option<(usize, usize, usize)>,
    /// Pick-target pixel of the last requested GPU pick (native). A new pick is only
    /// requested when the cursor moves or the view changes, so a stationary hover
    /// stays idle (0 GPU) instead of re-picking every frame.
    #[cfg(not(target_arch = "wasm32"))]
    last_pick_px: Option<(u32, u32)>,
    /// Whether the VMD-style orientation axes gizmo is shown in the viewport.
    axes_on: bool,
    /// Which viewport corner the axes gizmo is anchored to.
    axes_corner: Corner,
    /// Active tab in the top-bar "view settings" (hamburger) menu.
    view_tab: ViewTab,
    /// Whether the view-settings (hamburger) window is open. A real `Window` rather
    /// than a `Popup` so nested click-to-open dropdowns / color pickers work; closed
    /// manually on a click outside it (see `view_settings_window`).
    view_menu_open: bool,
    /// The view-settings window's rect **as drawn last frame** — the geometry the user
    /// actually clicked on. The close-on-click-outside test must use this, not the
    /// current frame's rect: switching tabs re-lays-out the (right-pivoted) window in
    /// the *same* frame, so the freshly-narrowed rect no longer covers the leftmost
    /// tab the click landed on (see `view_settings_window`).
    view_menu_rect: Option<egui::Rect>,
    /// Browser file-open channel: the async `<input type=file>` picker reads the
    /// chosen file and sends `(filename, bytes)` here; `ui()` drains it and loads
    /// the structure. Cloned per pick; the receiver is polled each frame. Wasm only.
    #[cfg(target_arch = "wasm32")]
    file_tx: std::sync::mpsc::Sender<(String, Vec<u8>)>,
    #[cfg(target_arch = "wasm32")]
    file_rx: std::sync::mpsc::Receiver<(String, Vec<u8>)>,
    /// Browser trajectory-load channel: the picker sends `(molecule, filename,
    /// bytes)` here; `ui()` drains it into an incremental [`data::traj_wasm::TrajStream`]
    /// per molecule (in `wasm_loaders`), whose frames are streamed into the
    /// trajectory a batch per frame. Wasm only.
    #[cfg(target_arch = "wasm32")]
    traj_tx: std::sync::mpsc::Sender<(MolId, String, Vec<u8>)>,
    #[cfg(target_arch = "wasm32")]
    traj_rx: std::sync::mpsc::Receiver<(MolId, String, Vec<u8>)>,
    #[cfg(target_arch = "wasm32")]
    wasm_loaders: HashMap<MolId, data::traj_wasm::TrajStream>,
    /// Active interactive-drawing session (Draw mode), or `None` when off. Mutually
    /// exclusive with the pick modes (`pick_mode`): turning Draw on forces `pick_mode
    /// = Off`, and choosing any pick mode clears `draw`. See the Draw-mode types at
    /// the bottom of this file.
    draw: Option<DrawSession>,
    /// Whether the scripting console window is open (toggled from the Edit menu).
    console_open: bool,
    /// Scripting-console scrollback + input + history (see `script::console`).
    console: crate::script::ScriptConsole,
    /// Persistent Rhai REPL backing the console: keeps the engine + a `Scope` alive
    /// across input lines so `let` bindings survive between lines (see `script.rs`).
    script: crate::script::ScriptSession,
    /// External command channel for the native Python module (`molar_vis_py`): jobs
    /// queued from the Python thread are drained + run with `&mut App` at the top of
    /// each `ui()`, so Python can drive the running viewer. `None` for the standalone
    /// app and wasm. See [`AppJob`].
    jobs_rx: Option<std::sync::mpsc::Receiver<AppJob>>,
}


/// A unit of work run on the viewer (UI) thread with mutable [`App`] access. The
/// native Python module sends these over a channel from the Python thread (e.g. "add
/// this shared molecule", "set this rep's style"); they're drained at the top of each
/// [`App::ui`]. The closure is `Send` so it can cross the thread boundary (it may
/// capture pyo3 `Py<_>` handles, which are `Send`).
pub type AppJob = Box<dyn FnOnce(&mut App) + Send>;


/// Tabs in the top-bar "view settings" (hamburger) menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum ViewTab {
    #[default]
    Camera,
    Lighting,
    Scene,
}


/// Tabs in the program-settings dialog (the cogwheel modal).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum SettingsPage {
    #[default]
    Appearance,
    Rendering,
    View,
    Representations,
    Behavior,
}


/// A viewport corner, for anchoring the axes gizmo.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}


/// How a lasso gesture combines with the molecule's existing active selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LassoOp {
    /// Plain drag: the lasso becomes the new active selection.
    Replace,
    /// Shift+drag: union the lassoed atoms into the active selection.
    Add,
    /// Ctrl/⌘+drag: remove the lassoed atoms from the active selection.
    Subtract,
}

impl App {
    /// Install the external job channel (native Python module). Jobs sent on the
    /// paired `Sender` are run on the UI thread each frame; while connected, the
    /// viewport polls for them so commands from Python apply within a frame or two.
    pub fn set_jobs(&mut self, rx: std::sync::mpsc::Receiver<AppJob>) {
        self.jobs_rx = Some(rx);
    }

    /// Drain + run any jobs queued by the Python thread (native module). Collected
    /// first so the receiver borrow is released before each job takes `&mut self`.
    fn run_external_jobs(&mut self) {
        let jobs: Vec<AppJob> = match &self.jobs_rx {
            Some(rx) => rx.try_iter().collect(),
            None => return,
        };
        for job in jobs {
            job(self);
        }
    }

    /// Mark every shared (pymolar-backed) molecule's geometry as coords-dirty, so the
    /// render loop re-reads its externally-owned coordinates this frame. That's how a
    /// Python-side `sel.translate(...)` (which mutates the shared `State` in place)
    /// shows up live. Cheap rebuild (reuses the cached secondary structure, no DSSP);
    /// a coords version stamp would let us skip unchanged frames, but isn't needed for
    /// interactive sizes. Only runs while the external (Python) channel is connected.
    fn mark_shared_dirty(&mut self) {
        for mol in &mut self.scene.molecules {
            if !mol.data.is_shared() {
                continue;
            }
            for rep in &mut mol.reps {
                rep.coords_dirty = true;
            }
            if mol.pending.is_some() {
                mol.glow_dirty = true;
            }
            if mol.show_box {
                mol.box_dirty = true;
            }
        }
    }

    // --- External (native Python module) API, run via `AppJob`s on the UI thread. ---

    /// Add a molecule backed by a shared external source (pymolar), rendered
    /// zero-copy. Frames the camera if it's the first molecule. Returns its index.
    pub fn add_shared_molecule(
        &mut self,
        source: Box<dyn crate::moldata::SharedSource>,
        name: String,
    ) -> Result<usize, String> {
        let was_empty = self.scene.molecules.is_empty();
        let bond_params = self.settings.behavior.bond_params();
        self.scene
            .add_shared(name, source, &bond_params, &self.rep_defaults)?;
        let idx = self.scene.molecules.len() - 1;
        self.scene.selected_mol = Some(idx);
        if let Some(mol) = self.scene.molecules.last_mut() {
            mol.trajectory.speed_fps = self.settings.behavior.traj_fps;
            mol.trajectory.loop_mode = self.settings.behavior.loop_mode;
        }
        if was_empty {
            if let Some((min, max)) = self.scene.bbox() {
                self.camera = Camera::frame_bbox(min, max, self.settings.view.fill);
                self.settings.view.seed_camera(&mut self.camera);
            }
        }
        self.view_dirty = true;
        Ok(idx)
    }

    /// Append a default representation to molecule `mol`; returns the new rep index.
    pub fn add_rep_default(&mut self, mol: usize) -> Result<usize, String> {
        self.execute_command(crate::script::Command::AddRep { mol })?;
        let n = self
            .scene
            .molecules
            .get(mol)
            .map(|m| m.reps.len())
            .ok_or_else(|| format!("no molecule {mol}"))?;
        Ok(n.saturating_sub(1))
    }

    /// Set representation `(mol, rep)`'s draw style (e.g. "vdw", "cartoon").
    pub fn set_rep_style(&mut self, mol: usize, rep: usize, kind: &str) -> Result<(), String> {
        self.execute_command(crate::script::Command::Style {
            mol,
            rep: crate::script::RepRef::Index(rep),
            kind: kind.to_string(),
        })
    }

    /// Set representation `(mol, rep)`'s color scheme (e.g. "chain", "ss").
    pub fn set_rep_color(&mut self, mol: usize, rep: usize, method: &str) -> Result<(), String> {
        self.execute_command(crate::script::Command::Color {
            mol,
            rep: crate::script::RepRef::Index(rep),
            method: method.to_string(),
        })
    }

    /// Set representation `(mol, rep)`'s material (e.g. "Transparent").
    pub fn set_rep_material(&mut self, mol: usize, rep: usize, name: &str) -> Result<(), String> {
        self.execute_command(crate::script::Command::Material {
            mol,
            rep: crate::script::RepRef::Index(rep),
            name: name.to_string(),
        })
    }

    /// Set representation `(mol, rep)`'s selection to exactly `indices` (e.g. a
    /// pymolar `Sel`'s atoms), via a compact `index lo:hi …` selection string.
    pub fn set_rep_selection(&mut self, mol: usize, rep: usize, indices: &[usize]) -> Result<(), String> {
        let text = crate::pick::index_selection_string(indices)
            .ok_or("selection is empty")?;
        self.execute_command(crate::script::Command::Select {
            mol,
            rep: crate::script::RepRef::Index(rep),
            text,
        })
    }

    /// Recompile dirty selections and rebuild/reupload dirty geometry. Returns
    /// true if any geometry was uploaded (so the frame needs re-rendering).
    fn rebuild_dirty(&mut self, rs: &eframe::egui_wgpu::RenderState) -> bool {
        let mut changed = false;
        // Whether wrapping bonds are drawn as dashed minimum-image half-bonds (read
        // once: the molecule loop below borrows `self.scene` mutably).
        let dashed = self.settings.behavior.dashed_pbc_bonds;
        // A structural change (molecule add/remove/reorder/visibility) shifts molecule
        // indices, so the GPU pick geometry's baked `mol+1` ids must be rebuilt.
        #[cfg(not(target_arch = "wasm32"))]
        let structure_changed = self.view_dirty;
        for (_mi, mol) in self.scene.molecules.iter_mut().enumerate() {
            #[cfg(not(target_arch = "wasm32"))]
            if structure_changed {
                mol.pick_dirty = true;
            }
            #[cfg(not(target_arch = "wasm32"))]
            let pick_pending = mol.pick_dirty;
            #[cfg(target_arch = "wasm32")]
            let pick_pending = false;
            let any_rep_dirty = mol
                .reps
                .iter()
                .any(|r| r.sel_dirty || r.geom_dirty || r.coords_dirty);
            if !(any_rep_dirty
                || (mol.show_box && mol.box_dirty)
                || mol.aromatic_dirty
                || mol.glow_dirty
                || mol.hover_dirty
                || mol.hover_detail_dirty
                || pick_pending)
            {
                continue;
            }
            // The coordinates to render: the current trajectory frame, read by
            // reference (no copy into the System), or the static structure state.
            let render_state: &State = match mol.trajectory.frames.get(mol.trajectory.current) {
                Some(frame) => frame,
                None => mol.data.state(),
            };
            let n_atoms = mol.n_atoms;
            // Whether any rep's geometry was (re)built this pass — if so and there's
            // an active selection, its glow must follow the new style/coords.
            let mut rep_geom_changed = false;
            for rep in &mut mol.reps {
                if rep.sel_dirty {
                    // Parse + evaluate the selection (against the System's own
                    // state). On error keep the previous selection/geometry and
                    // just surface the message.
                    match mol.data.evaluate(rep.sel_text.as_str()) {
                        Ok((expr, sel)) => {
                            rep.expr = Some(expr);
                            rep.sel = Some(sel);
                            rep.sel_error = None;
                            rep.sel_error_span = None;
                            rep.sel_empty = false;
                            rep.geom_dirty = true;
                        }
                        // Valid selection that matches no atoms: not an error — drop
                        // the geometry (render nothing), keep the text, and flag the
                        // field. The viewport must re-render to clear the old mesh.
                        Err(scene::EvalError::Empty) => {
                            rep.expr = None;
                            rep.sel = None;
                            rep.sel_error = None;
                            rep.sel_error_span = None;
                            rep.sel_empty = true;
                            rep.gpu = Default::default();
                            changed = true;
                        }
                        Err(scene::EvalError::Invalid { message, span }) => {
                            // molar trims the input before parsing, so shift the span
                            // past any leading whitespace to align it with the field's
                            // text (leading whitespace is ASCII, so bytes == chars).
                            let lead = rep
                                .sel_text
                                .bytes()
                                .take_while(|b| *b == b' ' || *b == b'\t')
                                .count();
                            rep.sel_error = Some(message);
                            rep.sel_error_span = span.map(|r| r.start + lead..r.end + lead);
                            rep.sel_empty = false;
                        }
                    }
                    rep.sel_dirty = false;
                }
                let Some(sel) = &rep.sel else {
                    rep.geom_dirty = false;
                    rep.coords_dirty = false;
                    continue;
                };

                // Trajectory smoothing: a transient Savitzky–Golay blend of the
                // frames around `current`, computed here and dropped after the
                // build (nothing stored). Falls back to the raw current frame.
                let smoothed = (rep.smooth_window > 1)
                    .then(|| mol.trajectory.smoothed_state(rep.smooth_window))
                    .flatten();
                let state: &State = smoothed.as_ref().unwrap_or(render_state);

                if rep.geom_dirty {
                    // Full structural rebuild: (re)compute secondary structure
                    // into the cache, build geometry, recreate GPU buffers.
                    let (geom, fresh_ss) = {
                        let bound = mol.data.bind_with_state(sel, state);
                        let ss = geometry::needs_ss(&rep.params, rep.color)
                            .then(|| SsMap::compute(&bound, rep.ss_algo));
                        let geom = geometry::build(
                            &bound, n_atoms, &mol.bonds, &rep.params, rep.color, rep.material,
                            ss.as_ref(), dashed,
                        );
                        (geom, ss)
                    };
                    rep.ss_cache = fresh_ss;
                    rep.gpu = self.renderer.upload(rs, &geom);
                    // Cache the cartoon ribbon CPU mesh (with residue tags) for the
                    // selection glow to extract sub-ribbons from; clear for other styles.
                    rep.cartoon_cache = if matches!(rep.kind, RepKind::Cartoon) {
                        Some(geom.mesh)
                    } else {
                        None
                    };
                    rep.geom_dirty = false;
                    rep.coords_dirty = false;
                    changed = true;
                    rep_geom_changed = true;
                } else if rep.coords_dirty {
                    // Coordinates-only frame change: rebuild geometry reusing the
                    // cached secondary structure (no DSSP), then update the
                    // existing GPU buffers in place (no reallocation).
                    let geom = {
                        let bound = mol.data.bind_with_state(sel, state);
                        geometry::build(
                            &bound, n_atoms, &mol.bonds, &rep.params, rep.color, rep.material,
                            rep.ss_cache.as_ref(), dashed,
                        )
                    };
                    self.renderer.update(rs, &mut rep.gpu, &geom);
                    if matches!(rep.kind, RepKind::Cartoon) {
                        rep.cartoon_cache = Some(geom.mesh); // keep the glow's cache fresh
                    }
                    rep.coords_dirty = false;
                    changed = true;
                    rep_geom_changed = true;
                }
            }
            // Periodic-box wireframe: (re)build when dirty, regardless of whether
            // it's currently shown — both the molecule-level box toggle *and* a
            // rep's periodic `Box` toggle draw this geometry, and the latter isn't
            // tracked by `box_dirty`, so keep `box_gpu` ready whenever a box exists.
            // Use the current frame's box (tracks NPT box changes); fall back to the
            // structure's own box when a trajectory frame carries none.
            if mol.box_dirty {
                let pb = render_state
                    .pbox
                    .as_ref()
                    .or_else(|| mol.data.state().pbox.as_ref());
                let lines = pb.map(geometry::box_wireframe).unwrap_or_default();
                let geom = geometry::GeometryData { lines, ..Default::default() };
                mol.box_gpu = self.renderer.upload(rs, &geom);
                mol.box_dirty = false;
                changed = true;
            }

            // Aromatic-ring circles: depth-tested 3-D line geometry (built from the
            // perceived rings at the displayed coords), so they occlude correctly.
            if mol.aromatic_dirty || (rep_geom_changed && !mol.aromatic_rings.is_empty()) {
                let lines = geometry::aromatic_circles(&mol.aromatic_rings, &render_state.coords);
                let geom = geometry::GeometryData { lines, ..Default::default() };
                mol.aromatic_gpu = self.renderer.upload(rs, &geom);
                mol.aromatic_dirty = false;
                changed = true;
            }

            // If any rep's geometry was rebuilt (style/selection/coords changed) and
            // there's a pending/hover highlight, rebuild its glow so it follows.
            if rep_geom_changed && mol.pending.is_some() {
                mol.glow_dirty = true;
            }
            if rep_geom_changed && mol.hover.is_some() {
                mol.hover_dirty = true;
            }
            if rep_geom_changed && mol.hover_detail.is_some() {
                mol.hover_detail_dirty = true;
            }
            if rep_geom_changed {
                mol.hover_grid = None; // its filtered atom set depends on the reps/coords
            }

            // Active-selection glow: rebuild the pending atoms in each rep's own
            // style (so the highlight glows in the current style), or clear it. Runs
            // after the rep loop so Cartoon reps' `ss_cache` is already populated.
            if mol.glow_dirty {
                let geom = match &mol.pending {
                    Some(pending) => build_glow(
                        &mol.data, &mol.bonds, &mol.reps, &pending.atoms, render_state, n_atoms,
                        dashed,
                    ),
                    None => geometry::GeometryData::default(),
                };
                mol.glow_gpu = self.renderer.upload(rs, &geom);
                mol.glow_dirty = false;
                changed = true;
            }
            // Hover highlight: same builder, the hovered residue's atoms (steady glow).
            if mol.hover_dirty {
                let geom = match &mol.hover {
                    Some(atoms) => build_glow(
                        &mol.data, &mol.bonds, &mol.reps, atoms, render_state, n_atoms, dashed,
                    ),
                    None => geometry::GeometryData::default(),
                };
                mol.hover_gpu = self.renderer.upload(rs, &geom);
                mol.hover_dirty = false;
                changed = true;
            }
            // Hover detail lens: faded CPK ball-and-stick of the atoms near the
            // cursor view-line (built from `hover_detail`), over a Cartoon/Surface rep.
            if mol.hover_detail_dirty {
                let geom = match &mol.hover_detail {
                    Some(d) => {
                        build_hover_detail(&mol.data, &mol.bonds, d, render_state, n_atoms, dashed)
                    }
                    None => geometry::GeometryData::default(),
                };
                mol.hover_detail_gpu = self.renderer.upload(rs, &geom);
                mol.hover_detail_dirty = false;
                changed = true;
            }
            // GPU pick geometry (native): rebuild when the molecule's geometry/coords
            // changed (rep_geom_changed covers both) or it was flagged dirty (init /
            // structure change). Mirrors the atoms CPU `pick` would ray-cast.
            #[cfg(not(target_arch = "wasm32"))]
            if rep_geom_changed || mol.pick_dirty {
                let geom = build_pick(mol, _mi, render_state);
                mol.pick_gpu = self.renderer.upload(rs, &geom);
                mol.pick_dirty = false;
                // No `changed = true`: pick geometry isn't drawn in render_scene, so
                // it doesn't require a scene re-render on its own.
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

        // Native Python module: apply any jobs queued by the Python thread, and —
        // while that channel is connected — keep polling for more (egui only calls
        // `ui` on input/repaint, so without this a job sent while the window is idle
        // wouldn't be picked up). The poll only repaints egui; the 3D scene still
        // re-renders only on an actual change (render-skip), so idle stays cheap.
        if self.jobs_rx.is_some() {
            self.run_external_jobs();
            self.mark_shared_dirty();
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }

        // Work around a winit/egui Wayland IME bug that otherwise breaks all text
        // entry (only the first char of a field is accepted). See `defuse_broken_ime`.
        #[cfg(target_os = "linux")]
        defuse_broken_ime(&ctx);

        // Browser file picker results: load each (filename, bytes) the async picker
        // delivered (see `pick_file`) as a new molecule.
        #[cfg(target_arch = "wasm32")]
        while let Ok((name, bytes)) = self.file_rx.try_recv() {
            match data::load_from_bytes(&name, bytes, &self.settings.behavior.bond_params()) {
                Ok(raw) => self.add_loaded(raw),
                Err(e) => {
                    log::error!("{e}");
                    self.status = e;
                }
            }
        }

        // Browser trajectory picker results: open an incremental stream over the
        // bytes (seeding frame 0 with the structure first), to be drained below.
        #[cfg(target_arch = "wasm32")]
        while let Ok((mol_id, name, bytes)) = self.traj_rx.try_recv() {
            let Some(mol) = self.scene.molecules.iter_mut().find(|m| m.id == mol_id) else {
                continue;
            };
            mol.seed_frame0();
            let expected = mol.n_atoms;
            match data::traj_wasm::TrajStream::new(
                &name,
                bytes,
                LoadOptions::default(),
                expected,
            ) {
                Ok(stream) => {
                    self.wasm_loaders.insert(mol_id, stream);
                    self.status = format!("Loading {name}…");
                }
                Err(e) => {
                    log::error!("{e}");
                    self.status = e;
                }
            }
        }

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

        // Drain background trajectory loaders so the slider reflects arrived frames.
        self.poll_loaders();
        #[cfg(target_arch = "wasm32")]
        self.poll_wasm_loaders(&ctx);

        let panel_dirty = self.draw_left_panel(ui);
        self.view_dirty |= panel_dirty;

        // The "Load trajectory" / "Delete frames" modals float above everything.
        self.draw_load_dialog(&ctx);
        self.draw_delete_frames_dialog(&ctx);
        self.draw_rename_dialog(&ctx);
        self.draw_settings_dialog(&ctx, frame);

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

        // Advance playback for any playing molecule (time-based, so the fps knob
        // is honored regardless of the render rate). `tick` is a no-op unless
        // playing, and stops itself at the ends in play-once mode.
        let dt = ctx.input(|i| i.stable_dt).min(0.1) as f64;
        let mut animating = false;
        let mut frame_advanced = false;
        for mol in &mut self.scene.molecules {
            if mol.trajectory.tick(dt) {
                mol.apply_current_frame();
                frame_advanced = true;
            }
            animating |= mol.trajectory.playing;
        }
        if frame_advanced {
            self.view_dirty = true;
        }
        // Keep repainting while animating or loading; otherwise idle = 0 GPU.
        if animating || !self.loaders.is_empty() {
            ctx.request_repaint();
        }

        // View/selection controls live in a top toolbar above the viewport (right of
        // the left panel); the central panel then fills the rest with the 3D image.
        self.draw_view_toolbar(ui);
        // Vertical drawing-tools palette on the right (only while Draw mode is on);
        // a panel, so it reserves its strip before the viewport fills the rest.
        self.draw_tools_panel(ui);
        // Scripting console as a resizable bottom panel (when open), claimed before
        // the central viewport so the 3D view fills the space above it.
        self.draw_console(ui);
        self.draw_viewport(ui, frame);

        // Record a checkpoint once the gesture has settled (coalesces drags/typing).
        let settled = !ctx.egui_is_using_pointer() && !ctx.egui_wants_keyboard_input();
        if settled {
            self.history.maybe_record(EditState::capture(&self.scene));
        }
    }
}


// Regression test for the Wayland IME workaround (`defuse_broken_ime`). Reproduces the
// broken event stream a recent Wayland/winit combo emits — a flood of `Ime(Disabled)`
// plus one `Ime(Commit)` per keystroke, with no `Enabled`/`Preedit` — which egui's
// `TextEdit` otherwise drops after the first character. Linux-only (the workaround and
// the bug are Linux/Wayland-specific); CI runs on Linux.
#[cfg(all(test, target_os = "linux"))]
mod ime_workaround_tests {
    use super::*;

    fn raw(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(400.0, 400.0),
            )),
            events,
            ..Default::default()
        }
    }

    fn run(ctx: &egui::Context, text: &mut String, id: egui::Id, events: Vec<egui::Event>) {
        let _ = ctx.run_ui(raw(events), |ui| {
            defuse_broken_ime(ui.ctx());
            ui.add(egui::TextEdit::singleline(text).id(id));
        });
    }

    /// Typing `a`,`b`,`c` arrives as `Ime(Commit)` amid `Ime(Disabled)` noise; with the
    /// workaround every character is inserted (without it, egui keeps only the first).
    #[test]
    fn ime_commit_stream_accumulates_into_empty_field() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("f");
        let mut text = String::new();
        ctx.memory_mut(|m| m.request_focus(id));
        run(&ctx, &mut text, id, vec![egui::Event::Ime(egui::ImeEvent::Disabled)]);
        for ch in ["a", "b", "c"] {
            run(
                &ctx,
                &mut text,
                id,
                vec![
                    egui::Event::Ime(egui::ImeEvent::Disabled),
                    egui::Event::Ime(egui::ImeEvent::Commit(ch.into())),
                    egui::Event::Ime(egui::ImeEvent::Disabled),
                ],
            );
        }
        assert_eq!(text, "abc");
    }

    /// The same stream must also append to *pre-existing* text (the cursor starts > 0,
    /// which is the case egui's commit gate rejects outright).
    #[test]
    fn ime_commit_stream_appends_to_existing_text() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("f");
        let mut text = String::from("all");
        ctx.memory_mut(|m| m.request_focus(id));
        // One frame to place the cursor at the end of the existing text.
        run(&ctx, &mut text, id, vec![]);
        for ch in ["X", "Y"] {
            run(
                &ctx,
                &mut text,
                id,
                vec![egui::Event::Ime(egui::ImeEvent::Commit(ch.into()))],
            );
        }
        assert_eq!(text, "allXY");
    }
}
