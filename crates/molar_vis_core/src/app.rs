//! The eframe application: owns UI state, the camera, the scene (molecules and
//! their representations), and the 3D renderer. Lays out the VMD-style left
//! control panel (Scene → Molecules → Representations → Rep controls) plus the
//! central 3D viewport, and only re-renders the scene when something changed.

use std::path::PathBuf;

use eframe::egui;
use molar::prelude::{AtomLike, AtomProvider, Measure, ParticleIterProvider, SsAlgorithm, State};
#[cfg(not(target_arch = "wasm32"))]
use molar::prelude::FileHandler;

use crate::camera::{Ao, Background, BgKind, Camera, Corner, CueMode, DepthCue, Projection};
use crate::color::{ChargeKind, ColorMethod};
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
use crate::scene::{
    self, GroupId, MolId, MoleculeSource, Representation, Reveal, Scene, SettingsTab,
};
use crate::secstruct::SsMap;
#[cfg(not(target_arch = "wasm32"))]
use crate::session::{Session, ViewState};
use crate::settings::{RepDefaults, Settings, ThemeMode};
#[cfg(not(target_arch = "wasm32"))]
use crate::scene::{MolGroup, TrajLoad};
use crate::trajectory::{LoadMode, LoadMsg, LoadOptions, LoopMode, Trajectory};

use egui_phosphor::regular as icon;

// `pub(crate)`: the ray tracer gathers interaction dashes through `build::build_interactions`
// too, so the trace shows the same contact lines the raster does.
mod align_dialog;
pub(crate) mod build;
mod console;
mod dihedral;
#[cfg(not(target_arch = "wasm32"))]
mod docking_dialog;
mod draw;
mod draw_input;
mod export;
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

use draw::DrawSession;
use loaders::{DeleteFramesDialog, LoadDialog};
// Only `ModalState` is named here (the five modal field types); the shell itself is
// imported by each module that draws a modal.
use widgets::ModalState;


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
    /// settings dialog (see [`SettingsDialog`]).
    settings: Settings,
    /// Effective defaults for a new representation = `settings.reps`, with the kind
    /// overridden by the `MOLAR_VIS_DEBUG_REP` env hook. Recomputed when settings
    /// change. Used for the initial rep of each loaded molecule + the add-rep button.
    rep_defaults: RepDefaults,
    /// Open program-settings dialog (the draft being edited + its active tab), if any.
    settings_dialog: Option<SettingsDialog>,
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
    /// Everything that produces an image outside the live rasterized viewport: the ray
    /// tracer's jobs, the image export, and the debug UI screenshot. See [`RtState`].
    rt: RtState,
    /// `(molecule index, rep index)` whose selection field is focused/expanded.
    editing_rep: Option<(usize, usize)>,
    /// Open trajectory-load dialog, if any (one at a time).
    load_dialog: Option<ModalState<LoadDialog>>,
    /// Open "Load docking data…" dialog, if any (native: it reads several files from disk).
    #[cfg(not(target_arch = "wasm32"))]
    docking_dialog: Option<ModalState<docking_dialog::DockingDialog>>,
    /// Open **Analysis ▸ Align…** dialog, if any. A non-modal window (it is driven partly by
    /// clicks on the tree / the 3-D view), so it is not a [`ModalState`].
    align_dialog: Option<align_dialog::AlignDialog>,
    /// Open "delete trajectory frames" dialog, if any.
    delete_frames_dialog: Option<ModalState<DeleteFramesDialog>>,
    /// Open "rename molecule" dialog: the target molecule + the edit buffer.
    rename_dialog: Option<ModalState<RenameDialog>>,
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
    /// Active **"choose a representation"** mode, or `None` — and what the choice is for.
    /// While set, hovering a rep's geometry (viewport) or a rep row (panel) highlights the
    /// whole rep and clicking delivers it; Esc / empty-click cancels. Transient.
    rep_pick: Option<RepPick>,
    /// Open per-type **Settings** dialog of an Interactions rep, if any: which rep, plus
    /// the active type tab. A movable `egui::Window` (`draw_interactions_dialog`) edits
    /// the rep's `InteractionSettings`. Transient.
    interactions_dialog: Option<InteractionsDialog>,
    /// The UI theme the viewport background was last matched to, so a theme change can be
    /// detected (`System` mode also flips at runtime when the desktop does). See
    /// `follow_theme_background`.
    themed_bg: Option<egui::Theme>,
    /// Result of the last [Compute charges] press, as `(rep index, message)` for the
    /// molecule it ran on — shown in that rep's **Color** tab. A leading `!` marks an
    /// error (rendered red); anything else is an informational summary. Transient.
    charge_status: Option<(usize, String)>,
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
    /// The top-bar "view settings" (hamburger) menu: open state, active tab, and the
    /// close-on-click-outside geometry. See [`ViewMenu`].
    view_menu: ViewMenu,
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
    /// Active interactive-drawing session (Draw mode), or `None` when off. Mutually
    /// exclusive with the pick modes (`pick_mode`): turning Draw on forces `pick_mode
    /// = Off`, and choosing any pick mode clears `draw`. See the Draw-mode types at
    /// the bottom of this file.
    draw: Option<DrawSession>,
    /// The in-app scripting console: open state, scrollback, and the REPL behind it.
    #[cfg(feature = "scripting")]
    console: Console,
    /// External command channel for the native Python module (`molar_vis_py`) and the
    /// wasm JavaScript API (`molar_vis_js`): jobs queued from the host are drained + run
    /// with `&mut App` at the top of each `ui()`, so an external driver can control the
    /// running viewer. `None` for the standalone app (native + the trunk web demo);
    /// connected by `molar_vis_py::spawn` / `molar_vis_js::start`. See [`AppJob`].
    jobs_rx: Option<std::sync::mpsc::Receiver<AppJob>>,
}


