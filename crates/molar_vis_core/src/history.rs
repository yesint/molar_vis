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

use molar::prelude::*;
use serde::{Deserialize, Serialize};

use crate::color::ColorMethod;
use crate::geometry::{RepKind, RepParams};
use crate::material::Material;
use crate::scene::{
    GroupId, MolGroup, MolId, Molecule, MoleculeSource, PeriodicParams, Representation, Scene,
};

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
    /// Interactions rep only: the partner rep, keyed by the partner molecule's
    /// [`MoleculeSource`] + its rep index. Serializable + stable across reload, so it
    /// round-trips through both undo/redo and sessions. `#[serde(default)]` keeps older
    /// files loading.
    #[serde(default)]
    pub partner: Option<(MoleculeSource, usize)>,
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
            partner: r.partner.clone(),
        }
    }

    /// Build a fresh (unbuilt, dirty) representation from this snapshot.
    pub fn to_representation(&self) -> Representation {
        let mut rep = Representation::restore(
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
        );
        rep.partner = self.partner.clone();
        rep
    }
}

/// The minimal per-atom fields needed to reconstruct a drawn molecule's `Atom`.
/// (Mass/vdW are re-derived from the name by molar's `guess`; `atomic_number` is
/// then pinned exactly so an unusual name can't mis-elementize the atom.)
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AtomLite {
    pub name: String,
    pub atomic_number: u8,
    pub resname: String,
    pub resid: i32,
}

/// A full structure snapshot of an **editable** (drawn) molecule — atoms, their
/// coordinates, and the bond graph with orders. Captured per checkpoint only for
/// molecules flagged `editable`; loaded molecules carry `None` (referenced by
/// source / id instead), so this adds zero cost to the normal load path. Small by
/// construction (sketches are tens of atoms), so cloning it each frame is cheap.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct StructureSnapshot {
    pub atoms: Vec<AtomLite>,
    pub coords: Vec<[f32; 3]>,
    /// The bond graph with orders (molar's [`Bond`] derives serde directly).
    pub bonds: Vec<Bond>,
}

impl StructureSnapshot {
    /// Snapshot a molecule's live structure (topology atoms + system coords + the
    /// editor's bond graph).
    fn capture(mol: &Molecule) -> Self {
        let topo = mol.data.topology();
        let atoms = (0..mol.n_atoms)
            .filter_map(|i| topo.get_atom(i))
            .map(|a| AtomLite {
                name: a.name.as_str().to_string(),
                atomic_number: a.atomic_number,
                resname: a.resname.as_str().to_string(),
                resid: a.resid,
            })
            .collect();
        let coords = mol
            .data
            .state()
            .coords
            .iter()
            .map(|p| [p.x, p.y, p.z])
            .collect();
        StructureSnapshot {
            atoms,
            coords,
            bonds: mol.bonds.clone(),
        }
    }
}

/// The editable state of a molecule (its representations + visibility, plus — for a
/// drawn molecule — its full structure). A *loaded* molecule's heavy data (the molar
/// `System`, source, trajectory) is referenced by [`MolId`] for undo/redo and by
/// source for sessions, never snapshotted; a *drawn* molecule has no source to
/// reload from, so its structure rides in `structure`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct MolState {
    /// Runtime identity; only meaningful within a live session (undo/redo). Not
    /// persisted — a loaded session reassigns ids when it reloads molecules.
    #[serde(skip)]
    pub id: MolId,
    pub visible: bool,
    pub reps: Vec<RepState>,
    /// Full structure snapshot for an editable (drawn) molecule; `None` for loaded
    /// molecules. Drives undo/redo of atom/bond edits.
    #[serde(default)]
    pub structure: Option<StructureSnapshot>,
}

/// The undoable state of a [`MolGroup`]: its membership, group-level visibility, and
/// the **shared** representations (captured from the shown member's prefix). The
/// shown-member index is *view state* (like a trajectory frame) and deliberately
/// **not** captured, so cycling members never lands on the undo stack. Undo-only
/// (no serde — sessions use [`crate::session::GroupSession`]).
#[derive(Clone, PartialEq)]
pub struct GroupState {
    pub id: GroupId,
    pub members: Vec<MolId>,
    pub visible: bool,
    pub shared_reps: Vec<RepState>,
}

/// A snapshot of the editable document. `PartialEq` decides whether anything
/// undoable changed.
#[derive(Clone, PartialEq, Default)]
pub struct EditState {
    mols: Vec<MolState>,
    groups: Vec<GroupState>,
}

