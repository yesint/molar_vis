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

use pyo3::prelude::*;
use pyo3::types::PyBytes;

use molar::prelude::{Sel, SelectionDef, SelectionExpr, State, Topology};
use molar_python::{Sel as PySel, System as PySystem};
use molar_vis_core::eframe;
use molar_vis_core::{
    App, AppJob, AppLaunch, CueMode, EvalError, SharedSource, parse_corner, parse_cue_mode,
    parse_projection,
};

use pyo3::exceptions::PyValueError;
use molar_vis_core::{
    AnimationHandle as CoreAnimation, Easing, Movie as CoreMovie, PreparedMovie, TrajectoryTrack,
    VideoOptions, ViewAction, ViewStep,
};

#[pyclass(name = "Animation")]
struct Animation {
    handle: CoreAnimation,
}
#[pymethods]
impl Animation {
    fn cancel(&self) {
        self.handle.cancel();
    }
    #[getter]
    fn done(&self) -> bool {
        self.handle.is_done()
    }
    #[getter]
    fn state(&self) -> String {
        format!("{:?}", self.handle.progress().state).to_lowercase()
    }
    #[getter]
    fn completed(&self) -> u64 {
        self.handle.progress().completed
    }
    #[getter]
    fn total(&self) -> u64 {
        self.handle.progress().total
    }
    #[getter]
    fn error(&self) -> Option<String> {
        self.handle.progress().error
    }
    fn wait(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| self.handle.wait())
            .map_err(pyo3::exceptions::PyRuntimeError::new_err)
    }
}

fn schedule_view(jobs: &Sender<AppJob>, py: Python<'_>, action: ViewAction, duration: Option<f64>, easing: &str) -> PyResult<Animation> {
    action.validate().map_err(PyValueError::new_err)?;
    if duration.is_some_and(|d| !d.is_finite() || d < 0.0) { return Err(PyValueError::new_err("duration must be finite and non-negative")); }
    let easing = Easing::parse(easing).map_err(PyValueError::new_err)?;
    query_job(jobs, py, move |app| app.program_view(action, duration, easing)).map(|handle| Animation { handle })
}