/// A unit of work run on the viewer (UI) thread with mutable [`App`] access. The
/// native Python module (and the wasm JavaScript API) send these over a channel (e.g.
/// "add this shared molecule", "set this rep's style"); they're drained at the top of
/// each [`App::ui`].
///
/// Native: the closure is `Send` so it can cross the Python→UI thread boundary (it may
/// capture pyo3 `Py<_>` handles, which are `Send`). Wasm is single-threaded — the
/// channel never crosses a thread — so the `Send` bound is dropped there, which lets a
/// closure capture non-`Send` data like the `Rc<System>` the JS `Visualizer` shares.
#[cfg(not(target_arch = "wasm32"))]
pub type AppJob = Box<dyn FnOnce(&mut App) + Send>;
#[cfg(target_arch = "wasm32")]
pub type AppJob = Box<dyn FnOnce(&mut App)>;


/// What a picked representation is for — the destination of the one "choose a rep" gesture
/// (finger cursor, whole-rep glow, click in the tree *or* in the 3-D view, Escape to cancel).
///
/// The gesture is the same wherever the answer goes, so it stays one mechanism with a
/// destination attached rather than one flag per feature that wants it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RepPick {
    /// Assign it as the partner of this Interactions rep, given as `(molecule, rep)`.
    Partner(MolId, usize),
    /// Fill this side of the alignment dialog from it.
    Align(align_dialog::AlignSide),
}

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


/// Offscreen rendering and capture: the ray tracer's jobs plus the two things they feed,
/// the image export and the debug UI screenshot.
///
/// These nine fields are the one genuinely cohesive cluster in `App` — they are read and
/// written together, several at a time, by `draw_viewport`'s trace controller and by
/// `export.rs` (`viewport.rs:122-124` reads three, `:179-188` writes three, `export.rs`
/// writes five in six lines). Nothing outside those two modules touches any of them.
#[derive(Default)]
struct RtState {
    /// Scene geometry changed since the tracer last uploaded it (re-gather before the next
    /// trace). Also set on a camera change, since a multi-order bond's strands and a line's
    /// world radius are baked at gather time from the traced camera.
    scene_dirty: bool,
    /// A trace was just requested but has not started: its "Ray tracing…/Saving…" overlay is
    /// shown for `warm_shown`'s frame first, then the (possibly blocking) scene gather + trace
    /// begin run — so the overlay appears immediately rather than after the gather.
    warm: Option<RtKind>,
    warm_shown: bool,
    /// A trace in progress, pumped a few tile-submits per frame so the UI stays responsive.
    job: Option<RtJob>,
    /// A finished R-key still is showing, held until any camera/scene/size change drops back
    /// to the realtime view.
    still: bool,
    /// Pending "Save image" request: the supersampling scale (× the viewport). Set by the
    /// dialog, serviced after `draw_viewport` (where the wgpu render state is available).
    export_request: Option<u32>,
    /// Wasm only: an in-flight image readback + its download filename, polled each frame until
    /// the GPU→CPU map resolves (native exports synchronously, so it needs no slot).
    #[cfg(target_arch = "wasm32")]
    pending_capture: Option<(crate::render::CaptureReadback, String)>,
    /// Open "Render ▸ Image…" save dialog (the chosen output scale), if any.
    image_dialog: Option<ModalState<ImageDialog>>,
    /// Frames elapsed for the `MOLAR_VIS_DEBUG_SAVE_UI` verification hook (it needs a few to
    /// let the panels settle — and egui's `Area` fade-in run out — before requesting egui's
    /// screenshot). Native-only.
    #[cfg(not(target_arch = "wasm32"))]
    debug_ui_frames: u32,
}

