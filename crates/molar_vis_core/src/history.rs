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
use crate::edit_journal::EditOp;
use crate::geometry::{RepKind, RepParams};
use crate::material::Material;
use crate::scene::{
    reserve_rep_id, GroupId, MolGroup, MolId, Molecule, MoleculeSource, PeriodicParams, RepId, Representation, Scene,
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
    /// Runtime identity survives undo/redo but is refreshed when a session is loaded.
    #[serde(skip, default = "reserve_rep_id")]
    pub id: RepId,
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
    /// Which charge the `Charge` scheme paints — an option of that scheme, not a scheme of
    /// its own. `#[serde(default)]` keeps older files loading.
    #[serde(default)]
    pub charge_kind: crate::color::ChargeKind,
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
pub(crate) enum SsAlgorithmDef {
    Dssp,
    DsspGmx,
    Dss,
}

impl RepState {
    /// Snapshot a live representation's editable fields.
    pub fn capture(r: &Representation) -> Self {
        Self {
            id: r.id,
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
            charge_kind: r.charge_kind,
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
        rep.charge_kind = self.charge_kind;
        rep.id = self.id;
        rep.partner = self.partner.clone();
        rep
    }
}

/// Atom data for structural undo and sessions. Legacy snapshots without properties
/// derive defaults from the atom name; current snapshots retain all atom metadata.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AtomLite {
    pub name: String,
    pub atomic_number: u8,
    pub resname: String,
    pub resid: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<AtomProperties>,
}

/// Preserve chemical and residue metadata across topology edits and H caching.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AtomProperties {
    pub resindex: usize,
    pub mass: f32,
    pub charge: f32,
    pub chain: char,
    pub bfactor: f32,
    pub occupancy: f32,
    pub formal_charge: Option<i32>,
    pub type_name: Option<String>,
    pub type_id: Option<u32>,
    pub flags: Option<(bool, bool)>,
}

impl AtomLite {
    pub(crate) fn capture(a: &impl AtomLike) -> Self {
        Self {
            name: a.get_name().into(), atomic_number: a.get_atomic_number(),
            resname: a.get_resname().into(), resid: a.get_resid() as i32,
            properties: Some(AtomProperties {
                resindex: a.get_resindex(), mass: a.get_mass(), charge: a.get_charge(),
                chain: a.get_chain(), bfactor: a.get_bfactor(), occupancy: a.get_occupancy(),
                formal_charge: a.get_formal_charge(), type_name: a.get_type_name().map(str::to_owned),
                type_id: a.get_type_id(),
                flags: a.get_chem_flags().map(|f| (f.is_in_ring(), f.is_aromatic())),
            }),
        }
    }

    pub(crate) fn to_atom(&self) -> Atom {
        let mut a = Atom::new().with_name(&self.name).with_resname(&self.resname)
            .with_resid(self.resid).guess();
        a.atomic_number = self.atomic_number;
        if let Some(p) = &self.properties {
            a.resindex = p.resindex; a.mass = p.mass; a.charge = p.charge; a.chain = p.chain;
            a.bfactor = p.bfactor; a.occupancy = p.occupancy; a.formal_charge = p.formal_charge;
            a.type_name = p.type_name.as_ref().map(|v| v.parse().expect("stored atom type"));
            a.type_id = p.type_id;
            a.chem_flags = p.flags.map(|(ring, aromatic)| {
                let mut f = AtomChemFlags::default(); f.set_in_ring(ring); f.set_aromatic(aromatic); f
            });
        }
        a
    }
}

/// The editable **document** state of a molecule: its representations + visibility.
/// A molecule's *structure* (atoms/coords/bonds) is **not** here — it's undone via
/// self-contained [`StructEdit`] deltas on the same timeline (see [`Step`]), so the
/// per-settle document diff never touches per-atom data. A molecule's existence
/// (add/delete) rides the document (reconciled by `MolId` from the scene/trash, which
/// preserves the live structure across an add/delete undo).
#[derive(Clone, PartialEq)]
pub struct MolState {
    /// Runtime identity; only meaningful within a live session (undo/redo).
    pub id: MolId,
    pub visible: bool,
    pub reps: Vec<RepState>,
    pub meshes: Vec<crate::mesh::MeshState>,
}