#[pyclass(name = "Movie")]
struct Movie {
    jobs: Sender<AppJob>,
    recipe: CoreMovie,
    prepared: Option<PreparedMovie>,
}
impl Movie {
    fn push(&mut self, action: ViewAction, duration: f64, easing: &str) -> PyResult<()> {
        let step = ViewStep {
            action,
            duration,
            easing: Easing::parse(easing).map_err(PyValueError::new_err)?,
        };
        step.validate().map_err(PyValueError::new_err)?;
        self.recipe.steps.push(step);
        self.prepared = None;
        Ok(())
    }
    fn prepare(&mut self, py: Python<'_>) -> PyResult<PreparedMovie> {
        if let Some(movie) = &self.prepared {
            return Ok(movie.clone());
        }
        let recipe = self.recipe.clone();
        let movie = query_job(&self.jobs, py, move |app| {
            app.prepare_movie_viewport(&recipe)
        })?;
        self.prepared = Some(movie.clone());
        Ok(movie)
    }
}
#[pymethods]
impl Movie {
    #[pyo3(signature = (yaw, pitch=0.0, duration=1.0, easing="smooth"))]
    fn rotate(&mut self, yaw: f32, pitch: f32, duration: f64, easing: &str) -> PyResult<()> {
        self.push(ViewAction::rotation(yaw, pitch), duration, easing)
    }
    #[pyo3(signature = (factor, duration=1.0, easing="smooth"))]
    fn zoom(&mut self, factor: f32, duration: f64, easing: &str) -> PyResult<()> {
        self.push(ViewAction::zoom(factor), duration, easing)
    }
    #[pyo3(signature = (duration, yaw=0.0, pitch=0.0, roll=0.0, zoom=1.0, pan=(0.0,0.0), easing="smooth"))]
    #[allow(clippy::too_many_arguments)] // Python keyword arguments keep export recipes readable.
    fn move_view(
        &mut self,
        duration: f64,
        yaw: f32,
        pitch: f32,
        roll: f32,
        zoom: f32,
        pan: (f32, f32),
        easing: &str,
    ) -> PyResult<()> {
        self.push(
            ViewAction::Move {
                yaw,
                pitch,
                roll,
                zoom,
                pan: [pan.0, pan.1],
            },
            duration,
            easing,
        )
    }
    fn hold(&mut self, duration: f64) -> PyResult<()> {
        self.push(ViewAction::Hold, duration, "linear")
    }
    #[pyo3(signature = (rep, duration=1.0, zoom_out=1.0, resolution=256, easing="smooth"))]
    fn unobstructed_view(
        &mut self,
        rep: &RepHandle,
        duration: f64,
        zoom_out: f32,
        resolution: usize,
        easing: &str,
    ) -> PyResult<()> {
        self.push(
            ViewAction::Unobstructed {
                targets: vec![(rep.mol, rep.rep)],
                zoom_out,
                resolution,
            },
            duration,
            easing,
        )
    }
    #[pyo3(signature = (reps, duration=1.0, zoom_out=1.0, resolution=256, easing="smooth"))]
    fn unobstructed_view_multi(
        &mut self,
        reps: Vec<PyRef<'_, RepHandle>>,
        duration: f64,
        zoom_out: f32,
        resolution: usize,
        easing: &str,
    ) -> PyResult<()> {
        self.push(
            ViewAction::Unobstructed {
                targets: reps.iter().map(|r| (r.mol, r.rep)).collect(),
                zoom_out,
                resolution,
            },
            duration,
            easing,
        )
    }
    #[pyo3(signature = (mol, selection, duration=1.0, easing="smooth"))]
    fn focus(
        &mut self,
        mol: &MolHandle,
        selection: String,
        duration: f64,
        easing: &str,
    ) -> PyResult<()> {
        self.push(
            ViewAction::Focus {
                mol: mol.mol,
                selection,
            },
            duration,
            easing,
        )
    }
    #[pyo3(signature = (duration=1.0, easing="smooth"))]
    fn reset_view(&mut self, duration: f64, easing: &str) -> PyResult<()> {
        self.push(ViewAction::Reset, duration, easing)
    }
    #[pyo3(signature = (view_json, duration=1.0, easing="smooth"))]
    fn go_to_view(&mut self, view_json: &str, duration: f64, easing: &str) -> PyResult<()> {
        let view =
            serde_json::from_str(view_json).map_err(|e| PyValueError::new_err(e.to_string()))?;
        self.push(ViewAction::To { view }, duration, easing)
    }
    fn trajectory(&mut self, mol: &MolHandle, start: usize, end: usize, fps: f64) -> PyResult<()> {
        if !fps.is_finite() || fps <= 0.0 {
            return Err(PyValueError::new_err(
                "trajectory fps must be finite and positive",
            ));
        }
        self.recipe.trajectories.retain(|t| t.mol != mol.mol);
        self.recipe.trajectories.push(TrajectoryTrack {
            mol: mol.mol,
            start,
            end,
            fps,
        });
        self.prepared = None;
        Ok(())
    }
    #[getter]
    fn duration(&self) -> f64 {
        self.recipe.duration()
    }
    fn preview(&mut self, py: Python<'_>) -> PyResult<Animation> {
        let movie = self.prepare(py)?;
        query_job(&self.jobs, py, move |app| app.preview_movie(movie))
            .map(|handle| Animation { handle })
    }
    fn seek(&mut self, py: Python<'_>, time: f64) -> PyResult<()> {
        let movie = self.prepare(py)?;
        query_job(&self.jobs, py, move |app| {
            app.cancel_animation();
            app.seek_movie(&movie, time)
        })
    }
    #[pyo3(signature = (path, width=1920, height=1080, fps=30, raytrace=false, samples=128, crf=18, ffmpeg="ffmpeg", png_sequence=false, background=false))]
    #[allow(clippy::too_many_arguments)] // Export options are Python keyword arguments.
    fn render(
        &self,
        py: Python<'_>,
        path: String,
        width: u32,
        height: u32,
        fps: u32,
        raytrace: bool,
        samples: u32,
        crf: u8,
        ffmpeg: &str,
        png_sequence: bool,
        background: bool,
    ) -> PyResult<Animation> {
        let options = VideoOptions {
            width,
            height,
            fps,
            raytrace,
            samples,
            crf,
            ffmpeg: ffmpeg.into(),
            png_sequence,
            background,
        };
        options.validate().map_err(PyValueError::new_err)?;
        let movie = self.recipe.clone();
        query_job(&self.jobs, py, move |app| {
            app.render_movie(&movie, path.into(), options)
        })
        .map(|handle| Animation { handle })
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
        let version =
            sp.py_st().get().coords_version_atomic() as *const std::sync::atomic::AtomicU64;
        Self {
            sys,
            top,
            st,
            version,
        }
    }
}

impl SharedSource for PySystemSource {
    fn snapshot(&self) -> Result<(Topology, State), String> {
        Python::attach(|py| {
            let system = self.sys.bind(py).get();
            let state = system.r_st();
            Ok((system.r_top().clone(), State {
                coords: state.coords.clone(), time: state.time, pbox: state.pbox.clone(),
                ..Default::default()
            }))
        })
    }

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
            let expr = SelectionExpr::new(text).map_err(|e| EvalError::Invalid {
                message: e.to_string(),
                span: None,
            })?;
            match (&expr).into_sel_index(sp, None) {
                Ok(svec) => Sel::from_svec(svec)
                    .map(|sel| (expr, sel))
                    .map_err(|_| EvalError::Empty),
                Err(e) => Err(EvalError::Invalid {
                    message: e.to_string(),
                    span: None,
                }),
            }
        })
    }
}

/// Send a job to the viewer thread; errors if the window has closed.
fn send_job(jobs: &Sender<AppJob>, job: AppJob) -> PyResult<()> {
    jobs.send(job)
        .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("viewer window is closed"))
}

fn query_job<R: Send + 'static>(
    jobs: &Sender<AppJob>, py: Python<'_>,
    f: impl FnOnce(&mut App) -> Result<R, String> + Send + 'static,
) -> PyResult<R> {
    let (tx, rx) = std::sync::mpsc::channel();
    send_job(jobs, Box::new(move |app| { let _ = tx.send(f(app)); }))?;
    py.detach(move || rx.recv())
        .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("viewer window is closed"))?
        .map_err(pyo3::exceptions::PyRuntimeError::new_err)
}

/// A live handle to a running viewer window. Returned by [`spawn`]; queues jobs to
/// the viewer (UI) thread.
#[pyclass]
struct Visualizer {
    jobs: Sender<AppJob>,
}

