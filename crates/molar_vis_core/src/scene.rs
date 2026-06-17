//! The scene graph: a set of molecules, each with its own representations.

use std::collections::HashMap;
use std::path::PathBuf;

use glam::Vec3;
use molar::prelude::*;
use serde::{Deserialize, Serialize};

use crate::color::ColorMethod;
use crate::data::RawMolecule;
use crate::geometry::{RepKind, RepParams};
use crate::material::Material;
use crate::render::RepGpu;
use crate::secstruct::SsMap;
use crate::trajectory::Trajectory;

/// Stable per-molecule identity, so undo/redo can reference molecules across
/// deletion (a deleted molecule is parked in [`Scene::trash`] by this id).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct MolId(pub u64);

/// Periodic-image display for a representation: render extra copies of the
/// selection shifted by integer combinations of the box lattice vectors `a,b,c`.
/// This is **purely a rendering** concern — molar stores only the "self" coords;
/// images are drawn by re-running the same GPU geometry under a translated camera,
/// so nothing is duplicated on the CPU or GPU. Only meaningful when the molecule
/// has a periodic box. In `EditState` (undoable).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct PeriodicParams {
    /// Render the central, un-shifted copy.
    pub self_img: bool,
    /// Draw the periodic box wireframe, replicated across every shown image.
    pub show_box: bool,
    /// Image counts in the −a, −b, −c directions.
    pub neg: [u32; 3],
    /// Image counts in the +a, +b, +c directions.
    pub pos: [u32; 3],
}

impl Default for PeriodicParams {
    fn default() -> Self {
        Self { self_img: true, show_box: false, neg: [0; 3], pos: [0; 3] }
    }
}

impl PeriodicParams {
    /// World-space translation offsets of every image this rep draws (the central
    /// `(0,0,0)` image included iff `self_img`), as integer combinations of the box
    /// lattice vectors `a,b,c` (nm). Shared by the renderer (one camera per offset)
    /// and the picker (hit-test every drawn image) so they always agree.
    pub fn offsets(&self, a: Vec3, b: Vec3, c: Vec3) -> Vec<Vec3> {
        let mut out = Vec::new();
        for i in -(self.neg[0] as i32)..=(self.pos[0] as i32) {
            for j in -(self.neg[1] as i32)..=(self.pos[1] as i32) {
                for k in -(self.neg[2] as i32)..=(self.pos[2] as i32) {
                    if i == 0 && j == 0 && k == 0 && !self.self_img {
                        continue;
                    }
                    out.push(a * i as f32 + b * j as f32 + c * k as f32);
                }
            }
        }
        out
    }
}

/// Where a molecule's structure was loaded from, so a saved visualization state
/// can reload the same atoms. Sessions reference molecules by source rather than
/// embedding their coordinates (that is a separate "save molecules to file"
/// feature) — small, and lets the structure file evolve independently.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MoleculeSource {
    /// A structure file on disk (native). Reloaded with [`crate::data::load`].
    File(PathBuf),
    /// In-memory bytes (the browser file picker, or the bundled demo): there is no
    /// path to reload from, so a session referencing this cannot restore the atoms
    /// in a fresh process. We keep the original name for display/diagnostics.
    Bytes { name: String },
}

impl Default for MoleculeSource {
    fn default() -> Self {
        MoleculeSource::Bytes { name: "molecule".to_string() }
    }
}

/// A record of one trajectory file loaded into a molecule, so a saved session can
/// replay the same loads. Multiple loads concatenate (see [`Trajectory`]); the
/// list preserves that order. Native-only in practice (paths), but the type is
/// platform-agnostic so the session format is uniform.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrajLoad {
    pub path: PathBuf,
    pub from: usize,
    pub to: Option<usize>,
    pub stride: usize,
}

