//! Undo/redo via lightweight snapshots of the editable "document".
//!
//! The document is just each molecule's representations (style, params, selection
//! text, visibility) plus per-molecule visibility — tiny and cheap to clone and
//! compare every frame. The heavy per-molecule data (atom arrays, molar `System`,
//! GPU buffers) is immutable and lives in the `Scene`; it is never snapshotted.
//! Camera/projection and the selection *highlight* are view state and excluded.
//!
//! Capture is automatic and coalesced: the app records a checkpoint only when the
//! interaction has *settled* (no active pointer drag, no focused text field), so a
//! whole slider drag becomes one undo step, not sixty.

use std::collections::HashMap;

use crate::geometry::{RepKind, RepParams};
use crate::scene::{MolId, Molecule, Representation, Scene};

#[derive(Clone, PartialEq)]
struct RepState {
    kind: RepKind,
    params: RepParams,
    sel_text: String,
    visible: bool,
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
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    /// Reconcile the live scene to this snapshot, reusing molecules (by id, from
    /// the scene or the trash) and rebuilding only the reps that changed.
    pub fn apply(&self, scene: &mut Scene) {
        let mut pool: HashMap<MolId, Molecule> =
            scene.molecules.drain(..).map(|m| (m.id, m)).collect();

        let mut new_mols = Vec::with_capacity(self.mols.len());
        for ms in &self.mols {
            let Some(mut mol) = pool.remove(&ms.id).or_else(|| scene.trash.remove(&ms.id)) else {
                continue; // unknown id (shouldn't happen) — skip
            };
            mol.visible = ms.visible;
            reconcile_reps(&mut mol, &ms.reps);
            new_mols.push(mol);
        }
        // Molecules no longer in the document are parked in the trash so a
        // subsequent undo/redo can bring them back without reloading.
        for (id, mol) in pool {
            scene.trash.insert(id, mol);
        }
        scene.molecules = new_mols;
        scene.clamp_selection();
    }
}

fn reconcile_reps(mol: &mut Molecule, target: &[RepState]) {
    if mol.reps.len() != target.len() {
        mol.reps = target
            .iter()
            .map(|s| Representation::restore(s.kind, s.params, s.sel_text.clone(), s.visible))
            .collect();
        return;
    }
    for (i, s) in target.iter().enumerate() {
        let needs_rebuild = {
            let cur = &mol.reps[i];
            cur.kind != s.kind || cur.params != s.params || cur.sel_text != s.sel_text
        };
        if needs_rebuild {
            mol.reps[i] = Representation::restore(s.kind, s.params, s.sel_text.clone(), s.visible);
        } else if mol.reps[i].visible != s.visible {
            // Visibility-only change needs no geometry rebuild.
            mol.reps[i].visible = s.visible;
        }
    }
}

pub struct History {
    undo: Vec<EditState>,
    redo: Vec<EditState>,
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
            self.undo.push(self.committed.clone());
            if self.undo.len() > self.cap {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.committed = current;
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Step back; returns the state to apply, or `None` if nothing to undo.
    pub fn undo(&mut self) -> Option<EditState> {
        let prev = self.undo.pop()?;
        self.redo.push(self.committed.clone());
        self.committed = prev;
        Some(self.committed.clone())
    }

    /// Step forward; returns the state to apply, or `None` if nothing to redo.
    pub fn redo(&mut self) -> Option<EditState> {
        let next = self.redo.pop()?;
        self.undo.push(self.committed.clone());
        self.committed = next;
        Some(self.committed.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // Core logic is GPU-free (RepGpu::default(), no wgpu), so we can exercise it
    // headlessly against a real molecule.
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
        assert_eq!(scene.molecules[0].reps.len(), 1);

        scene.molecules[0].reps.push(Representation::new(RepKind::Vdw));
        hist.maybe_record(EditState::capture(&scene));
        assert!(hist.can_undo());

        hist.undo().unwrap().apply(&mut scene);
        assert_eq!(scene.molecules[0].reps.len(), 1);

        hist.redo().unwrap().apply(&mut scene);
        assert_eq!(scene.molecules[0].reps.len(), 2);
    }

    #[test]
    fn undo_restores_deleted_molecule() {
        let mut scene = load_scene();
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb");
        scene.add(crate::data::load(Path::new(path)).unwrap(), RepKind::Lines);
        let mut hist = History::new(EditState::capture(&scene));
        assert_eq!(scene.molecules.len(), 2);

        let m = scene.molecules.remove(0);
        scene.trash.insert(m.id, m);
        scene.clamp_selection();
        hist.maybe_record(EditState::capture(&scene));
        assert_eq!(scene.molecules.len(), 1);

        hist.undo().unwrap().apply(&mut scene);
        assert_eq!(scene.molecules.len(), 2, "deleted molecule restored from trash");
        assert!(scene.trash.is_empty());
    }

    #[test]
    fn no_record_without_change() {
        let scene = load_scene();
        let mut hist = History::new(EditState::capture(&scene));
        hist.maybe_record(EditState::capture(&scene));
        assert!(!hist.can_undo());
    }

    #[test]
    fn selection_text_edit_is_undoable() {
        let mut scene = load_scene();
        let mut hist = History::new(EditState::capture(&scene));
        scene.molecules[0].reps[0].sel_text = "name CA".to_string();
        hist.maybe_record(EditState::capture(&scene));

        hist.undo().unwrap().apply(&mut scene);
        assert_eq!(scene.molecules[0].reps[0].sel_text, "all");
    }
}