/// A reversible structural change to one molecule, undone/redone in place (the delta
/// half of the undo timeline). Coordinate moves store only the atoms that moved; topology
/// changes carry full before/after snapshots (molar re-indexes atoms on removal, so
/// per-atom topology deltas aren't feasible — but topology edits are rare and small, and
/// the snapshots are `Arc`-shared).
pub enum StructEdit {
    /// A coordinate-only move (dihedral twist, minimizer/cleanup): the moved atoms and
    /// their positions before/after, plus the coordinate store the delta targets —
    /// a specific trajectory frame (`Some`) or the owned System (`None`), captured at edit
    /// time so undo hits the same store even after the displayed frame/trajectory changes.
    Coords {
        atoms: Vec<usize>,
        before: Vec<[f32; 3]>,
        after: Vec<[f32; 3]>,
        frame: Option<usize>,
    },
    /// An accepted drawing-editor edit: the journal of small reversible ops of all its
    /// gestures (see [`crate::edit_journal`]), replayed in order (redo) or inverted in
    /// reverse (undo). No full structure is stored.
    Journal(Vec<EditOp>),
    /// A per-atom partial-charge assignment (espaloma): the charged atoms and their
    /// charges before/after. Shaped like [`StructEdit::Coords`] but with no frame target —
    /// charges live in the topology, not per trajectory frame. Only ever produced natively
    /// (espaloma is a native-only dependency).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Charges {
        atoms: Vec<usize>,
        before: Vec<f32>,
        after: Vec<f32>,
    },
}

