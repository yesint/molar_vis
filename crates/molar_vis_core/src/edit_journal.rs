//! Local, delta-based structure editing history for the drawing editor.
//!
//! While a molecule is open in the drawing editor, each gesture (a click, a drag, H+/H−,
//! a relax, a bond twist) becomes one [`LocalEdit`]: the list of small reversible
//! [`EditOp`]s that the gesture made. The editor's own Ctrl+Z walks these local edits.
//! When the edit is *accepted* (Finish, another target molecule, leaving Draw mode),
//! all local edits go onto the global timeline as **one**
//! [`crate::history::StructEdit::Journal`] step, so global undo reverts the accepted
//! edit as a whole and never the single drawing steps.
//!
//! No op stores a full structure. Atom removal keeps only the removed atoms, their
//! coordinates and their bonds; its inverse inserts them back at their old indices
//! (molar has no insert, so the inverse rebuilds the `System` in O(N) — but keeps no
//! copy of it). Selections, the hydrogen cache and the aromatic rings are captured once
//! per gesture: the cache is an `Arc` with copy-on-write, so an unchanged cache costs a
//! pointer copy.
//!
//! **Invariant.** The structural ops ([`EditOp::Append`], [`EditOp::Remove`], the bond
//! ops, [`EditOp::Element`], [`EditOp::Coords`]) never touch the selections, the cache
//! or the rings; the gesture-level set-ops ([`EditOp::Selections`],
//! [`EditOp::HydrogenCache`], [`EditOp::Rings`]) carry those. So a replay in either
//! direction restores each part exactly.

use std::sync::Arc;

use molar::prelude::*;

use crate::hydrogens::CachedHydrogen;
use crate::scene::{MolId, Molecule, RepId};

/// The element-defining fields that [`Molecule::set_atom_element`] changes.
#[derive(Clone, PartialEq, Debug)]
pub struct ElementData {
    pub name: String,
    pub atomic_number: u8,
    pub mass: f32,
}

/// One reversible change of a molecule's structure.
pub enum EditOp {
    /// One atom appended at the end (index = atom count before the append).
    Append { atom: Atom, pos: Pos },
    /// Atoms removed in one batch. `indices` are sorted old indices; `bonds` are the
    /// removed bonds as (position in the old bond list, bond in old indices), ascending.
    Remove {
        indices: Vec<usize>,
        atoms: Vec<Atom>,
        coords: Vec<Pos>,
        bonds: Vec<(usize, Bond)>,
    },
    /// A bond pushed at the end of the bond list.
    AddBond(Bond),
    /// The bond at list position `k` removed.
    RemoveBond { k: usize, bond: Bond },
    /// The order of the bond at list position `k` changed.
    BondOrder { k: usize, before: BondOrder, after: BondOrder },
    /// Atom `i` changed its element.
    Element { i: usize, before: ElementData, after: ElementData },
    /// Coordinates of `atoms` moved, in the store `frame` designates
    /// (see [`Molecule::coord_edit_target`]).
    Coords {
        atoms: Vec<usize>,
        before: Vec<[f32; 3]>,
        after: Vec<[f32; 3]>,
        frame: Option<usize>,
    },
    /// The selection texts of the molecule's reps, by rep id.
    Selections {
        before: Vec<(RepId, String)>,
        after: Vec<(RepId, String)>,
    },
    /// The hydrogen cache.
    HydrogenCache {
        before: Arc<Vec<CachedHydrogen>>,
        after: Arc<Vec<CachedHydrogen>>,
    },
    /// The perceived aromatic rings (ring-circle overlay).
    Rings { before: Vec<Vec<usize>>, after: Vec<Vec<usize>> },
}

impl EditOp {
    fn is_coords(&self) -> bool {
        matches!(self, EditOp::Coords { .. })
    }
}

/// The ops of one editor gesture, with a label for the editor's undo menu.
pub struct LocalEdit {
    pub label: String,
    pub ops: Vec<EditOp>,
}