/// One representation of a molecule: a selection rendered in a given style.
pub struct Representation {
    pub kind: RepKind,
    pub params: RepParams,
    pub color: ColorMethod,
    /// Appearance preset (lighting + opacity); see [`crate::material::Material`].
    pub material: Material,
    /// Secondary-structure algorithm driving the Cartoon shape and the
    /// "Structure" color scheme (DSSP vanilla / PyMOL dss).
    pub ss_algo: SsAlgorithm,
    /// Editable selection text — the UI buffer / draft (egui needs a `&mut String`,
    /// and it can hold not-yet-valid input). The committed text also lives in
    /// `expr` (`SelectionExpr::get_str`) once it parses.
    pub sel_text: String,
    /// Compiled selection (molar `SelectionExpr`), parsed from `sel_text` on commit.
    /// Re-evaluated per trajectory frame later; `None` until first successful parse.
    pub expr: Option<SelectionExpr>,
    /// Evaluated atom set for the current state; bind to the `System` for coords.
    pub sel: Option<Sel>,
    /// Last selection error, shown in the UI; `None` if the selection is valid.
    pub sel_error: Option<String>,
    /// For a parse error, the character offset into `sel_text` the molar caret
    /// points at, so the UI can highlight the erroring part of the text in place.
    /// `None` for non-positional errors. Transient, not in `EditState`.
    pub sel_error_caret: Option<usize>,
    /// The selection is valid but matches **zero atoms** (molar's "empty" error,
    /// surfaced as a non-destructive warning). The field is flagged in the UI and
    /// the rep renders nothing; the text is kept. Transient, not in `EditState`.
    pub sel_empty: bool,
    /// Periodic-image display (see [`PeriodicParams`]). In `EditState`.
    pub periodic: PeriodicParams,
    /// Trajectory smoothing window (odd; `1` = off). When `> 1`, the rendered
    /// coordinates are a Savitzky–Golay blend of the nearby frames, computed
    /// transiently at build time (`Trajectory::smoothed_state`). In `EditState`.
    pub smooth_window: u32,
    pub visible: bool,
    /// Re-evaluate the (compiled) selection every time the System's State changes
    /// (i.e. each trajectory frame). For coordinate-dependent selections like
    /// `within …`.
    pub dynamic: bool,
    /// For Cartoon / SecStruct reps: recompute secondary structure on every
    /// trajectory frame (else compute once and reuse `ss_cache`). DSSP is the main
    /// per-frame cost, so this defaults to off. Part of `EditState` (undoable).
    pub ss_per_frame: bool,
    /// Cached secondary structure from the last full (structural) build, reused
    /// for coordinate-only frame updates when `ss_per_frame` is off. Transient.
    pub ss_cache: Option<SsMap>,
    /// Cached CPU copy of the last-built **Cartoon** ribbon mesh (with per-vertex
    /// `vert_res` residue tags), so the selection glow can extract just the chosen
    /// residues' sub-ribbon from this *exact* geometry (coincident → no z-fight, and
    /// works for a single residue). `None` for non-cartoon reps. Transient.
    pub cartoon_cache: Option<crate::geometry::MeshData>,
    /// Transient UI state: whether this rep's inline settings panel is expanded.
    /// Not part of `EditState` (view state, not undoable).
    pub params_open: bool,
    /// Transient UI state: which tab of the settings panel is shown.
    pub settings_tab: SettingsTab,
    /// `sel_text` changed → recompile the selection.
    pub sel_dirty: bool,
    /// Selection/style/color/params changed → full geometry rebuild + buffer
    /// re-create (`renderer.upload`).
    pub geom_dirty: bool,
    /// Only coordinates changed (a trajectory frame, same selection/structure) →
    /// recompute geometry and update existing GPU buffers in place
    /// (`renderer.update`), avoiding reallocation. Ignored if `geom_dirty` is set.
    pub coords_dirty: bool,
    pub gpu: RepGpu,
}

impl Representation {
    pub fn new(kind: RepKind) -> Self {
        Self::restore(
            kind,
            RepParams::for_kind(kind),
            ColorMethod::Element,
            SsAlgorithm::default(),
            "all".to_string(),
            true,
            false,
            false,
            Material::default(),
            PeriodicParams::default(),
            1, // smooth_window: off
        )
    }