/// A just-requested ray trace, held one frame so its overlay paints before the (blocking)
/// scene gather, then turned into the matching [`RtJob`].
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum RtKind {
    /// The R-key viewport still.
    Still,
    /// A "Save image" file render at `scale ×` the viewport to `path` (the destination is
    /// chosen up front, before the render).
    Save { scale: u32, path: std::path::PathBuf },
}

/// An in-progress ray trace, pumped a few tile-submits per frame so the UI stays responsive.
/// At most one runs at a time (they share the tracer's accumulator + cursor).
// The ray tracer needs compute (WebGPU/native); on the WebGL2 wasm build it never runs, so
// this is dead there.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum RtJob {
    /// The R-key viewport still: traces into the 1× display texture; on completion the result
    /// is held (`rt_still`) until the camera moves.
    Still,
    /// A "Save image" file render at `out` resolution to `path` (native): trace, then read
    /// back + write the PNG to the pre-chosen `path`. `reading` holds the GPU→CPU readback once
    /// the trace has converged.
    Save {
        out: [u32; 2],
        path: std::path::PathBuf,
        reading: Option<crate::render::CaptureReadback>,
    },
}

/// The Render ▸ Image… save dialog: the chosen output size (a multiple of the viewport).
/// Format is PNG-only for now (a disabled dropdown, room to grow).
struct ImageDialog {
    /// Output size as a multiple of the viewport (1× / 2× / 4×).
    scale: u32,
}

/// The in-app scripting console (see `script::console` / `script::engine`).
///
/// Open state, scrollback and REPL are one cluster: every site that touches one touches
/// another — opening the panel also asks the input field for focus, and running a line
/// appends the echo, the REPL's output and any error to the same scrollback.
#[cfg(feature = "scripting")]
#[derive(Default)]
struct Console {
    /// Whether the console panel is showing (toggled from the View menu).
    open: bool,
    /// Scrollback + input buffer + input history.
    ui: crate::script::ScriptConsole,
    /// Persistent Rhai REPL: keeps the engine + a `Scope` alive across input lines, so
    /// `let` bindings survive between them.
    repl: crate::script::ScriptSession,
}

/// The program-settings dialog: a working copy of the settings (edit-then-apply — **Save**
/// commits, **Cancel**/Escape discards) plus its active tab.
struct SettingsDialog {
    draft: Settings,
    tab: SettingsPage,
}

/// The per-type **Settings** dialog of an Interactions rep: which rep it edits (by molecule
/// id, so a reorder can't retarget it) plus the active interaction-type tab.
struct InteractionsDialog {
    mol: MolId,
    rep: usize,
    tab: crate::interactions::InteractionKind,
}

/// The "rename molecule" dialog: the target molecule and the edit buffer.
struct RenameDialog {
    mol: MolId,
    name: String,
}