#[pymethods]
impl Visualizer {
    /// Set the camera orbit pivot (x, y, z) in nanometers.
    fn rotation_center(&self, x: f32, y: f32, z: f32) -> PyResult<()> {
        if ![x, y, z].iter().all(|v| v.is_finite()) { return Err(PyValueError::new_err("rotation center must be finite")); }
        send_job(&self.jobs, Box::new(move |app| app.set_rotation_center_xyz([x, y, z])))
    }
    /// JSON snapshot of current element, secondary, gradient, and discrete colors.
    fn colors_json(&self, py: Python<'_>) -> PyResult<String> {
        let (tx, rx) = std::sync::mpsc::channel();
        send_job(
            &self.jobs,
            Box::new(move |app| {
                let _ = tx.send(app.color_preferences_json());
            }),
        )?;
        py.detach(move || rx.recv())
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("viewer closed"))?
            .map_err(PyValueError::new_err)
    }
    /// Apply atom, secondary-structure and gradient colors from a viewer preset.
    fn set_color_preset(&self, name: &str) -> PyResult<()> {
        let preset = molar_vis_core::ColorPreset::parse(name).map_err(PyValueError::new_err)?;
        send_job(&self.jobs, Box::new(move |app| {
            if let Err(e) = app.set_color_preset(preset) {
                log::error!("color preset: {e}");
            }
        }))
    }

    /// The current preset name, or Custom after manual edits.
    fn color_preset(&self, py: Python<'_>) -> PyResult<String> {
        query_job(&self.jobs, py, |app| Ok(app.color_preset_name().to_string()))
    }

    fn set_palette(&self, colors: Vec<(u8, u8, u8, u8)>) -> PyResult<()> {
        if colors.is_empty() {
            return Err(PyValueError::new_err(
                "palette must contain at least one color",
            ));
        }
        let colors: Vec<[u8; 4]> = colors.into_iter().map(|c| [c.0, c.1, c.2, c.3]).collect();
        send_job(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                c.categorical = colors;
                if let Err(e) = app.set_color_preferences(c) {
                    log::error!("palette: {e}");
                }
            }),
        )
    }
    fn reset_colors(&self) -> PyResult<()> {
        send_job(
            &self.jobs,
            Box::new(move |app| {
                if let Err(e) =
                    app.set_color_preferences(molar_vis_core::ColorPreferences::default())
                {
                    log::error!("reset colors: {e}");
                }
            }),
        )
    }
    /// Override an element color (atomic number 0..=118) with RGBA8.
    fn set_element_color(&self, number: u8, rgba: (u8, u8, u8, u8)) -> PyResult<()> {
        if number > 118 {
            return Err(PyValueError::new_err("atomic number must be 0..=118"));
        }
        send_job(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                c.elements.insert(number, [rgba.0, rgba.1, rgba.2, rgba.3]);
                if let Err(e) = app.set_color_preferences(c) {
                    log::error!("element color: {e}");
                }
            }),
        )
    }
    /// Reset one element to its built-in color.
    fn reset_element_color(&self, number: u8) -> PyResult<()> {
        send_job(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                c.elements.remove(&number);
                if let Err(e) = app.set_color_preferences(c) {
                    log::error!("element color: {e}");
                }
            }),
        )
    }
    /// Change one of nine secondary-structure colors (0..=8; alpha, 3-10, pi, sheet, bridge, turn, bend, polyproline, coil).
    fn set_secondary_color(&self, index: usize, rgba: (u8, u8, u8, u8)) -> PyResult<()> {
        if index >= 9 {
            return Err(PyValueError::new_err("secondary index must be 0..=8"));
        }
        send_job(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                c.secondary[index] = [rgba.0, rgba.1, rgba.2, rgba.3];
                if let Err(e) = app.set_color_preferences(c) {
                    log::error!("secondary color: {e}");
                }
            }),
        )
    }
    /// Set the three RGBA8 continuous gradient stops and midpoint (0.01..=0.99).
    fn set_continuous_gradient(&self, stops: [(u8, u8, u8, u8); 3], midpoint: f32) -> PyResult<()> {
        if !midpoint.is_finite() || !(0.01..=0.99).contains(&midpoint) {
            return Err(PyValueError::new_err("midpoint must be 0.01..=0.99"));
        }
        let stops = stops.map(|c| [c.0, c.1, c.2, c.3]);
        send_job(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                c.continuous = molar_vis_core::Gradient { stops, midpoint };
                if let Err(e) = app.set_color_preferences(c) {
                    log::error!("continuous gradient: {e}");
                }
            }),
        )
    }
    /// Edit one slot in the shared 18-color discrete palette.
    fn set_palette_color(&self, index: usize, rgba: (u8, u8, u8, u8)) -> PyResult<()> {
        send_job(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                if let Some(slot) = c.categorical.get_mut(index) {
                    *slot = [rgba.0, rgba.1, rgba.2, rgba.3];
                    if let Err(e) = app.set_color_preferences(c) {
                        log::error!("palette color: {e}");
                    }
                } else {
                    log::error!("palette index {index} out of range");
                }
            }),
        )
    }
    fn add_palette_color(&self, rgba: (u8, u8, u8, u8)) -> PyResult<()> {
        send_job(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                c.categorical.push([rgba.0, rgba.1, rgba.2, rgba.3]);
                if let Err(e) = app.set_color_preferences(c) {
                    log::error!("palette color: {e}");
                }
            }),
        )
    }
    fn remove_palette_color(&self, index: usize) -> PyResult<()> {
        send_job(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                if c.categorical.len() > 1 && index < c.categorical.len() {
                    c.categorical.remove(index);
                    if let Err(e) = app.set_color_preferences(c) {
                        log::error!("palette color: {e}");
                    }
                } else {
                    log::error!("cannot remove palette index {index}");
                }
            }),
        )
    }
    /// Persist current program settings, including color edits.
    fn save_settings(&self, py: Python<'_>) -> PyResult<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        send_job(
            &self.jobs,
            Box::new(move |app| {
                let _ = tx.send(app.save_settings());
            }),
        )?;
        py.detach(move || rx.recv())
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("viewer closed"))?
            .map_err(pyo3::exceptions::PyRuntimeError::new_err)
    }

    /// Add a molecule, rendering **directly** from the pymolar `System` (zero-copy);
    /// returns a handle. The window updates as soon as the UI thread picks it up.
    ///
    /// `source_path` (optional): the structure file this System was loaded from. It is
    /// recorded as the molecule's session source (`MoleculeSource::File`) so a
    /// `save_session` can be reloaded standalone (with the atoms) in a fresh process;
    /// without it the shared molecule serializes as in-memory bytes and a session
    /// cannot restore coordinates.
    #[pyo3(signature = (sys, source_path=None))]
    fn add_mol(&self, sys: Py<PySystem>, source_path: Option<String>) -> PyResult<MolHandle> {
        let mol = molar_vis_core::reserve_mol_id().0;
        let name = format!("mol {mol}");
        send_job(
            &self.jobs,
            Box::new(move |app: &mut App| {
                let src = Python::attach(|py| PySystemSource::new(py, sys));
                match app.add_shared_molecule_with_id(mol, Box::new(src), name) {
                    Ok(()) => {
                        if let Some(p) = &source_path {
                            if let Err(e) = app.resolve_molecule(mol).and_then(|idx| app.set_molecule_source_file(idx, p)) {
                                log::error!("add_mol set source: {e}");
                            }
                        }
                    }
                    Err(e) => log::error!("add_mol failed: {e}"),
                }
            }),
        )?;
        Ok(MolHandle { mol, jobs: self.jobs.clone() })
    }

    /// The molecules currently in the scene, as handles.
    #[getter]
    fn mols(&self, py: Python<'_>) -> PyResult<Vec<MolHandle>> {
        let ids = query_job(&self.jobs, py, |app| Ok(app.molecule_ids()))?;
        Ok(ids.into_iter().map(|mol| MolHandle { mol, jobs: self.jobs.clone() }).collect())
    }

    // --- View controls (mirror the view-settings UI). All apply live. ---

    /// Build a sequence for preview, seeking, or native video export.
    fn movie(&self) -> Movie {
        Movie {
            jobs: self.jobs.clone(),
            recipe: CoreMovie::default(),
            prepared: None,
        }
    }
    fn view_json(&self, py: Python<'_>) -> PyResult<String> {
        query_job(&self.jobs, py, |app| {
            serde_json::to_string(&app.view_pose()).map_err(|e| e.to_string())
        })
    }
    #[pyo3(signature = (yaw, pitch=0.0, duration=None, easing="smooth"))]
    fn rotate(
        &self,
        py: Python<'_>,
        yaw: f32,
        pitch: f32,
        duration: Option<f64>,
        easing: &str,
    ) -> PyResult<Animation> {
        schedule_view(
            &self.jobs,
            py,
            ViewAction::rotation(yaw, pitch),
            duration,
            easing,
        )
    }
    #[pyo3(signature = (angle, duration=None, easing="smooth"))]
    fn roll(&self, py: Python<'_>, angle: f32, duration: Option<f64>, easing: &str) -> PyResult<Animation> {
        schedule_view(
            &self.jobs,
            py,
            ViewAction::Move {
                yaw: 0.0,
                pitch: 0.0,
                roll: angle,
                zoom: 1.0,
                pan: [0.0; 2],
            },
            duration,
            easing,
        )
    }
    #[pyo3(signature = (dx, dy, duration=None, easing="smooth"))]
    fn pan(
        &self,
        py: Python<'_>,
        dx: f32,
        dy: f32,
        duration: Option<f64>,
        easing: &str,
    ) -> PyResult<Animation> {
        schedule_view(
            &self.jobs,
            py,
            ViewAction::Move {
                yaw: 0.0,
                pitch: 0.0,
                roll: 0.0,
                zoom: 1.0,
                pan: [dx, dy],
            },
            duration,
            easing,
        )
    }
    #[pyo3(signature = (factor, duration=None, easing="smooth"))]
    fn zoom(
        &self,
        py: Python<'_>,
        factor: f32,
        duration: Option<f64>,
        easing: &str,
    ) -> PyResult<Animation> {
        schedule_view(&self.jobs, py, ViewAction::zoom(factor), duration, easing)
    }
    #[pyo3(signature = (duration=None, easing="smooth"))]
    fn reset_view(&self, py: Python<'_>, duration: Option<f64>, easing: &str) -> PyResult<Animation> {
        schedule_view(&self.jobs, py, ViewAction::Reset, duration, easing)
    }
    #[pyo3(signature = (view_json, duration=None, easing="smooth"))]
    fn go_to_view(
        &self,
        py: Python<'_>,
        view_json: &str,
        duration: Option<f64>,
        easing: &str,
    ) -> PyResult<Animation> {
        let view =
            serde_json::from_str(view_json).map_err(|e| PyValueError::new_err(e.to_string()))?;
        schedule_view(&self.jobs, py, ViewAction::To { view }, duration, easing)
    }
    fn cancel_animation(&self) -> PyResult<()> {
        send_job(&self.jobs, Box::new(|app| app.cancel_animation()))
    }

    /// Set the projection: `"perspective"` or `"orthographic"`.
    fn projection(&self, mode: &str) -> PyResult<()> {
        let p = parse_projection(mode).map_err(PyValueError::new_err)?;
        send_job(&self.jobs, Box::new(move |app| app.set_projection(p)))
    }

    /// Flat background color, RGB in 0–1.
    fn background(&self, r: f32, g: f32, b: f32) -> PyResult<()> {
        send_job(
            &self.jobs,
            Box::new(move |app| app.set_background_solid([r, g, b])),
        )
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
        let c = corner
            .map(parse_corner)
            .transpose()
            .map_err(PyValueError::new_err)?;
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
            other => (
                enabled,
                parse_cue_mode(other).map_err(PyValueError::new_err)?,
            ),
        };
        send_job(
            &self.jobs,
            Box::new(move |app| app.set_depth_cue(enabled, mode, strength, start)),
        )
    }

    /// Screen-space ambient occlusion: `strength` darkening, `radius` in nm.
    #[pyo3(signature = (enabled=true, strength=0.9, radius=0.4))]
    fn ambient_occlusion(&self, enabled: bool, strength: f32, radius: f32) -> PyResult<()> {
        send_job(
            &self.jobs,
            Box::new(move |app| app.set_ambient_occlusion(enabled, strength, radius)),
        )
    }

    /// Real-time cast shadows: `strength` scales how dark shadowed areas get.
    #[pyo3(signature = (enabled=true, strength=0.6))]
    fn shadows(&self, enabled: bool, strength: f32) -> PyResult<()> {
        send_job(
            &self.jobs,
            Box::new(move |app| app.set_shadows(enabled, strength)),
        )
    }

    /// Save the current scene + view as a JSON session file (molecules by path, every
    /// representation, camera + view settings). Blocks until the file is written.
    fn save_session(&self, py: Python<'_>, path: String) -> PyResult<()> {
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let dest = std::path::PathBuf::from(&path);
        send_job(
            &self.jobs,
            Box::new(move |app: &mut App| {
                app.save_session_file(&dest);
                let _ = done_tx.send(());
            }),
        )?;
        py.detach(move || done_rx.recv()).map_err(|_| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "viewer window closed before the session was saved",
            )
        })
    }

    /// Zoom the camera to fit `sel` (a pymolar `Sel`) of the molecule `mol`
    /// (a [`MolHandle`]) — VMD-style zoom-to-selection.
    #[pyo3(signature = (mol, sel, duration=None, easing="smooth"))]
    fn focus(
        &self,
        py: Python<'_>,
        mol: &MolHandle,
        sel: Py<PySel>,
        duration: Option<f64>,
        easing: &str,
    ) -> PyResult<Animation> {
        let indices = sel.bind(py).get().index().to_vec();
        if indices.is_empty() {
            return Err(PyValueError::new_err("selection is empty"));
        }
        let selection = format!(
            "index {}",
            indices
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        );
        schedule_view(
            &self.jobs,
            py,
            ViewAction::Focus {
                mol: mol.mol,
                selection,
            },
            duration,
            easing,
        )
    }
    #[pyo3(signature = (rep, zoom_out=1.0, resolution=256, duration=None, easing="smooth"))]
    fn unobstructed_view(
        &self,
        py: Python<'_>,
        rep: &RepHandle,
        zoom_out: f32,
        resolution: usize,
        duration: Option<f64>,
        easing: &str,
    ) -> PyResult<Animation> {
        schedule_view(
            &self.jobs,
            py,
            ViewAction::Unobstructed {
                targets: vec![(rep.mol, rep.rep)],
                zoom_out,
                resolution,
            },
            duration,
            easing,
        )
    }
    #[pyo3(signature = (reps, zoom_out=1.0, resolution=256, duration=None, easing="smooth"))]
    fn unobstructed_view_multi(
        &self,
        py: Python<'_>,
        reps: Vec<PyRef<'_, RepHandle>>,
        zoom_out: f32,
        resolution: usize,
        duration: Option<f64>,
        easing: &str,
    ) -> PyResult<Animation> {
        schedule_view(
            &self.jobs,
            py,
            ViewAction::Unobstructed {
                targets: reps.iter().map(|r| (r.mol, r.rep)).collect(),
                zoom_out,
                resolution,
            },
            duration,
            easing,
        )
    }

    /// Render the current scene **offscreen** to a PNG at `path`, at a window-independent
    /// `width × height`. Works whether the viewer is visible or hidden (`spawn(visible=False)`).
    /// With `raytrace=True` it goes through the GPU ray tracer (AO / shadows / GI), tracing
    /// `samples` paths per pixel; otherwise it captures the rasterized view.
    ///
    /// The render runs on the UI thread and this call **blocks** until the file is written
    /// (releasing the GIL meanwhile, so the UI thread can read the shared pymolar data).
    #[pyo3(signature = (path, width=1200, height=1000, raytrace=false, samples=128))]
    fn render(
        &self,
        py: Python<'_>,
        path: String,
        width: u32,
        height: u32,
        raytrace: bool,
        samples: u32,
    ) -> PyResult<()> {
        let (done_tx, done_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let dest = std::path::PathBuf::from(&path);
        send_job(
            &self.jobs,
            Box::new(move |app: &mut App| {
                let res = app.render_to_file(&dest, width, height, raytrace, samples);
                // The receiver may be gone if Python was interrupted; ignore a send error.
                let _ = done_tx.send(res);
            }),
        )?;
        // Block on the UI thread finishing the render + file write, but release the GIL so
        // the UI thread can attach to it (a shared pymolar molecule's selection is evaluated
        // under the GIL during the pre-render `rebuild_dirty`) — otherwise this deadlocks.
        let outcome = py.detach(move || done_rx.recv());
        match outcome {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(pyo3::exceptions::PyRuntimeError::new_err(e)),
            Err(_) => Err(pyo3::exceptions::PyRuntimeError::new_err(
                "viewer window closed before the render completed",
            )),
        }
    }

    fn __repr__(&self) -> String {
        "<molar_vis.Visualizer>".to_string()
    }
}

