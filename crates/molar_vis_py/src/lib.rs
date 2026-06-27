//! `molar_vis` — the **native Python module** that drives the molar_vis viewer from
//! Python, rendering **directly** from pymolar objects (zero-copy):
//!
//! ```python
//! import molar_vis as mv
//! s   = mv.System('1.pdb')          # pymolar System (re-exported here)
//! sel = s('name CA')
//! vis = mv.spawn()                  # opens the viewer, stays responsive
//! mol = vis.add_mol(s)             # share s's data — no copy
//! rep = mol.add_rep(sel, style='cartoon', color='ss')
//! sel.translate([1,0,0])           # the view updates live
//! ```
//!
//! The single extension module re-exports pymolar's pyclasses (`System`/`Sel`/…) via
//! [`molar_python::register_molar`] *and* the viewer API, so a `System` created here
//! and the viewer share one PyO3 type identity.
//!
//! This file currently lands the **data bridge** ([`PySystemSource`], the
//! [`SharedSource`] impl that lets the viewer's [`MolData::Shared`] read a pymolar
//! `System` by reference) and the module skeleton. The non-blocking event loop
//! (`spawn`) and the handle API (`add_mol`/`add_rep`/setters) are the next steps.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use pyo3::prelude::*;

use molar::prelude::{Sel, SelectionDef, SelectionExpr, State, Topology};
use molar_python::{Sel as PySel, System as PySystem};
use molar_vis_core::eframe;
use molar_vis_core::{App, AppJob, AppLaunch, Corner, CueMode, EvalError, Projection, SharedSource};

use pyo3::exceptions::PyValueError;

fn parse_projection(s: &str) -> PyResult<Projection> {
    match s.to_ascii_lowercase().as_str() {
        "perspective" | "persp" | "p" => Ok(Projection::Perspective),
        "orthographic" | "ortho" | "o" => Ok(Projection::Orthographic),
        _ => Err(PyValueError::new_err(format!(
            "unknown projection {s:?} (use 'perspective' or 'orthographic')"
        ))),
    }
}

fn parse_corner(s: &str) -> PyResult<Corner> {
    match s.to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
        "top_left" | "tl" => Ok(Corner::TopLeft),
        "top_right" | "tr" => Ok(Corner::TopRight),
        "bottom_left" | "bl" => Ok(Corner::BottomLeft),
        "bottom_right" | "br" => Ok(Corner::BottomRight),
        _ => Err(PyValueError::new_err(format!(
            "unknown corner {s:?} (top_left/top_right/bottom_left/bottom_right)"
        ))),
    }
}

fn parse_cue_mode(s: &str) -> PyResult<CueMode> {
    match s.to_ascii_lowercase().as_str() {
        "linear" => Ok(CueMode::Linear),
        "exp" => Ok(CueMode::Exp),
        "exp2" | "exp²" => Ok(CueMode::Exp2),
        _ => Err(PyValueError::new_err(format!(
            "unknown depth-cue mode {s:?} (linear/exp/exp2, or 'none' to disable)"
        ))),
    }
}

/// A [`SharedSource`] backed by a pymolar `System` — the viewer renders its topology
/// and live coordinates **by reference**.
///
/// `top`/`st` are raw pointers into Python-managed memory (the `Topology`/`State`
/// inside the `System`'s `TopologyPy`/`StatePy`), kept alive by the `Py<System>`
/// handle. This mirrors pymolar's own `UnsafeCell` model: access is sound as long as
/// it's serialized under the GIL and the topology isn't reallocated (coordinate
/// mutation — e.g. `sel.translate` — writes in place; a structural change bumps a
/// version the viewer re-syncs on, refreshing these pointers). Constructed only on a
/// thread holding the GIL ([`PySystemSource::new`]).
struct PySystemSource {
    sys: Py<PySystem>,
    top: *const Topology,
    st: *const State,
    /// The backing `State`'s coordinate generation counter (inside `StatePy`, kept
    /// alive by `sys`). Polled lock-free by the viewer to detect external coord edits.
    version: *const std::sync::atomic::AtomicU64,
}

// The pointers reference GIL-guarded, `Py`-kept-alive memory; the source is moved to
// the viewer thread together with its `Py` handle and only read under the GIL.
unsafe impl Send for PySystemSource {}

impl PySystemSource {
    fn new(py: Python<'_>, sys: Py<PySystem>) -> Self {
        let sp = sys.bind(py).get();
        let top = sp.r_top() as *const Topology;
        let st = sp.r_st() as *const State;
        let version = sp.py_st().get().coords_version_atomic() as *const std::sync::atomic::AtomicU64;
        Self { sys, top, st, version }
    }
}

