//! The command vocabulary a script line produces. The Rhai handle methods (in
//! [`super::evaluate_script`] — `mol(i)` → [`MolHandle`](super::MolHandle), `.rep(j)`
//! → [`RepHandle`](super::RepHandle)) **push** these; [`super::apply_scene_command`]
//! applies them against the live scene.
//!
//! Targets are explicit (the handle carries the molecule index + a [`RepRef`]).
//! `RepRef::Last` lets a freshly `add_rep`'d representation be chained
//! (`mol(0).add_rep("cartoon").set_color("ss")`) — it resolves to the molecule's
//! last rep at apply time. Enum-valued arguments (color method, style kind,
//! material) ride as **raw strings** and are parsed in `apply_scene_command`, so a
//! bad value surfaces exactly one clean console error.

use std::path::PathBuf;

use crate::color::{ColorMethod, DEFAULT_SOLID};
use crate::material::Material;

/// Which representation of a molecule a command targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepRef {
    /// An existing representation by index.
    Index(usize),
    /// The molecule's most recently added representation (resolved at apply time) —
    /// the handle `add_rep` returns, so further `.set_*` calls chain onto it.
    Last,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Set a representation's selection text (recompiled via the normal `sel_dirty` path).
    Select { mol: usize, rep: RepRef, text: String },
    /// Set a representation's color method (e.g. "element", "chain", "resid").
    Color { mol: usize, rep: RepRef, method: String },
    /// Set a representation's draw style (e.g. "vdw", "licorice", "cartoon").
    Style { mol: usize, rep: RepRef, kind: String },
    /// Set a representation's material (e.g. "Transparent", "Glossy").
    Material { mol: usize, rep: RepRef, name: String },
    /// Append a new default representation to a molecule (becomes its `RepRef::Last`).
    AddRep { mol: usize },
    /// Remove a representation by index.
    DeleteRep { mol: usize, rep: usize },
    /// Show or hide a whole molecule.
    ShowMol { mol: usize, visible: bool },
    /// Jump the molecule's trajectory to a frame.
    Frame { mol: usize, index: usize },
    /// Start/stop trajectory playback.
    Play { mol: usize, on: bool },
    /// Zoom the camera to fit a selection of a molecule.
    Focus { mol: usize, text: String },
    /// Load a structure file as a new molecule (native only).
    Load(PathBuf),
}

/// Parse a color-scheme name (mirrors the `MOLAR_VIS_DEBUG_COLOR` hook).
pub fn parse_color(s: &str) -> Option<ColorMethod> {
    match s.trim().to_ascii_lowercase().as_str() {
        "element" => Some(ColorMethod::Element),
        "chain" => Some(ColorMethod::Chain),
        "resid" => Some(ColorMethod::ResId),
        "resname" => Some(ColorMethod::ResName),
        "index" => Some(ColorMethod::Index),
        "beta" | "bfactor" | "b-factor" => Some(ColorMethod::Beta),
        "secstruct" | "structure" | "ss" => Some(ColorMethod::SecStruct),
        "solid" => Some(ColorMethod::Solid(DEFAULT_SOLID)),
        _ => None,
    }
}

/// Parse a material name by its display label (case-insensitive; e.g. "Transparent").
pub fn parse_material(s: &str) -> Option<Material> {
    let s = s.trim();
    Material::ALL.into_iter().find(|m| m.label().eq_ignore_ascii_case(s))
}
