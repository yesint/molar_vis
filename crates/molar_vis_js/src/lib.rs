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

use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc::Sender;

use wasm_bindgen::prelude::*;

use molar::prelude::{
    FileHandler, FileFormatError, IndexSliceProvider, Sel as MSel, SelectionExpr, State, System as MSystem, Topology,
};
use molar_vis_core::eframe;
use molar_vis_core::{
    App, AppJob, AppLaunch, CueMode, EvalError, SharedSource, parse_corner, parse_cue_mode,
    parse_projection,
};

use molar_vis_core::{
    AnimationHandle as CoreAnimation, Easing, Movie as CoreMovie, PreparedMovie, TaskState,
    ViewAction, ViewStep,
};

#[wasm_bindgen]
pub struct Animation {
    handle: CoreAnimation,
    jobs: Sender<AppJob>,
}
#[wasm_bindgen]
impl Animation {
    pub fn cancel(&self) {
        self.handle.cancel();
    }
    #[wasm_bindgen(getter)]
    pub fn done(&self) -> bool {
        self.handle.is_done()
    }
    pub fn progress_json(&self) -> Result<String, JsError> {
        serde_json::to_string(&self.handle.progress()).map_err(|e| JsError::new(&molar_vis_core::error_chain(&e)))
    }
    pub async fn wait(&self) -> Result<(), JsError> {
        let handle = self.handle.clone();
        while !handle.is_done() {
            // Yield to the viewer's next frame; no busy loop on the browser thread.
            query_job(&self.jobs, |_| Ok(())).await?;
        }
        let p = handle.progress();
        match p.state {
            TaskState::Completed => Ok(()),
            TaskState::Cancelled => Err(JsError::new("animation cancelled")),
            _ => Err(JsError::new(
                &p.error.unwrap_or_else(|| "animation failed".into()),
            )),
        }
    }
}
async fn schedule_view(
    jobs: &Sender<AppJob>,
    action: ViewAction,
    duration: Option<f64>,
    easing: Option<String>,
) -> Result<Animation, JsError> {
    action.validate().map_err(|e| JsError::new(&e))?;
    if duration.is_some_and(|d| !d.is_finite() || d < 0.0) { return Err(JsError::new("duration must be finite and non-negative")); }
    let easing = Easing::parse(easing.as_deref().unwrap_or("smooth")).map_err(|e| JsError::new(&e))?;
    let handle = query_job(jobs, move |app| app.program_view(action, duration, easing)).await?;
    Ok(Animation {
        handle,
        jobs: jobs.clone(),
    })
}

#[wasm_bindgen]
pub struct Movie {
    recipe: CoreMovie,
    prepared: Option<PreparedMovie>,
    jobs: Sender<AppJob>,
}
impl Movie {
    fn push(
        &mut self,
        action: ViewAction,
        duration: f64,
        easing: Option<String>,
    ) -> Result<(), JsError> {
        let step = ViewStep {
            action,
            duration,
            easing: Easing::parse(easing.as_deref().unwrap_or("smooth"))
                .map_err(|e| JsError::new(&e))?,
        };
        step.validate().map_err(|e| JsError::new(&e))?;
        self.recipe.steps.push(step);
        self.prepared = None;
        Ok(())
    }
    async fn prepare(&mut self) -> Result<PreparedMovie, JsError> {
        if let Some(movie) = &self.prepared {
            return Ok(movie.clone());
        }
        let recipe = self.recipe.clone();
        let movie = query_job(&self.jobs, move |app| app.prepare_movie_viewport(&recipe)).await?;
        self.prepared = Some(movie.clone());
        Ok(movie)
    }
}
#[wasm_bindgen]
impl Movie {
    pub fn rotate(
        &mut self,
        yaw: f32,
        pitch: f32,
        duration: f64,
        easing: Option<String>,
    ) -> Result<(), JsError> {
        self.push(ViewAction::rotation(yaw, pitch), duration, easing)
    }
    pub fn zoom(
        &mut self,
        factor: f32,
        duration: f64,
        easing: Option<String>,
    ) -> Result<(), JsError> {
        self.push(ViewAction::zoom(factor), duration, easing)
    }
    pub fn hold(&mut self, duration: f64) -> Result<(), JsError> {
        self.push(ViewAction::Hold, duration, None)
    }
    pub fn unobstructed_view(
        &mut self,
        rep: &RepHandle,
        duration: f64,
        zoom_out: Option<f32>,
        resolution: Option<usize>,
    ) -> Result<(), JsError> {
        self.push(
            ViewAction::Unobstructed {
                targets: vec![(rep.mol, rep.rep)],
                zoom_out: zoom_out.unwrap_or(1.0),
                resolution: resolution.unwrap_or(256),
            },
            duration,
            None,
        )
    }
    pub fn focus(
        &mut self,
        mol: &MolHandle,
        selection: String,
        duration: f64,
    ) -> Result<(), JsError> {
        self.push(
            ViewAction::Focus {
                mol: mol.mol,
                selection,
            },
            duration,
            None,
        )
    }
    /// Add any shared ViewStep, including simultaneous motion and saved-view transitions.
    pub fn add_step_json(&mut self, json: &str) -> Result<(), JsError> {
        let step: ViewStep =
            serde_json::from_str(json).map_err(|e| JsError::new(&molar_vis_core::error_chain(&e)))?;
        step.validate().map_err(|e| JsError::new(&e))?;
        self.recipe.steps.push(step);
        self.prepared = None;
        Ok(())
    }
    pub fn trajectory(&mut self, mol: &MolHandle, start: usize, end: usize, fps: f64) -> Result<(), JsError> {
        if !fps.is_finite() || fps <= 0.0 { return Err(JsError::new("trajectory fps must be finite and positive")); }
        self.recipe.trajectories.retain(|track| track.mol != mol.mol);
        self.recipe.trajectories.push(molar_vis_core::TrajectoryTrack { mol: mol.mol, start, end, fps });
        self.prepared = None;
        Ok(())
    }
    #[wasm_bindgen(getter)]
    pub fn duration(&self) -> f64 {
        self.recipe.duration()
    }
    pub async fn preview(&mut self) -> Result<Animation, JsError> {
        let movie = self.prepare().await?;
        let handle = query_job(&self.jobs, move |app| app.preview_movie(movie)).await?;
        Ok(Animation {
            handle,
            jobs: self.jobs.clone(),
        })
    }
    pub async fn seek(&mut self, time: f64) -> Result<(), JsError> {
        let movie = self.prepare().await?;
        query_job(&self.jobs, move |app| {
            app.cancel_animation();
            app.seek_movie(&movie, time)
        })
        .await
    }
}

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