/// Handle to a molecule in the viewer.
#[pyclass]
struct MolHandle {
    mol: u64,
    jobs: Sender<AppJob>,
}

#[pymethods]
impl MolHandle {
    /// Import a molecule-owned PLY mesh from memory. Coordinates are in nanometers.
    fn add_mesh_ply(&self, py: Python<'_>, name: String, bytes: &[u8]) -> PyResult<MeshHandle> {
        molar_vis_core::import_ply(bytes).map_err(PyValueError::new_err)?;
        let id = molar_vis_core::reserve_mesh_id();
        let mol = self.mol;
        let data = bytes.to_vec();
        query_job(&self.jobs, py, move |app| {
            let mi = app.resolve_molecule(mol)?;
            app.add_mesh_ply(mi, id, name, &data)
        })?;
        Ok(MeshHandle { mol, id, jobs: self.jobs.clone() })
    }

    /// Import a PLY file into this molecule.
    fn load_mesh(&self, py: Python<'_>, path: String) -> PyResult<MeshHandle> {
        let bytes = std::fs::read(&path)?;
        let name = std::path::Path::new(&path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        self.add_mesh_ply(py, name, &bytes)
    }

    #[getter]
    fn meshes(&self, py: Python<'_>) -> PyResult<Vec<MeshHandle>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mol = self.mol;
        send_job(
            &self.jobs,
            Box::new(move |app| {
                let _ = tx.send(app.resolve_molecule(mol).and_then(|mi| app.mesh_ids(mi)));
            }),
        )?;
        let ids = py
            .detach(move || rx.recv())
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("viewer closed"))?
            .map_err(PyValueError::new_err)?;
        Ok(ids
            .into_iter()
            .map(|id| MeshHandle { mol, id, jobs: self.jobs.clone() })
            .collect())
    }