impl LocalEdit {
    /// A label from the ops of a topology gesture.
    pub fn label_for(ops: &[EditOp]) -> String {
        let has = |f: fn(&EditOp) -> bool| ops.iter().any(f);
        let label = if has(|o| matches!(o, EditOp::Append { .. })) {
            "add atom"
        } else if has(|o| matches!(o, EditOp::Remove { .. })) {
            "delete atom"
        } else if has(|o| matches!(o, EditOp::Element { .. })) {
            "change element"
        } else if has(|o| matches!(o, EditOp::AddBond(_))) {
            "add bond"
        } else if has(|o| matches!(o, EditOp::RemoveBond { .. })) {
            "delete bond"
        } else if has(|o| matches!(o, EditOp::BondOrder { .. })) {
            "change bond order"
        } else {
            "edit structure"
        };
        label.into()
    }
}

/// The selection texts after replaying `ops` forward (`forward`) or backward.
pub fn selections_after(ops: &[EditOp], forward: bool) -> Option<&[(RepId, String)]> {
    fn sel(o: &EditOp, forward: bool) -> Option<&[(RepId, String)]> {
        match o {
            EditOp::Selections { before, after } => Some(if forward { after } else { before }),
            _ => None,
        }
    }
    if forward {
        ops.iter().rev().find_map(|o| sel(o, true))
    } else {
        ops.iter().find_map(|o| sel(o, false))
    }
}

/// The drawing editor's local undo/redo stacks for the molecule being edited.
#[derive(Default)]
pub struct LocalHistory {
    /// The molecule the local edits belong to.
    pub mol: Option<MolId>,
    /// The molecule was created in this editing session. Its "add molecule" document
    /// step already covers the result, so no global journal step is recorded for it.
    pub created: bool,
    pub undo: Vec<LocalEdit>,
    pub redo: Vec<LocalEdit>,
    /// The newest local edit is a topology gesture whose automatic relax has not run
    /// yet; that relax joins the same edit (one click = one editor undo step).
    pub relax_open: bool,
}

impl LocalHistory {
    /// The global step for the accepted edit: every applied local edit's ops in order,
    /// plus a label. `None` when nothing is to be recorded. Clears the stacks.
    ///
    /// For a molecule created in this session the edits need no step (its "add
    /// molecule" document step restores the result). But once such edits were accepted
    /// — because another global step had to follow them — later edits must be recorded,
    /// since that step depends on the structure in between.
    pub fn accept(&mut self) -> Option<(MolId, Vec<EditOp>, String)> {
        self.redo.clear();
        self.relax_open = false;
        if self.undo.is_empty() {
            return None;
        }
        let undo = std::mem::take(&mut self.undo);
        if std::mem::take(&mut self.created) {
            return None;
        }
        let label = match undo.as_slice() {
            [one] => one.label.clone(),
            many => format!("edit structure ({} steps)", many.len()),
        };
        let ops = undo.into_iter().flat_map(|e| e.ops).collect();
        Some((self.mol?, ops, label))
    }
}

/// The open journal of a gesture on a molecule (see [`Molecule::begin_edit`]).
pub(crate) struct Journal {
    ops: Vec<EditOp>,
    sel_before: Vec<(RepId, String)>,
    hcache_before: Arc<Vec<CachedHydrogen>>,
    rings_before: Vec<Vec<usize>>,
}

impl Molecule {
    fn selection_texts(&self) -> Vec<(RepId, String)> {
        self.reps.iter().map(|r| (r.id, r.sel_text.clone())).collect()
    }

    /// Start recording the structural mutations of one gesture. A shared molecule
    /// can't be edited, so it records nothing.
    pub(crate) fn begin_edit(&mut self) {
        if self.data.is_shared() {
            return;
        }
        self.journal = Some(Journal {
            ops: Vec::new(),
            sel_before: self.selection_texts(),
            hcache_before: self.hydrogen_cache.clone(),
            rings_before: self.aromatic_rings.clone(),
        });
    }

    /// Stop recording and return the gesture's ops (empty if nothing changed).
    pub(crate) fn end_edit(&mut self) -> Vec<EditOp> {
        let Some(j) = self.journal.take() else { return Vec::new() };
        let mut ops = j.ops;
        let sel_after = self.selection_texts();
        if sel_after != j.sel_before {
            ops.push(EditOp::Selections { before: j.sel_before, after: sel_after });
        }
        if !Arc::ptr_eq(&j.hcache_before, &self.hydrogen_cache) {
            ops.push(EditOp::HydrogenCache { before: j.hcache_before, after: self.hydrogen_cache.clone() });
        }
        if self.aromatic_rings != j.rings_before {
            ops.push(EditOp::Rings { before: j.rings_before, after: self.aromatic_rings.clone() });
        }
        ops
    }

