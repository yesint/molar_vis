//! `molar_vis` — the **wasm-bindgen JavaScript API** that drives the molar_vis viewer
//! from a surrounding web page, rendering directly from a structure parsed in-browser:
//!
//! ```js
//! import init, { start, System } from "./molar_vis.js";
//! await init();
//! const vis = start("molar_vis_canvas");          // boot the viewer on a <canvas>
//! const sys = System.from_bytes("p.pdb", bytes);  // parse bytes -> a molar System
//! const mol = vis.add_mol(sys);                    // render it (by reference)
//! const rep = mol.add_rep(sys.select("protein"), "cartoon", "ss");
//! rep.style = "lines";                             // setters apply live
//! vis.rotate(30, 15); vis.projection("perspective");
//! ```
//!
//! This is the web half of the dual-host scripting plan and mirrors the native Python
//! module (`molar_vis_py`) almost line-for-line: the same `Visualizer`/`MolHandle`/
//! `RepHandle` surface, the same command-queue seam ([`AppJob`] over a channel drained
//! in `App::ui`), the same `SharedSource` rendering-by-reference. The differences are
//! all because the browser is single-threaded and owns its own data:
//!
//! - The viewer runs on the **same thread** as the page (eframe's `WebRunner` under
//!   `requestAnimationFrame`), so the `AppJob` channel never crosses a thread — the wasm
//!   `AppJob` alias has no `Send` bound, which lets a job capture the non-`Send`
//!   `Rc<System>` the JS `System` shares.
//! - There is no external owner like pymolar: a JS `System` **owns** its molar `System`
//!   in an `Rc`, so [`WebSystemSource`] implements [`SharedSource`] with plain safe
//!   borrows (no raw pointers / GIL). v1 coordinates are static after load
//!   (`coords_version` is constant); live JS coordinate edits are a future item.

#![cfg(target_arch = "wasm32")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::Sender;

use wasm_bindgen::prelude::*;

use molar::prelude::{
    FileHandler, IndexSliceProvider, Sel as MSel, SelectionExpr, State, System as MSystem, Topology,
};
use molar_vis_core::eframe;
use molar_vis_core::{
    parse_corner, parse_cue_mode, parse_projection, App, AppJob, AppLaunch, CueMode, EvalError,
    SharedSource,
};

// At most one viewer per page for v1 (single-threaded wasm + one global job channel).
thread_local! {
    static VIEWER_STARTED: Cell<bool> = const { Cell::new(false) };
}

/// Queue a job onto the viewer (UI) thread; logs if the window has been closed.
fn send(jobs: &Sender<AppJob>, job: AppJob) {
    if jobs.send(job).is_err() {
        log::error!("molar_vis: the viewer window is closed");
    }
}

/// Format a selection-evaluation failure for JS.
fn eval_error_msg(e: &EvalError) -> String {
    match e {
        EvalError::Empty => "selection matched no atoms".to_string(),
        EvalError::Invalid { message, .. } => message.clone(),
    }
}

/// A [`SharedSource`] backed by an `Rc<System>` owned jointly with the JS [`System`]
/// handle. The `Rc` keeps the `System` alive for as long as the source, so the borrows
/// returned by `topology`/`state` are sound — no raw pointers, no `unsafe`, no `Send`
/// (single-threaded wasm; the trait has no `Send` bound and the wasm `AppJob` doesn't
/// require it). `version` is a generation counter the viewer polls to re-render on a
/// coordinate edit; it stays `0` in v1 (coordinates are static after load).
struct WebSystemSource {
    system: Rc<MSystem>,
    version: Rc<Cell<u64>>,
}

impl SharedSource for WebSystemSource {
    fn topology(&self) -> &Topology {
        self.system.topology()
    }

    fn state(&self) -> &State {
        self.system.state()
    }

    fn coords_version(&self) -> u64 {
        self.version.get()
    }

    fn evaluate(&self, text: &str) -> Result<(SelectionExpr, MSel), EvalError> {
        // We own a full molar `System`, so compile + run the selection against it
        // directly via the core helper (same path the owned/standalone app uses).
        molar_vis_core::evaluate(&self.system, text)
    }
}

/// A structure parsed in the browser. Owns a molar `System` in an `Rc`; `add_mol`
/// shares it into the scene by reference (a cheap `Rc` clone), so the `System` stays
/// usable from JS (e.g. for more `select` calls) afterward.
#[wasm_bindgen]
pub struct System {
    system: Rc<MSystem>,
    version: Rc<Cell<u64>>,
    name: String,
    n_atoms: usize,
}

