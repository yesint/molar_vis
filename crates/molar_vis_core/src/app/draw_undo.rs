//! The drawing editor's local undo/redo and its link to the global timeline.
//!
//! Each editor gesture is a [`LocalEdit`] on the session's [`LocalHistory`]; Ctrl+Z in
//! Draw mode walks those. The local edits are **accepted** — recorded as one global
//! [`StructEdit::Journal`] step — when the editor leaves the molecule: Finish, leaving
//! Draw mode, another target molecule, New, the target leaving the scene. They are also
//! accepted before any other step is recorded on the global timeline (a document change,
//! a charge assignment, an alignment), which keeps the global timeline strictly LIFO.

use super::*;
use crate::edit_journal::{EditOp, LocalEdit, LocalHistory};
use crate::history::StructEdit;

/// Record the accepted local edits of `local` on the global timeline as one step.
pub(super) fn accept_local(history: &mut History, local: &mut LocalHistory) {
    if let Some((mol, ops, label)) = local.accept() {
        history.record_struct(mol, StructEdit::Journal(ops), label);
    }
}

impl App {
    /// Accept the drawing editor's local edits (one global step), keeping Draw mode on.
    pub(super) fn accept_draw_edit(&mut self) {
        if let Some(d) = self.draw.as_mut() {
            accept_local(&mut self.history, &mut d.local);
        }
    }

    /// Leave Draw mode, accepting its edits first.
    pub(super) fn end_draw(&mut self) {
        self.accept_draw_edit();
        self.draw = None;
    }

    /// Point the editor at molecule `target`; an edit of a different molecule is
    /// accepted first.
    pub(super) fn retarget_draw(&mut self, target: Option<MolId>) {
        let Some(d) = self.draw.as_mut() else { return };
        if d.target != target {
            accept_local(&mut self.history, &mut d.local);
            d.target = target;
        }
    }

    /// Record a document checkpoint, first accepting pending editor edits if the
    /// document changed (so the global steps stay in the order the changes happened).
    pub(super) fn record_doc_checkpoint(&mut self) {
        let current = EditState::capture(&self.scene);
        if self.history.doc_differs(&current) {
            self.accept_draw_edit();
        }
        self.history.maybe_record(current);
    }

    /// Push one editor gesture's `ops` on molecule `mol` as a local edit.
    pub(super) fn push_local_edit(&mut self, mol: MolId, label: String, ops: Vec<EditOp>) {
        if ops.is_empty() {
            return;
        }
        if let Some(texts) = crate::edit_journal::selections_after(&ops, true) {
            // The gesture's own selection changes are not a separate document step.
            self.history.adopt_selections(mol, texts);
        }
        let Some(d) = self.draw.as_mut() else {
            // Outside the editor (a headless hook) a gesture is accepted at once.
            self.history.record_struct(mol, StructEdit::Journal(ops), label);
            return;
        };
        if d.local.mol != Some(mol) {
            accept_local(&mut self.history, &mut d.local);
            d.local.mol = Some(mol);
            d.local.created = false;
        }
        d.local.undo.push(LocalEdit { label, ops });
        d.local.redo.clear();
        d.local.relax_open = false;
    }

    /// Undo (`forward == false`) or redo up to `n` local editor steps. Returns the labels.
    pub(super) fn replay_local(&mut self, n: usize, forward: bool) -> Vec<String> {
        let mut done = Vec::new();
        for _ in 0..n {
            let Some(d) = self.draw.as_mut() else { break };
            let Some(mol) = d.local.mol else { break };
            let Some(mi) = self.scene.mol_index(mol) else { break };
            let (from, to) = if forward { (&mut d.local.redo, &mut d.local.undo) } else { (&mut d.local.undo, &mut d.local.redo) };
            let Some(edit) = from.pop() else { break };
            self.scene.molecules[mi].replay(&edit.ops, forward);
            if let Some(texts) = crate::edit_journal::selections_after(&edit.ops, forward) {
                self.history.adopt_selections(mol, texts);
            }
            done.push(edit.label.clone());
            to.push(edit);
        }
        if let Some(d) = self.draw.as_mut().filter(|_| !done.is_empty()) {
            // In-flight tool state may hold atom indices of the replaced structure.
            d.local.relax_open = false;
            d.minimize_pending = false;
            d.relax = None;
            d.drag = super::draw::DrawDrag::Idle;
            d.dihedral = super::dihedral::DihedralState::default();
            self.view_dirty = true;
        }
        done
    }

    /// Run a topology-editing gesture `f` on molecule `mi` and push it as one local
    /// editor step. A shared molecule records nothing. When `f` deletes the whole
    /// molecule, the document's "delete molecule" step covers it.
    pub(super) fn record_topology<F: FnOnce(&mut Self)>(&mut self, mi: usize, label: Option<&str>, f: F) {
        let Some(mol) = self.scene.molecules.get_mut(mi) else {
            f(self);
            return;
        };
        let id = mol.id;
        mol.begin_edit();
        f(self);
        let Some(mi) = self.scene.mol_index(id) else {
            // Deleted by the gesture: drop the open journal with the molecule.
            if let Some(m) = self.scene.trash.get_mut(&id) {
                m.journal = None;
            }
            return;
        };
        let ops = self.scene.molecules[mi].end_edit();
        if ops.is_empty() {
            return;
        }
        let label = label.map_or_else(|| LocalEdit::label_for(&ops), str::to_owned);
        self.push_local_edit(id, label, ops);
        if let Some(d) = self.draw.as_mut() {
            d.local.relax_open = true;
        }
    }

    /// Push a coordinate move of `atoms` (dihedral twist, relax) as a local step, or —
    /// for the automatic relax right after a topology gesture — add it to that step.
    pub(super) fn push_local_coords(
        &mut self,
        mol: MolId,
        label: &str,
        merge_into_last: bool,
        atoms: Vec<usize>,
        before: Vec<[f32; 3]>,
        after: Vec<[f32; 3]>,
        frame: Option<usize>,
    ) {
        if atoms.is_empty() {
            return;
        }
        let op = EditOp::Coords { atoms, before, after, frame };
        if merge_into_last {
            if let Some(d) = self.draw.as_mut() {
                if d.local.relax_open && d.local.mol == Some(mol) && d.local.redo.is_empty() {
                    if let Some(last) = d.local.undo.last_mut() {
                        last.ops.push(op);
                        d.local.relax_open = false;
                        return;
                    }
                }
            }
        }
        self.push_local_edit(mol, label.into(), vec![op]);
    }

    /// Labels of the editor's local undo (`redo == false`) or redo stack, newest first.
    pub(super) fn local_labels(&self, redo: bool) -> Vec<String> {
        self.draw.as_ref().map_or_else(Vec::new, |d| {
            let stack = if redo { &d.local.redo } else { &d.local.undo };
            stack.iter().rev().map(|e| e.label.clone()).collect()
        })
    }
}