    /// Add a representation. `sel` is a pymolar `Sel` (its atoms become the rep's
    /// selection); `style`/`color` are names like "cartoon"/"ss". Returns a handle.
    #[pyo3(signature = (sel=None, style=None, color=None, material=None, width=None))]
    fn add_rep(
        &self,
        py: Python<'_>,
        sel: Option<Py<PySel>>,
        style: Option<String>,
        color: Option<String>,
        material: Option<String>,
        width: Option<f32>,
    ) -> PyResult<RepHandle> {
        let rep = molar_vis_core::reserve_rep_id().0;
        // Read the selection's atom indices now (we hold the GIL here).
        let indices: Option<Vec<usize>> = sel.map(|s| s.bind(py).get().index().to_vec());
        let mol = self.mol;
        query_job(&self.jobs, py, move |app: &mut App| {
                app.add_rep_default_with_id(mol, rep)?;
                let (mi, ri) = app.resolve_rep(mol, rep)?;
                if let Some(idx) = &indices {
                    app.set_rep_selection(mi, ri, idx)?;
                }
                if let Some(st) = &style {
                    app.set_rep_style(mi, ri, st)?;
                }
                if let Some(c) = &color {
                    app.set_rep_color(mi, ri, c)?;
                }
                if let Some(m) = &material {
                    app.set_rep_material(mi, ri, m)?;
                }
                if let Some(w) = width {
                    app.set_rep_width(mi, ri, w)?;
                }
                Ok(())
        })?;
        Ok(RepHandle {
            mol,
            rep,
            jobs: self.jobs.clone(),
        })
    }