    /// Whether a gesture is being recorded (so a mutator must log its op).
    pub(crate) fn journaling(&self) -> bool {
        self.journal.is_some()
    }

    /// Record `op` into the open journal, if any.
    pub(crate) fn log_op(&mut self, op: EditOp) {
        if let Some(j) = self.journal.as_mut() {
            j.ops.push(op);
        }
    }

    /// Remove the sorted, de-duplicated, in-range atoms `removed` from the System and
    /// re-index the bonds — the structural core of an atom removal, shared by the edit
    /// and its replay. Returns the old→new index map, or `None` if molar refused.
    pub(crate) fn remove_atoms_raw(&mut self, removed: &[usize]) -> Option<Vec<Option<usize>>> {
        let mut map = vec![None; self.n_atoms];
        let mut next = 0;
        for (i, entry) in map.iter_mut().enumerate() {
            if removed.binary_search(&i).is_err() {
                *entry = Some(next);
                next += 1;
            }
        }
        if let Err(error) = self.data.remove(removed.iter().copied()) {
            log::error!("cannot remove atoms: {}", crate::errors::error_chain(&error));
            return None;
        }
        self.bonds = self
            .bonds
            .iter()
            .filter_map(|b| Some(Bond::with_order(map[b.i1]?, map[b.i2]?, b.order)))
            .collect();
        self.n_atoms = next;
        Some(map)
    }

    /// Insert `atoms` (with `coords`) so that they land at the sorted final indices
    /// `indices`; the other atoms keep their order. molar has no insert, so this builds
    /// the System again from its atoms (O(N) time, no stored copy). Bonds are not
    /// touched. Velocities and forces, if present, get zero rows for the new atoms.
    fn insert_atoms(&mut self, indices: &[usize], atoms: &[Atom], coords: &[Pos]) {
        let Some(sys) = self.data.system() else { return };
        let old_top = sys.topology();
        let old_st = sys.state();
        let n_new = self.n_atoms + indices.len();
        // molar's removal leaves the molecule ranges as they are, so the insert does too.
        let mut top = Topology { molecules: old_top.molecules.clone(), ..Default::default() };
        let mut st = State { time: old_st.time, pbox: old_st.pbox.clone(), ..Default::default() };
        st.coords.reserve(n_new);
        let (has_vel, has_force) = (old_st.has_vel(), old_st.has_force());
        let (mut r, mut j) = (0, 0);
        for i in 0..n_new {
            if r < indices.len() && indices[r] == i {
                top.atoms.push(&atoms[r]);
                st.coords.push(coords[r]);
                if has_vel { st.velocities.push(Default::default()); }
                if has_force { st.forces.push(Default::default()); }
                r += 1;
            } else {
                top.atoms.push(&old_top.atoms.to_atom(j));
                st.coords.push(old_st.coords[j]);
                if has_vel { st.velocities.push(old_st.velocities[j]); }
                if has_force { st.forces.push(old_st.forces[j]); }
                j += 1;
            }
        }
        match System::new(top, st) {
            Ok(sys) => {
                self.data = crate::moldata::MolData::Owned(sys);
                self.n_atoms = n_new;
            }
            Err(error) => log::error!("cannot insert atoms: {}", crate::errors::error_chain(&error)),
        }
    }

    fn set_element_data(&mut self, i: usize, e: &ElementData) {
        let mut bound = self.data.select_all_bound_mut();
        if let Some(mut a) = bound.get_atom_mut(i) {
            a.set_name(&e.name);
            a.set_atomic_number(e.atomic_number);
            a.set_mass(e.mass);
        }
    }

    pub(crate) fn element_data(&self, i: usize) -> Option<ElementData> {
        let a = self.data.topology().get_atom(i)?;
        Some(ElementData {
            name: a.get_name().to_owned(),
            atomic_number: a.get_atomic_number(),
            mass: a.get_mass(),
        })
    }