#[wasm_bindgen]
impl System {
    /// Parse a structure from in-memory bytes (a fetched/uploaded `Uint8Array`). The
    /// format is taken from `name`'s extension (pdb/gro/xyz/…). Throws on a read/parse
    /// error.
    pub fn from_bytes(name: &str, bytes: &[u8]) -> Result<System, JsError> {
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let mut fh = FileHandler::from_reader(&ext, std::io::Cursor::new(bytes.to_vec()))
            .map_err(|e| JsError::new(&format!("can't read {name}: {e}")))?;
        let (top, st) = fh
            .read()
            .map_err(|e| JsError::new(&format!("failed to parse {name}: {e}")))?;
        let n_atoms = top.atoms.len();
        let system =
            MSystem::new(top, st).map_err(|e| JsError::new(&format!("invalid structure in {name}: {e}")))?;
        Ok(System {
            system: Rc::new(system),
            version: Rc::new(Cell::new(0)),
            name: name.to_string(),
            n_atoms,
        })
    }

    /// Evaluate a VMD-like selection string against this structure, returning a `Sel`
    /// (its atom indices). Throws on a syntax error or an empty match.
    pub fn select(&self, text: &str) -> Result<Sel, JsError> {
        let (_expr, sel) =
            molar_vis_core::evaluate(&self.system, text).map_err(|e| JsError::new(&eval_error_msg(&e)))?;
        Ok(Sel { indices: sel.get_index_slice().to_vec() })
    }

    #[wasm_bindgen(getter)]
    pub fn name(&self) -> String {
        self.name.clone()
    }

    #[wasm_bindgen(getter, js_name = numAtoms)]
    pub fn num_atoms(&self) -> usize {
        self.n_atoms
    }
}

/// A frozen set of atom indices (the result of `System.select`). Consumed by
/// `add_rep`/`select` to set a representation's selection.
#[wasm_bindgen]
pub struct Sel {
    indices: Vec<usize>,
}

#[wasm_bindgen]
impl Sel {
    #[wasm_bindgen(getter, js_name = numAtoms)]
    pub fn num_atoms(&self) -> usize {
        self.indices.len()
    }
}

/// Per-molecule representation counts, shared between the [`Visualizer`] and its handles
/// (the single-threaded analog of the Python module's `Arc<Mutex<Vec<usize>>>`).
/// Molecules/reps are added only from JS (append-only), so this mirrors the scene's
/// structure without a query channel back from the viewer. Index `i` = molecule `i`'s
/// current rep count.
type VisState = Rc<RefCell<Vec<usize>>>;

/// A live handle to the running viewer. Returned by [`start`]; queues jobs to the
/// viewer, which applies them on the next frame. All methods apply live.
#[wasm_bindgen]
pub struct Visualizer {
    jobs: Sender<AppJob>,
    state: VisState,
}

#[wasm_bindgen]
impl Visualizer {
    /// Add a molecule, rendering **directly** from the parsed `System` (by reference);
    /// returns a handle.
    pub fn add_mol(&self, sys: &System) -> MolHandle {
        let mol = {
            let mut s = self.state.borrow_mut();
            let idx = s.len();
            s.push(1); // a new molecule comes with one default representation
            idx
        };
        let name = format!("mol {mol}");
        let src = WebSystemSource { system: sys.system.clone(), version: sys.version.clone() };
        send(
            &self.jobs,
            Box::new(move |app: &mut App| {
                if let Err(e) = app.add_shared_molecule(Box::new(src), name) {
                    log::error!("add_mol failed: {e}");
                }
            }),
        );
        MolHandle { mol, jobs: self.jobs.clone(), state: self.state.clone() }
    }

    /// The molecules currently in the scene, as handles.
    #[wasm_bindgen(getter)]
    pub fn mols(&self) -> Vec<MolHandle> {
        let n = self.state.borrow().len();
        (0..n)
            .map(|mol| MolHandle { mol, jobs: self.jobs.clone(), state: self.state.clone() })
            .collect()
    }

    // --- View controls (mirror the view-settings UI + the Python module). ---

    /// Orbit the camera by absolute angles in degrees (yaw about up, pitch about right).
    pub fn rotate(&self, yaw: f32, pitch: f32) {
        send(&self.jobs, Box::new(move |app| app.rotate_view(yaw, pitch)));
    }

    /// Roll the camera about the view axis by an angle in degrees.
    pub fn roll(&self, angle: f32) {
        send(&self.jobs, Box::new(move |app| app.roll_view(angle)));
    }