    /// Add an **Interactions** representation to this molecule, detecting non-covalent
    /// contacts (H-bonds, hydrophobic, salt bridges, π-stacking, π-cation, halogen) against
    /// `partner` — a [`RepHandle`] (its rep) or a [`MolHandle`] (that molecule's first rep).
    /// Returns a handle to the new rep.
    fn add_interactions(&self, py: Python<'_>, partner: &Bound<'_, PyAny>) -> PyResult<RepHandle> {
        let (partner_mol, partner_rep) = if let Ok(r) = partner.cast::<RepHandle>() {
            let r = r.borrow();
            (r.mol, r.rep)
        } else if let Ok(m) = partner.cast::<MolHandle>() {
            let mol = m.borrow().mol;
            let first = query_job(&self.jobs, py, move |app| app.rep_ids(mol))?.into_iter().next()
                .ok_or_else(|| PyValueError::new_err("partner molecule has no representations"))?;
            (mol, first)
        } else {
            return Err(PyValueError::new_err(
                "partner must be a RepHandle or a MolHandle",
            ));
        };
        let rep = molar_vis_core::reserve_rep_id().0;
        let mol = self.mol;
        query_job(&self.jobs, py, move |app| app.add_interactions_rep_with_id(mol, rep, partner_mol, partner_rep))?;
        Ok(RepHandle {
            mol,
            rep,
            jobs: self.jobs.clone(),
        })
    }

