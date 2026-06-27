//! The per-molecule **data backend** a [`Molecule`](crate::scene::Molecule) renders
//! from — the keystone of the zero-copy "drive the viewer from pymolar" work.
//!
//! Every molecule normally owns a molar [`System`] by value. To let the native
//! Python module render **directly** from a pymolar `System` (so `sel.translate(...)`
//! in Python updates the view live, with no copy), the viewer binds / evaluates /
//! reads coordinates through this abstraction, backed *either* by an owned `System`
//! (standalone app, wasm, the drawing editor) *or* by a shared, interior-mutable
//! external source (pymolar's `Py<Topology>+Py<State>`) via [`SharedSource`], whose
//! only implementor lives in the `molar_vis_py` crate.
//!
//! It's kept as a **directly-borrowable field** on `Molecule` (`mol.data`), not
//! behind `Molecule` methods, because many rebuild loops read the data while holding
//! `&mut mol.reps`; only a sibling-field borrow (`&mol.data` + `&mut mol.reps`) is
//! allowed by the borrow checker. So the methods live here, on the field's type.
//!
//! molar is fully provider-generic — `System::bind_with_state` and `Sel::bind_to`
//! both yield a [`SelBoundParts`] (borrowed `(&Topology, &State)` + indices) that
//! implements the element providers, and selection runs over those providers — so a
//! shared source supplies exactly the same shape an owned `System` does. `bind`
//! therefore returns `SelBoundParts` for both backends; the only `SelBound`-specific
//! path (file save, which needs `SaveTopologyState`) is owned-only and routes through
//! [`system`](MolData::system).
use molar::prelude::*;

use crate::scene::EvalError;

/// A shared, interior-mutable molecule data source — coordinates/topology owned
/// **outside** the viewer (a pymolar `System`), rendered by reference. Implemented in
/// the native `molar_vis_py` crate over `Py<SystemPy>`; this trait keeps
/// `molar_vis_core` free of any pyo3 dependency (so it still builds for wasm).
///
/// The borrows returned by [`topology`](Self::topology)/[`state`](Self::state) must
/// stay valid for as long as `&self` — the implementor is responsible for the
/// interior-mutability / lifetime discipline (mirroring how pymolar uses `UnsafeCell`
/// under the GIL).
pub trait SharedSource {
    /// Borrow the externally-owned topology.
    fn topology(&self) -> &Topology;
    /// Borrow the externally-owned state (live coordinates / box / time).
    fn state(&self) -> &State;
    /// Compile + evaluate a selection string against the source (delegated here
    /// because a full selection needs bond/molecule providers the bare
    /// `SelBoundParts` lacks, but the pymolar `System` supplies).
    fn evaluate(&self, text: &str) -> Result<(SelectionExpr, Sel), EvalError>;

    /// A monotonic counter that increases whenever the source's coordinates are
    /// mutated externally. The viewer polls it (lock-free) to re-render the shared
    /// molecule only when it actually changes, instead of every frame.
    fn coords_version(&self) -> u64;
}

/// Where a molecule's topology + coordinates live.
pub enum MolData {
    /// An owned molar `System` — standalone app, wasm, and the drawing editor.
    Owned(System),
    /// A shared, externally-owned source (pymolar). Constructed only by the separate
    /// `molar_vis_py` crate, so it's never built in a `molar_vis_core`-only compile.
    #[allow(dead_code)]
    Shared(Box<dyn SharedSource>),
}

impl MolData {
    /// Whether this molecule renders from an external shared source (pymolar). Such
    /// molecules have their coordinates mutated from outside the viewer, so the
    /// render loop polls [`coords_version`](Self::coords_version) to re-read them when
    /// they change (see `App::mark_shared_dirty`).
    pub fn is_shared(&self) -> bool {
        matches!(self, MolData::Shared(_))
    }

    /// The shared source's coordinate generation counter (0 for an owned molecule —
    /// owned coordinates only change through the viewer's own dirty flags).
    pub fn coords_version(&self) -> u64 {
        match self {
            MolData::Owned(_) => 0,
            MolData::Shared(s) => s.coords_version(),
        }
    }

    /// Borrow the topology (per-atom identities, bonds metadata).
    pub fn topology(&self) -> &Topology {
        match self {
            MolData::Owned(s) => s.topology(),
            MolData::Shared(s) => s.topology(),
        }
    }