    /// A copy with the same style/selection but fresh (unbuilt) GPU state, so it
    /// recompiles and uploads its own geometry on the next frame.
    pub fn duplicate(&self) -> Self {
        Self::restore(
            self.kind,
            self.params,
            self.color,
            self.ss_algo,
            self.sel_text.clone(),
            self.visible,
            self.dynamic,
            self.ss_per_frame,
            self.material,
            self.periodic,
            self.smooth_window,
        )
    }

    /// Reconstruct a representation from saved editable fields (used by undo/redo).
    /// Starts dirty so its selection recompiles and geometry rebuilds next frame.
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        kind: RepKind,
        params: RepParams,
        color: ColorMethod,
        ss_algo: SsAlgorithm,
        sel_text: String,
        visible: bool,
        dynamic: bool,
        ss_per_frame: bool,
        material: Material,
        periodic: PeriodicParams,
        smooth_window: u32,
    ) -> Self {
        Self {
            kind,
            params,
            color,
            material,
            ss_algo,
            sel_text,
            expr: None,
            sel: None,
            sel_error: None,
            sel_error_caret: None,
            sel_empty: false,
            periodic,
            smooth_window,
            visible,
            dynamic,
            ss_per_frame,
            ss_cache: None,
            cartoon_cache: None,
            params_open: false,
            settings_tab: SettingsTab::default(),
            sel_dirty: true,
            geom_dirty: false,
            coords_dirty: false,
            gpu: RepGpu::default(),
        }
    }
}

/// Which tab of a representation's settings panel is shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SettingsTab {
    /// Style-specific geometry parameters.
    #[default]
    Style,
    /// Trajectory / per-frame behavior.
    Traj,
    /// Periodic-image rendering.
    Periodic,
}

/// A freshly captured selection (e.g. from the lasso) that has not yet been
/// committed to a real [`Representation`]. It is **view state** (not undoable, not
/// in `EditState`): it renders as a glowing highlight over the atoms exactly as the
/// existing reps already draw them, and the panel shows it with a minimal
/// accept/discard interface instead of the normal rep controls. Accepting it
/// creates a normal Ball-and-Stick representation over [`PendingSelection::sel_text`];
/// discarding drops it. (The two-step scheme leaves room for later set operations —
/// e.g. unioning a new lasso into the active selection with Shift held.)
pub struct PendingSelection {
    /// molar selection text reproducing the captured atom set (e.g. `index 1:3 7`),
    /// used both when the selection is accepted as a representation and to rebuild
    /// the glow geometry (intersected with each rep's own selection / style).
    pub sel_text: String,
    /// Captured atoms' global indices (sorted ascending). The glow geometry is built
    /// per visible rep as (rep selection ∩ these atoms) in that rep's style.
    pub atoms: Vec<usize>,
}