/// The top-bar view-settings (hamburger) menu.
///
/// Unlike the transient dialogs this is **not** an `Option` whose `Some` means "open": the
/// menu is a toolbar popover the user flips open and shut constantly, and `tab` is sticky
/// across that — reopening lands on the tab last used, not back on Camera. So `open` rides
/// inside, and only `last_rect` is optional.
#[derive(Default)]
struct ViewMenu {
    /// Whether the window is showing. A real `Window` rather than a `Popup` so nested
    /// click-to-open dropdowns / color pickers work; closed manually on a click outside it
    /// (see `view_settings_window`).
    open: bool,
    /// Active tab (Camera / Lighting / Scene).
    tab: ViewTab,
    /// The window's rect **as drawn last frame** — the geometry the user actually clicked
    /// on. The close-on-click-outside test must use this, not the current frame's rect:
    /// switching tabs re-lays-out the (right-pivoted) window in the *same* frame, so the
    /// freshly-narrowed rect no longer covers the leftmost tab the click landed on (see
    /// `view_settings_window`). Cleared when the menu closes, never mid-life.
    last_rect: Option<egui::Rect>,
    /// Whether one of the window's child popups (a dropdown, a colour picker) was open
    /// **last frame**.
    ///
    /// Needed because a popup can extend past the window's own bottom edge, and choosing an
    /// item closes the popup *within the same frame* — so by the time the
    /// close-on-click-outside test runs there is no open popup to detect, and a click on such
    /// an item reads as a click outside the window. That is what dismissed the whole menu when
    /// the depth-cue type was set to `Exp²`: it is the lowest of four items in a dropdown
    /// anchored near the top of the content, and its centre lands ~9 px below the window.
    /// While a popup is up, every click belongs to it — on an item, or outside to dismiss it —
    /// so the window must sit the frame out either way.
    popup_open: bool,
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

impl Scene {
    /// For each shared (pymolar-backed) molecule whose external coordinates changed
    /// since we last rendered, mark its geometry coords-dirty so the render loop
    /// re-reads them. That's how a Python-side `sel.translate(...)` (which mutates the
    /// shared `State` in place) shows up live. Change is detected by polling the
    /// source's coordinate version counter (lock-free, no GIL) and comparing it to the
    /// last-rendered value — so a *static* shared molecule costs nothing and the
    /// viewer's "idle = 0 GPU" still holds. Cheap rebuild (reuses the cached secondary
    /// structure, no DSSP). Only runs while the external (Python) channel is connected.
    fn mark_shared_dirty(&mut self) {
        for mol in &mut self.molecules {
            if !mol.data.is_shared() {
                continue;
            }
            let version = mol.data.coords_version();
            if version == mol.shared_coords_version {
                continue; // coordinates unchanged since the last render
            }
            mol.shared_coords_version = version;
            // Shared-source polling doesn't drive the GPU pick buffer → pick = false.
            mol.mark_coords_dirty(false);
        }
    }
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
                self.reframe_camera(min, max);
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

    // --- View controls (mirror the view-settings UI), for the Python API. Camera
    // mutations re-render automatically (Camera `PartialEq` vs `last_render_camera`);
    // `view_dirty` also covers the non-camera `axes_on`. ---

    /// Orbit the camera by absolute angles in degrees (yaw about up, pitch about right).
    pub fn rotate_view(&mut self, yaw_deg: f32, pitch_deg: f32) {
        self.camera.rotate_deg(yaw_deg, pitch_deg);
        self.view_dirty = true;
    }

    /// Roll the camera about the view axis by an absolute angle (degrees).
    pub fn roll_view(&mut self, deg: f32) {
        self.camera.roll_deg(deg);
        self.view_dirty = true;
    }

    /// Pan by a fraction of the viewport height (`+x` right, `+y` up).
    pub fn pan_view(&mut self, dx: f32, dy: f32) {
        self.camera.pan_fraction(dx, dy);
        self.view_dirty = true;
    }

    /// Zoom by a factor (`>1` closer, `<1` farther).
    pub fn zoom_view(&mut self, factor: f32) {
        self.camera.zoom_by(factor);
        self.view_dirty = true;
    }

    /// Replace the camera with one framing `[min, max]` and seed the user's default view
    /// (projection / background / depth cue / lighting), as a fresh document gets.
    ///
    /// `Camera::frame_bbox` builds a *whole* camera, so it also resets the view settings —
    /// which is why `ViewDefaults::seed_camera` follows it. But the defaults have no entry
    /// for the axes gizmo, so its state has to be carried across by hand or a user's toggle
    /// would silently switch off the first time a molecule lands in an empty scene.
    fn reframe_camera(&mut self, min: glam::Vec3, max: glam::Vec3) {
        let axes = (self.camera.axes_on, self.camera.axes_corner);
        self.camera = Camera::frame_bbox(min, max, self.settings.view.fill);
        self.settings.view.seed_camera(&mut self.camera);
        (self.camera.axes_on, self.camera.axes_corner) = axes;
    }

