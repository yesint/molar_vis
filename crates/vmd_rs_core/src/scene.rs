//! The scene graph: a set of molecules, each with its own representations.

use std::collections::HashMap;

use glam::Vec3;
use molar::prelude::*;

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
    /// VMD-like selection string (e.g. "protein", "name CA", "all").
    pub sel_text: String,
    /// Compiled atom indices for `sel_text` (into the molecule's arrays).
    pub sel_indices: Vec<usize>,
    /// Last selection error, shown in the UI; `None` if the selection is valid.
    pub sel_error: Option<String>,
    pub visible: bool,
    /// `sel_text` changed → recompile the selection.
    pub sel_dirty: bool,
    /// Selection/params/style changed → rebuild + reupload geometry.
    pub geom_dirty: bool,
    pub gpu: RepGpu,
}

impl Representation {
    pub fn new(kind: RepKind) -> Self {
        Self {
            kind,
            params: RepParams::for_kind(kind),
            sel_text: "all".to_string(),
            sel_indices: Vec::new(),
            sel_error: None,
            visible: true,
            sel_dirty: true, // compile + build on the first frame
            geom_dirty: false,
            gpu: RepGpu::default(),
        }
    }

    /// Row label: "<selection>/<style>", e.g. "name CA/VDW".
    pub fn summary(&self) -> String {
        format!("{}/{}", self.sel_text, self.kind.label())
    }

    /// A copy with the same style/selection but fresh (unbuilt) GPU state, so it
    /// recompiles and uploads its own geometry on the next frame.
    pub fn duplicate(&self) -> Self {
        Self::restore(self.kind, self.params, self.sel_text.clone(), self.visible)
    }

    /// Reconstruct a representation from saved editable fields (used by undo/redo).
    /// Starts dirty so its selection recompiles and geometry rebuilds next frame.
    pub fn restore(kind: RepKind, params: RepParams, sel_text: String, visible: bool) -> Self {
        Self {
            kind,
            params,
            sel_text,
            sel_indices: Vec::new(),
            sel_error: None,
            visible,
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

/// Compile a VMD-like selection string against a molar `System` into atom
/// indices. Empty/invalid selections come back as `Err` (molar treats an empty
/// match as an error), which the UI surfaces without disturbing the geometry.
pub fn compile_selection(system: &System, text: &str) -> Result<Vec<usize>, String> {
    match system.select(text) {
        Ok(sel) => Ok(sel.get_index_slice().to_vec()),
        Err(e) => Err(e.to_string()),
    }
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