    /// Borrow the state (coordinates, box, time) the data itself holds. Note the
    /// *displayed* coordinates may be a trajectory frame instead — callers pass that
    /// to [`bind_with_state`](Self::bind_with_state); this is the structure state /
    /// trajectory-frame-0 fallback (and the live state for a shared source).
    pub fn state(&self) -> &State {
        match self {
            MolData::Owned(s) => s.state(),
            MolData::Shared(s) => s.state(),
        }
    }

    /// Bind `sel` against an explicit `state` (e.g. a trajectory frame), reading
    /// coordinates by reference — the zero-copy render path.
    pub fn bind_with_state<'a>(&'a self, sel: &'a Sel, state: &'a State) -> SelBoundParts<'a> {
        match self {
            MolData::Owned(s) => s.bind_with_state(sel, state),
            MolData::Shared(s) => sel.bind_to(s.topology(), state),
        }
    }

    /// Bind `sel` against the data's own state, by reference. Returns
    /// [`SelBoundParts`] for both backends (the `SelBound`-only file-save path uses
    /// [`system`](Self::system) instead).
    pub fn bind<'a>(&'a self, sel: &'a Sel) -> SelBoundParts<'a> {
        match self {
            MolData::Owned(s) => s.bind_with_state(sel, s.state()),
            MolData::Shared(s) => sel.bind_to(s.topology(), s.state()),
        }
    }

    /// The underlying owned `System`, if this is an owned molecule. An escape hatch
    /// for native, owned-only paths (saving the whole molecule to a file via molar's
    /// `FileHandler::write`, which takes a `&dyn SaveTopologyState`). `None` for a
    /// shared (pymolar-backed) molecule — save it from Python via the `System`'s own
    /// `.save()` instead.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn system(&self) -> Option<&System> {
        match self {
            MolData::Owned(s) => Some(s),
            MolData::Shared(_) => None,
        }
    }

    /// Mutable access to the owned `System` — for owned-only structure mutation that
    /// goes through molar's `System` API directly (the drawing editor's force-field
    /// relaxation). `None` for a shared molecule (it mutates via its own source).
    pub fn system_mut(&mut self) -> Option<&mut System> {
        match self {
            MolData::Owned(s) => Some(s),
            MolData::Shared(_) => None,
        }
    }

    /// A selection of every atom.
    pub fn select_all(&self) -> Sel {
        match self {
            MolData::Owned(s) => s.select_all(),
            MolData::Shared(s) => Sel::from_vec((0..s.topology().len()).collect())
                .expect("select_all on an empty shared molecule"),
        }
    }

    /// Compile + evaluate a selection string against this data (see
    /// [`crate::scene::evaluate`]).
    pub fn evaluate(&self, text: &str) -> Result<(SelectionExpr, Sel), EvalError> {
        match self {
            MolData::Owned(s) => crate::scene::evaluate(s, text),
            MolData::Shared(s) => s.evaluate(text),
        }
    }

    // --- Mutation (owned molecules only: the drawing editor + the frame-swap trick).
    // A shared molecule is mutated through its own Python `System`, never these, so
    // the shared arms are unreachable invariants.

    /// Swap in a new state, returning the previous one (the `seed_frame0` /
    /// save-displayed trick).
    pub fn set_state(&mut self, st: State) -> Result<State, SelectionError> {
        match self {
            MolData::Owned(s) => s.set_state(st),
            MolData::Shared(_) => unimplemented!("set_state on a shared molecule"),
        }
    }

    /// Append one atom at `pos`; returns its singleton selection.
    pub fn append_atom(&mut self, atom: &Atom, pos: &Pos) -> Result<Sel, SelectionError> {
        match self {
            MolData::Owned(s) => s.append_atom(atom, pos),
            MolData::Shared(_) => unimplemented!("append_atom on a shared molecule"),
        }
    }

    /// Remove the atoms at `indices`.
    pub fn remove(
        &mut self,
        indices: impl Iterator<Item = usize> + Clone,
    ) -> Result<(), BuilderError> {
        match self {
            MolData::Owned(s) => s.remove(indices),
            MolData::Shared(_) => unimplemented!("remove on a shared molecule"),
        }
    }

    /// A mutable bound over all atoms (in-place coordinate edits).
    pub fn select_all_bound_mut(&mut self) -> SelOwnBoundMut<'_> {
        match self {
            MolData::Owned(s) => s.select_all_bound_mut(),
            MolData::Shared(_) => unimplemented!("select_all_bound_mut on a shared molecule"),
        }
    }
}
