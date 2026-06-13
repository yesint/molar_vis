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

use crate::geometry::{RepKind, RepParams};
use crate::scene::{MolId, Molecule, Representation, Scene};

#[derive(Clone, PartialEq)]
struct RepState {
    kind: RepKind,
    params: RepParams,
    sel_text: String,
    visible: bool,
    dynamic: bool,
}

#[derive(Clone, PartialEq)]
struct MolState {
    id: MolId,
    visible: bool,
    reps: Vec<RepState>,
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
                    reps: m
                        .reps
                        .iter()
                        .map(|r| RepState {
                            kind: r.kind,
                            params: r.params,
                            sel_text: r.sel_text.clone(),
                            visible: r.visible,
                            dynamic: r.dynamic,
                        })
                        .collect(),
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
        mol.reps = target.iter().map(rep_from_state).collect();
        return;
    }
    for (i, s) in target.iter().enumerate() {
        let needs_rebuild = {
            let cur = &mol.reps[i];
            cur.kind != s.kind || cur.params != s.params || cur.sel_text != s.sel_text
        };
        if needs_rebuild {
            mol.reps[i] = rep_from_state(s);
        } else {
            // Cheap, no-geometry changes.
            mol.reps[i].visible = s.visible;
            mol.reps[i].dynamic = s.dynamic;
        }
    }
}

fn rep_from_state(s: &RepState) -> Representation {
    Representation::restore(s.kind, s.params, s.sel_text.clone(), s.visible, s.dynamic)
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
            if o.dynamic != n.dynamic {
                return "toggle dynamic".into();
            }
            if o.visible != n.visible {
                return "toggle representation".into();
            }
        }
    }
    "edit".into()
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