impl SharedSource for PySystemSource {
    fn topology(&self) -> &Topology {
        // SAFETY: see the struct docs — GIL-guarded, kept alive by `self.sys`.
        unsafe { &*self.top }
    }

    fn state(&self) -> &State {
        unsafe { &*self.st }
    }

    fn coords_version(&self) -> u64 {
        // SAFETY: the atomic lives in the `StatePy` kept alive by `self.sys`; an
        // atomic load needs no GIL. `Acquire` pairs with pymolar's `Release` bump so
        // we observe the coordinate writes that preceded the version we read.
        unsafe { (*self.version).load(std::sync::atomic::Ordering::Acquire) }
    }

    fn evaluate(&self, text: &str) -> Result<(SelectionExpr, Sel), EvalError> {
        // pymolar's `System` is a full selection provider, so apply the compiled
        // expression to it directly (mirrors pymolar's own `__call__`).
        Python::attach(|py| {
            let sp = self.sys.bind(py).get();
            let expr = SelectionExpr::new(text)
                .map_err(|e| EvalError::Invalid { message: e.to_string(), span: None })?;
            match (&expr).into_sel_index(sp, None) {
                Ok(svec) => Sel::from_svec(svec)
                    .map(|sel| (expr, sel))
                    .map_err(|_| EvalError::Empty),
                Err(e) => Err(EvalError::Invalid { message: e.to_string(), span: None }),
            }
        })
    }
}

/// Send a job to the viewer thread; errors if the window has closed.
fn send_job(jobs: &Sender<AppJob>, job: AppJob) -> PyResult<()> {
    jobs.send(job)
        .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("viewer window is closed"))
}

/// Per-molecule representation counts, shared between the [`Visualizer`] and its
/// handles. Molecules/reps are added only from Python (append-only) here, so this
/// mirrors the scene's structure without needing a query channel back from the UI
/// thread. Index `i` = molecule `i`'s current rep count.
type VisState = Arc<Mutex<Vec<usize>>>;

/// A live handle to a running viewer window. Returned by [`spawn`]; queues jobs to
/// the viewer (UI) thread.
#[pyclass]
struct Visualizer {
    jobs: Sender<AppJob>,
    state: VisState,
}

#[pymethods]
impl Visualizer {
    /// Add a molecule, rendering **directly** from the pymolar `System` (zero-copy);
    /// returns a handle. The window updates as soon as the UI thread picks it up.
    fn add_mol(&self, sys: Py<PySystem>) -> PyResult<MolHandle> {
        let mol = {
            let mut s = self.state.lock().unwrap();
            let idx = s.len();
            s.push(1); // a new molecule comes with one default representation
            idx
        };
        let name = format!("mol {mol}");
        send_job(
            &self.jobs,
            Box::new(move |app: &mut App| {
                let src = Python::attach(|py| PySystemSource::new(py, sys));
                if let Err(e) = app.add_shared_molecule(Box::new(src), name) {
                    log::error!("add_mol failed: {e}");
                }
            }),
        )?;
        Ok(MolHandle { mol, jobs: self.jobs.clone(), state: self.state.clone() })
    }

    /// The molecules currently in the scene, as handles.
    #[getter]
    fn mols(&self) -> Vec<MolHandle> {
        let n = self.state.lock().unwrap().len();
        (0..n)
            .map(|mol| MolHandle { mol, jobs: self.jobs.clone(), state: self.state.clone() })
            .collect()
    }

    // --- View controls (mirror the view-settings UI). All apply live. ---

    /// Orbit the camera by absolute angles in degrees (yaw about up, pitch about right).
    fn rotate(&self, yaw: f32, pitch: f32) -> PyResult<()> {
        send_job(&self.jobs, Box::new(move |app| app.rotate_view(yaw, pitch)))
    }

    /// Roll the camera about the view axis by an angle in degrees.
    fn roll(&self, angle: f32) -> PyResult<()> {
        send_job(&self.jobs, Box::new(move |app| app.roll_view(angle)))
    }

    /// Pan by a fraction of the viewport height (`+x` right, `+y` up).
    fn pan(&self, dx: f32, dy: f32) -> PyResult<()> {
        send_job(&self.jobs, Box::new(move |app| app.pan_view(dx, dy)))
    }

    /// Zoom by a factor (`>1` closer, `<1` farther).
    fn zoom(&self, factor: f32) -> PyResult<()> {
        send_job(&self.jobs, Box::new(move |app| app.zoom_view(factor)))
    }

    /// Re-frame all molecules (zoom-to-fit), keeping projection/background/lighting.
    fn reset_view(&self) -> PyResult<()> {
        send_job(&self.jobs, Box::new(|app| app.reset_view()))
    }