impl StructEdit {
    /// Apply this edit to `mol` in `scene`. `forward` = redo (write `after`); `!forward`
    /// = undo (write `before`). No-op if the molecule id is gone (dangling reference).
    fn apply(&self, scene: &mut Scene, mol: MolId, forward: bool) {
        let Some(mi) = scene.mol_index(mol) else { return };
        match self {
            StructEdit::Coords { atoms, before, after, frame } => {
                let coords = if forward { after } else { before };
                let m = &mut scene.molecules[mi];
                m.set_coords(atoms, coords, *frame);
                m.mark_coords_dirty(true);
                m.refresh_bbox();
            }
            StructEdit::Journal(ops) => scene.molecules[mi].replay(ops, forward),
            StructEdit::Charges { atoms, before, after } => {
                let charges = if forward { after } else { before };
                scene.molecules[mi].set_charges(atoms, charges);
            }
        }
    }
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
                    meshes: m.meshes.iter().map(|s| s.state.clone()).collect(),
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
            if mol.meshes.len() != ms.meshes.len() || mol.meshes.iter().zip(&ms.meshes).any(|(m, s)| m.state != *s) {
                mol.meshes = ms.meshes.iter().cloned().map(crate::mesh::Mesh::from_state).collect();
            }
            // Strip any group-shared prefix and reconcile this member's OWN reps only
            // (`ms.reps`); the shared reps are re-materialized from the group state
            // below. Clear the group tag — it's re-set when groups are reconciled.
            let ns = mol.n_shared.min(mol.reps.len());
            if ns > 0 {
                mol.reps.drain(0..ns);
            }
            mol.n_shared = 0;
            mol.group = None;
            // Structure is not part of the document — it's preserved across add/delete
            // undo by reusing the molecule (with its live System) from the scene/trash,
            // and edited via `StructEdit` deltas. So only reps are reconciled here.
            reconcile_reps(&mut mol, &ms.reps);
            new_mols.push(mol);
        }
        for (id, mol) in pool {
            // A molecule leaving the document stops its trajectory loader, exactly as an
            // explicit delete does (`Scene::trash_molecule`); otherwise the loader keeps
            // reading and its frames are discarded while the molecule is in the trash.
            scene.drop_loaders(id);
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
                    members_expanded: false,
                docking_sync: None,
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

/// Reconcile a molecule's reps to `target`, matching them by [`RepId`] — not by position —
/// so adding, deleting or reordering one rep keeps every other rep's built geometry (an
/// SES surface is expensive to compute again).
fn reconcile_reps(mol: &mut Molecule, target: &[RepState]) {
    let mut old: Vec<Option<Representation>> = mol.reps.drain(..).map(Some).collect();
    mol.reps = target
        .iter()
        .map(|s| {
            let cur = old
                .iter_mut()
                .find(|r| r.as_ref().is_some_and(|r| r.id == s.id))
                .and_then(Option::take);
            match cur {
                Some(mut cur) => {
                    update_rep(&mut cur, s);
                    cur
                }
                None => s.to_representation(),
            }
        })
        .collect();
}

/// Bring the live rep `cur` (same id as `s`) to state `s`: rebuild it if a
/// geometry-affecting field differs, else apply the cheap fields in place.
fn update_rep(cur: &mut Representation, s: &RepState) {
    let needs_rebuild = cur.kind != s.kind
        || cur.params != s.params
        || cur.color != s.color
        || cur.charge_kind != s.charge_kind
        || cur.partner != s.partner
        || cur.ss_algo != s.ss_algo
        || cur.sel_text != s.sel_text
        || (cur.material != s.material && cur.kind != crate::geometry::RepKind::Surface);
    if needs_rebuild {
        *cur = s.to_representation();
        return;
    }
    cur.set_material(s.material);
    // Cheap, no-geometry changes (periodic display is render-only too).
    cur.visible = s.visible;
    cur.dynamic = s.dynamic;
    cur.ss_per_frame = s.ss_per_frame;
    cur.periodic = s.periodic;
    // Smoothing changes the rendered coords → mark for an incremental rebuild.
    if cur.smooth_window != s.smooth_window {
        cur.smooth_window = s.smooth_window;
        cur.coords_dirty = true;
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
        match nm.meshes.len().cmp(&om.meshes.len()) {
            Greater => return "add mesh".into(),
            Less => return "delete mesh".into(),
            Equal => {}
        }
        for (o, n) in om.meshes.iter().zip(&nm.meshes) {
            if o.visible != n.visible { return "toggle mesh".into(); }
            if o.material != n.material { return "change mesh material".into(); }
            if o.style != n.style { return "change mesh style".into(); }
            if o.point_size != n.point_size || o.line_width != n.line_width { return "change mesh size".into(); }
            if o.smoothing != n.smoothing { return "smooth mesh".into(); }
        }
        // (Structure edits — atom/bond/coordinate — are `StructEdit` steps with their
        // own labels; the document diff no longer sees them.)
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
            if o.material != n.material {
                return "change material".into();
            }
            if o.charge_kind != n.charge_kind {
                return "change charge type".into();
            }
            if o.partner != n.partner {
                return "change interaction partner".into();
            }
            if o.ss_per_frame != n.ss_per_frame {
                return "toggle per-frame SS".into();
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
        if og.members != ng.members {
            return "reorder group".into();
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

/// One step on the unified undo timeline.
enum Step {
    /// A document change: the stored [`EditState`] is the state to restore when this step
    /// is undone (the "before"; redo stores the "after" symmetrically). Applied against
    /// the rolling `committed` baseline.
    Doc(EditState),
    /// One or more structural/coordinate changes (self-contained before+after), applied in
    /// place and independent of `committed`. Usually a single molecule's delta; a list when
    /// one gesture edits several — a group-wide charge assignment touches every member, and
    /// a single Ctrl+Z has to take all of it back.
    Struct(Vec<(MolId, StructEdit)>),
}

struct Entry {
    step: Step,
    /// Label of this action (shown in the Edit menu).
    label: String,
}

/// Undo/redo history: a single chronological, strict-LIFO stack of [`Step`]s (document
/// snapshots interleaved with structural deltas), so one Ctrl+Z walks both. `committed`
/// is the rolling baseline for the document steps; structural steps don't touch it (they
/// edit disjoint per-atom state), which is what lets the two mechanisms compose.
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    committed: EditState,
    cap: usize,
    /// Molecules whose structural steps were trimmed off the bottom of the undo stack.
    /// Those edits are now permanent, so they still count for
    /// [`struct_edited_ids`](Self::struct_edited_ids).
    retired_struct: std::collections::HashSet<MolId>,
    /// Set when steps were dropped (redo cleared or cap trim), so trashed molecules may
    /// have become unreachable; [`collect_garbage`](Self::collect_garbage) then runs.
    gc_pending: bool,
}

impl History {
    pub fn new(initial: EditState) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            committed: initial,
            cap: 200,
            retired_struct: Default::default(),
            gc_pending: false,
        }
    }

    /// Push a new step onto the undo stack (trimming to `cap`) and clear redo — the one
    /// place both the document and structural record paths funnel through.
    fn push_undo(&mut self, step: Step, label: String) {
        self.undo.push(Entry { step, label });
        if self.undo.len() > self.cap {
            let old = self.undo.remove(0);
            if let Step::Struct(edits) = &old.step {
                self.retired_struct.extend(edits.iter().map(|(mol, _)| *mol));
            }
            self.gc_pending = true;
        }
        if !self.redo.is_empty() {
            self.redo.clear();
            self.gc_pending = true;
        }
    }

    /// Whether `current` differs from the last committed document state (a document
    /// step would be recorded now).
    pub fn doc_differs(&self, current: &EditState) -> bool {
        *current != self.committed
    }

    /// Free trashed molecules and groups that no step can restore any more. A deleted
    /// molecule stays in [`Scene::trash`] (with its System, frames and GPU buffers) only
    /// while a document state on the timeline still lists it. Cheap when nothing was
    /// dropped since the last call.
    pub fn collect_garbage(&mut self, scene: &mut Scene) {
        if !std::mem::take(&mut self.gc_pending) {
            return;
        }
        let mut mols = std::collections::HashSet::new();
        let mut groups = std::collections::HashSet::new();
        let docs = self.undo.iter().chain(&self.redo).filter_map(|e| match &e.step {
            Step::Doc(s) => Some(s),
            Step::Struct(_) => None,
        });
        for s in std::iter::once(&self.committed).chain(docs) {
            mols.extend(s.mols.iter().map(|m| m.id));
            groups.extend(s.groups.iter().map(|g| g.id));
        }
        scene.purge_trash(&mols, &groups);
    }

    /// Re-target trajectory-frame coordinate edits of `mol` after frames were deleted.
    /// `map[old] = Some(new)` for a kept frame, `None` for a deleted one. An edit of a
    /// deleted frame is gone with its frame, so it is removed (and so is a step that
    /// becomes empty).
    pub fn remap_frames(&mut self, mol: MolId, map: &[Option<usize>]) {
        for stack in [&mut self.undo, &mut self.redo] {
            for e in stack.iter_mut() {
                if let Step::Struct(edits) = &mut e.step {
                    edits.retain_mut(|(id, edit)| {
                        let StructEdit::Coords { frame: Some(f), .. } = edit else { return true };
                        if *id != mol {
                            return true;
                        }
                        match map.get(*f).copied().flatten() {
                            Some(n) => {
                                *f = n;
                                true
                            }
                            None => false,
                        }
                    });
                }
            }
            stack.retain(|e| !matches!(&e.step, Step::Struct(v) if v.is_empty()));
        }
    }

    /// Record a **document** checkpoint if `current` differs from the last committed
    /// state. Call only when the interaction has settled (coalesces drags/typing).
    pub fn maybe_record(&mut self, current: EditState) {
        if current != self.committed {
            let label = describe_change(&self.committed, &current);
            let before = std::mem::replace(&mut self.committed, current);
            self.push_undo(Step::Doc(before), label);
        }
    }

    /// Record a **structural** delta (coordinate/topology edit) as its own step. Recorded
    /// eagerly at gesture completion; since structure is not part of the document, the
    /// settle-time [`maybe_record`](Self::maybe_record) stays silent for it (no
    /// double-record), and being pushed mid-frame it lands before that frame's doc step.
    pub fn record_struct(&mut self, mol: MolId, edit: StructEdit, label: String) {
        self.record_structs(vec![(mol, edit)], label)
    }

    /// Record several molecules' deltas as **one** step, so undo/redo takes them together.
    pub fn record_structs(&mut self, edits: Vec<(MolId, StructEdit)>, label: String) {
        if edits.is_empty() {
            return;
        }
        for (mol, edit) in &edits { self.adopt_structure_selections(*mol, edit, true); }
        self.push_undo(Step::Struct(edits), label)
    }

    /// Index remaps and newly included atoms belong to the structural edit, rather
    /// than a second document step at frame settle.
    fn adopt_structure_selections(&mut self, id: MolId, edit: &StructEdit, forward: bool) {
        let StructEdit::Journal(ops) = edit else { return };
        if let Some(texts) = crate::edit_journal::selections_after(ops, forward) {
            self.adopt_selections(id, texts);
        }
    }

    /// Write the selection texts `texts` (by rep id) of molecule `id` into the committed
    /// document, so a selection change that an editor gesture made is not recorded again
    /// as a document step.
    pub fn adopt_selections(&mut self, id: MolId, texts: &[(RepId, String)]) {
        let reps = self.committed.mols.iter_mut().filter(|m| m.id == id).flat_map(|m| &mut m.reps)
            .chain(self.committed.groups.iter_mut().flat_map(|g| &mut g.shared_reps));
        for rep in reps {
            if let Some((_, text)) = texts.iter().find(|(rid, _)| *rid == rep.id) {
                rep.sel_text = text.clone();
            }
        }
    }

    #[cfg(test)]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    #[cfg(test)]
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

    /// The molecule ids that currently carry a **net applied** structural edit — a `Struct`
    /// step still on the undo stack, i.e. not one that's been fully undone (which moves it
    /// to the redo stack). This is the correct "structure differs from what was loaded"
    /// signal for session save: `structure_version` only ever counts up (an undo bumps it
    /// too), so it can't tell a live edit from one that was made and then reverted.
    // Consumed only by the native-only session save, so it's dead code on wasm.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn struct_edited_ids(&self) -> std::collections::HashSet<MolId> {
        self.undo
            .iter()
            .filter_map(|e| match &e.step {
                Step::Struct(edits) => Some(edits.iter().map(|(mol, _)| *mol)),
                Step::Doc(_) => None,
            })
            .flatten()
            .chain(self.retired_struct.iter().copied())
            .collect()
    }

    /// Undo one step, applying it to `scene` in place; returns its label (or `None` if
    /// the undo stack is empty). Steps are replayed individually in strict LIFO order — a
    /// structural delta must each be inverted in sequence, so there is no "jump to a
    /// committed snapshot" shortcut. Loop this for a multi-step undo.
    pub fn undo(&mut self, scene: &mut Scene) -> Option<String> {
        let entry = self.undo.pop()?;
        let label = entry.label.clone();
        let redo_entry = match entry.step {
            Step::Doc(before) => {
                let after = std::mem::replace(&mut self.committed, before);
                self.committed.apply(scene);
                Entry { step: Step::Doc(after), label }
            }
            Step::Struct(edits) => {
                // Reverse order, so a compound step unwinds exactly as it was applied.
                for (mol, edit) in edits.iter().rev() {
                    edit.apply(scene, *mol, false);
                    self.adopt_structure_selections(*mol, edit, false);
                }
                Entry { step: Step::Struct(edits), label }
            }
        };
        let out = redo_entry.label.clone();
        self.redo.push(redo_entry);
        Some(out)
    }

    /// Redo one step (the inverse of [`undo`](Self::undo)); returns its label.
    pub fn redo(&mut self, scene: &mut Scene) -> Option<String> {
        let entry = self.redo.pop()?;
        let undo_entry = match entry.step {
            Step::Doc(after) => {
                let before = std::mem::replace(&mut self.committed, after);
                self.committed.apply(scene);
                Entry { step: Step::Doc(before), label: entry.label }
            }
            Step::Struct(edits) => {
                for (mol, edit) in &edits {
                    edit.apply(scene, *mol, true);
                    self.adopt_structure_selections(*mol, edit, true);
                }
                Entry { step: Step::Struct(edits), label: entry.label }
            }
        };
        let out = undo_entry.label.clone();
        self.undo.push(undo_entry);
        Some(out)
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

    /// Capture the coords of `atoms` in molecule 0 as `[f32;3]`s (test helper).
    fn coords_of(scene: &Scene, atoms: &[usize]) -> Vec<[f32; 3]> {
        let st = scene.molecules[0].data.state();
        atoms.iter().map(|&a| { let p = st.coords[a]; [p.x, p.y, p.z] }).collect()
    }

    #[test]
    fn undo_redo_add_rep() {
        let mut scene = load_scene();
        let mut hist = History::new(EditState::capture(&scene));
        scene.molecules[0].reps.push(Representation::new(RepKind::Vdw));
        hist.maybe_record(EditState::capture(&scene));
        assert!(hist.can_undo());
        assert_eq!(hist.undo_label(0), "add representation");

        hist.undo(&mut scene);
        assert_eq!(scene.molecules[0].reps.len(), 1);
        hist.redo(&mut scene);
        assert_eq!(scene.molecules[0].reps.len(), 2);
    }

    #[test]
    fn identical_rep_reorder_restores_runtime_id_order() {
        let mut scene = load_scene();
        scene.molecules[0].reps.push(Representation::new(RepKind::Vdw));
        scene.molecules[0].reps.push(Representation::new(RepKind::Vdw));
        let before: Vec<_> = scene.molecules[0].reps.iter().map(|r| r.id).collect();
        assert_ne!(before[1], before[2]);
        let mut history = History::new(EditState::capture(&scene));
        scene.molecules[0].reps.swap(1, 2);
        history.maybe_record(EditState::capture(&scene));
        assert!(history.can_undo());
        history.undo(&mut scene);
        assert_eq!(scene.molecules[0].reps.iter().map(|r| r.id).collect::<Vec<_>>(), before);
    }

    #[test]
    fn rep_runtime_id_survives_snapshot_but_refreshes_on_deserialize() {
        let rep = Representation::new(RepKind::Vdw);
        let snapshot = RepState::capture(&rep);
        assert_eq!(snapshot.to_representation().id, rep.id);
        let json = serde_json::to_string(&snapshot).unwrap();
        let restored: RepState = serde_json::from_str(&json).unwrap();
        assert_ne!(restored.id, rep.id);
        assert_eq!(restored.to_representation().id, restored.id);
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

        // Undo all three (one step at a time) → back to the original single "all" rep.
        for _ in 0..3 {
            hist.undo(&mut scene);
        }
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

        hist.undo(&mut scene);
        assert_eq!(scene.molecules.len(), 2);
        assert!(scene.trash.is_empty());
    }

    #[test]
    fn undo_redo_structure_edit() {
        use glam::Vec3;
        let raw = crate::data::RawMolecule::single_atom(
            "Drawn",
            Atom::new().with_name("C").guess(),
            Vec3::new(0.0, 0.0, 0.0),
        )
        .unwrap();
        let mut scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        let mut hist = History::new(EditState::capture(&scene));
        let id = scene.molecules[0].id;

        // Add a second atom + bond it to the first, journaled as an editor gesture.
        let mol = &mut scene.molecules[0];
        mol.begin_edit();
        let idx = mol
            .add_atom(&Atom::new().with_name("C").guess(), Vec3::new(0.15, 0.0, 0.0))
            .unwrap();
        assert!(mol.add_bond(0, idx, BondOrder::Single));
        let ops = mol.end_edit();
        assert_eq!(crate::edit_journal::LocalEdit::label_for(&ops), "add atom");
        hist.record_struct(id, StructEdit::Journal(ops), "add atom".into());

        // Undo → back to the lone carbon, no bonds.
        hist.undo(&mut scene);
        assert_eq!(scene.molecules[0].n_atoms, 1);
        assert_eq!(scene.molecules[0].bonds.len(), 0);
        assert_eq!(scene.molecules[0].data.state().coords.len(), 1);

        // Redo → two atoms + one bond restored.
        hist.redo(&mut scene);
        assert_eq!(scene.molecules[0].n_atoms, 2);
        assert_eq!(scene.molecules[0].bonds.len(), 1);
        assert_eq!(scene.molecules[0].bonds[0].order, BondOrder::Single);
    }

    /// A coordinate edit (a dihedral twist) is undoable on any molecule via a `Coords`
    /// delta that stores only the moved atoms' before/after positions.
    #[test]
    fn undo_redo_coordinate_edit() {
        use glam::Vec3;
        let mut scene = load_scene(); // 2lao — a plain file-loaded molecule
        let mut hist = History::new(EditState::capture(&scene));
        let id = scene.molecules[0].id;

        let atoms = vec![10usize, 11, 12];
        let before = coords_of(&scene, &atoms);
        scene.molecules[0].rotate_fragment(&atoms, Vec3::ZERO, Vec3::X, 0.7);
        let after = coords_of(&scene, &atoms);
        assert_ne!(before, after, "rotation should move the atoms");

        hist.record_struct(
            id,
            StructEdit::Coords {
                atoms: atoms.clone(),
                before: before.clone(),
                after: after.clone(),
                frame: None,
            },
            "rotate bond".into(),
        );
        assert_eq!(hist.undo_label(0), "rotate bond");

        hist.undo(&mut scene);
        assert_eq!(coords_of(&scene, &atoms), before, "undo restores coordinates");
        hist.redo(&mut scene);
        assert_eq!(coords_of(&scene, &atoms), after, "redo re-applies the twist");
    }

    /// A charge assignment is a structural delta too, so Ctrl+Z reverts it. Charges live
    /// in the topology rather than per frame, so the edit carries no frame target.
    #[test]
    fn undo_redo_charge_assignment() {
        let mut scene = load_scene();
        let mut hist = History::new(EditState::capture(&scene));
        let id = scene.molecules[0].id;
        let charges_of = |scene: &Scene, atoms: &[usize]| -> Vec<f32> {
            let topo = scene.molecules[0].data.topology();
            atoms.iter().map(|&a| topo.get_atom(a).unwrap().get_charge()).collect()
        };

        let atoms = vec![0usize, 1, 2];
        let before = charges_of(&scene, &atoms);
        let after = vec![-0.5f32, 0.25, 0.25];
        assert_ne!(before, after);

        scene.molecules[0].set_charges(&atoms, &after);
        assert_eq!(charges_of(&scene, &atoms), after, "charges are written into the topology");
        hist.record_struct(
            id,
            StructEdit::Charges {
                atoms: atoms.clone(),
                before: before.clone(),
                after: after.clone(),
            },
            "compute charges".into(),
        );
        assert_eq!(hist.undo_label(0), "compute charges");

        hist.undo(&mut scene);
        assert_eq!(charges_of(&scene, &atoms), before, "undo restores the old charges");
        hist.redo(&mut scene);
        assert_eq!(charges_of(&scene, &atoms), after, "redo re-applies them");
    }

    /// A gesture that edits several molecules records **one** step, so a single Ctrl+Z
    /// takes all of it back — a group-wide charge assignment is the case that needs this.
    #[test]
    fn compound_struct_step_undoes_every_molecule() {
        let mut scene = load_scene();
        // A second molecule, so the step spans two.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb");
        let raw = crate::data::load(Path::new(path)).expect("load 2lao.pdb");
        scene.add(raw, &crate::settings::RepDefaults::default());
        let ids: Vec<MolId> = scene.molecules.iter().map(|m| m.id).collect();
        assert_eq!(ids.len(), 2);

        let atoms = vec![0usize, 1];
        let charges_of = |scene: &Scene, mi: usize| -> Vec<f32> {
            let topo = scene.molecules[mi].data.topology();
            atoms.iter().map(|&a| topo.get_atom(a).unwrap().get_charge()).collect()
        };
        let before: Vec<Vec<f32>> = (0..2).map(|mi| charges_of(&scene, mi)).collect();
        let after = vec![-0.4f32, 0.4];

        let mut hist = History::new(EditState::capture(&scene));
        let mut edits = Vec::new();
        for (mi, &id) in ids.iter().enumerate() {
            scene.molecules[mi].set_charges(&atoms, &after);
            edits.push((
                id,
                StructEdit::Charges {
                    atoms: atoms.clone(),
                    before: before[mi].clone(),
                    after: after.clone(),
                },
            ));
        }
        hist.record_structs(edits, "compute charges".into());
        assert_eq!(hist.undo_len(), 1, "one gesture → one step");

        hist.undo(&mut scene);
        for mi in 0..2 {
            assert_eq!(charges_of(&scene, mi), before[mi], "molecule {mi} restored by one undo");
        }
        hist.redo(&mut scene);
        for mi in 0..2 {
            assert_eq!(charges_of(&scene, mi), after, "molecule {mi} re-charged by one redo");
        }
    }

    /// Document and structural steps interleave on one timeline: undo walks them in
    /// strict reverse order (the §1a invariant from the design review) and redo replays
    /// them forward.
    #[test]
    fn interleaved_doc_and_struct_undo() {
        use glam::Vec3;
        let mut scene = load_scene();
        let mut hist = History::new(EditState::capture(&scene));
        let id = scene.molecules[0].id;
        let orig_color = scene.molecules[0].reps[0].color;

        // (1) Doc: change a rep's color.
        scene.molecules[0].reps[0].color = ColorMethod::Chain;
        hist.maybe_record(EditState::capture(&scene));
        // (2) Struct: twist some atoms.
        let atoms = vec![5usize, 6, 7];
        let before = coords_of(&scene, &atoms);
        scene.molecules[0].rotate_fragment(&atoms, Vec3::ZERO, Vec3::Y, 0.5);
        let after = coords_of(&scene, &atoms);
        hist.record_struct(
            id,
            StructEdit::Coords { atoms: atoms.clone(), before: before.clone(), after, frame: None },
            "rotate bond".into(),
        );
        // (3) Doc: hide the rep.
        scene.molecules[0].reps[0].visible = false;
        hist.maybe_record(EditState::capture(&scene));

        // Undo all three in reverse: hide → twist → color.
        hist.undo(&mut scene);
        assert!(scene.molecules[0].reps[0].visible, "undo (3) restores visibility");
        hist.undo(&mut scene);
        assert_eq!(coords_of(&scene, &atoms), before, "undo (2) restores coordinates");
        hist.undo(&mut scene);
        assert_eq!(scene.molecules[0].reps[0].color, orig_color, "undo (1) restores color");

        // Redo all three forward.
        hist.redo(&mut scene);
        hist.redo(&mut scene);
        hist.redo(&mut scene);
        assert_eq!(scene.molecules[0].reps[0].color, ColorMethod::Chain);
        assert!(!scene.molecules[0].reps[0].visible);
    }

    /// Two captures with no edit in between are equal (no spurious step).
    #[test]
    fn idle_recapture_is_not_a_change() {
        let scene = load_scene();
        assert!(EditState::capture(&scene) == EditState::capture(&scene));
    }

    fn coords_step(frame: Option<usize>) -> StructEdit {
        StructEdit::Coords { atoms: vec![0], before: vec![[0.0; 3]], after: vec![[1.0; 3]], frame }
    }

    /// A structural edit trimmed off the bottom of the stack is permanent; session save
    /// must still see the molecule as edited.
    #[test]
    fn trimmed_struct_step_still_counts_as_edited() {
        let mut scene = load_scene();
        let id = scene.molecules[0].id;
        let mut hist = History::new(EditState::capture(&scene));
        hist.cap = 2;
        hist.record_struct(id, coords_step(None), "rotate bond".into());
        for vis in [false, true, false] {
            scene.molecules[0].reps[0].visible = vis;
            hist.maybe_record(EditState::capture(&scene));
        }
        assert_eq!(hist.undo_len(), 2);
        assert!(hist.struct_edited_ids().contains(&id));
    }

    /// Deleting frames re-targets frame edits and drops the edits of deleted frames.
    #[test]
    fn frame_deletion_remaps_coordinate_steps() {
        let scene = load_scene();
        let id = scene.molecules[0].id;
        let mut hist = History::new(EditState::capture(&scene));
        for f in [0, 1, 4] {
            hist.record_struct(id, coords_step(Some(f)), format!("f{f}"));
        }
        let mut traj = crate::trajectory::Trajectory::default();
        traj.frames = vec![State::default(); 5];
        let map = traj.delete_range_map(1, 2);
        assert_eq!(map, vec![Some(0), None, None, Some(1), Some(2)]);
        hist.remap_frames(id, &map);
        let frames: Vec<_> = hist.undo.iter().map(|e| match &e.step {
            Step::Struct(v) => match &v[0].1 { StructEdit::Coords { frame, .. } => *frame, _ => None },
            Step::Doc(_) => None,
        }).collect();
        assert_eq!(frames, vec![Some(0), Some(2)], "frame 1 edit gone, frame 4 → 2");
        assert_eq!(traj.decimate_map(2), vec![Some(0), None, Some(1), None, Some(2)]);
    }

    /// A trashed molecule is freed once no document state on the timeline lists it.
    #[test]
    fn unreachable_trash_is_freed() {
        let mut scene = load_scene();
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb");
        scene.add(crate::data::load(Path::new(path)).unwrap(), &crate::settings::RepDefaults::default());
        let mut hist = History::new(EditState::capture(&scene));
        scene.trash_molecule(1);
        hist.maybe_record(EditState::capture(&scene));
        scene.molecules[0].reps[0].visible = false;
        hist.maybe_record(EditState::capture(&scene));
        hist.collect_garbage(&mut scene);
        assert_eq!(scene.trash.len(), 1, "still restorable by undo");
        hist.cap = 1; // the next record trims the delete step away
        scene.molecules[0].reps[0].visible = true;
        hist.maybe_record(EditState::capture(&scene));
        hist.collect_garbage(&mut scene);
        assert!(scene.trash.is_empty(), "no step can restore it any more");
    }

    /// Undoing "add molecule" stops that molecule's trajectory loader.
    #[test]
    fn undo_of_add_stops_loader() {
        let mut scene = load_scene();
        let mut hist = History::new(EditState::capture(&scene));
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb");
        let id = scene.add(crate::data::load(Path::new(path)).unwrap(), &crate::settings::RepDefaults::default());
        hist.maybe_record(EditState::capture(&scene));
        let (_tx, rx) = std::sync::mpsc::channel();
        scene.loaders.insert(id, rx);
        hist.undo(&mut scene);
        assert!(scene.trash.contains_key(&id));
        assert!(scene.loaders.is_empty());
    }

    /// Undoing a rep deletion rebuilds only that rep; the others keep their geometry.
    #[test]
    fn rep_undo_keeps_unchanged_reps_built() {
        let mut scene = load_scene();
        scene.molecules[0].reps.push(Representation::new(RepKind::Vdw));
        scene.molecules[0].reps.push(Representation::new(RepKind::Vdw));
        let ids: Vec<_> = scene.molecules[0].reps.iter().map(|r| r.id).collect();
        let mut hist = History::new(EditState::capture(&scene));
        scene.molecules[0].reps.remove(1);
        hist.maybe_record(EditState::capture(&scene));
        for r in &mut scene.molecules[0].reps {
            r.geom_dirty = false; // stands in for "built"
        }
        hist.undo(&mut scene);
        let reps = &scene.molecules[0].reps;
        assert_eq!(reps.iter().map(|r| r.id).collect::<Vec<_>>(), ids);
        assert!(!reps[0].geom_dirty && !reps[2].geom_dirty, "untouched reps not rebuilt");
        assert!(reps[1].sel.is_none(), "restored rep is a fresh, unbuilt rep");
    }

    /// Atoms, metadata, coordinates, bonds (with list order), selections and the
    /// hydrogen cache of `mol` — everything a journal replay must restore exactly.
    fn full_state(mol: &Molecule) -> (Vec<AtomLite>, Vec<[f32; 3]>, Vec<Bond>, Vec<String>, usize) {
        let top = mol.data.topology();
        (
            (0..mol.n_atoms).map(|i| AtomLite::capture(&top.get_atom(i).unwrap())).collect(),
            mol.data.state().coords.iter().map(|p| [p.x, p.y, p.z]).collect(),
            mol.bonds.clone(),
            mol.reps.iter().map(|r| r.sel_text.clone()).collect(),
            mol.hydrogen_cache.len(),
        )
    }

    /// A journal of mixed edits on a real protein replays back and forth exactly: the
    /// inverse of an atom removal puts the same atoms back at the same indices, with
    /// their bonds at the same places in the bond list.
    #[test]
    fn journal_replay_is_exact_both_ways() {
        use glam::Vec3;
        let mut scene = load_scene();
        scene.molecules[0].reps[0].sel_text = "index 3 to 40".into();
        let id = scene.molecules[0].id;
        let start = full_state(&scene.molecules[0]);
        let mut hist = History::new(EditState::capture(&scene));

        let mol = &mut scene.molecules[0];
        mol.begin_edit();
        mol.remove_atoms(&[5, 11, 12, 300]);
        mol.remove_atom(0);
        let c = mol.add_atom(&Atom::new().with_name("C").guess(), Vec3::new(1.0, 1.0, 1.0)).unwrap();
        assert!(mol.add_bond(1, c, BondOrder::Double));
        mol.cycle_bond_order(3);
        mol.remove_bond_at(7);
        mol.set_atom_element(2, &Atom::new().with_name("N").guess());
        let text = format!("{} or index {c}", mol.reps[0].sel_text); // as `include_atom` does
        mol.reps[0].sel_text = text;
        let ops = mol.end_edit();
        let edited = full_state(&scene.molecules[0]);
        assert!(edited != start);
        hist.record_struct(id, StructEdit::Journal(ops), "edit".into());

        hist.undo(&mut scene);
        assert!(full_state(&scene.molecules[0]) == start, "undo restores everything");
        hist.redo(&mut scene);
        assert!(full_state(&scene.molecules[0]) == edited, "redo repeats everything");
        hist.undo(&mut scene);
        assert!(full_state(&scene.molecules[0]) == start, "second undo is exact too");
    }

}
