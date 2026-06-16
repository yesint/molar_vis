//! Undo/redo via lightweight snapshots of the editable "document".
//!
//! The document is just each molecule's representations (style, params, selection
//! text, visibility, dynamic flag) plus per-molecule visibility — tiny and cheap
//! to clone and compare every frame. The heavy per-molecule data (the molar
//! `System`, bonds, GPU buffers) is immutable and lives in the `Scene`; it is
//! never snapshotted. Camera/projection and the selection *highlight* are view
//! state and excluded.
//!
//! Capture is automatic and coalesced: the app records a checkpoint only when the
//! interaction has *settled* (no active pointer drag, no focused text field), so a
//! whole slider drag becomes one undo step. Each step is tagged with a descriptive
//! label (derived by diffing the two states) shown in the undo/redo dropdowns.

use molar::prelude::SsAlgorithm;
use serde::{Deserialize, Serialize};

use crate::color::ColorMethod;
use crate::geometry::{RepKind, RepParams};
use crate::material::Material;
use crate::scene::{MolId, Molecule, PeriodicParams, Representation, Scene};

/// The editable state of a single representation — the canonical "document" unit.
///
/// **This is the one place to add a persisted, undoable representation field.**
/// Both undo/redo (via [`EditState`]) and save/load (via [`crate::session`]) read
/// and write reps through this struct, so a field added here is automatically
/// snapshotted *and* serialized — no other site to remember. New fields should
/// carry `#[serde(default)]` so older saved sessions still load.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct RepState {
    pub kind: RepKind,
    pub params: RepParams,
    pub color: ColorMethod,
    #[serde(with = "SsAlgorithmDef")]
    pub ss_algo: SsAlgorithm,
    pub sel_text: String,
    pub visible: bool,
    pub dynamic: bool,
    pub ss_per_frame: bool,
    pub material: Material,
    pub periodic: PeriodicParams,
    pub smooth_window: u32,
}

/// serde mirror for molar's foreign [`SsAlgorithm`] (which derives no serde). Used
/// via `#[serde(with = ...)]`; the variants must stay in sync with molar's enum.
#[derive(Serialize, Deserialize)]
#[serde(remote = "SsAlgorithm")]
enum SsAlgorithmDef {
    Dssp,
    DsspGmx,
    Dss,
}

impl RepState {
    /// Snapshot a live representation's editable fields.
    pub fn capture(r: &Representation) -> Self {
        Self {
            kind: r.kind,
            params: r.params,
            color: r.color,
            ss_algo: r.ss_algo,
            sel_text: r.sel_text.clone(),
            visible: r.visible,
            dynamic: r.dynamic,
            ss_per_frame: r.ss_per_frame,
            material: r.material,
            periodic: r.periodic,
            smooth_window: r.smooth_window,
        }
    }

    /// Build a fresh (unbuilt, dirty) representation from this snapshot.
    pub fn to_representation(&self) -> Representation {
        Representation::restore(
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
}

/// The editable state of a molecule (its representations + visibility). The
/// molecule's heavy data (the molar `System`, source, trajectory) is referenced
/// by [`MolId`] for undo/redo and by source for sessions — never snapshotted here.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct MolState {
    /// Runtime identity; only meaningful within a live session (undo/redo). Not
    /// persisted — a loaded session reassigns ids when it reloads molecules.
    #[serde(skip)]
    pub id: MolId,
    pub visible: bool,
    pub reps: Vec<RepState>,
}

/// A snapshot of the editable document. `PartialEq` decides whether anything
/// undoable changed.
#[derive(Clone, PartialEq, Default)]
pub struct EditState {
    mols: Vec<MolState>,
}

impl EditState {
    pub fn capture(scene: &Scene) -> Self {
        EditState {
            mols: scene
                .molecules
                .iter()
                .map(|m| MolState {
                    id: m.id,
                    visible: m.visible,
                    reps: m.reps.iter().map(RepState::capture).collect(),
                })
                .collect(),
        }
    }