fn rgba(bytes: &[u8]) -> Result<[u8; 4], JsError> {
    bytes
        .try_into()
        .map_err(|_| JsError::new("expected exactly four RGBA8 bytes"))
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
#[derive(Clone)]
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
            .map_err(|e| JsError::new(&format!("can't read {name}: {}", molar_vis_core::error_chain(&e))))?;
        let (top, st) = fh
            .read()
            .map_err(|e| JsError::new(&format!("failed to parse {name}: {}", molar_vis_core::error_chain(&e))))?;
        let n_atoms = top.atoms.len();
        let system = MSystem::new(top, st)
            .map_err(|e| JsError::new(&format!("invalid structure in {name}: {}", molar_vis_core::error_chain(&e))))?;
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
        let (_expr, sel) = molar_vis_core::evaluate(&self.system, text)
            .map_err(|e| JsError::new(&eval_error_msg(&e)))?;
        Ok(Sel {
            indices: sel.get_index_slice().to_vec(),
        })
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

/// All molecule records parsed once from a local file. No rendering or network
/// access is required to inspect counts or obtain an individual molecule.
#[wasm_bindgen]
pub struct StructureFile {
    molecules: Vec<System>,
    n_atoms: usize,
}

#[wasm_bindgen]
impl StructureFile {
    pub fn from_bytes(name: &str, bytes: &[u8]) -> Result<StructureFile, JsError> {
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let mut reader = FileHandler::from_reader(&ext, std::io::Cursor::new(bytes.to_vec()))
            .map_err(|e| JsError::new(&molar_vis_core::error_chain(&e)))?;
        let mut molecules = Vec::new();
        let mut n_atoms = 0;
        loop {
            match reader.read() {
                Ok((topology, state)) => {
                    let count = topology.atoms.len();
                    let system = MSystem::new(topology, state)
                        .map_err(|e| JsError::new(&molar_vis_core::error_chain(&e)))?;
                    n_atoms += count;
                    molecules.push(System {
                        system: Rc::new(system), version: Rc::new(Cell::new(0)),
                        name: format!("{name} #{}", molecules.len() + 1), n_atoms: count,
                    });
                }
                Err(error) if matches!(error.kind(), FileFormatError::Eof) => break,
                Err(error) => return Err(JsError::new(&format!("molecule {}: {}", molecules.len() + 1, molar_vis_core::error_chain(&error)))),
            }
        }
        if molecules.is_empty() { return Err(JsError::new("the file contains no molecules")); }
        Ok(StructureFile { molecules, n_atoms })
    }

    #[wasm_bindgen(getter, js_name = numMolecules)]
    pub fn num_molecules(&self) -> usize { self.molecules.len() }

    #[wasm_bindgen(getter, js_name = numAtoms)]
    pub fn num_atoms(&self) -> usize { self.n_atoms }

    /// Return a shared handle to a zero-based record. The file can be freed
    /// without invalidating this handle or a viewer that uses it.
    pub fn molecule(&self, index: usize) -> Result<System, JsError> {
        self.molecules.get(index).cloned().ok_or_else(|| JsError::new("molecule index is out of range"))
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

async fn query_job<R: 'static>(jobs: &Sender<AppJob>, f: impl FnOnce(&mut App) -> Result<R, String> + 'static) -> Result<R, JsError> {
    let (tx, rx) = futures_channel::oneshot::channel();
    send(jobs, Box::new(move |app| { let _ = tx.send(f(app)); }));
    rx.await.map_err(|_| JsError::new("viewer closed"))?.map_err(|e| JsError::new(&e))
}

/// A live handle to the running viewer. Returned by [`start`]; queues jobs to the
/// viewer, which applies them on the next frame. All methods apply live.
#[wasm_bindgen]
pub struct Visualizer {
    jobs: Sender<AppJob>,
}

#[wasm_bindgen]
impl Visualizer {
    /// Apply sidebar/selection-toolbar visibility on the next viewer frame.
    pub async fn set_ui_visibility(&self, selection_panel: bool, selection_toolbar: bool) -> Result<(), JsError> {
        query_job(&self.jobs, move |app| { app.set_ui_visibility(selection_panel, selection_toolbar); Ok(()) }).await
    }

    /// Read the actual UI state, including changes made through the viewer.
    pub async fn ui_visibility_json(&self) -> Result<String, JsError> {
        query_job(&self.jobs, |app| {
            let (selection_panel, selection_toolbar) = app.ui_visibility();
            Ok(serde_json::json!({"selection_panel": selection_panel, "selection_toolbar": selection_toolbar}).to_string())
        }).await
    }

    pub async fn clear(&self) -> Result<(), JsError> {
        query_job(&self.jobs, |app| { app.clear(); Ok(()) }).await
    }

    /// Set the camera orbit pivot (x, y, z) in nanometers.
    pub fn rotation_center(&self, x: f32, y: f32, z: f32) -> Result<(), JsError> {
        if ![x, y, z].iter().all(|v| v.is_finite()) { return Err(JsError::new("rotation center must be finite")); }
        send(&self.jobs, Box::new(move |app| app.set_rotation_center_xyz([x, y, z]))); Ok(())
    }
    pub async fn focus(
        &self,
        mol: &MolHandle,
        sel: &Sel,
        duration: Option<f64>,
        easing: Option<String>,
    ) -> Result<Animation, JsError> {
        let selection = format!(
            "index {}",
            sel.indices
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        );
        schedule_view(
            &self.jobs,
            ViewAction::Focus {
                mol: mol.mol,
                selection,
            },
            duration,
            easing,
        )
        .await
    }
    pub async fn unobstructed_view(
        &self,
        rep: &RepHandle,
        zoom_out: f32,
        resolution: usize,
        duration: Option<f64>,
        easing: Option<String>,
    ) -> Result<Animation, JsError> {
        schedule_view(
            &self.jobs,
            ViewAction::Unobstructed {
                targets: vec![(rep.mol, rep.rep)],
                zoom_out,
                resolution,
            },
            duration,
            easing,
        )
        .await
    }
    pub async fn unobstructed_view_multi(
        &self,
        reps: &js_sys::Array,
        zoom_out: f32,
        resolution: usize,
        duration: Option<f64>,
        easing: Option<String>,
    ) -> Result<Animation, JsError> {
        let targets = reps
            .iter()
            .map(|value| {
                let read = |name: &str| -> Result<u64, JsError> {
                    let n = js_sys::Reflect::get(&value, &JsValue::from_str(name))
                        .map_err(|_| JsError::new("could not read representation handle"))?
                        .as_f64()
                        .ok_or_else(|| JsError::new("reps must contain RepHandle objects"))?;
                    if !n.is_finite() || n < 0.0 || n.fract() != 0.0 || n > 9_007_199_254_740_991.0
                    {
                        return Err(JsError::new("invalid representation ID"));
                    }
                    Ok(n as u64)
                };
                Ok((read("molId")?, read("repId")?))
            })
            .collect::<Result<Vec<_>, JsError>>()?;
        schedule_view(
            &self.jobs,
            ViewAction::Unobstructed {
                targets,
                zoom_out,
                resolution,
            },
            duration,
            easing,
        )
        .await
    }

    /// JSON snapshot of current colors, including viewer UI/session edits.
    pub async fn colors_json(&self) -> Result<String, JsError> {
        let (tx, rx) = futures_channel::oneshot::channel();
        send(
            &self.jobs,
            Box::new(move |app| {
                let _ = tx.send(app.color_preferences_json());
            }),
        );
        rx.await
            .map_err(|_| JsError::new("viewer closed"))?
            .map_err(|e| JsError::new(&e))
    }
    /// Apply atom, secondary-structure and gradient colors from a viewer preset.
    pub fn set_color_preset(&self, name: &str) -> Result<(), JsError> {
        let preset = molar_vis_core::ColorPreset::parse(name).map_err(|e| JsError::new(&e))?;
        send(&self.jobs, Box::new(move |app| {
            if let Err(e) = app.set_color_preset(preset) {
                log::error!("color preset: {e}");
            }
        }));
        Ok(())
    }

    /// The current preset name, or Custom after manual edits.
    pub async fn color_preset(&self) -> Result<String, JsError> {
        let (tx, rx) = futures_channel::oneshot::channel();
        send(&self.jobs, Box::new(move |app| {
            let _ = tx.send(app.color_preset_name().to_string());
        }));
        rx.await.map_err(|_| JsError::new("viewer closed"))
    }

    /// Replace the discrete palette with consecutive RGBA8 colors.
    pub fn set_palette(&self, colors: &[u8]) -> Result<(), JsError> {
        if colors.is_empty() || colors.len() % 4 != 0 {
            return Err(JsError::new("palette requires one or more RGBA8 colors"));
        }
        let palette: Vec<[u8; 4]> = colors
            .chunks_exact(4)
            .map(|c| c.try_into().unwrap())
            .collect();
        send(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                c.categorical = palette;
                if let Err(e) = app.set_color_preferences(c) {
                    log::error!("palette: {e}");
                }
            }),
        );
        Ok(())
    }
    pub fn reset_colors(&self) {
        send(
            &self.jobs,
            Box::new(move |app| {
                if let Err(e) =
                    app.set_color_preferences(molar_vis_core::ColorPreferences::default())
                {
                    log::error!("reset colors: {e}");
                }
            }),
        );
    }
    /// Persist current program settings, including color edits.
    pub async fn save_settings(&self) -> Result<(), JsError> {
        let (tx, rx) = futures_channel::oneshot::channel();
        send(
            &self.jobs,
            Box::new(move |app| {
                let _ = tx.send(app.save_settings());
            }),
        );
        rx.await
            .map_err(|_| JsError::new("viewer closed"))?
            .map_err(|e| JsError::new(&e))
    }
    pub fn set_element_color(&self, number: u8, color: &[u8]) -> Result<(), JsError> {
        if number > 118 {
            return Err(JsError::new("atomic number must be 0..=118"));
        }
        let color = rgba(color)?;
        send(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                c.elements.insert(number, color);
                if let Err(e) = app.set_color_preferences(c) {
                    log::error!("element color: {e}");
                }
            }),
        );
        Ok(())
    }
    pub fn reset_element_color(&self, number: u8) {
        send(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                c.elements.remove(&number);
                if let Err(e) = app.set_color_preferences(c) {
                    log::error!("element color: {e}");
                }
            }),
        );
    }
    pub fn set_secondary_color(&self, index: usize, color: &[u8]) -> Result<(), JsError> {
        if index >= 9 {
            return Err(JsError::new("secondary index must be 0..=8"));
        }
        let color = rgba(color)?;
        send(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                c.secondary[index] = color;
                if let Err(e) = app.set_color_preferences(c) {
                    log::error!("secondary color: {e}");
                }
            }),
        );
        Ok(())
    }
    /// `stops` contains three consecutive RGBA8 colors (12 bytes total).
    pub fn set_continuous_gradient(&self, stops: &[u8], midpoint: f32) -> Result<(), JsError> {
        if stops.len() != 12 || !midpoint.is_finite() || !(0.01..=0.99).contains(&midpoint) {
            return Err(JsError::new(
                "expected 12 stop bytes and midpoint 0.01..=0.99",
            ));
        }
        let stops = [
            rgba(&stops[0..4])?,
            rgba(&stops[4..8])?,
            rgba(&stops[8..12])?,
        ];
        send(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                c.continuous = molar_vis_core::Gradient { stops, midpoint };
                if let Err(e) = app.set_color_preferences(c) {
                    log::error!("continuous gradient: {e}");
                }
            }),
        );
        Ok(())
    }
    pub fn set_palette_color(&self, index: usize, color: &[u8]) -> Result<(), JsError> {
        let color = rgba(color)?;
        send(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                if let Some(slot) = c.categorical.get_mut(index) {
                    *slot = color;
                    if let Err(e) = app.set_color_preferences(c) {
                        log::error!("palette color: {e}");
                    }
                } else {
                    log::error!("palette index {index} out of range");
                }
            }),
        );
        Ok(())
    }
    pub fn add_palette_color(&self, color: &[u8]) -> Result<(), JsError> {
        let color = rgba(color)?;
        send(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                c.categorical.push(color);
                if let Err(e) = app.set_color_preferences(c) {
                    log::error!("palette: {e}");
                }
            }),
        );
        Ok(())
    }
    pub fn remove_palette_color(&self, index: usize) {
        send(
            &self.jobs,
            Box::new(move |app| {
                let mut c = app.color_preferences();
                if c.categorical.len() > 1 && index < c.categorical.len() {
                    c.categorical.remove(index);
                    if let Err(e) = app.set_color_preferences(c) {
                        log::error!("palette: {e}");
                    }
                } else {
                    log::error!("cannot remove palette index {index}");
                }
            }),
        );
    }
    /// Add a molecule, rendering **directly** from the parsed `System` (by reference);
    /// returns a handle.
    pub fn add_mol(&self, sys: &System) -> MolHandle {
        let mol = molar_vis_core::reserve_mol_id().0;
        let name = format!("mol {mol}");
        let src = WebSystemSource {
            system: sys.system.clone(),
            version: sys.version.clone(),
        };
        send(
            &self.jobs,
            Box::new(move |app: &mut App| {
                if let Err(e) = app.add_shared_molecule_with_id(mol, Box::new(src), name) {
                    log::error!("add_mol failed: {e}");
                }
            }),
        );
        MolHandle { mol, jobs: self.jobs.clone() }
    }

    /// The molecules currently in the scene, as handles.
    pub async fn mols(&self) -> Result<Vec<MolHandle>, JsError> {
        let ids = query_job(&self.jobs, |app| Ok(app.molecule_ids())).await?;
        Ok(ids.into_iter().map(|mol| MolHandle { mol, jobs: self.jobs.clone() }).collect())
    }

    // --- View controls (mirror the view-settings UI + the Python module). ---

    pub fn movie(&self) -> Movie {
        Movie {
            recipe: CoreMovie::default(),
            prepared: None,
            jobs: self.jobs.clone(),
        }
    }
    pub async fn view_json(&self) -> Result<String, JsError> {
        query_job(&self.jobs, |app| {
            serde_json::to_string(&app.view_pose()).map_err(|e| molar_vis_core::error_chain(&e))
        })
        .await
    }
    pub async fn animate_json(&self, json: &str) -> Result<Animation, JsError> {
        let step: ViewStep =
            serde_json::from_str(json).map_err(|e| JsError::new(&molar_vis_core::error_chain(&e)))?;
        step.validate().map_err(|e| JsError::new(&e))?;
        let handle = query_job(&self.jobs, move |app| app.animate_view(step)).await?;
        Ok(Animation {
            handle,
            jobs: self.jobs.clone(),
        })
    }
    pub async fn rotate(
        &self,
        yaw: f32,
        pitch: f32,
        duration: Option<f64>,
        easing: Option<String>,
    ) -> Result<Animation, JsError> {
        schedule_view(
            &self.jobs,
            ViewAction::rotation(yaw, pitch),
            duration,
            easing,
        )
        .await
    }
    pub async fn roll(
        &self,
        angle: f32,
        duration: Option<f64>,
        easing: Option<String>,
    ) -> Result<Animation, JsError> {
        schedule_view(
            &self.jobs,
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
        .await
    }
    pub async fn pan(
        &self,
        dx: f32,
        dy: f32,
        duration: Option<f64>,
        easing: Option<String>,
    ) -> Result<Animation, JsError> {
        schedule_view(
            &self.jobs,
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
        .await
    }
    pub async fn zoom(
        &self,
        factor: f32,
        duration: Option<f64>,
        easing: Option<String>,
    ) -> Result<Animation, JsError> {
        schedule_view(&self.jobs, ViewAction::zoom(factor), duration, easing).await
    }
    pub async fn reset_view(
        &self,
        duration: Option<f64>,
        easing: Option<String>,
    ) -> Result<Animation, JsError> {
        schedule_view(&self.jobs, ViewAction::Reset, duration, easing).await
    }
    pub async fn go_to_view(
        &self,
        json: &str,
        duration: Option<f64>,
        easing: Option<String>,
    ) -> Result<Animation, JsError> {
        let view = serde_json::from_str(json).map_err(|e| JsError::new(&molar_vis_core::error_chain(&e)))?;
        schedule_view(&self.jobs, ViewAction::To { view }, duration, easing).await
    }
    pub fn cancel_animation(&self) {
        send(&self.jobs, Box::new(|app| app.cancel_animation()));
    }

    /// Set the projection: `"perspective"` or `"orthographic"`.
    pub fn projection(&self, mode: &str) -> Result<(), JsError> {
        let p = parse_projection(mode).map_err(|e| JsError::new(&e))?;
        send(&self.jobs, Box::new(move |app| app.set_projection(p)));
        Ok(())
    }

    /// Flat background color, RGB in 0–1.
    pub fn background(&self, r: f32, g: f32, b: f32) {
        send(
            &self.jobs,
            Box::new(move |app| app.set_background_solid([r, g, b])),
        );
    }

    /// Vertical gradient background; `top`/`bottom` are `[r, g, b]` in 0–1.
    pub fn background_gradient(&self, top: &[f32], bottom: &[f32]) -> Result<(), JsError> {
        if top.len() < 3 || bottom.len() < 3 {
            return Err(JsError::new("background_gradient expects [r, g, b] arrays"));
        }
        let (t, b) = ([top[0], top[1], top[2]], [bottom[0], bottom[1], bottom[2]]);
        send(
            &self.jobs,
            Box::new(move |app| app.set_background_gradient(t, b)),
        );
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
    pub fn depth_cue(
        &self,
        enabled: bool,
        mode: &str,
        strength: f32,
        start: f32,
    ) -> Result<(), JsError> {
        let (enabled, mode) = match mode.to_ascii_lowercase().as_str() {
            "none" | "off" => (false, CueMode::Linear),
            other => (
                enabled,
                parse_cue_mode(other).map_err(|e| JsError::new(&e))?,
            ),
        };
        send(
            &self.jobs,
            Box::new(move |app| app.set_depth_cue(enabled, mode, strength, start)),
        );
        Ok(())
    }

    /// Screen-space ambient occlusion: `strength` darkening, `radius` in nm.
    pub fn ambient_occlusion(&self, enabled: bool, strength: f32, radius: f32) {
        send(
            &self.jobs,
            Box::new(move |app| app.set_ambient_occlusion(enabled, strength, radius)),
        );
    }

    /// Real-time cast shadows: `strength` scales how dark shadowed areas get.
    pub fn shadows(&self, enabled: bool, strength: f32) {
        send(
            &self.jobs,
            Box::new(move |app| app.set_shadows(enabled, strength)),
        );
    }
}

/// Handle to a molecule in the viewer.
#[wasm_bindgen]
pub struct MolHandle {
    mol: u64,
    jobs: Sender<AppJob>,
}

#[wasm_bindgen]
impl MolHandle {
    /// Add an interactions representation against a partner representation.
    pub fn add_interactions(&self, partner: &RepHandle) -> RepHandle {
        let rep = molar_vis_core::reserve_rep_id().0;
        let (m, pm, pr) = (self.mol, partner.mol, partner.rep);
        send(&self.jobs, Box::new(move |app| { if let Err(e) = app.add_interactions_rep_with_id(m, rep, pm, pr) { log::error!("add interactions: {e}"); } }));
        RepHandle { mol: m, rep, jobs: self.jobs.clone() }
    }
    /// Import molecule-owned PLY bytes (positions are nanometers).
    pub fn add_mesh_ply(&self, name: String, bytes: &[u8]) -> Result<MeshHandle, JsError> {
        molar_vis_core::import_ply(bytes).map_err(|e| JsError::new(&e))?;
        let id = molar_vis_core::reserve_mesh_id();
        let queued = bytes.to_vec();
        let mol = self.mol;
        send(
            &self.jobs,
            Box::new(move |app| {
                if let Err(e) = app.resolve_molecule(mol).and_then(|mi| app.add_mesh_ply(mi, id, name, &queued)) {
                    log::error!("add mesh: {e}");
                }
            }),
        );
        Ok(MeshHandle {
            mol,
            id,
            jobs: self.jobs.clone(),
        })
    }

    /// Includes meshes created by the viewer UI or restored from a session.
    pub async fn meshes(&self) -> Result<Vec<MeshHandle>, JsError> {
        let (tx, rx) = futures_channel::oneshot::channel();
        let mol = self.mol;
        send(
            &self.jobs,
            Box::new(move |app| {
                let _ = tx.send(app.resolve_molecule(mol).and_then(|mi| app.mesh_ids(mi)));
            }),
        );
        let ids = rx
            .await
            .map_err(|_| JsError::new("viewer closed"))?
            .map_err(|e| JsError::new(&e))?;
        Ok(ids
            .into_iter()
            .map(|id| MeshHandle {
                mol,
                id,
                jobs: self.jobs.clone(),
            })
            .collect())
    }

    /// Add a representation. `sel` (a `Sel`, consumed) sets the selection;
    /// `style`/`color`/`material` are names like `"cartoon"`/`"ss"`/`"Transparent"`.
    /// Any argument may be omitted. Returns a handle.
    pub fn add_rep(
        &self,
        sel: Option<Sel>,
        style: Option<String>,
        color: Option<String>,
        material: Option<String>,
        width: Option<f32>,
    ) -> RepHandle {
        let rep = molar_vis_core::reserve_rep_id().0;
        let indices = sel.map(|s| s.indices);
        let mol = self.mol;
        send(
            &self.jobs,
            Box::new(move |app: &mut App| {
                if let Err(e) = app.add_rep_default_with_id(mol, rep) {
                    log::error!("add_rep failed: {e}");
                    return;
                }
                let (mi, ri) = match app.resolve_rep(mol, rep) { Ok(pair) => pair, Err(e) => { log::error!("add_rep resolve: {e}"); return; } };
                if let Some(idx) = &indices {
                    if let Err(e) = app.set_rep_selection(mi, ri, idx) {
                        log::error!("add_rep select failed: {e}");
                    }
                }
                if let Some(st) = &style {
                    if let Err(e) = app.set_rep_style(mi, ri, st) {
                        log::error!("add_rep style failed: {e}");
                    }
                }
                if let Some(c) = &color {
                    if let Err(e) = app.set_rep_color(mi, ri, c) {
                        log::error!("add_rep color failed: {e}");
                    }
                }
                if let Some(m) = &material {
                    if let Err(e) = app.set_rep_material(mi, ri, m) {
                        log::error!("add_rep material failed: {e}");
                    }
                }
                if let Some(w) = width {
                    if let Err(e) = app.set_rep_width(mi, ri, w) {
                        log::error!("add_rep width failed: {e}");
                    }
                }
            }),
        );
        RepHandle {
            mol,
            rep,
            jobs: self.jobs.clone(),
        }
    }

    /// This molecule's representations, as handles.
    pub async fn reps(&self) -> Result<Vec<RepHandle>, JsError> {
        let mol = self.mol;
        let ids = query_job(&self.jobs, move |app| app.rep_ids(mol)).await?;
        Ok(ids.into_iter().map(|rep| RepHandle { mol, rep, jobs: self.jobs.clone() }).collect())
    }
}

/// Stable handle to a molecule-owned PLY mesh.
#[wasm_bindgen]
pub struct MeshHandle {
    mol: u64,
    id: u64,
    jobs: Sender<AppJob>,
}

#[wasm_bindgen]
impl MeshHandle {
    /// JSON snapshot of name, visibility, material, style, sizes, smoothing, and counts.
    pub async fn details_json(&self) -> Result<String, JsError> {
        let (tx, rx) = futures_channel::oneshot::channel();
        let (m, id) = (self.mol, self.id);
        send(
            &self.jobs,
            Box::new(move |app| {
                let _ = tx.send(app.resolve_molecule(m).and_then(|mi| app.mesh_details_json(mi, id)));
            }),
        );
        rx.await
            .map_err(|_| JsError::new("viewer closed"))?
            .map_err(|e| JsError::new(&e))
    }
    #[wasm_bindgen(setter)]
    pub fn set_visible(&self, value: bool) {
        let (m, id) = (self.mol, self.id);
        send(
            &self.jobs,
            Box::new(move |app| {
                if let Err(e) = app.resolve_molecule(m).and_then(|mi| app.set_mesh_visible(mi, id, value)) {
                    log::error!("mesh visible: {e}");
                }
            }),
        );
    }
    #[wasm_bindgen(setter)]
    pub fn set_material(&self, value: String) {
        let (m, id) = (self.mol, self.id);
        send(
            &self.jobs,
            Box::new(move |app| {
                if let Err(e) = app.resolve_molecule(m).and_then(|mi| app.set_mesh_material(mi, id, &value)) {
                    log::error!("mesh material: {e}");
                }
            }),
        );
    }
    #[wasm_bindgen(setter)]
    pub fn set_style(&self, value: String) {
        let (m, id) = (self.mol, self.id);
        send(
            &self.jobs,
            Box::new(move |app| {
                if let Err(e) = app.resolve_molecule(m).and_then(|mi| app.set_mesh_style(mi, id, &value)) {
                    log::error!("mesh style: {e}");
                }
            }),
        );
    }
    #[wasm_bindgen(setter)]
    pub fn set_point_size(&self, value: f32) -> Result<(), JsError> {
        if !value.is_finite() || !(1.0..=10.0).contains(&value) {
            return Err(JsError::new("point_size must be 1..=10"));
        }
        let (m, id) = (self.mol, self.id);
        send(
            &self.jobs,
            Box::new(move |app| {
                if let Err(e) = app.resolve_molecule(m).and_then(|mi| app.set_mesh_point_size(mi, id, value)) {
                    log::error!("mesh point size: {e}");
                }
            }),
        );
        Ok(())
    }
    #[wasm_bindgen(setter)]
    pub fn set_line_width(&self, value: f32) -> Result<(), JsError> {
        if !value.is_finite() || !(1.0..=10.0).contains(&value) {
            return Err(JsError::new("line_width must be 1..=10"));
        }
        let (m, id) = (self.mol, self.id);
        send(
            &self.jobs,
            Box::new(move |app| {
                if let Err(e) = app.resolve_molecule(m).and_then(|mi| app.set_mesh_line_width(mi, id, value)) {
                    log::error!("mesh line width: {e}");
                }
            }),
        );
        Ok(())
    }
    #[wasm_bindgen(setter)]
    pub fn set_smoothing(&self, value: u32) -> Result<(), JsError> {
        if value > 5 {
            return Err(JsError::new("smoothing must be 0..=5"));
        }
        let (m, id) = (self.mol, self.id);
        send(
            &self.jobs,
            Box::new(move |app| {
                if let Err(e) = app.resolve_molecule(m).and_then(|mi| app.set_mesh_smoothing(mi, id, value)) {
                    log::error!("mesh smoothing: {e}");
                }
            }),
        );
        Ok(())
    }
    /// Export original PLY bytes. Display smoothing does not modify the source.
    pub async fn ply_bytes(&self) -> Result<Vec<u8>, JsError> {
        let (tx, rx) = futures_channel::oneshot::channel();
        let (m, id) = (self.mol, self.id);
        send(
            &self.jobs,
            Box::new(move |app| {
                let _ = tx.send(app.resolve_molecule(m).and_then(|mi| app.mesh_ply(mi, id)));
            }),
        );
        rx.await
            .map_err(|_| JsError::new("viewer closed"))?
            .map_err(|e| JsError::new(&e))
    }
    pub fn delete(&self) {
        let (m, id) = (self.mol, self.id);
        send(
            &self.jobs,
            Box::new(move |app| {
                if let Err(e) = app.resolve_molecule(m).and_then(|mi| app.delete_mesh(mi, id)) {
                    log::error!("delete mesh: {e}");
                }
            }),
        );
    }
}

/// Handle to a representation. Property setters (`rep.style = "lines"`, …) apply live.
#[wasm_bindgen]
pub struct RepHandle {
    mol: u64,
    rep: u64,
    jobs: Sender<AppJob>,
}

#[wasm_bindgen]
impl RepHandle {
    #[wasm_bindgen(getter, js_name = molId)]
    pub fn mol_id(&self) -> f64 { self.mol as f64 }
    #[wasm_bindgen(getter, js_name = repId)]
    pub fn rep_id(&self) -> f64 { self.rep as f64 }
    #[wasm_bindgen(setter)]
    pub fn set_width(&self, value: f32) -> Result<(), JsError> {
        if !value.is_finite() || value < 1.0 { return Err(JsError::new("width must be at least 1 pixel")); }
        let (m, r) = (self.mol, self.rep);
        send(&self.jobs, Box::new(move |app| { if let Err(e) = app.resolve_rep(m, r).and_then(|(mi,ri)| app.set_rep_width(mi, ri, value)) { log::error!("rep width: {e}"); } })); Ok(())
    }
    #[wasm_bindgen(setter)]
    pub fn set_style(&self, value: String) {
        let (mol, rep) = (self.mol, self.rep);
        send(
            &self.jobs,
            Box::new(move |app| {
                if let Err(e) = app.resolve_rep(mol, rep).and_then(|(mi,ri)| app.set_rep_style(mi, ri, &value)) {
                    log::error!("set style: {e}");
                }
            }),
        );
    }

    #[wasm_bindgen(setter)]
    pub fn set_color(&self, value: String) {
        let (mol, rep) = (self.mol, self.rep);
        send(
            &self.jobs,
            Box::new(move |app| {
                if let Err(e) = app.resolve_rep(mol, rep).and_then(|(mi,ri)| app.set_rep_color(mi, ri, &value)) {
                    log::error!("set color: {e}");
                }
            }),
        );
    }

    #[wasm_bindgen(setter)]
    pub fn set_material(&self, value: String) {
        let (mol, rep) = (self.mol, self.rep);
        send(
            &self.jobs,
            Box::new(move |app| {
                if let Err(e) = app.resolve_rep(mol, rep).and_then(|(mi,ri)| app.set_rep_material(mi, ri, &value)) {
                    log::error!("set material: {e}");
                }
            }),
        );
    }

    /// Set the selection from a `Sel`'s atoms.
    pub fn select(&self, sel: &Sel) {
        let (mol, rep) = (self.mol, self.rep);
        let indices = sel.indices.clone();
        send(
            &self.jobs,
            Box::new(move |app| {
                if let Err(e) = app.resolve_rep(mol, rep).and_then(|(mi,ri)| app.set_rep_selection(mi, ri, &indices)) {
                    log::error!("select: {e}");
                }
            }),
        );
    }
}

/// Boot the viewer onto an existing `<canvas id=canvas_id>` and return a handle to it.
/// Runs the eframe `WebRunner` on the page's animation-frame loop (single-threaded), so
/// it shares the page's thread; jobs sent through the returned [`Visualizer`] are
/// applied on the next frame. Throws if the canvas is missing or a viewer is already
/// running on this page (one viewer per page in v1).
#[wasm_bindgen]
pub fn start(canvas_id: &str) -> Result<Visualizer, JsError> {
    let (canvas, tx, rx) = prepare_viewer(canvas_id)?;
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = run_viewer(canvas, rx, false).await {
            log::error!("molar_vis failed to start: {error:?}");
        }
    });
    Ok(Visualizer { jobs: tx })
}