    /// Pan by a fraction of the viewport height (`+x` right, `+y` up).
    pub fn pan(&self, dx: f32, dy: f32) {
        send(&self.jobs, Box::new(move |app| app.pan_view(dx, dy)));
    }

    /// Zoom by a factor (`>1` closer, `<1` farther).
    pub fn zoom(&self, factor: f32) {
        send(&self.jobs, Box::new(move |app| app.zoom_view(factor)));
    }

    /// Re-frame all molecules (zoom-to-fit), keeping projection/background/lighting.
    pub fn reset_view(&self) {
        send(&self.jobs, Box::new(|app| app.reset_view()));
    }

    /// Set the projection: `"perspective"` or `"orthographic"`.
    pub fn projection(&self, mode: &str) -> Result<(), JsError> {
        let p = parse_projection(mode).map_err(|e| JsError::new(&e))?;
        send(&self.jobs, Box::new(move |app| app.set_projection(p)));
        Ok(())
    }

    /// Flat background color, RGB in 0–1.
    pub fn background(&self, r: f32, g: f32, b: f32) {
        send(&self.jobs, Box::new(move |app| app.set_background_solid([r, g, b])));
    }

    /// Vertical gradient background; `top`/`bottom` are `[r, g, b]` in 0–1.
    pub fn background_gradient(&self, top: &[f32], bottom: &[f32]) -> Result<(), JsError> {
        if top.len() < 3 || bottom.len() < 3 {
            return Err(JsError::new("background_gradient expects [r, g, b] arrays"));
        }
        let (t, b) = ([top[0], top[1], top[2]], [bottom[0], bottom[1], bottom[2]]);
        send(&self.jobs, Box::new(move |app| app.set_background_gradient(t, b)));
        Ok(())
    }

    /// Show/hide the orientation-axes gizmo, optionally setting its `corner`
    /// (`"top_left"` / `"top_right"` / `"bottom_left"` / `"bottom_right"`).
    pub fn axes(&self, show: bool, corner: Option<String>) -> Result<(), JsError> {
        let c = corner
            .map(|s| parse_corner(&s))
            .transpose()
            .map_err(|e| JsError::new(&e))?;
        send(
            &self.jobs,
            Box::new(move |app| {
                app.show_axes(show);
                if let Some(c) = c {
                    app.set_axes_corner(c);
                }
            }),
        );
        Ok(())
    }

    /// Depth cueing (fog): `mode` is `"linear"`/`"exp"`/`"exp2"` (or `"none"`/`"off"` to
    /// disable); `strength` = opacity at the back, `start` = where it begins (0–1).
    pub fn depth_cue(&self, enabled: bool, mode: &str, strength: f32, start: f32) -> Result<(), JsError> {
        let (enabled, mode) = match mode.to_ascii_lowercase().as_str() {
            "none" | "off" => (false, CueMode::Linear),
            other => (enabled, parse_cue_mode(other).map_err(|e| JsError::new(&e))?),
        };
        send(&self.jobs, Box::new(move |app| app.set_depth_cue(enabled, mode, strength, start)));
        Ok(())
    }

    /// Screen-space ambient occlusion: `strength` darkening, `radius` in nm.
    pub fn ambient_occlusion(&self, enabled: bool, strength: f32, radius: f32) {
        send(&self.jobs, Box::new(move |app| app.set_ambient_occlusion(enabled, strength, radius)));
    }

    /// Real-time cast shadows: `strength` scales how dark shadowed areas get.
    pub fn shadows(&self, enabled: bool, strength: f32) {
        send(&self.jobs, Box::new(move |app| app.set_shadows(enabled, strength)));
    }
}

/// Handle to a molecule in the viewer.
#[wasm_bindgen]
pub struct MolHandle {
    mol: usize,
    jobs: Sender<AppJob>,
    state: VisState,
}