    /// Reconcile the live scene to this snapshot, reusing molecules (by id, from
    /// the scene or the trash) and rebuilding only the reps that changed.
    pub fn apply(&self, scene: &mut Scene) {
        let mut pool: std::collections::HashMap<MolId, Molecule> =
            scene.molecules.drain(..).map(|m| (m.id, m)).collect();

        let mut new_mols = Vec::with_capacity(self.mols.len());
        for ms in &self.mols {
            let Some(mut mol) = pool.remove(&ms.id).or_else(|| scene.trash.remove(&ms.id)) else {
                continue;
            };
            mol.visible = ms.visible;
            reconcile_reps(&mut mol, &ms.reps);
            new_mols.push(mol);
        }
        for (id, mol) in pool {
            scene.trash.insert(id, mol);
        }
        scene.molecules = new_mols;
        scene.clamp_selection();
    }
}

fn reconcile_reps(mol: &mut Molecule, target: &[RepState]) {
    if mol.reps.len() != target.len() {
        mol.reps = target.iter().map(RepState::to_representation).collect();
        return;
    }
    for (i, s) in target.iter().enumerate() {
        let needs_rebuild = {
            let cur = &mol.reps[i];
            cur.kind != s.kind
                || cur.params != s.params
                || cur.color != s.color
                || cur.ss_algo != s.ss_algo
                || cur.sel_text != s.sel_text
                || cur.material != s.material
        };
        if needs_rebuild {
            mol.reps[i] = s.to_representation();
        } else {
            // Cheap, no-geometry changes (periodic display is render-only too).
            mol.reps[i].visible = s.visible;
            mol.reps[i].dynamic = s.dynamic;
            mol.reps[i].ss_per_frame = s.ss_per_frame;
            mol.reps[i].periodic = s.periodic;
            // Smoothing changes the rendered coords → mark for an incremental rebuild.
            if mol.reps[i].smooth_window != s.smooth_window {
                mol.reps[i].smooth_window = s.smooth_window;
                mol.reps[i].coords_dirty = true;
            }
        }
    }
}

/// A short human-readable label for the change from `old` to `new`, shown in the
/// undo/redo dropdowns.
fn describe_change(old: &EditState, new: &EditState) -> String {
    use std::cmp::Ordering::*;
    match new.mols.len().cmp(&old.mols.len()) {
        Less => return "delete molecule".into(),
        Greater => return "add molecule".into(),
        Equal => {}
    }
    for nm in &new.mols {
        let Some(om) = old.mols.iter().find(|m| m.id == nm.id) else {
            return "add molecule".into();
        };
        if om.visible != nm.visible {
            return "toggle molecule".into();
        }
        match nm.reps.len().cmp(&om.reps.len()) {
            Greater => return "add representation".into(),
            Less => return "delete representation".into(),
            Equal => {}
        }
        if om.reps != nm.reps && is_permutation(&om.reps, &nm.reps) {
            return "reorder representations".into();
        }
        for (o, n) in om.reps.iter().zip(nm.reps.iter()) {
            if o.sel_text != n.sel_text {
                return "edit selection".into();
            }
            if o.kind != n.kind {
                return "change style".into();
            }
            if o.params != n.params {
                return "change parameters".into();
            }
            if o.color != n.color {
                return "change coloring".into();
            }
            if o.ss_algo != n.ss_algo {
                return "change SS algorithm".into();
            }
            if o.dynamic != n.dynamic {
                return "toggle dynamic".into();
            }
            if o.visible != n.visible {
                return "toggle representation".into();
            }
            if o.periodic != n.periodic {
                return "change periodicity".into();
            }
            if o.smooth_window != n.smooth_window {
                return "change smoothing".into();
            }
        }
    }
    "edit".into()
}

/// Whether `b` is a reordering of `a` (same multiset of rep states).
fn is_permutation(a: &[RepState], b: &[RepState]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut remaining: Vec<&RepState> = b.iter().collect();
    for x in a {
        match remaining.iter().position(|y| *y == x) {
            Some(p) => {
                remaining.swap_remove(p);
            }
            None => return false,
        }
    }
    remaining.is_empty()
}

struct Entry {
    state: EditState,
    /// Label of the action that moved *away* from `state`.
    label: String,
}

pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    committed: EditState,
    cap: usize,
}