impl EditState {
    pub fn capture(scene: &Scene) -> Self {
        let mols = scene
            .molecules
            .iter()
            .map(|m| {
                // Group members' shared reps belong to the group (captured below); a
                // member's `reps` prefix is `n_shared` shared mirrors — keep only its
                // own reps here. A grouped member's visibility is fully derived from
                // the group (shown member ∧ group eye), so capturing the live flag —
                // which flips on a member switch — would make *cycling* undoable.
                // Pin it constant so only the group eye (in `GroupState`) is undoable.
                let ns = m.n_shared.min(m.reps.len());
                MolState {
                    id: m.id,
                    visible: if m.group.is_some() { true } else { m.visible },
                    reps: m.reps[ns..].iter().map(RepState::capture).collect(),
                    // Only drawn molecules carry a structure snapshot.
                    structure: m.editable.then(|| StructureSnapshot::capture(m)),
                }
            })
            .collect();
        let groups = scene
            .groups
            .iter()
            .enumerate()
            .map(|(gi, g)| GroupState {
                id: g.id,
                members: g.members.clone(),
                visible: g.visible,
                shared_reps: scene.group_shared_reps(gi),
            })
            .collect();
        EditState { mols, groups }
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
            // Strip any group-shared prefix and reconcile this member's OWN reps only
            // (`ms.reps`); the shared reps are re-materialized from the group state
            // below. Clear the group tag — it's re-set when groups are reconciled.
            let ns = mol.n_shared.min(mol.reps.len());
            if ns > 0 {
                mol.reps.drain(0..ns);
            }
            mol.n_shared = 0;
            mol.group = None;
            // Restore the drawn structure first (rebuilds the System, resets atom
            // count), then reconcile reps over it.
            mol.editable = ms.structure.is_some();
            if let Some(snap) = &ms.structure {
                reconcile_structure(&mut mol, snap);
            }
            reconcile_reps(&mut mol, &ms.reps);
            new_mols.push(mol);
        }
        for (id, mol) in pool {
            scene.trash.insert(id, mol);
        }
        scene.molecules = new_mols;

        // Reconcile groups, mirroring the molecule reconciliation (a group trash keeps
        // removed groups for redo; the shown-member index / expand state ride along on
        // the recovered group since they aren't part of the snapshot).
        let mut gpool: std::collections::HashMap<GroupId, MolGroup> =
            scene.groups.drain(..).map(|g| (g.id, g)).collect();
        let mut new_groups = Vec::with_capacity(self.groups.len());
        for gs in &self.groups {
            let mut g = gpool
                .remove(&gs.id)
                .or_else(|| scene.group_trash.remove(&gs.id))
                .unwrap_or_else(|| MolGroup {
                    id: gs.id,
                    name: String::new(),
                    source: MoleculeSource::default(),
                    members: Vec::new(),
                    current: 0,
                    visible: true,
                    expanded: false,
                });
            g.members = gs.members.clone();
            g.visible = gs.visible;
            if g.current >= g.members.len() {
                g.current = g.members.len().saturating_sub(1);
            }
            new_groups.push(g);
        }
        for (id, g) in gpool {
            scene.group_trash.insert(id, g);
        }
        scene.groups = new_groups;

        // Re-tag members and materialize each group's shared reps onto its shown member.
        for (gi, gs) in self.groups.iter().enumerate() {
            let members = scene.groups[gi].members.clone();
            for &id in &members {
                if let Some(mi) = scene.mol_index(id) {
                    scene.molecules[mi].group = Some(gs.id);
                }
            }
            let cur = scene.groups[gi].current;
            if let Some(&cur_id) = members.get(cur) {
                if let Some(mi) = scene.mol_index(cur_id) {
                    let live: Vec<Representation> =
                        gs.shared_reps.iter().map(|s| s.to_representation()).collect();
                    let n = live.len();
                    let mol = &mut scene.molecules[mi];
                    mol.reps.splice(0..0, live);
                    mol.n_shared = n;
                }
            }
            scene.apply_group_visibility(gi);
        }

        scene.clamp_selection();
    }
}