/// A loaded molecule. The live molar `System` is the single source of per-atom
/// data (positions, elements, radii); we additionally keep only the guessed
/// connectivity and a cached bounding box, plus the representations.
pub struct Molecule {
    pub id: MolId,
    pub name: String,
    /// Where the structure was loaded from (for saving/reloading sessions). Only
    /// read by the (native) session capture, hence allowed-dead on wasm.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub source: MoleculeSource,
    /// Trajectory files loaded into this molecule, in load order, so a session can
    /// replay them. Appended whenever frames are loaded from a file.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub traj_loads: Vec<TrajLoad>,
    pub system: System,
    pub bonds: Vec<[usize; 2]>,
    pub n_atoms: usize,
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
    /// World positions of the molecule's **outermost** atoms (capped), computed
    /// once at load. The reflective floor sits at the lowest of these in view space
    /// — i.e. at the molecule's *actual* bottom surface, so it touches the molecule
    /// (no gap → no perceived "bounce" as the view rotates, and the reflection stays
    /// attached). Using the real atoms, not the AABB corners (which overshoot) or a
    /// conservative sphere (which sits far below).
    pub shell: Vec<Vec3>,
    pub visible: bool,
    pub reps: Vec<Representation>,
    pub selected_rep: Option<usize>,
    /// Transient UI state: whether this molecule's representations block is
    /// expanded in the panel. Not part of `EditState` (view state, not undoable).
    pub reps_open: bool,
    /// Loaded MD frames + playback state. Empty until a trajectory is loaded
    /// (then frame 0 is the structure coords; see [`Molecule::seed_frame0`]).
    /// Not part of `EditState` — frame/playback is view state, like the camera.
    pub trajectory: Trajectory,
    /// Show the periodic box as a wireframe overlay (transient view toggle).
    pub show_box: bool,
    /// GPU buffer for the box wireframe (lines only); rebuilt when `box_dirty`.
    pub box_gpu: RepGpu,
    /// Box geometry needs (re)building — toggled on, or coordinates changed.
    pub box_dirty: bool,
    /// A not-yet-committed selection (e.g. captured by a lasso), shown as a glowing
    /// highlight with a minimal accept/discard UI. View state, not undoable; see
    /// [`PendingSelection`]. `None` when there is no active selection.
    pub pending: Option<PendingSelection>,
    /// GPU geometry for the active-selection glow: the pending atoms rebuilt in each
    /// rep's own style (so the highlight glows in the current style). Empty when
    /// there's no pending selection.
    pub glow_gpu: RepGpu,
    /// The glow geometry needs (re)building — pending changed, or its coords moved.
    pub glow_dirty: bool,
    /// Transient hover highlight (Residues hover-pick mode): the hovered residue's
    /// atoms, glowing in the current style like a pending selection but **steady**
    /// (no pulse) and with no accept/discard UI. Recomputed as the cursor moves;
    /// not undoable, not in `EditState`. `None` when nothing is hovered.
    pub hover: Option<Vec<usize>>,
    /// GPU geometry for the steady hover highlight (built from `hover`).
    pub hover_gpu: RepGpu,
    /// The hover-highlight geometry needs (re)building — `hover` set changed.
    pub hover_dirty: bool,
    /// GPU pick geometry: one id-stamped sphere impostor per **pickable** atom (the
    /// atoms CPU `pick` ray-casts: eligible atoms of visible reps, at their displayed
    /// position and effective radius). Rendered into the id-buffer for GPU picking.
    /// Native only (GPU picking needs a synchronous readback wasm can't do).
    #[cfg(not(target_arch = "wasm32"))]
    pub pick_gpu: RepGpu,
    /// The pick geometry needs (re)building — geometry/coords/visibility changed.
    #[cfg(not(target_arch = "wasm32"))]
    pub pick_dirty: bool,
}

impl Molecule {
    pub fn new(id: MolId, raw: RawMolecule, default_rep: RepKind) -> Self {
        // Outer-shell atoms for the reflective floor: keep the atoms farthest from
        // the bbox center (support candidates), capped, so the per-frame floor-height
        // search is cheap. Computed once here.
        let center = (raw.bbox_min + raw.bbox_max) * 0.5;
        let shell = {
            const CAP: usize = 4096;
            let all = raw.system.select_all();
            let mut atoms: Vec<(f32, Vec3)> = raw
                .system
                .bind(&all)
                .iter_particle()
                .map(|p| {
                    let w = Vec3::new(p.pos.x, p.pos.y, p.pos.z);
                    ((w - center).length_squared(), w)
                })
                .collect();
            if atoms.len() > CAP {
                // Keep the CAP farthest-from-center atoms (the outer surface).
                atoms.sort_by(|a, b| b.0.total_cmp(&a.0));
                atoms.truncate(CAP);
            }
            atoms.into_iter().map(|(_, w)| w).collect::<Vec<_>>()
        };
        Self {
            id,
            name: raw.name,
            source: raw.source,
            traj_loads: Vec::new(),
            system: raw.system,
            bonds: raw.bonds,
            n_atoms: raw.n_atoms,
            bbox_min: raw.bbox_min,
            bbox_max: raw.bbox_max,
            shell,
            visible: true,
            reps: vec![Representation::new(default_rep)],
            selected_rep: Some(0),
            reps_open: true,
            trajectory: Trajectory::default(),
            show_box: false,
            box_gpu: RepGpu::default(),
            // Build the box geometry up front (if the molecule has one) so a rep's
            // periodic `Box` toggle can draw it without the molecule-level box ever
            // being shown. Cheap (24 verts); a no-op when there's no box.
            box_dirty: true,
            pending: None,
            glow_gpu: RepGpu::default(),
            glow_dirty: false,
            hover: None,
            hover_gpu: RepGpu::default(),
            hover_dirty: false,
            #[cfg(not(target_arch = "wasm32"))]
            pick_gpu: RepGpu::default(),
            #[cfg(not(target_arch = "wasm32"))]
            pick_dirty: true,
        }
    }

