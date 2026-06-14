//! The scene graph: a set of molecules, each with its own representations.

use std::collections::HashMap;

use glam::Vec3;
use molar::prelude::*;

use crate::color::ColorMethod;
use crate::data::RawMolecule;
use crate::geometry::{RepKind, RepParams};
use crate::material::Material;
use crate::render::RepGpu;
use crate::secstruct::SsMap;
use crate::trajectory::Trajectory;

/// Stable per-molecule identity, so undo/redo can reference molecules across
/// deletion (a deleted molecule is parked in [`Scene::trash`] by this id).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MolId(pub u64);

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
    /// Transient UI state: whether this rep's inline params panel is expanded.
    /// Not part of `EditState` (view state, not undoable).
    pub params_open: bool,
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
            visible,
            dynamic,
            ss_per_frame,
            ss_cache: None,
            params_open: false,
            sel_dirty: true,
            geom_dirty: false,
            coords_dirty: false,
            gpu: RepGpu::default(),
        }
    }
}

/// A loaded molecule. The live molar `System` is the single source of per-atom
/// data (positions, elements, radii); we additionally keep only the guessed
/// connectivity and a cached bounding box, plus the representations.
pub struct Molecule {
    pub id: MolId,
    pub name: String,
    pub system: System,
    pub bonds: Vec<[usize; 2]>,
    pub n_atoms: usize,
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
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
}

impl Molecule {
    pub fn new(id: MolId, raw: RawMolecule, default_rep: RepKind) -> Self {
        Self {
            id,
            name: raw.name,
            system: raw.system,
            bonds: raw.bonds,
            n_atoms: raw.n_atoms,
            bbox_min: raw.bbox_min,
            bbox_max: raw.bbox_max,
            visible: true,
            reps: vec![Representation::new(default_rep)],
            selected_rep: Some(0),
            reps_open: true,
            trajectory: Trajectory::default(),
            show_box: false,
            box_gpu: RepGpu::default(),
            box_dirty: false,
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
        let needs_eval = self.reps.iter().any(|r| r.dynamic);
        if needs_eval {
            if let Some(frame) = self.trajectory.frames.get(self.trajectory.current) {
                let _ = self.system.set_state(frame.clone());
            }
        }
        for rep in &mut self.reps {
            if rep.dynamic {
                rep.sel_dirty = true;
            } else if rep.ss_per_frame && crate::geometry::needs_ss(&rep.params, rep.color) {
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

/// Parse a VMD-like selection string into a compiled `SelectionExpr` and evaluate
/// it against `system` to produce the current `Sel`. Returns both so the caller
/// can keep the compiled expression (for per-frame re-evaluation in trajectories)
/// alongside the evaluated index set. Syntax errors and empty matches come back as
/// `Err`, which the UI surfaces without disturbing the existing geometry.
pub fn evaluate(system: &System, text: &str) -> Result<(SelectionExpr, Sel), String> {
    let expr = SelectionExpr::new(text).map_err(|e| e.to_string())?;
    let sel = system.select(&expr).map_err(|e| e.to_string())?;
    Ok((expr, sel))
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