    fn apply_op(&mut self, op: &EditOp, forward: bool) {
        match op {
            EditOp::Append { atom, pos } => {
                if forward {
                    if self.data.append_atom(atom, pos).is_ok() {
                        self.n_atoms += 1;
                    }
                } else if self.n_atoms > 0 {
                    self.remove_atoms_raw(&[self.n_atoms - 1]);
                }
            }
            EditOp::Remove { indices, atoms, coords, bonds } => {
                if forward {
                    self.remove_atoms_raw(indices);
                } else {
                    // Old index of each kept atom, then re-index the kept bonds back.
                    let n_old = self.n_atoms + indices.len();
                    let up: Vec<usize> =
                        (0..n_old).filter(|i| indices.binary_search(i).is_err()).collect();
                    self.insert_atoms(indices, atoms, coords);
                    for b in &mut self.bonds {
                        *b = Bond::with_order(up[b.i1], up[b.i2], b.order);
                    }
                    for (k, b) in bonds {
                        let k = (*k).min(self.bonds.len());
                        self.bonds.insert(k, *b);
                    }
                }
            }
            EditOp::AddBond(b) => {
                if forward {
                    self.bonds.push(*b);
                } else {
                    self.bonds.pop();
                }
            }
            EditOp::RemoveBond { k, bond } => {
                if forward {
                    if *k < self.bonds.len() {
                        self.bonds.remove(*k);
                    }
                } else {
                    let k = (*k).min(self.bonds.len());
                    self.bonds.insert(k, *bond);
                }
            }
            EditOp::BondOrder { k, before, after } => {
                if let Some(b) = self.bonds.get_mut(*k) {
                    b.order = if forward { *after } else { *before };
                }
            }
            EditOp::Element { i, before, after } => {
                self.set_element_data(*i, if forward { after } else { before });
            }
            EditOp::Coords { atoms, before, after, frame } => {
                self.set_coords(atoms, if forward { after } else { before }, *frame);
            }
            EditOp::Selections { before, after } => {
                let texts = if forward { after } else { before };
                for rep in &mut self.reps {
                    if let Some((_, t)) = texts.iter().find(|(id, _)| *id == rep.id) {
                        rep.sel_text = t.clone();
                    }
                }
            }
            EditOp::HydrogenCache { before, after } => {
                self.hydrogen_cache = if forward { after } else { before }.clone();
            }
            EditOp::Rings { before, after } => {
                self.aromatic_rings = if forward { after } else { before }.clone();
            }
        }
    }

    /// Replay `ops` forward (redo) or backward (undo, inverse ops in reverse order),
    /// then refresh everything derived from the structure.
    pub(crate) fn replay(&mut self, ops: &[EditOp], forward: bool) {
        if self.data.is_shared() || ops.is_empty() {
            return;
        }
        if forward {
            ops.iter().for_each(|op| self.apply_op(op, true));
        } else {
            ops.iter().rev().for_each(|op| self.apply_op(op, false));
        }
        if ops.iter().all(EditOp::is_coords) {
            // Only positions moved: the in-place coordinate update suffices.
            self.mark_coords_dirty(true);
            self.refresh_bbox();
            if !self.aromatic_rings.is_empty() {
                self.aromatic_dirty = true;
            }
            return;
        }
        self.structure_replayed();
    }