    /// Capture the molecule's current structure coordinates as trajectory frame 0,
    /// if no frames are loaded yet. `System` has no state getter, so we use the
    /// `set_state` swap trick: swap in a same-length placeholder to take ownership
    /// of the real state, clone it, and swap the real state back.
    pub fn seed_frame0(&mut self) {
        if !self.trajectory.frames.is_empty() {
            return;
        }
        let placeholder = State::new_fake(self.n_atoms);
        if let Ok(real) = self.system.set_state(placeholder) {
            self.trajectory.frames.push(real.clone());
            let _ = self.system.set_state(real); // restore the live state
        }
    }

    /// Append loaded frames to the trajectory (sync load).
    pub fn append_frames(&mut self, frames: Vec<State>) {
        self.trajectory.frames.extend(frames);
    }

    /// Append one streamed frame (async load).
    pub fn push_frame(&mut self, frame: State) {
        self.trajectory.frames.push(frame);
    }

    /// Mark representations dirty for the current trajectory frame. The frame's
    /// coordinates are read **by reference** at rebuild time (`bind_with_state`),
    /// so the per-frame state is NOT copied into the `System` — except for
    /// molecules with `dynamic` reps, whose selections are re-evaluated against
    /// the system's own state, so those (rare) get the frame copied in.
    ///
    /// Routing per rep:
    /// - `dynamic` → `sel_dirty` (re-evaluate selection, full rebuild);
    /// - Cartoon/SecStruct with `ss_per_frame` → `geom_dirty` (SS may restructure);
    /// - otherwise → `coords_dirty` (coords only → incremental in-place GPU update,
    ///   reusing the cached secondary structure — no DSSP).
    pub fn apply_current_frame(&mut self) {
        if self.trajectory.frames.get(self.trajectory.current).is_none() {
            return;
        }
        self.box_dirty = true; // the box can change per frame (e.g. NPT)
        if self.pending.is_some() {
            self.glow_dirty = true; // the glow follows the atoms' new positions
        }
        let needs_eval = self.reps.iter().any(|r| r.dynamic);
        if needs_eval {
            if let Some(frame) = self.trajectory.frames.get(self.trajectory.current) {
                let _ = self.system.set_state(frame.clone());
            }
        }
        for rep in &mut self.reps {
            if rep.dynamic {
                rep.sel_dirty = true;
            } else if matches!(rep.kind, RepKind::Surface)
                || (rep.ss_per_frame && crate::geometry::needs_ss(&rep.params, rep.color))
            {
                // The surface mesh is rebuilt from scratch each frame (its topology
                // changes with the coordinates), so it can't use the in-place
                // coords-only GPU update.
                rep.geom_dirty = true;
            } else {
                rep.coords_dirty = true;
            }
        }
    }

    /// The state currently displayed: the active trajectory frame, or the static
    /// structure state when no trajectory is loaded.
    pub fn render_state(&self) -> &State {
        self.trajectory
            .frames
            .get(self.trajectory.current)
            .unwrap_or_else(|| self.system.state())
    }