impl History {
    pub fn new(initial: EditState) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            committed: initial,
            cap: 200,
        }
    }

    /// Record a checkpoint if `current` differs from the last committed state.
    /// Call only when the interaction has settled (coalesces drags/typing).
    pub fn maybe_record(&mut self, current: EditState) {
        if current != self.committed {
            let label = describe_change(&self.committed, &current);
            let prev = std::mem::replace(&mut self.committed, current);
            self.undo.push(Entry { state: prev, label });
            if self.undo.len() > self.cap {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
    /// Label of the `d`-th undo action from the top (0 = most recent).
    pub fn undo_label(&self, d: usize) -> &str {
        &self.undo[self.undo.len() - 1 - d].label
    }
    /// Label of the `d`-th redo action from the top (0 = next to redo).
    pub fn redo_label(&self, d: usize) -> &str {
        &self.redo[self.redo.len() - 1 - d].label
    }

    /// Undo `n` steps cumulatively; returns the state to apply (or `None`).
    pub fn undo_n(&mut self, n: usize) -> Option<EditState> {
        let n = n.min(self.undo.len());
        if n == 0 {
            return None;
        }
        for _ in 0..n {
            let entry = self.undo.pop().unwrap();
            let cur = std::mem::replace(&mut self.committed, entry.state);
            self.redo.push(Entry { state: cur, label: entry.label });
        }
        Some(self.committed.clone())
    }

    /// Redo `n` steps cumulatively; returns the state to apply (or `None`).
    pub fn redo_n(&mut self, n: usize) -> Option<EditState> {
        let n = n.min(self.redo.len());
        if n == 0 {
            return None;
        }
        for _ in 0..n {
            let entry = self.redo.pop().unwrap();
            let cur = std::mem::replace(&mut self.committed, entry.state);
            self.undo.push(Entry { state: cur, label: entry.label });
        }
        Some(self.committed.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn load_scene() -> Scene {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb");
        let raw = crate::data::load(Path::new(path)).expect("load 2lao.pdb");
        let mut scene = Scene::default();
        scene.add(raw, RepKind::Lines);
        scene
    }

    #[test]
    fn undo_redo_add_rep() {
        let mut scene = load_scene();
        let mut hist = History::new(EditState::capture(&scene));
        scene.molecules[0].reps.push(Representation::new(RepKind::Vdw));
        hist.maybe_record(EditState::capture(&scene));
        assert!(hist.can_undo());
        assert_eq!(hist.undo_label(0), "add representation");

        hist.undo_n(1).unwrap().apply(&mut scene);
        assert_eq!(scene.molecules[0].reps.len(), 1);
        hist.redo_n(1).unwrap().apply(&mut scene);
        assert_eq!(scene.molecules[0].reps.len(), 2);
    }

    #[test]
    fn cumulative_undo() {
        let mut scene = load_scene();
        let mut hist = History::new(EditState::capture(&scene));
        // Three separate edits.
        scene.molecules[0].reps[0].sel_text = "protein".into();
        hist.maybe_record(EditState::capture(&scene));
        scene.molecules[0].reps.push(Representation::new(RepKind::Vdw));
        hist.maybe_record(EditState::capture(&scene));
        scene.molecules[0].reps[0].visible = false;
        hist.maybe_record(EditState::capture(&scene));
        assert_eq!(hist.undo_len(), 3);
        assert_eq!(hist.undo_label(0), "toggle representation");

        // Undo all three at once → back to the original single "all" rep, visible.
        hist.undo_n(3).unwrap().apply(&mut scene);
        assert_eq!(scene.molecules[0].reps.len(), 1);
        assert_eq!(scene.molecules[0].reps[0].sel_text, "all");
        assert!(scene.molecules[0].reps[0].visible);
    }

    #[test]
    fn undo_restores_deleted_molecule() {
        let mut scene = load_scene();
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb");
        scene.add(crate::data::load(Path::new(path)).unwrap(), RepKind::Lines);
        let mut hist = History::new(EditState::capture(&scene));

        let m = scene.molecules.remove(0);
        scene.trash.insert(m.id, m);
        scene.clamp_selection();
        hist.maybe_record(EditState::capture(&scene));
        assert_eq!(hist.undo_label(0), "delete molecule");

        hist.undo_n(1).unwrap().apply(&mut scene);
        assert_eq!(scene.molecules.len(), 2);
        assert!(scene.trash.is_empty());
    }
}