    /// Set the projection: `"perspective"` or `"orthographic"`.
    fn projection(&self, mode: &str) -> PyResult<()> {
        let p = parse_projection(mode)?;
        send_job(&self.jobs, Box::new(move |app| app.set_projection(p)))
    }

    /// Flat background color, RGB in 0–1.
    fn background(&self, r: f32, g: f32, b: f32) -> PyResult<()> {
        send_job(&self.jobs, Box::new(move |app| app.set_background_solid([r, g, b])))
    }

    /// Vertical gradient background; `top`/`bottom` are `(r, g, b)` in 0–1.
    fn background_gradient(&self, top: (f32, f32, f32), bottom: (f32, f32, f32)) -> PyResult<()> {
        send_job(
            &self.jobs,
            Box::new(move |app| {
                app.set_background_gradient([top.0, top.1, top.2], [bottom.0, bottom.1, bottom.2])
            }),
        )
    }

    /// Show/hide the orientation-axes gizmo, optionally setting its `corner`
    /// (`"top_left"` / `"top_right"` / `"bottom_left"` / `"bottom_right"`).
    #[pyo3(signature = (show=true, corner=None))]
    fn axes(&self, show: bool, corner: Option<&str>) -> PyResult<()> {
        let c = corner.map(parse_corner).transpose()?;
        send_job(
            &self.jobs,
            Box::new(move |app| {
                app.show_axes(show);
                if let Some(c) = c {
                    app.set_axes_corner(c);
                }
            }),
        )
    }

    /// Depth cueing (fog): `mode` is `"linear"`/`"exp"`/`"exp2"` (or `"none"` to
    /// disable); `strength` = opacity at the back, `start` = where it begins (0–1).
    #[pyo3(signature = (enabled=true, mode="linear", strength=0.55, start=0.3))]
    fn depth_cue(&self, enabled: bool, mode: &str, strength: f32, start: f32) -> PyResult<()> {
        let (enabled, mode) = match mode.to_ascii_lowercase().as_str() {
            "none" | "off" => (false, CueMode::Linear),
            other => (enabled, parse_cue_mode(other)?),
        };
        send_job(&self.jobs, Box::new(move |app| app.set_depth_cue(enabled, mode, strength, start)))
    }

    /// Screen-space ambient occlusion: `strength` darkening, `radius` in nm.
    #[pyo3(signature = (enabled=true, strength=0.9, radius=0.4))]
    fn ambient_occlusion(&self, enabled: bool, strength: f32, radius: f32) -> PyResult<()> {
        send_job(&self.jobs, Box::new(move |app| app.set_ambient_occlusion(enabled, strength, radius)))
    }

    /// Real-time cast shadows: `strength` scales how dark shadowed areas get.
    #[pyo3(signature = (enabled=true, strength=0.6))]
    fn shadows(&self, enabled: bool, strength: f32) -> PyResult<()> {
        send_job(&self.jobs, Box::new(move |app| app.set_shadows(enabled, strength)))
    }

    fn __repr__(&self) -> String {
        format!("<molar_vis.Visualizer: {} molecule(s)>", self.state.lock().unwrap().len())
    }
}

/// Handle to a molecule in the viewer.
#[pyclass]
struct MolHandle {
    mol: usize,
    jobs: Sender<AppJob>,
    state: VisState,
}

#[pymethods]
impl MolHandle {
    /// Add a representation. `sel` is a pymolar `Sel` (its atoms become the rep's
    /// selection); `style`/`color` are names like "cartoon"/"ss". Returns a handle.
    #[pyo3(signature = (sel=None, style=None, color=None, material=None))]
    fn add_rep(
        &self,
        py: Python<'_>,
        sel: Option<Py<PySel>>,
        style: Option<String>,
        color: Option<String>,
        material: Option<String>,
    ) -> PyResult<RepHandle> {
        let rep = {
            let mut s = self.state.lock().unwrap();
            let cnt = s
                .get_mut(self.mol)
                .ok_or_else(|| pyo3::exceptions::PyRuntimeError::new_err("stale molecule handle"))?;
            let idx = *cnt;
            *cnt += 1;
            idx
        };
        // Read the selection's atom indices now (we hold the GIL here).
        let indices: Option<Vec<usize>> = sel.map(|s| s.bind(py).get().index().to_vec());
        let mol = self.mol;
        send_job(
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
        )?;
        Ok(RepHandle { mol, rep, jobs: self.jobs.clone() })
    }

    /// This molecule's representations, as handles (for `for rep in mol.reps: …`).
    #[getter]
    fn reps(&self) -> Vec<RepHandle> {
        let cnt = self.state.lock().unwrap().get(self.mol).copied().unwrap_or(0);
        (0..cnt)
            .map(|rep| RepHandle { mol: self.mol, rep, jobs: self.jobs.clone() })
            .collect()
    }