/// Rebuild a molecule's molar `System` + bond graph from a [`StructureSnapshot`]
/// (undo/redo of a drawn molecule). Editable molecules are tiny, so an
/// unconditional rebuild is fine. Marks the reps for a geometry rebuild.
fn reconcile_structure(mol: &mut Molecule, snap: &StructureSnapshot) {
    let mut top = Topology::default();
    for lite in &snap.atoms {
        let mut a = Atom::new()
            .with_name(&lite.name)
            .with_resname(&lite.resname)
            .with_resid(lite.resid)
            .guess();
        a.atomic_number = lite.atomic_number; // pin the element exactly
        top.atoms.push(a);
    }
    top.assign_resindex();
    let st = State {
        coords: snap.coords.iter().map(|c| Pos::new(c[0], c[1], c[2])).collect(),
        ..Default::default()
    };
    if let Ok(sys) = System::new(top, st) {
        mol.data = crate::moldata::MolData::Owned(sys);
        mol.bonds = snap.bonds.clone();
        mol.n_atoms = snap.atoms.len();
        mol.hover_grid = None;
        mol.refresh_bbox();
        for rep in &mut mol.reps {
            // The atom set changed, so any cached compiled selection holds indices
            // from the old (possibly larger) system — re-evaluate it against the new
            // one before building, else molar rejects the stale indices ("selection
            // out of bounds"). `reconcile_reps` won't recompile a rep whose text is
            // unchanged (e.g. "all"), so force it here.
            rep.sel = None;
            rep.expr = None;
            rep.sel_dirty = true;
            rep.geom_dirty = true;
            rep.coords_dirty = false;
            rep.ss_cache = None;
            rep.cartoon_cache = None;
        }
        // Transient highlights may reference now-gone atoms — drop them.
        mol.pending = None;
        mol.glow_dirty = true;
        mol.hover = None;
        mol.hover_dirty = true;
        mol.hover_detail = None;
        mol.hover_detail_dirty = true;
        #[cfg(not(target_arch = "wasm32"))]
        {
            mol.pick_dirty = true;
        }
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
        // Structure edits on a drawn molecule.
        if om.structure != nm.structure {
            if let (Some(o), Some(n)) = (&om.structure, &nm.structure) {
                use std::cmp::Ordering::*;
                match n.atoms.len().cmp(&o.atoms.len()) {
                    Greater => return "add atom".into(),
                    Less => return "delete atom".into(),
                    Equal => {}
                }
                match n.bonds.len().cmp(&o.bonds.len()) {
                    Greater => return "add bond".into(),
                    Less => return "delete bond".into(),
                    Equal => {}
                }
                // Equal atom/bond counts: a bond difference is an order change
                // (or a re-bond); otherwise only the coordinates moved.
                if n.bonds != o.bonds {
                    return "change bond order".into();
                }
                return "edit geometry".into(); // coordinates changed (minimize / move)
            }
            return "draw".into();
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
    // Group-level changes (shared reps, membership, group visibility).
    match new.groups.len().cmp(&old.groups.len()) {
        Less => return "delete group".into(),
        Greater => return "add group".into(),
        Equal => {}
    }
    for ng in &new.groups {
        let Some(og) = old.groups.iter().find(|g| g.id == ng.id) else {
            return "add group".into();
        };
        if og.visible != ng.visible {
            return "toggle group".into();
        }
        if og.members.len() != ng.members.len() {
            return "edit group".into();
        }
        match ng.shared_reps.len().cmp(&og.shared_reps.len()) {
            Greater => return "add shared representation".into(),
            Less => return "delete shared representation".into(),
            Equal => {}
        }
        for (o, n) in og.shared_reps.iter().zip(ng.shared_reps.iter()) {
            if o.sel_text != n.sel_text {
                return "edit shared selection".into();
            }
            if o.kind != n.kind {
                return "change shared style".into();
            }
            if o != n {
                return "edit shared representation".into();
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
    // Depth accessors for the (now test-only) cumulative undo/redo machinery; the
    // UI dropdown that used them was folded into the single-step Edit-menu items.
    #[allow(dead_code)]
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
    #[allow(dead_code)]
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
        scene.add(raw, &crate::settings::RepDefaults::default());
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
        scene.add(
            crate::data::load(Path::new(path)).unwrap(),
            &crate::settings::RepDefaults::default(),
        );
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

    #[test]
    fn undo_redo_structure_edit() {
        use glam::Vec3;
        // A drawn molecule: one carbon, flagged editable.
        let raw = crate::data::RawMolecule::single_atom(
            "Drawn",
            Atom::new().with_name("C").guess(),
            Vec3::new(0.0, 0.0, 0.0),
        )
        .unwrap();
        let mut scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        scene.molecules[0].editable = true;
        let mut hist = History::new(EditState::capture(&scene));

        // Add a second atom + bond it to the first.
        let mol = &mut scene.molecules[0];
        let idx = mol
            .add_atom(&Atom::new().with_name("C").guess(), Vec3::new(0.15, 0.0, 0.0))
            .unwrap();
        assert!(mol.add_bond(0, idx, BondOrder::Single));
        hist.maybe_record(EditState::capture(&scene));
        assert_eq!(hist.undo_label(0), "add atom");

        // Undo → back to the lone carbon, no bonds, still editable.
        hist.undo_n(1).unwrap().apply(&mut scene);
        assert_eq!(scene.molecules[0].n_atoms, 1);
        assert_eq!(scene.molecules[0].bonds.len(), 0);
        // The molar System itself was rebuilt from the snapshot, not just n_atoms.
        assert_eq!(scene.molecules[0].data.state().coords.len(), 1);
        assert!(scene.molecules[0].editable);

        // Redo → two atoms + one bond restored.
        hist.redo_n(1).unwrap().apply(&mut scene);
        assert_eq!(scene.molecules[0].n_atoms, 2);
        assert_eq!(scene.molecules[0].bonds.len(), 1);
        assert_eq!(scene.molecules[0].bonds[0].order, BondOrder::Single);
    }
}