/// Start an embedded viewer with its selection sidebar and toolbar hidden.
/// Resolves after graphics initialization; rejects if the renderer cannot start.
#[wasm_bindgen]
pub async fn start_embedded(canvas_id: &str) -> Result<Visualizer, JsError> {
    let (canvas, tx, rx) = prepare_viewer(canvas_id)?;
    run_viewer(canvas, rx, true).await?;
    Ok(Visualizer { jobs: tx })
}

fn prepare_viewer(canvas_id: &str) -> Result<(web_sys::HtmlCanvasElement, Sender<AppJob>, std::sync::mpsc::Receiver<AppJob>), JsError> {
    use wasm_bindgen::JsCast as _;
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    let document = web_sys::window().and_then(|w| w.document()).ok_or_else(|| JsError::new("no document"))?;
    let canvas = document.get_element_by_id(canvas_id)
        .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .ok_or_else(|| JsError::new(&format!("page has no canvas {canvas_id:?}")))?;
    if VIEWER_STARTED.with(|f| f.replace(true)) {
        return Err(JsError::new("a molar_vis viewer is already running on this page"));
    }
    let (tx, rx) = std::sync::mpsc::channel::<AppJob>();
    Ok((canvas, tx, rx))
}

async fn run_viewer(canvas: web_sys::HtmlCanvasElement, rx: std::sync::mpsc::Receiver<AppJob>, embedded: bool) -> Result<(), JsError> {
    let result = eframe::WebRunner::new().start(
        canvas, eframe::WebOptions::default(),
        Box::new(move |cc| {
            let mut app = App::new(cc, AppLaunch::default())?;
            if embedded { app.set_ui_visibility(false, false); }
            app.set_jobs(rx);
            Ok(Box::new(app) as Box<dyn eframe::App>)
        }),
    ).await;
    if let Err(error) = result {
        VIEWER_STARTED.with(|f| f.set(false));
        return Err(JsError::new(&molar_vis_core::js_error_chain(&error)));
    }
    Ok(())
}