    /// This molecule's representations, as handles (for `for rep in mol.reps: …`).
    #[getter]
    fn reps(&self, py: Python<'_>) -> PyResult<Vec<RepHandle>> {
        let mol = self.mol;
        let ids = query_job(&self.jobs, py, move |app| app.rep_ids(mol))?;
        Ok(ids.into_iter().map(|rep| RepHandle { mol, rep, jobs: self.jobs.clone() }).collect())
    }

    fn __repr__(&self) -> String {
        format!("<molar_vis.MolHandle mol={}>", self.mol)
    }
}

/// Stable handle to a molecule-owned PLY mesh.
#[pyclass]
struct MeshHandle {
    mol: u64,
    id: u64,
    jobs: Sender<AppJob>,
}

impl MeshHandle {
    fn export_bytes(&self, py: Python<'_>) -> PyResult<Vec<u8>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let (m, id) = (self.mol, self.id);
        send_job(&self.jobs, Box::new(move |app| { let _ = tx.send(app.resolve_molecule(m).and_then(|mi| app.mesh_ply(mi, id))); }))?;
        py.detach(move || rx.recv())
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("viewer closed"))?
            .map_err(PyValueError::new_err)
    }
    fn snapshot(&self, py: Python<'_>) -> PyResult<molar_vis_core::MeshState> {
        let (tx, rx) = std::sync::mpsc::channel();
        let (m, id) = (self.mol, self.id);
        send_job(
            &self.jobs,
            Box::new(move |app| {
                let _ = tx.send(app.resolve_molecule(m).and_then(|mi| app.mesh_state(mi, id)));
            }),
        )?;
        py.detach(move || rx.recv())
            .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("viewer closed"))?
            .map_err(PyValueError::new_err)
    }
}

#[pymethods]
impl MeshHandle {
    #[getter]
    fn name(&self, py: Python<'_>) -> PyResult<String> {
        Ok(self.snapshot(py)?.name)
    }
    #[getter]
    fn visible(&self, py: Python<'_>) -> PyResult<bool> {
        Ok(self.snapshot(py)?.visible)
    }
    #[getter]
    fn material(&self, py: Python<'_>) -> PyResult<String> {
        Ok(self.snapshot(py)?.material.label().to_owned())
    }
    #[getter]
    fn style(&self, py: Python<'_>) -> PyResult<String> {
        Ok(self.snapshot(py)?.style.label().to_ascii_lowercase())
    }
    #[getter]
    fn point_size(&self, py: Python<'_>) -> PyResult<f32> {
        Ok(self.snapshot(py)?.point_size)
    }
    #[getter]
    fn line_width(&self, py: Python<'_>) -> PyResult<f32> {
        Ok(self.snapshot(py)?.line_width)
    }
    #[getter]
    fn smoothing(&self, py: Python<'_>) -> PyResult<u32> {
        Ok(self.snapshot(py)?.smoothing)
    }
    #[setter]
    fn set_visible(&self, py: Python<'_>, value: bool) -> PyResult<()> {
        let (m, id) = (self.mol, self.id);
        query_job(&self.jobs, py, move |app| { let mi = app.resolve_molecule(m)?; app.set_mesh_visible(mi, id, value) })
    }
    #[setter]
    fn set_material(&self, py: Python<'_>, value: String) -> PyResult<()> {
        let (m, id) = (self.mol, self.id);
        query_job(&self.jobs, py, move |app| { let mi = app.resolve_molecule(m)?; app.set_mesh_material(mi, id, &value) })
    }
    #[setter]
    fn set_style(&self, py: Python<'_>, value: String) -> PyResult<()> {
        let (m, id) = (self.mol, self.id);
        query_job(&self.jobs, py, move |app| { let mi = app.resolve_molecule(m)?; app.set_mesh_style(mi, id, &value) })
    }
    #[setter]
    fn set_point_size(&self, py: Python<'_>, value: f32) -> PyResult<()> {
        if !value.is_finite() || !(1.0..=10.0).contains(&value) {
            return Err(PyValueError::new_err("point_size must be 1..=10"));
        }
        let (m, id) = (self.mol, self.id);
        query_job(&self.jobs, py, move |app| { let mi = app.resolve_molecule(m)?; app.set_mesh_point_size(mi, id, value) })
    }
    #[setter]
    fn set_line_width(&self, py: Python<'_>, value: f32) -> PyResult<()> {
        if !value.is_finite() || !(1.0..=10.0).contains(&value) {
            return Err(PyValueError::new_err("line_width must be 1..=10"));
        }
        let (m, id) = (self.mol, self.id);
        query_job(&self.jobs, py, move |app| { let mi = app.resolve_molecule(m)?; app.set_mesh_line_width(mi, id, value) })
    }
    #[setter]
    fn set_smoothing(&self, py: Python<'_>, value: u32) -> PyResult<()> {
        if value > 5 {
            return Err(PyValueError::new_err("smoothing must be 0..=5"));
        }
        let (m, id) = (self.mol, self.id);
        query_job(&self.jobs, py, move |app| { let mi = app.resolve_molecule(m)?; app.set_mesh_smoothing(mi, id, value) })
    }
    /// Export original PLY geometry, unaffected by display smoothing.
    fn ply_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, &self.export_bytes(py)?))
    }
    fn save(&self, py: Python<'_>, path: String) -> PyResult<()> {
        std::fs::write(path, self.export_bytes(py)?)?;
        Ok(())
    }
    fn delete(&self, py: Python<'_>) -> PyResult<()> {
        let (m, id) = (self.mol, self.id);
        query_job(&self.jobs, py, move |app| { let mi = app.resolve_molecule(m)?; app.delete_mesh(mi, id) })
    }
}