    /// Refresh the state derived from the atoms and bonds after a structural replay.
    fn structure_replayed(&mut self) {
        self.sync_bonds_to_topology();
        self.structure_version += 1;
        self.hover_grid = None;
        self.interaction_hbond_chemistry.take();
        self.interaction_rings = None;
        self.aromatic_dirty = true;
        if self.n_atoms > 0 {
            self.refresh_bbox();
        }
        for rep in &mut self.reps {
            // Cached compiled selections hold indices of the old atom set.
            rep.sel = None;
            rep.expr = None;
            rep.sel_dirty = true;
            rep.geom_dirty = true;
            rep.coords_dirty = false;
            rep.ss_cache = None;
            rep.mesh_cache = None;
        }
        // Transient highlights may reference atoms that are gone.
        self.pending = None;
        self.glow_dirty = true;
        self.hover = None;
        self.hover_dirty = true;
        self.hover_detail = None;
        self.hover_detail_dirty = true;
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.pick_dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{EditState, History, StructEdit};
    use crate::scene::Scene;
    use glam::Vec3;

    fn drawn_scene() -> Scene {
        let raw = crate::data::RawMolecule::single_atom("Drawn", Atom::new().with_name("C").guess(), Vec3::ZERO)
            .unwrap();
        let mut scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        scene
    }

    /// One editor gesture: add a carbon bonded to atom `to`.
    fn add_carbon(scene: &mut Scene, local: &mut LocalHistory, to: usize, x: f32) {
        let mol = &mut scene.molecules[0];
        mol.begin_edit();
        let i = mol.add_atom(&Atom::new().with_name("C").guess(), Vec3::new(x, 0.0, 0.0)).unwrap();
        mol.add_bond(to, i, BondOrder::Single);
        let ops = mol.end_edit();
        local.mol = Some(mol.id);
        local.undo.push(LocalEdit { label: LocalEdit::label_for(&ops), ops });
        local.redo.clear();
    }

    /// Editor steps undo locally; the accepted edit is one global step.
    #[test]
    fn local_steps_then_one_global_step() {
        let mut scene = drawn_scene();
        let id = scene.molecules[0].id;
        let mut hist = History::new(EditState::capture(&scene));
        let mut local = LocalHistory::default();
        add_carbon(&mut scene, &mut local, 0, 0.15);
        add_carbon(&mut scene, &mut local, 1, 0.30);
        add_carbon(&mut scene, &mut local, 2, 0.45);
        assert_eq!(scene.molecules[0].n_atoms, 4);

        // Editor undo of the last step only.
        let edit = local.undo.pop().unwrap();
        scene.molecules[0].replay(&edit.ops, false);
        local.redo.push(edit);
        assert_eq!(scene.molecules[0].n_atoms, 3);

        // Accept: the remaining two steps are one global step; the undone one is gone.
        let (mol, ops, label) = local.accept().unwrap();
        assert_eq!(mol, id);
        assert_eq!(label, "edit structure (2 steps)");
        assert!(local.undo.is_empty() && local.redo.is_empty());
        hist.record_struct(mol, StructEdit::Journal(ops), label);
        assert_eq!(hist.undo_len(), 1);

        hist.undo(&mut scene);
        assert_eq!(scene.molecules[0].n_atoms, 1, "global undo reverts the whole edit");
        assert!(scene.molecules[0].bonds.is_empty());
        hist.redo(&mut scene);
        assert_eq!(scene.molecules[0].n_atoms, 3);
        assert_eq!(scene.molecules[0].bonds.len(), 2);
    }

    /// A molecule created in the session needs no journal step; but after an accept
    /// forced by another global step, later edits are recorded.
    #[test]
    fn created_molecule_rules() {
        let mut scene = drawn_scene();
        let mut local = LocalHistory { mol: Some(scene.molecules[0].id), created: true, ..Default::default() };
        assert!(local.accept().is_none(), "nothing drawn yet");
        assert!(local.created, "an empty accept keeps the session state");
        add_carbon(&mut scene, &mut local, 0, 0.15);
        assert!(local.accept().is_none(), "covered by the add-molecule step");
        assert!(!local.created);
        add_carbon(&mut scene, &mut local, 1, 0.30);
        assert!(local.accept().is_some(), "later edits are recorded");
    }

    /// An automatic relax joined to its gesture is undone together with it.
    #[test]
    fn coords_in_the_same_edit_undo_together() {
        let mut scene = drawn_scene();
        let mut local = LocalHistory::default();
        add_carbon(&mut scene, &mut local, 0, 0.15);
        let before = [[0.15f32, 0.0, 0.0]];
        let after = [[0.154f32, 0.0, 0.0]];
        scene.molecules[0].set_coords(&[1], &after, None);
        local.undo.last_mut().unwrap().ops.push(EditOp::Coords {
            atoms: vec![1],
            before: before.to_vec(),
            after: after.to_vec(),
            frame: None,
        });
        let edit = local.undo.pop().unwrap();
        scene.molecules[0].replay(&edit.ops, false);
        assert_eq!(scene.molecules[0].n_atoms, 1);
        scene.molecules[0].replay(&edit.ops, true);
        let x = scene.molecules[0].data.state().coords[1].x;
        assert!((x - 0.154).abs() < 1e-6, "redo restores the relaxed position");
    }
}