    fn __repr__(&self) -> String {
        format!("<molar_vis.MolHandle mol={}>", self.mol)
    }
}

/// Handle to a representation. Property setters (`rep.style = "lines"`, …) apply live.
#[pyclass]
struct RepHandle {
    mol: usize,
    rep: usize,
    jobs: Sender<AppJob>,
}

#[pymethods]
impl RepHandle {
    #[setter]
    fn set_style(&self, value: &str) -> PyResult<()> {
        let (mol, rep, v) = (self.mol, self.rep, value.to_string());
        send_job(&self.jobs, Box::new(move |app| {
            if let Err(e) = app.set_rep_style(mol, rep, &v) { log::error!("set style: {e}"); }
        }))
    }

    #[setter]
    fn set_color(&self, value: &str) -> PyResult<()> {
        let (mol, rep, v) = (self.mol, self.rep, value.to_string());
        send_job(&self.jobs, Box::new(move |app| {
            if let Err(e) = app.set_rep_color(mol, rep, &v) { log::error!("set color: {e}"); }
        }))
    }

    #[setter]
    fn set_material(&self, value: &str) -> PyResult<()> {
        let (mol, rep, v) = (self.mol, self.rep, value.to_string());
        send_job(&self.jobs, Box::new(move |app| {
            if let Err(e) = app.set_rep_material(mol, rep, &v) { log::error!("set material: {e}"); }
        }))
    }

    /// Set the selection from a pymolar `Sel`'s atoms.
    fn select(&self, py: Python<'_>, sel: Py<PySel>) -> PyResult<()> {
        let indices = sel.bind(py).get().index().to_vec();
        let (mol, rep) = (self.mol, self.rep);
        send_job(&self.jobs, Box::new(move |app| {
            if let Err(e) = app.set_rep_selection(mol, rep, &indices) { log::error!("select: {e}"); }
        }))
    }

    fn __repr__(&self) -> String {
        format!("<molar_vis.RepHandle mol={} rep={}>", self.mol, self.rep)
    }
}

/// Open the viewer window and return immediately — it runs on a background thread, so
/// the Python REPL stays responsive. Jobs (add molecules, change reps, …) are sent to
/// it through the returned [`Visualizer`].
///
/// The window's event loop runs off the main thread (`with_any_thread`), which winit
/// supports on Linux/Windows; macOS requires the main thread and isn't handled yet.
#[pyfunction]
fn spawn() -> PyResult<Visualizer> {
    let (tx, rx) = std::sync::mpsc::channel::<AppJob>();

    std::thread::Builder::new()
        .name("molar_vis-ui".into())
        .spawn(move || {
            let native_options = eframe::NativeOptions {
                renderer: eframe::Renderer::Wgpu,
                event_loop_builder: Some(Box::new(|builder| {
                    // Permit the event loop on this non-main thread.
                    #[cfg(target_os = "linux")]
                    {
                        use winit::platform::wayland::EventLoopBuilderExtWayland;
                        use winit::platform::x11::EventLoopBuilderExtX11;
                        EventLoopBuilderExtWayland::with_any_thread(builder, true);
                        EventLoopBuilderExtX11::with_any_thread(builder, true);
                    }
                    #[cfg(target_os = "windows")]
                    {
                        use winit::platform::windows::EventLoopBuilderExtWindows;
                        EventLoopBuilderExtWindows::with_any_thread(builder, true);
                    }
                    let _ = builder;
                })),
                ..Default::default()
            };

            let result = eframe::run_native(
                "molar_vis",
                native_options,
                Box::new(move |cc| {
                    let mut app = App::new(cc, AppLaunch::default())?;
                    app.set_jobs(rx);
                    Ok(Box::new(app) as Box<dyn eframe::App>)
                }),
            );
            if let Err(e) = result {
                log::error!("molar_vis viewer thread exited with error: {e}");
            }
        })
        .map_err(|e| {
            pyo3::exceptions::PyRuntimeError::new_err(format!("failed to start viewer thread: {e}"))
        })?;

    Ok(Visualizer { jobs: tx, state: Arc::new(Mutex::new(Vec::new())) })
}

/// The `molar_vis` extension module: pymolar's API + the viewer's.
#[pymodule]
fn molar_vis(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Re-export every pymolar class/function (System, Sel, FileHandler, rmsd, …) so
    // they live in this module with a consistent PyO3 type identity.
    molar_python::register_molar(m)?;
    m.add_class::<Visualizer>()?;
    m.add_class::<MolHandle>()?;
    m.add_class::<RepHandle>()?;
    m.add_function(wrap_pyfunction!(spawn, m)?)?;
    Ok(())
}