    /// Bounding box (nm) of selection `sel` at the currently displayed frame.
    pub fn sel_bbox(&self, sel: &Sel) -> (Vec3, Vec3) {
        let (min, max) = self.system.bind_with_state(sel, self.render_state()).min_max();
        (Vec3::new(min.x, min.y, min.z), Vec3::new(max.x, max.y, max.z))
    }

    /// Bounding box (nm) of the whole molecule at the currently displayed frame.
    pub fn current_bbox(&self) -> (Vec3, Vec3) {
        self.sel_bbox(&self.system.select_all())
    }
}

/// Outcome of a failed [`evaluate`]. molar treats a selection that matches zero
/// atoms as an error, but the GUI distinguishes it from a real (syntax) error: an
/// empty match is a non-destructive *warning* (the text stays, the field is flagged)
/// while an invalid selection keeps the prior geometry and shows the message.
#[derive(Debug)]
pub enum EvalError {
    /// Valid syntax, but the selection matched no atoms.
    Empty,
    /// Syntax (or other) error, with the message to surface.
    Invalid(String),
}

/// Parse a VMD-like selection string into a compiled `SelectionExpr` and evaluate
/// it against `system` to produce the current `Sel`. Returns both so the caller
/// can keep the compiled expression (for per-frame re-evaluation in trajectories)
/// alongside the evaluated index set. `Err(Empty)` = valid but zero atoms;
/// `Err(Invalid)` = a syntax/other error.
pub fn evaluate(system: &System, text: &str) -> Result<(SelectionExpr, Sel), EvalError> {
    let expr = SelectionExpr::new(text).map_err(|e| EvalError::Invalid(e.to_string()))?;
    match system.select(&expr) {
        Ok(sel) => Ok((expr, sel)),
        Err(e) if is_empty_selection(&e) => Err(EvalError::Empty),
        Err(e) => Err(EvalError::Invalid(e.to_string())),
    }
}

/// Whether a `SelectionError` just means "matched nothing" (vs a real error) — the
/// family of `Empty*` variants molar raises for a valid expression with no results.
fn is_empty_selection(e: &SelectionError) -> bool {
    matches!(
        e,
        SelectionError::EmptyExpr(_)
            | SelectionError::EmptySlice
            | SelectionError::EmptyRange
            | SelectionError::EmptySplit
            | SelectionError::EmptyIntersection
            | SelectionError::EmptyDifference
            | SelectionError::EmptyInvert
    )
}

#[derive(Default)]
pub struct Scene {
    pub molecules: Vec<Molecule>,
    pub selected_mol: Option<usize>,
    /// Molecules removed from the document but retained so a delete can be undone.
    pub trash: HashMap<MolId, Molecule>,
    next_id: u64,
}

impl Scene {
    /// Load a molecule into the scene, assigning it a fresh [`MolId`].
    pub fn add(&mut self, raw: RawMolecule, default_rep: RepKind) -> MolId {
        let id = MolId(self.next_id);
        self.next_id += 1;
        self.molecules.push(Molecule::new(id, raw, default_rep));
        id
    }

    /// Clamp `selected_mol`/`selected_rep` to valid ranges (after add/remove).
    pub fn clamp_selection(&mut self) {
        if self.molecules.is_empty() {
            self.selected_mol = None;
        } else {
            let m = self.selected_mol.unwrap_or(0).min(self.molecules.len() - 1);
            self.selected_mol = Some(m);
        }
        for mol in &mut self.molecules {
            if mol.reps.is_empty() {
                mol.selected_rep = None;
            } else {
                let r = mol.selected_rep.unwrap_or(0).min(mol.reps.len() - 1);
                mol.selected_rep = Some(r);
            }
        }
    }

    /// Combined bounding box over all molecules (for camera framing).
    pub fn bbox(&self) -> Option<(Vec3, Vec3)> {
        let mut iter = self.molecules.iter();
        let first = iter.next()?;
        let mut min = first.bbox_min;
        let mut max = first.bbox_max;
        for m in iter {
            min = min.min(m.bbox_min);
            max = max.max(m.bbox_max);
        }
        Some((min, max))
    }
}
