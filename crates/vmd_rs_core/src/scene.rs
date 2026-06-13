//! The scene graph: a set of molecules, each with its own representations.

use glam::Vec3;
use molar::prelude::*;

use crate::data::RawMolecule;
use crate::geometry::{RepKind, RepParams};
use crate::render::RepGpu;

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
        Self {
            kind: self.kind,
            params: self.params,
            sel_text: self.sel_text.clone(),
            sel_indices: Vec::new(),
            sel_error: None,
            visible: self.visible,
            sel_dirty: true,
            geom_dirty: false,
            gpu: RepGpu::default(),
        }
    }
}

/// A loaded molecule: per-atom arrays, connectivity, the live molar `System`
/// (for selection evaluation), and its representations.
pub struct Molecule {
    pub name: String,
    pub system: System,
    pub positions: Vec<[f32; 3]>,
    pub vdw: Vec<f32>,
    pub colors: Vec<u32>,
    pub bonds: Vec<[usize; 2]>,
    pub n_atoms: usize,
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
    pub visible: bool,
    pub reps: Vec<Representation>,
    pub selected_rep: Option<usize>,
}

impl Molecule {
    pub fn new(raw: RawMolecule, default_rep: RepKind) -> Self {
        Self {
            name: raw.name,
            system: raw.system,
            positions: raw.positions,
            vdw: raw.vdw,
            colors: raw.colors,
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
}

impl Scene {
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