    /// Re-frame all molecules (zoom-to-fit + default orientation), keeping the current
    /// projection / background / lighting settings.
    pub fn reset_view(&mut self) {
        if let Some((min, max)) = self.scene.bbox() {
            let framed = Camera::frame_bbox(min, max, self.settings.view.fill);
            self.camera.target = framed.target;
            self.camera.distance = framed.distance;
            self.camera.scene_radius = framed.scene_radius;
            self.camera.orientation = framed.orientation;
        }
        self.view_dirty = true;
    }

    /// Perspective or orthographic projection.
    pub fn set_projection(&mut self, projection: Projection) {
        self.camera.projection = projection;
        self.view_dirty = true;
    }

    /// Flat background color (RGB, 0–1).
    pub fn set_background_solid(&mut self, rgb: [f32; 3]) {
        self.camera.background.kind = BgKind::Solid;
        self.camera.background.color = [rgb[0], rgb[1], rgb[2], 1.0];
        self.view_dirty = true;
    }

    /// Vertical gradient background (top/bottom RGB, 0–1).
    pub fn set_background_gradient(&mut self, top: [f32; 3], bottom: [f32; 3]) {
        self.camera.background = Background {
            kind: BgKind::Gradient,
            top: [top[0], top[1], top[2], 1.0],
            bottom: [bottom[0], bottom[1], bottom[2], 1.0],
            ..self.camera.background
        };
        self.view_dirty = true;
    }

    /// Show/hide the orientation-axes gizmo.
    pub fn show_axes(&mut self, on: bool) {
        self.camera.axes_on = on;
        self.view_dirty = true;
    }

    /// Which viewport corner the axes gizmo sits in.
    pub fn set_axes_corner(&mut self, corner: Corner) {
        self.camera.axes_corner = corner;
        self.view_dirty = true;
    }

    /// Depth cueing (fog): falloff `mode`, plus `strength` (back-of-scene opacity) and
    /// `start` (where it begins, as a fraction of scene depth). `enabled = false` off.
    pub fn set_depth_cue(&mut self, enabled: bool, mode: CueMode, strength: f32, start: f32) {
        self.camera.depth_cue = DepthCue { enabled, start, strength, mode };
        self.view_dirty = true;
    }

    /// Screen-space ambient occlusion: `strength` darkening, `radius` in nm.
    pub fn set_ambient_occlusion(&mut self, enabled: bool, strength: f32, radius: f32) {
        self.camera.ao = Ao { enabled, strength, radius };
        self.view_dirty = true;
    }

    /// Real-time cast shadows: `strength` scales how dark shadowed areas get.
    pub fn set_shadows(&mut self, enabled: bool, strength: f32) {
        self.camera.shadow.enabled = enabled;
        self.camera.shadow.strength = strength;
        self.view_dirty = true;
    }

}


impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // No continuous repaint: egui repaints on input (incl. active drags), and
        // we re-render the 3D scene only when it actually changed (see viewport).
        let ctx = ui.ctx().clone();