#[wasm_bindgen]
impl MolHandle {
    /// Add a representation. `sel` (a `Sel`, consumed) sets the selection;
    /// `style`/`color`/`material` are names like `"cartoon"`/`"ss"`/`"Transparent"`.
    /// Any argument may be omitted. Returns a handle.
    pub fn add_rep(
        &self,
        sel: Option<Sel>,
        style: Option<String>,
        color: Option<String>,
        material: Option<String>,
    ) -> RepHandle {
        let rep = {
            let mut s = self.state.borrow_mut();
            let cnt = s.get_mut(self.mol).expect("stale molecule handle");
            let idx = *cnt;
            *cnt += 1;
            idx
        };
        let indices = sel.map(|s| s.indices);
        let mol = self.mol;
        send(
            &self.jobs,
            Box::new(move |app: &mut App| {
                if let Err(e) = app.add_rep_default(mol) {
                    log::error!("add_rep failed: {e}");
                    return;
                }
                if let Some(idx) = &indices {
                    if let Err(e) = app.set_rep_selection(mol, rep, idx) {
                        log::error!("add_rep select failed: {e}");
                    }
                }
                if let Some(st) = &style {
                    if let Err(e) = app.set_rep_style(mol, rep, st) {
                        log::error!("add_rep style failed: {e}");
                    }
                }
                if let Some(c) = &color {
                    if let Err(e) = app.set_rep_color(mol, rep, c) {
                        log::error!("add_rep color failed: {e}");
                    }
                }
                if let Some(m) = &material {
                    if let Err(e) = app.set_rep_material(mol, rep, m) {
                        log::error!("add_rep material failed: {e}");
                    }
                }
            }),
        );
        RepHandle { mol, rep, jobs: self.jobs.clone() }
    }

    /// This molecule's representations, as handles.
    #[wasm_bindgen(getter)]
    pub fn reps(&self) -> Vec<RepHandle> {
        let cnt = self.state.borrow().get(self.mol).copied().unwrap_or(0);
        (0..cnt)
            .map(|rep| RepHandle { mol: self.mol, rep, jobs: self.jobs.clone() })
            .collect()
    }
}

/// Handle to a representation. Property setters (`rep.style = "lines"`, …) apply live.
#[wasm_bindgen]
pub struct RepHandle {
    mol: usize,
    rep: usize,
    jobs: Sender<AppJob>,
}

#[wasm_bindgen]
impl RepHandle {
    #[wasm_bindgen(setter)]
    pub fn set_style(&self, value: String) {
        let (mol, rep) = (self.mol, self.rep);
        send(&self.jobs, Box::new(move |app| {
            if let Err(e) = app.set_rep_style(mol, rep, &value) {
                log::error!("set style: {e}");
            }
        }));
    }

    #[wasm_bindgen(setter)]
    pub fn set_color(&self, value: String) {
        let (mol, rep) = (self.mol, self.rep);
        send(&self.jobs, Box::new(move |app| {
            if let Err(e) = app.set_rep_color(mol, rep, &value) {
                log::error!("set color: {e}");
            }
        }));
    }

    #[wasm_bindgen(setter)]
    pub fn set_material(&self, value: String) {
        let (mol, rep) = (self.mol, self.rep);
        send(&self.jobs, Box::new(move |app| {
            if let Err(e) = app.set_rep_material(mol, rep, &value) {
                log::error!("set material: {e}");
            }
        }));
    }

    /// Set the selection from a `Sel`'s atoms.
    pub fn select(&self, sel: &Sel) {
        let (mol, rep) = (self.mol, self.rep);
        let indices = sel.indices.clone();
        send(&self.jobs, Box::new(move |app| {
            if let Err(e) = app.set_rep_selection(mol, rep, &indices) {
                log::error!("select: {e}");
            }
        }));
    }
}

/// Boot the viewer onto an existing `<canvas id=canvas_id>` and return a handle to it.
/// Runs the eframe `WebRunner` on the page's animation-frame loop (single-threaded), so
/// it shares the page's thread; jobs sent through the returned [`Visualizer`] are
/// applied on the next frame. Throws if the canvas is missing or a viewer is already
/// running on this page (one viewer per page in v1).
#[wasm_bindgen]
pub fn start(canvas_id: &str) -> Result<Visualizer, JsError> {
    use wasm_bindgen::JsCast as _;

    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);

    if VIEWER_STARTED.with(|f| f.replace(true)) {
        return Err(JsError::new("a molar_vis viewer is already running on this page"));
    }

    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| JsError::new("no document"))?;
    let canvas = document
        .get_element_by_id(canvas_id)
        .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .ok_or_else(|| JsError::new(&format!("page has no <canvas id=\"{canvas_id}\">")))?;

    let (tx, rx) = std::sync::mpsc::channel::<AppJob>();

    wasm_bindgen_futures::spawn_local(async move {
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |cc| {
                    let mut app = App::new(cc, AppLaunch::default())?;
                    app.set_jobs(rx);
                    Ok(Box::new(app) as Box<dyn eframe::App>)
                }),
            )
            .await;
        match result {
            Ok(()) => log::info!("molar_vis: web runner started"),
            Err(e) => log::error!("molar_vis failed to start: {e:?}"),
        }
    });

    Ok(Visualizer { jobs: tx, state: Rc::new(RefCell::new(Vec::new())) })
}