/// Handle to a representation. Property setters (`rep.style = "lines"`, …) apply live.
#[pyclass]
struct RepHandle {
    mol: u64,
    rep: u64,
    jobs: Sender<AppJob>,
}

#[pymethods]
impl RepHandle {
    #[setter]
    fn set_style(&self, py: Python<'_>, value: &str) -> PyResult<()> {
        let (mol, rep, v) = (self.mol, self.rep, value.to_string());
        query_job(&self.jobs, py, move |app| { let (mi, ri) = app.resolve_rep(mol, rep)?; app.set_rep_style(mi, ri, &v) })
    }

    #[setter]
    fn set_color(&self, py: Python<'_>, value: &str) -> PyResult<()> {
        let (mol, rep, v) = (self.mol, self.rep, value.to_string());
        query_job(&self.jobs, py, move |app| { let (mi, ri) = app.resolve_rep(mol, rep)?; app.set_rep_color(mi, ri, &v) })
    }

    #[setter]
    fn set_material(&self, py: Python<'_>, value: &str) -> PyResult<()> {
        let (mol, rep, v) = (self.mol, self.rep, value.to_string());
        query_job(&self.jobs, py, move |app| { let (mi, ri) = app.resolve_rep(mol, rep)?; app.set_rep_material(mi, ri, &v) })
    }

    /// Set the line width in pixels (Lines / Interactions styles only; other styles ignore
    /// it with a logged warning).
    #[setter]
    fn set_width(&self, py: Python<'_>, value: f32) -> PyResult<()> {
        let (mol, rep) = (self.mol, self.rep);
        query_job(&self.jobs, py, move |app| { let (mi, ri) = app.resolve_rep(mol, rep)?; app.set_rep_width(mi, ri, value) })
    }

    /// Set the selection from a pymolar `Sel`'s atoms.
    fn select(&self, py: Python<'_>, sel: Py<PySel>) -> PyResult<()> {
        let indices = sel.bind(py).get().index().to_vec();
        let (mol, rep) = (self.mol, self.rep);
        query_job(&self.jobs, py, move |app| { let (mi, ri) = app.resolve_rep(mol, rep)?; app.set_rep_selection(mi, ri, &indices) })
    }

    fn __repr__(&self) -> String {
        format!("<molar_vis.RepHandle mol={} rep={}>", self.mol, self.rep)
    }
}

/// Open the viewer window and return immediately — it runs on a background thread, so
/// the Python REPL stays responsive. Jobs (add molecules, change reps, …) are sent to
/// it through the returned [`Visualizer`].
///
/// `visible=False` creates the window **invisible** (`ViewportBuilder::with_visible(false)`,
/// the same path as the `MOLAR_VIS_DEBUG_HIDDEN` hook), so a scene can be built and rendered
/// to a file headlessly without popping a window onto the desktop. It still obtains a real
/// wgpu device, so `Visualizer.render(...)` works; it does need a display server to create
/// the (hidden) window.
///
/// The window's event loop runs off the main thread (`with_any_thread`), which winit
/// supports on Linux/Windows; macOS requires the main thread and isn't handled yet.
#[pyfunction]
#[pyo3(signature = (visible=true))]
fn spawn(visible: bool) -> PyResult<Visualizer> {
    let (tx, rx) = std::sync::mpsc::channel::<AppJob>();

    std::thread::Builder::new()
        .name("molar_vis-ui".into())
        .spawn(move || {
            let mut viewport = eframe::egui::ViewportBuilder::default()
                .with_title("molar_vis")
                .with_inner_size([1200.0, 800.0]);
            if !visible {
                // Invisible window: headless render-to-file with no desktop window.
                viewport = viewport.with_visible(false);
            }
            let native_options = eframe::NativeOptions {
                renderer: eframe::Renderer::Wgpu,
                // Same early-Z device opt-in as the standalone binary (kills close-up
                // impostor overdraw when the adapter supports it).
                wgpu_options: molar_vis_core::early_z_wgpu_options(),
                viewport,
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

    Ok(Visualizer { jobs: tx })
}

/// The `molar_vis` extension module: pymolar's API + the viewer's.
#[pymodule]
fn molar_vis(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Re-export every pymolar class/function (System, Sel, FileHandler, rmsd, …) so
    // they live in this module with a consistent PyO3 type identity.
    molar_python::register_molar(m)?;
    m.add_class::<Visualizer>()?;
    m.add_class::<Animation>()?;
    m.add_class::<Movie>()?;
    m.add_class::<MolHandle>()?;
    m.add_class::<RepHandle>()?;
    m.add_class::<MeshHandle>()?;
    m.add_function(wrap_pyfunction!(spawn, m)?)?;
    Ok(())
}