        // A charge-computation failure is transient: drop it on the next interaction, so a
        // stale error can't sit under the Color tab reading as if it were still true. Tested
        // *before* the panels draw, and `clicked()` fires on release, so the press that ran
        // the computation is already past and the message survives to be seen.
        if self.charge_status.is_some()
            && ctx.input(|i| {
                i.pointer.any_pressed()
                    || i.smooth_scroll_delta != egui::Vec2::ZERO
                    || i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. }))
            })
        {
            self.charge_status = None;
        }

        // Native Python module: apply any jobs queued by the Python thread, and —
        // while that channel is connected — keep polling for more (egui only calls
        // `ui` on input/repaint, so without this a job sent while the window is idle
        // wouldn't be picked up). The poll only repaints egui; the 3D scene still
        // re-renders only on an actual change (render-skip), so idle stays cheap.
        if self.jobs_rx.is_some() {
            self.run_external_jobs();
            self.scene.mark_shared_dirty();
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }

        // Work around a winit/egui Wayland IME bug that otherwise breaks all text
        // entry (only the first char of a field is accepted). See `defuse_broken_ime`.
        #[cfg(target_os = "linux")]
        defuse_broken_ime(&ctx);

        // Camera telemetry (debug): while MOLAR_VIS_DEBUG_CAMERA_LOG=<path> is set,
        // write the live camera as JSON to <path> each frame, so a bug view positioned
        // interactively can be reproduced headlessly with MOLAR_VIS_DEBUG_CAMERA=<path>.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(path) = std::env::var_os("MOLAR_VIS_DEBUG_CAMERA_LOG") {
            if let Ok(json) = serde_json::to_string(&self.camera) {
                let _ = std::fs::write(&path, json);
            }
        }

        // Browser file picker results: load each (filename, bytes) the async picker
        // delivered (see `pick_file`) as a new molecule.
        #[cfg(target_arch = "wasm32")]
        while let Ok((name, bytes)) = self.file_rx.try_recv() {
            let bonds = self.settings.behavior.bond_params();
            let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            if matches!(ext.as_str(), "sdf" | "sd" | "mol") {
                // A multi-molecule SDF/MOL becomes a group; one record = one molecule.
                match data::load_records_from_bytes(&name, bytes, &bonds) {
                    Ok(records) if records.len() >= 2 => {
                        self.add_group(records, MoleculeSource::Bytes { name: name.clone() }, name);
                    }
                    Ok(mut records) => self.add_loaded(records.pop().unwrap()),
                    Err(e) => {
                        log::error!("{e}");
                        self.status = e;
                    }
                }
            } else {
                match data::load_from_bytes(&name, bytes, &bonds) {
                    Ok(raw) => self.add_loaded(raw),
                    Err(e) => {
                        log::error!("{e}");
                        self.status = e;
                    }
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
                    self.scene.wasm_loaders.insert(mol_id, stream);
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
        #[cfg(not(target_arch = "wasm32"))]
        self.draw_docking_dialog(&ctx);
        self.draw_load_dialog(&ctx);
        self.draw_delete_frames_dialog(&ctx);
        self.draw_rename_dialog(&ctx);
        self.draw_image_dialog(&ctx);
        self.draw_settings_dialog(&ctx, frame);
        self.draw_interactions_dialog(&ctx);
        self.draw_align_dialog(&ctx);

        // Apply undo/redo after the panel so list indices stay stable during draw. Each
        // step is replayed individually (a structural delta must be inverted in order —
        // no jump-to-snapshot shortcut); undo/redo apply to the scene in place.
        let mut applied = false;
        if let Some(n) = self.pending_undo_n.take() {
            for _ in 0..n {
                if self.history.undo(&mut self.scene).is_none() {
                    break;
                }
                applied = true;
            }
        }
        if let Some(n) = self.pending_redo_n.take() {
            for _ in 0..n {
                if self.history.redo(&mut self.scene).is_none() {
                    break;
                }
                applied = true;
            }
        }
        if applied {
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
        if animating || !self.scene.loaders.is_empty() {
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
        #[cfg(feature = "scripting")]
        self.draw_console(ui);
        self.draw_viewport(ui, frame);

        // Keep the viewport background in step with the UI theme (see the method).
        self.follow_theme_background(&ctx);

        // Flexible-docking pairs: propagate whichever of {shown pose, receptor frame} moved
        // this frame to the other. After the panels *and* the viewport, so it sees the pose
        // cycle bar, the trajectory bar and the playback tick alike.
        #[cfg(not(target_arch = "wasm32"))]
        self.sync_docking_frames();

        // MOLAR_VIS_DEBUG_SAVE_UI=<path>: capture the whole egui surface (panels included)
        // and quit — the offscreen alternative to screenshotting a real window.
        #[cfg(not(target_arch = "wasm32"))]
        self.rt.service_debug_ui_capture(&ctx);

        // Service a pending "Save image" request here: `frame` (the wgpu render state) is
        // available, and `draw_viewport` has just refreshed `last_size`. Native saves
        // synchronously; wasm stashes the readback for `poll_export` to finish + download.
        if let Some(scale) = self.rt.export_request.take() {
            self.export_image(frame, scale);
        }
        // Drive an in-progress frame-pumped "Save image" ray trace (native), and keep
        // repainting while any trace job runs so it advances each frame.
        #[cfg(not(target_arch = "wasm32"))]
        self.service_rt_save(frame);
        if self.rt.job.is_some() {
            ctx.request_repaint();
        }
        #[cfg(target_arch = "wasm32")]
        self.poll_export(&ctx);

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
