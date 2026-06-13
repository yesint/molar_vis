//! The scene graph: a set of molecules, each with its own representations.

use std::collections::HashMap;

use glam::Vec3;
use molar::prelude::*;

use crate::color::ColorMethod;
use crate::data::RawMolecule;
use crate::geometry::{RepKind, RepParams};
use crate::render::RepGpu;

/// Stable per-molecule identity, so undo/redo can reference molecules across
/// deletion (a deleted molecule is parked in [`Scene::trash`] by this id).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MolId(pub u64);

/// One representation of a molecule: a selection rendered in a given style.
pub struct Representation {
    pub kind: RepKind,
    pub params: RepParams,
    pub color: ColorMethod,
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
    /// `within …`; honored once trajectory playback lands.
    pub dynamic: bool,
    /// Transient UI state: whether this rep's inline params panel is expanded.
    /// Not part of `EditState` (view state, not undoable).
    pub params_open: bool,
    /// `sel_text` changed → recompile the selection.
    pub sel_dirty: bool,
    /// Selection/params/style changed → rebuild + reupload geometry.
    pub geom_dirty: bool,
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
        )
    }

    /// Reconstruct a representation from saved editable fields (used by undo/redo).
    /// Starts dirty so its selection recompiles and geometry rebuilds next frame.
    pub fn restore(
        kind: RepKind,
        params: RepParams,
        color: ColorMethod,
        ss_algo: SsAlgorithm,
        sel_text: String,
        visible: bool,
        dynamic: bool,
    ) -> Self {
        Self {
            kind,
            params,
            color,
            ss_algo,
            sel_text,
            expr: None,
            sel: None,
            sel_error: None,
            visible,
            dynamic,
            params_open: false,
            sel_dirty: true,
            geom_dirty: false,
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
        }
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
