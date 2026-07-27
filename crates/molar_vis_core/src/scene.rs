//! The scene graph: a set of molecules, each with its own representations.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use glam::Vec3;
use molar::prelude::*;
use serde::{Deserialize, Serialize};

use crate::color::ColorMethod;
use crate::data::{bond_storage, RawMolecule};
use crate::geometry::{RepKind, RepParams};
use crate::history::{RepState, StructureSnapshot};
use crate::material::Material;
use crate::minimize::{Bond, BondOrder};
use crate::moldata::MolData;
use crate::render::RepGpu;
use crate::secstruct::SsMap;
use crate::trajectory::Trajectory;

/// Stable per-molecule identity, so undo/redo can reference molecules across
/// deletion (a deleted molecule is parked in [`Scene::trash`] by this id).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct MolId(pub u64);

/// Stable per-group identity (parallel to [`MolId`]), so undo/redo and sessions
/// can reference a [`MolGroup`] across reordering.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct GroupId(pub u64);

/// A **molecular group**: several distinct molecules loaded together (the records
/// of a multi-molecule SDF) shown one at a time, with representations that are
/// **shared** across every member. The member molecules are ordinary [`Molecule`]s
/// living in [`Scene::molecules`] (so the whole render/rebuild/pick pipeline treats
/// them exactly like any molecule); this struct only adds the grouping layer.
///
/// **Shared reps** are not stored here. The live, editable shared
/// [`Representation`]s are the **first `n_shared` entries** of the *currently shown*
/// member's `reps` (see [`Molecule::n_shared`]) — that one materialized copy is the
/// single source of truth, so the renderer draws them with no group-awareness.
/// Switching members strips the prefix off the old member and re-materializes it
/// onto the new one ([`Scene::switch_group_member`]); snapshots (undo/session)
/// carry their own `Vec<RepState>` copies.
pub struct MolGroup {
    pub id: GroupId,
    /// Display name (the source file's name).
    pub name: String,
    /// Where the group was loaded from (the `.sdf`/`.mol`), so a session can reload it.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub source: MoleculeSource,
    /// Member molecule ids, in record order. The flat [`Scene::molecules`] holds the
    /// molecules themselves; this fixes their display order in the group.
    pub members: Vec<MolId>,
    /// Index into `members` of the member shown right now (view state, not undoable —
    /// like [`Trajectory::current`]).
    pub current: usize,
    /// Group-level visibility (the header eye). The shown member is visible iff this
    /// is true; all other members are always hidden.
    pub visible: bool,
    /// Transient UI: whether the group's top-level entry is expanded — showing its
    /// shared representations + the nested "Molecules" sub-expander. Not undoable.
    pub expanded: bool,
    /// Transient UI: whether the nested "Molecules" sub-expander is open — listing the
    /// member molecules (each foldable to its own reps). Independent of [`expanded`]
    /// (only rendered while it's true). Not undoable.
    pub members_expanded: bool,
    /// **Flexible-docking frame coupling**: the `(shown member, receptor frame)` pair as of
    /// the last reconcile, or `None` to force one. When this group's `Interactions` rep
    /// points at a molecule with exactly one frame per member, whichever of the two the user
    /// moved is propagated to the other (see `App::sync_docking_frames`) — comparing against
    /// the last pair is what tells them apart, and it means playback of the receptor
    /// trajectory cycles the poses too. Transient view state: not undoable, not serialized.
    pub docking_sync: Option<(usize, usize)>,
}

/// Periodic-image display for a representation: render extra copies of the
/// selection shifted by integer combinations of the box lattice vectors `a,b,c`.
/// This is **purely a rendering** concern — molar stores only the "self" coords;
/// images are drawn by re-running the same GPU geometry under a translated camera,
/// so nothing is duplicated on the CPU or GPU. Only meaningful when the molecule
/// has a periodic box. In `EditState` (undoable).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct PeriodicParams {
    /// Render the central, un-shifted copy.
    pub self_img: bool,
    /// Draw the periodic box wireframe, replicated across every shown image.
    pub show_box: bool,
    /// Image counts in the −a, −b, −c directions.
    pub neg: [u32; 3],
    /// Image counts in the +a, +b, +c directions.
    pub pos: [u32; 3],
}

impl Default for PeriodicParams {
    fn default() -> Self {
        Self { self_img: true, show_box: false, neg: [0; 3], pos: [0; 3] }
    }
}

impl PeriodicParams {
    /// World-space translation offsets of every image this rep draws (the central
    /// `(0,0,0)` image included iff `self_img`), as integer combinations of the box
    /// lattice vectors `a,b,c` (nm). Shared by the renderer (one camera per offset)
    /// and the picker (hit-test every drawn image) so they always agree.
    pub fn offsets(&self, a: Vec3, b: Vec3, c: Vec3) -> Vec<Vec3> {
        let mut out = Vec::new();
        for i in -(self.neg[0] as i32)..=(self.pos[0] as i32) {
            for j in -(self.neg[1] as i32)..=(self.pos[1] as i32) {
                for k in -(self.neg[2] as i32)..=(self.pos[2] as i32) {
                    if i == 0 && j == 0 && k == 0 && !self.self_img {
                        continue;
                    }
                    out.push(a * i as f32 + b * j as f32 + c * k as f32);
                }
            }
        }
        out
    }
}

/// Where a molecule's structure was loaded from, so a saved visualization state
/// can reload the same atoms. Sessions reference molecules by source rather than
/// embedding their coordinates (that is a separate "save molecules to file"
/// feature) — small, and lets the structure file evolve independently.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MoleculeSource {
    /// A structure file on disk (native). Reloaded with [`crate::data::load`].
    File(PathBuf),
    /// One `$$$$` record (0-based `index`) of a multi-molecule file on disk (a
    /// member of a [`MolGroup`]). A *whole-molecule* standalone reload is ambiguous
    /// here (the file holds many molecules), so this is treated like [`Bytes`] for
    /// the standalone "Save molecule"/session-as-molecule paths; the group's
    /// session reload re-opens the file and walks to `index`.
    SdfRecord { path: PathBuf, index: usize },
    /// In-memory bytes (the browser file picker, or the bundled demo): there is no
    /// path to reload from, so a session referencing this cannot restore the atoms
    /// in a fresh process. We keep the original name for display/diagnostics.
    Bytes { name: String },
}

impl Default for MoleculeSource {
    fn default() -> Self {
        MoleculeSource::Bytes { name: "molecule".to_string() }
    }
}

/// A record of one trajectory file loaded into a molecule, so a saved session can
/// replay the same loads. Multiple loads concatenate (see [`Trajectory`]); the
/// list preserves that order. Native-only in practice (paths), but the type is
/// platform-agnostic so the session format is uniform.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrajLoad {
    pub path: PathBuf,
    pub from: usize,
    pub to: Option<usize>,
    pub stride: usize,
}

/// One representation of a molecule: a selection rendered in a given style.
pub struct Representation {
    pub kind: RepKind,
    pub params: RepParams,
    /// For an **Interactions** rep only: the partner representation it detects contacts
    /// against, keyed by the partner molecule's [`MoleculeSource`] + its rep index. The
    /// source is stable *and* serializable, so this survives undo/redo **and** a session
    /// reload for free (via [`crate::history::RepState`]); it's resolved to a live
    /// molecule at use time (a stale/missing reference → "partner lost", nothing drawn).
    /// `None` = unset. (Two molecules sharing a source is ambiguous — the first wins.)
    pub partner: Option<(MoleculeSource, usize)>,
    pub color: ColorMethod,
    /// Appearance preset (lighting + opacity); see [`crate::material::Material`].
    pub material: Material,
    /// Secondary-structure algorithm driving the Cartoon shape and the
    /// "Structure" color scheme (DSSP vanilla / PyMOL dss).
    pub ss_algo: SsAlgorithm,
    /// Editable selection text — the UI buffer / draft (egui needs a `&mut String`,
    /// and it can hold not-yet-valid input). The committed text also lives in
    /// `expr` (`SelectionExpr::get_str`) once it parses.
    pub sel_text: String,
    /// Compiled selection (molar `SelectionExpr`), parsed from `sel_text` on commit.
    /// Re-evaluated per trajectory frame later; `None` until first successful parse.
    pub expr: Option<SelectionExpr>,
    /// Evaluated atom set for the current state; bind to the `System` for coords.
    pub sel: Option<Sel>,
    /// Last selection error, shown in the UI; `None` if the selection is valid.
    pub sel_error: Option<String>,
    /// For a parse error, the byte range of the offending word within `sel_text`
    /// (from molar's structured `SyntaxError::span`, shifted past leading whitespace),
    /// so the UI highlights the whole bad word in place. `None` for non-positional
    /// errors. Transient, not in `EditState`.
    pub sel_error_span: Option<std::ops::Range<usize>>,
    /// The selection is valid but matches **zero atoms** (molar's "empty" error,
    /// surfaced as a non-destructive warning). The field is flagged in the UI and
    /// the rep renders nothing; the text is kept. Transient, not in `EditState`.
    pub sel_empty: bool,
    /// Periodic-image display (see [`PeriodicParams`]). In `EditState`.
    pub periodic: PeriodicParams,
    /// Trajectory smoothing window (odd; `1` = off). When `> 1`, the rendered
    /// coordinates are a Savitzky–Golay blend of the nearby frames, computed
    /// transiently at build time (`Trajectory::smoothed_state`). In `EditState`.
    pub smooth_window: u32,
    /// An **option** of the [`ColorMethod::Charge`] scheme: which charge it paints,
    /// partial or formal. Edited in the rep settings' `Color` tab; ignored by every other
    /// scheme. In `EditState` (undoable + saved in sessions).
    pub charge_kind: crate::color::ChargeKind,
    pub visible: bool,
    /// Re-evaluate the (compiled) selection every time the System's State changes
    /// (i.e. each trajectory frame). For coordinate-dependent selections like
    /// `within …`.
    pub dynamic: bool,
    /// For Cartoon / SecStruct reps: recompute secondary structure on every
    /// trajectory frame (else compute once and reuse `ss_cache`). DSSP is the main
    /// per-frame cost, so this defaults to off. Part of `EditState` (undoable).
    pub ss_per_frame: bool,
    /// Cached secondary structure from the last full (structural) build, reused
    /// for coordinate-only frame updates when `ss_per_frame` is off. Transient.
    pub ss_cache: Option<SsMap>,
    /// Cached CPU copy of the last-built **Cartoon** ribbon mesh (with per-vertex
    /// `vert_res` residue tags), so the selection glow can extract just the chosen
    /// residues' sub-ribbon from this *exact* geometry (coincident → no z-fight, and
    /// works for a single residue). `None` for non-cartoon reps. Transient.
    pub cartoon_cache: Option<crate::geometry::MeshData>,
    /// Transient UI state: whether this rep's inline settings panel is expanded.
    /// Not part of `EditState` (view state, not undoable).
    pub params_open: bool,
    /// Transient UI state: which tab of the settings panel is shown.
    pub settings_tab: SettingsTab,
    /// `sel_text` changed → recompile the selection.
    pub sel_dirty: bool,
    /// Selection/style/color/params changed → full geometry rebuild + buffer
    /// re-create (`renderer.upload`).
    pub geom_dirty: bool,
    /// Only coordinates changed (a trajectory frame, same selection/structure) →
    /// recompute geometry and update existing GPU buffers in place
    /// (`renderer.update`), avoiding reallocation. Ignored if `geom_dirty` is set.
    pub coords_dirty: bool,
    pub gpu: RepGpu,
}

impl Representation {
    /// This rep's color scheme together with its options — what the geometry builders
    /// need to colorize atoms (see [`crate::color::ColorSpec`]).
    pub fn color_spec(&self) -> crate::color::ColorSpec {
        crate::color::ColorSpec { method: self.color, charge_kind: self.charge_kind }
    }

    pub fn new(kind: RepKind) -> Self {
        Self::restore(
            kind,
            RepParams::for_kind(kind),
            ColorMethod::Element,
            SsAlgorithm::default(),
            "all".to_string(),
            true,
            false,
            false,
            Material::default(),
            PeriodicParams::default(),
            1, // smooth_window: off
        )
    }

    /// A copy with the same style/selection but fresh (unbuilt) GPU state, so it
    /// recompiles and uploads its own geometry on the next frame.
    pub fn duplicate(&self) -> Self {
        let mut r = Self::restore(
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
        r.charge_kind = self.charge_kind;
        r.partner = self.partner.clone();
        r
    }

    /// Build a fresh representation from the program's [`RepDefaults`] (initial rep
    /// of a loaded molecule, the "add representation" button) — applying the default
    /// style, color, material, and selection (and the default Surface quality).
    pub fn from_defaults(d: &crate::settings::RepDefaults) -> Self {
        let mut params = RepParams::for_kind(d.kind);
        if let RepParams::Surface { quality, .. } = &mut params {
            *quality = d.surface_quality;
        }
        Self::restore(
            d.kind,
            params,
            d.color,
            SsAlgorithm::default(),
            d.selection.clone(),
            true,
            false,
            false,
            d.material,
            PeriodicParams::default(),
            1,
        )
    }

    /// Reconstruct a representation from saved editable fields (used by undo/redo).
    /// Starts dirty so its selection recompiles and geometry rebuilds next frame.
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        kind: RepKind,
        params: RepParams,
        color: ColorMethod,
        ss_algo: SsAlgorithm,
        sel_text: String,
        visible: bool,
        dynamic: bool,
        ss_per_frame: bool,
        material: Material,
        periodic: PeriodicParams,
        smooth_window: u32,
    ) -> Self {
        Self {
            kind,
            params,
            partner: None,
            color,
            material,
            ss_algo,
            sel_text,
            expr: None,
            sel: None,
            sel_error: None,
            sel_error_span: None,
            sel_empty: false,
            periodic,
            smooth_window,
            // An option of the Charge scheme, defaulted here and assigned by the callers
            // that carry one (`duplicate`, `RepState::to_representation`) — `restore`'s
            // positional list is long enough already.
            charge_kind: crate::color::ChargeKind::default(),
            visible,
            dynamic,
            ss_per_frame,
            ss_cache: None,
            cartoon_cache: None,
            params_open: false,
            settings_tab: SettingsTab::default(),
            sel_dirty: true,
            geom_dirty: false,
            coords_dirty: false,
            gpu: RepGpu::default(),
        }
    }
}

/// Which tab of a representation's settings panel is shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SettingsTab {
    /// Style-specific geometry parameters.
    #[default]
    Style,
    /// Trajectory / per-frame behavior.
    Traj,
    /// Periodic-image rendering.
    Periodic,
    /// Options of the active color scheme. Only shown for schemes that have any —
    /// currently just `Charge` (partial vs formal, and computing partial charges).
    Color,
}

/// A freshly captured selection (e.g. from the lasso) that has not yet been
/// committed to a real [`Representation`]. It is **view state** (not undoable, not
/// in `EditState`): it renders as a glowing highlight over the atoms exactly as the
/// existing reps already draw them, and the panel shows it with a minimal
/// accept/discard interface instead of the normal rep controls. Accepting it
/// creates a normal Ball-and-Stick representation over [`PendingSelection::sel_text`];
/// discarding drops it. (The two-step scheme leaves room for later set operations —
/// e.g. unioning a new lasso into the active selection with Shift held.)
pub struct PendingSelection {
    /// molar selection text reproducing the captured atom set (e.g. `index 1:3 7`),
    /// used both when the selection is accepted as a representation and to rebuild
    /// the glow geometry (intersected with each rep's own selection / style).
    pub sel_text: String,
    /// Captured atoms' global indices (sorted ascending). The glow geometry is built
    /// per visible rep as (rep selection ∩ these atoms) in that rep's style.
    pub atoms: Vec<usize>,
}

/// The hover detail "lens": the atoms within `radius` of the cursor view-line
/// (`ray_o`, `ray_d` in world space), shown as a faded ball-and-stick aid over a
/// Cartoon/Surface rep. The geometry is rebuilt from this when the ray moves.
pub struct HoverDetail {
    pub atoms: Vec<usize>,
    pub ray_o: Vec3,
    pub ray_d: Vec3,
    pub radius: f32,
}

/// A loaded molecule. The live molar `System` is the single source of per-atom
/// data (positions, elements, radii); we additionally keep only the guessed
/// connectivity and a cached bounding box, plus the representations.
pub struct Molecule {
    pub id: MolId,
    pub name: String,
    /// Where the structure was loaded from (for saving/reloading sessions). Only
    /// read by the (native) session capture, hence allowed-dead on wasm.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub source: MoleculeSource,
    /// Trajectory files loaded into this molecule, in load order, so a session can
    /// replay them. Appended whenever frames are loaded from a file.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub traj_loads: Vec<TrajLoad>,
    /// The molecule's topology + coordinates backend: an owned molar `System`, or
    /// (later) a shared external source (pymolar). See [`MolData`]. Kept as a
    /// directly-borrowable field so rebuild loops can read it while holding
    /// `&mut reps`.
    pub data: MolData,
    /// Connectivity used for rendering/picking and the editor. Each [`Bond`] carries
    /// its endpoints + chemical order (molar's type; guessed/file bonds without a
    /// recorded order are `Unspecified`). Mutated only via the helper methods so it
    /// stays consistent.
    pub bonds: Vec<Bond>,
    /// Monotonic counter bumped by every structural mutation (atom/bond add/remove,
    /// element/order change, `rotate_fragment`). Undo capture uses it to snapshot a
    /// molecule's structure only when it actually changed — see
    /// [`structure_snapshot`](Self::structure_snapshot) + [`crate::history`]. Transient.
    pub structure_version: u64,
    /// Cache for [`structure_snapshot`](Self::structure_snapshot): the last built
    /// `Arc<StructureSnapshot>` plus the `structure_version` it was built at, so a
    /// re-capture at the same version is a cheap `Arc` clone (interior mutability, since
    /// undo capture borrows the scene immutably). Transient.
    structure_cache: RefCell<Option<(u64, Arc<StructureSnapshot>)>>,
    pub n_atoms: usize,
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
    pub visible: bool,
    pub reps: Vec<Representation>,
    /// If this molecule is a member of a [`MolGroup`], its group id (else `None`).
    /// The flat scene treats grouped molecules like any other; only the panel and
    /// the group machinery consult this. Transient runtime state (groups are
    /// reconstructed on undo/session load), not serialized on the molecule.
    pub group: Option<GroupId>,
    /// Number of leading `reps` that are the group's **shared** reps, materialized
    /// onto this member because it is the one currently shown (`reps[0..n_shared]` =
    /// shared mirror, `reps[n_shared..]` = this member's own). `0` for a non-shown
    /// member or a non-grouped molecule. See [`MolGroup`].
    pub n_shared: usize,
    pub selected_rep: Option<usize>,
    /// Aromatic rings (atom-index loops) from the last [`Molecule::perceive_aromaticity`],
    /// for the in-ring aromatic-circle overlay in the drawing editor. Transient.
    pub aromatic_rings: Vec<Vec<usize>>,
    /// Aromatic rings (atom-index loops) for **interaction detection** (π-stacking /
    /// π-cation), computed lazily via `ensure_interaction_rings` (molar ring perception
    /// on a topology clone — no side effects). Topology-derived, so stable across
    /// trajectory frames (only the centroids move); `None` until first needed. Transient.
    pub interaction_rings: Option<Vec<Vec<usize>>>,
    /// Transient UI state: whether this molecule's representations block is
    /// expanded in the panel. Not part of `EditState` (view state, not undoable).
    pub reps_open: bool,
    /// Loaded MD frames + playback state. Empty until a trajectory is loaded
    /// (then frame 0 is the structure coords; see [`Molecule::seed_frame0`]).
    /// Not part of `EditState` — frame/playback is view state, like the camera.
    pub trajectory: Trajectory,
    /// Show the periodic box as a wireframe overlay (transient view toggle).
    pub show_box: bool,
    /// GPU buffer for the box wireframe (lines only); rebuilt when `box_dirty`.
    pub box_gpu: RepGpu,
    /// Box geometry needs (re)building — toggled on, or coordinates changed.
    pub box_dirty: bool,
    /// GPU line buffer for the aromatic-ring circles (drawn depth-tested in the scene,
    /// so they occlude correctly); built from `aromatic_rings` when `aromatic_dirty`.
    pub aromatic_gpu: RepGpu,
    /// Aromatic-circle geometry needs (re)building — perception ran or coords moved.
    pub aromatic_dirty: bool,
    /// A not-yet-committed selection (e.g. captured by a lasso), shown as a glowing
    /// highlight with a minimal accept/discard UI. View state, not undoable; see
    /// [`PendingSelection`]. `None` when there is no active selection.
    pub pending: Option<PendingSelection>,
    /// GPU geometry for the active-selection glow: the pending atoms rebuilt in each
    /// rep's own style (so the highlight glows in the current style). Empty when
    /// there's no pending selection.
    pub glow_gpu: RepGpu,
    /// The glow geometry needs (re)building — pending changed, or its coords moved.
    pub glow_dirty: bool,
    /// Transient hover highlight (Residues hover-pick mode): the hovered residue's
    /// atoms, glowing in the current style like a pending selection but **steady**
    /// (no pulse) and with no accept/discard UI. Recomputed as the cursor moves;
    /// not undoable, not in `EditState`. `None` when nothing is hovered.
    pub hover: Option<Vec<usize>>,
    /// GPU geometry for the steady hover highlight (built from `hover`).
    pub hover_gpu: RepGpu,
    /// The hover-highlight geometry needs (re)building — `hover` set changed.
    pub hover_dirty: bool,
    /// Hover **detail lens** (when hovering a Cartoon/Surface rep, where atoms are
    /// hidden): the atoms within a radius of the cursor view-line, shown as a
    /// distance-faded ball-and-stick aid. `None` when inactive.
    pub hover_detail: Option<HoverDetail>,
    /// GPU geometry for the detail lens (faded CPK ball-and-stick from `hover_detail`).
    pub hover_detail_gpu: RepGpu,
    pub hover_detail_dirty: bool,
    /// Lazily-built spatial grid of this molecule's atoms (over the displayed frame),
    /// for the lens's ray-neighborhood query. Invalidated (`None`) on a frame change.
    pub hover_grid: Option<crate::spatial::AtomGrid>,
    /// Last `MolData::coords_version` the viewer rendered, for a **shared** molecule —
    /// when the external (pymolar) coordinates change, this differs from the source's
    /// current version and the render loop re-reads the coords (see
    /// `App::mark_shared_dirty`). Always 0 / unused for an owned molecule.
    pub shared_coords_version: u64,
    /// GPU pick geometry: one id-stamped sphere impostor per **pickable** atom (the
    /// atoms CPU `pick` ray-casts: eligible atoms of visible reps, at their displayed
    /// position and effective radius). Rendered into the id-buffer for GPU picking.
    /// Native only (GPU picking needs a synchronous readback wasm can't do).
    #[cfg(not(target_arch = "wasm32"))]
    pub pick_gpu: RepGpu,
    /// The pick geometry needs (re)building — geometry/coords/visibility changed.
    #[cfg(not(target_arch = "wasm32"))]
    pub pick_dirty: bool,
}

impl Molecule {
    pub fn new(id: MolId, raw: RawMolecule, rep_defaults: &crate::settings::RepDefaults) -> Self {
        Self::from_parts(
            id,
            raw.name,
            raw.source,
            MolData::Owned(raw.system),
            raw.bonds,
            raw.n_atoms,
            raw.bbox_min,
            raw.bbox_max,
            rep_defaults,
        )
    }

    /// Build a molecule that renders from a **shared external source** (pymolar),
    /// zero-copy. Bonds + bounding box are guessed from the source's current
    /// topology/state (the source then keeps providing live coordinates by reference).
    pub fn new_shared(
        id: MolId,
        name: String,
        source: Box<dyn crate::moldata::SharedSource>,
        bond_params: &crate::data::bonds::BondParams,
        rep_defaults: &crate::settings::RepDefaults,
    ) -> Result<Self, String> {
        let (bonds, bbox_min, bbox_max, n) = {
            let topo = source.topology();
            let state = source.state();
            let n = topo.len();
            if n == 0 {
                return Err("cannot add an empty molecule".to_string());
            }
            let all = Sel::from_vec((0..n).collect()).map_err(|e| e.to_string())?;
            let bound = all.bind_to(topo, state);
            let (min, max) = bound.min_max();
            let mut positions = Vec::with_capacity(n);
            let mut vdw = Vec::with_capacity(n);
            for (pos, atom) in bound.iter_pos().zip(bound.iter_atoms()) {
                positions.push([pos.x, pos.y, pos.z]);
                vdw.push(atom.vdw());
            }
            let bonds = crate::data::bonds::resolve(
                &topo.bonds,
                &bound,
                &positions,
                &vdw,
                state.pbox.as_ref(),
                bond_params,
            );
            (
                bonds,
                Vec3::new(min.x, min.y, min.z),
                Vec3::new(max.x, max.y, max.z),
                n,
            )
        };
        Ok(Self::from_parts(
            id,
            name.clone(),
            MoleculeSource::Bytes { name },
            MolData::Shared(source),
            bonds,
            n,
            bbox_min,
            bbox_max,
            rep_defaults,
        ))
    }

    /// Shared field initialization for [`new`](Self::new)/[`new_shared`](Self::new_shared).
    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        id: MolId,
        name: String,
        source: MoleculeSource,
        data: MolData,
        bonds: Vec<Bond>,
        n_atoms: usize,
        bbox_min: Vec3,
        bbox_max: Vec3,
        rep_defaults: &crate::settings::RepDefaults,
    ) -> Self {
        let mut mol = Self {
            id,
            name,
            source,
            traj_loads: Vec::new(),
            data,
            bonds,
            structure_version: 0,
            structure_cache: RefCell::new(None),
            n_atoms,
            bbox_min,
            bbox_max,
            visible: true,
            reps: vec![Representation::from_defaults(rep_defaults)],
            group: None,
            n_shared: 0,
            selected_rep: Some(0),
            aromatic_rings: Vec::new(),
            interaction_rings: None,
            reps_open: true,
            trajectory: Trajectory::default(),
            show_box: false,
            box_gpu: RepGpu::default(),
            aromatic_gpu: RepGpu::default(),
            aromatic_dirty: false,
            // Build the box geometry up front (if the molecule has one) so a rep's
            // periodic `Box` toggle can draw it without the molecule-level box ever
            // being shown. Cheap (24 verts); a no-op when there's no box.
            box_dirty: true,
            pending: None,
            glow_gpu: RepGpu::default(),
            glow_dirty: false,
            hover: None,
            hover_gpu: RepGpu::default(),
            hover_dirty: false,
            hover_detail: None,
            hover_detail_gpu: RepGpu::default(),
            hover_detail_dirty: false,
            hover_grid: None,
            shared_coords_version: 0,
            #[cfg(not(target_arch = "wasm32"))]
            pick_gpu: RepGpu::default(),
            #[cfg(not(target_arch = "wasm32"))]
            pick_dirty: true,
        };
        // The resolved connectivity is generally not the structure file's own table, so
        // hand it to the topology that molar's bond-reading machinery consults.
        mol.sync_bonds_to_topology();
        mol
    }

    /// Republish `self.bonds` into the owned `System`'s topology.
    ///
    /// The viewer's connectivity is resolved at load ([`crate::data::bonds::resolve`]:
    /// what the file recorded ∪ what distances imply) and then edited by the structure
    /// editor, so it is generally *not* what the structure file's own bond table holds.
    /// Everything on molar's side that reads bonds — the `polh` / `apolh` selection
    /// keywords, [`perceive`], `molar_ff`'s force-field typing and espaloma charges —
    /// reads the topology, so it has to be told.
    ///
    /// Called at construction and after every bond edit; the flat `Vec<Bond>` stays the
    /// authority (it is also the only bond store a *shared* molecule has, since we don't
    /// own that topology — hence the no-op there).
    pub(crate) fn sync_bonds_to_topology(&mut self) {
        let bonds = bond_storage(&self.bonds);
        if let Some(sys) = self.data.system_mut() {
            // Only an index out of range can fail, and `self.bonds` is maintained in
            // range by the mutators below; a stale bond would just be dropped silently.
            if let Err(e) = sys.set_bonds(bonds) {
                log::warn!("could not publish bonds to the topology: {e}");
            }
        }
    }

    /// Capture the molecule's current structure coordinates as trajectory frame 0,
    /// if no frames are loaded yet. `System` has no state getter, so we use the
    /// `set_state` swap trick: swap in a same-length placeholder to take ownership
    /// of the real state, clone it, and swap the real state back.
    pub fn seed_frame0(&mut self) {
        if !self.trajectory.frames.is_empty() {
            return;
        }
        let placeholder = State::new_fake(self.n_atoms);
        if let Ok(real) = self.data.set_state(placeholder) {
            self.trajectory.frames.push(real.clone());
            let _ = self.data.set_state(real); // restore the live state
        }
    }

    /// Append loaded frames to the trajectory (sync load).
    pub fn append_frames(&mut self, frames: Vec<State>) {
        self.trajectory.frames.extend(frames);
    }

    /// Append one streamed frame (async load).
    pub fn push_frame(&mut self, frame: State) {
        self.trajectory.frames.push(frame);
    }

    /// Mark representations dirty for the current trajectory frame. The frame's
    /// coordinates are read **by reference** at rebuild time (`bind_with_state`),
    /// so the per-frame state is NOT copied into the `System` — except for
    /// molecules with `dynamic` reps, whose selections are re-evaluated against
    /// the system's own state, so those (rare) get the frame copied in.
    ///
    /// Routing per rep:
    /// - `dynamic` → `sel_dirty` (re-evaluate selection, full rebuild);
    /// - Cartoon/SecStruct with `ss_per_frame` → `geom_dirty` (SS may restructure);
    /// - otherwise → `coords_dirty` (coords only → incremental in-place GPU update,
    ///   reusing the cached secondary structure — no DSSP).
    pub fn apply_current_frame(&mut self) {
        if self.trajectory.frames.get(self.trajectory.current).is_none() {
            return;
        }
        self.box_dirty = true; // the box can change per frame (e.g. NPT)
        self.hover_grid = None; // positions changed → the lens grid is stale
        if self.pending.is_some() {
            self.glow_dirty = true; // the glow follows the atoms' new positions
        }
        let needs_eval = self.reps.iter().any(|r| r.dynamic);
        if needs_eval {
            if let Some(frame) = self.trajectory.frames.get(self.trajectory.current) {
                let _ = self.data.set_state(frame.clone());
            }
        }
        for rep in &mut self.reps {
            if rep.dynamic {
                rep.sel_dirty = true;
            } else if matches!(rep.kind, RepKind::Surface)
                || (rep.ss_per_frame && crate::geometry::needs_ss(&rep.params, rep.color))
            {
                // The surface mesh is rebuilt from scratch each frame (its topology
                // changes with the coordinates), so it can't use the in-place
                // coords-only GPU update.
                rep.geom_dirty = true;
            } else {
                rep.coords_dirty = true;
            }
        }
    }

    /// The state currently displayed: the active trajectory frame, or the static
    /// structure state when no trajectory is loaded.
    pub fn render_state(&self) -> &State {
        self.trajectory
            .frames
            .get(self.trajectory.current)
            .unwrap_or_else(|| self.data.state())
    }

    /// Bounding box (nm) of selection `sel` at the currently displayed frame.
    pub fn sel_bbox(&self, sel: &Sel) -> (Vec3, Vec3) {
        let (min, max) = self.data.bind_with_state(sel, self.render_state()).min_max();
        (Vec3::new(min.x, min.y, min.z), Vec3::new(max.x, max.y, max.z))
    }

    /// Bounding box (nm) of the whole molecule at the currently displayed frame.
    pub fn current_bbox(&self) -> (Vec3, Vec3) {
        self.sel_bbox(&self.data.select_all())
    }

    /// The molecule's structure snapshot (atoms + coords + bonds) for undo, as a
    /// shared `Arc`, (re)built **only** when [`structure_version`](Self::structure_version)
    /// has changed since the last call — so an unchanged molecule costs an `Arc` clone,
    /// not a full copy, at every undo checkpoint. Returns `None` for a **shared**
    /// (pymolar/JS) molecule: its coordinates live outside the viewer and can't be
    /// structure-edited or restored, so it has no undoable structure.
    pub fn structure_snapshot(&self) -> Option<Arc<StructureSnapshot>> {
        if self.data.is_shared() {
            return None;
        }
        let mut cache = self.structure_cache.borrow_mut();
        if let Some((v, arc)) = cache.as_ref() {
            if *v == self.structure_version {
                return Some(arc.clone());
            }
        }
        let arc = Arc::new(StructureSnapshot::capture(self));
        *cache = Some((self.structure_version, arc.clone()));
        Some(arc)
    }

    /// Adopt `snap` as this molecule's current structure snapshot after the caller has
    /// rebuilt the molecule to match it (undo/redo of a structural change). Bumps the
    /// version and seeds the cache with **this exact `Arc`**, so the next
    /// [`structure_snapshot`](Self::structure_snapshot) returns it *by identity* — the
    /// undo diff (`Arc::ptr_eq`) then sees no change and doesn't record a spurious step.
    pub fn adopt_structure_snapshot(&mut self, snap: Arc<StructureSnapshot>) {
        self.structure_version = self.structure_version.wrapping_add(1);
        *self.structure_cache.borrow_mut() = Some((self.structure_version, snap));
    }

    /// Recompute the cached bounding box from the live structure (guards the
    /// 0-atom case, where molar's `min_max` would panic).
    pub fn refresh_bbox(&mut self) {
        if self.n_atoms == 0 {
            return;
        }
        let (min, max) = self.current_bbox();
        self.bbox_min = min;
        self.bbox_max = max;
    }

    /// Flag the molecule's derived render state stale after a **coordinate-only** change
    /// (frame swap, dihedral twist, relax, live shared-source edit): every rep gets an
    /// in-place GPU coord update, and any pending-selection glow / periodic box / (native)
    /// GPU pick buffer follows. `pick` gates the native pick-buffer rebuild — some callers
    /// (e.g. shared-source polling) don't drive picking, so they pass `false`.
    pub fn mark_coords_dirty(&mut self, pick: bool) {
        for rep in &mut self.reps {
            rep.coords_dirty = true;
        }
        if self.pending.is_some() {
            self.glow_dirty = true;
        }
        if self.show_box {
            self.box_dirty = true;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if pick {
            self.pick_dirty = true;
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = pick;
        }
    }

    // --- Structure editing (drawing tool) ----------------------------------
    // These are the single source of bond/atom mutation; they keep `bonds` and
    // `bond_orders` index-aligned and the same length. The molar `System` is the
    // source of truth for per-atom data, so atoms go in/out through it.

    /// Append a new atom (already built by the caller, e.g. from the element
    /// palette) at world position `pos` (nm). Returns its global index, or `None`
    /// if molar rejected the append. Does **not** touch bonds.
    pub fn add_atom(&mut self, atom: &Atom, pos: Vec3) -> Option<usize> {
        let p = Pos::new(pos.x, pos.y, pos.z);
        match self.data.append_atom(atom, &p) {
            Ok(_) => {
                let idx = self.n_atoms;
                self.n_atoms += 1;
                self.bbox_min = self.bbox_min.min(pos);
                self.bbox_max = self.bbox_max.max(pos);
                self.hover_grid = None;
                self.structure_version += 1;
                Some(idx)
            }
            Err(_) => None,
        }
    }

    /// Index of the bond between `i` and `j` (either direction), if any.
    pub fn bond_between(&self, i: usize, j: usize) -> Option<usize> {
        self.bonds
            .iter()
            .position(|b| (b.i1 == i && b.i2 == j) || (b.i1 == j && b.i2 == i))
    }

    /// Add a bond `i–j` of the given order. No-op (returns `false`) for a self-bond,
    /// an out-of-range endpoint, or a duplicate of an existing bond.
    pub fn add_bond(&mut self, i: usize, j: usize, order: BondOrder) -> bool {
        if i == j || i >= self.n_atoms || j >= self.n_atoms || self.bond_between(i, j).is_some() {
            return false;
        }
        self.bonds.push(Bond::with_order(i, j, order));
        self.structure_version += 1;
        self.sync_bonds_to_topology();
        true
    }

    /// Remove the bond at index `k`.
    pub fn remove_bond_at(&mut self, k: usize) {
        if k < self.bonds.len() {
            self.bonds.remove(k);
            self.structure_version += 1;
            self.sync_bonds_to_topology();
        }
    }

    /// Bond `i–j` at the given order: override an existing bond's order, else add a new
    /// bond. Returns `true` if anything changed.
    pub fn set_or_add_bond(&mut self, i: usize, j: usize, order: BondOrder) -> bool {
        match self.bond_between(i, j) {
            Some(k) => {
                if self.bonds[k].order != order {
                    self.bonds[k].order = order;
                    self.structure_version += 1;
                    self.sync_bonds_to_topology();
                    return true;
                }
                false
            }
            None => self.add_bond(i, j, order),
        }
    }

    /// Replace atom `i`'s element in place (name / atomic number / mass), preserving
    /// its residue identity, coordinates, and bonds. `src` is a freshly built atom of
    /// the target element (e.g. from the palette).
    pub fn set_atom_element(&mut self, i: usize, src: &Atom) {
        if i >= self.n_atoms {
            return;
        }
        let mut bound = self.data.select_all_bound_mut();
        if let Some(mut a) = bound.get_atom_mut(i) {
            a.set_name(src.get_name());
            a.set_atomic_number(src.get_atomic_number());
            a.set_mass(src.get_mass());
        }
        self.hover_grid = None;
        self.structure_version += 1;
    }

    /// Rotate the atoms `atoms` (global indices) about the world-space line through
    /// point `c` with unit direction `u`, by `angle` radians — the dihedral-rotation
    /// tool's rigid fragment rotation. Mutates the **displayed** coordinates: the
    /// current trajectory frame if one is loaded, else the owned `System`'s state
    /// (a shared external molecule without frames is left untouched). Atoms lying on
    /// the axis line (the bond endpoints) don't move, so the axis stays fixed.
    pub fn rotate_fragment(&mut self, atoms: &[usize], c: Vec3, u: Vec3, angle: f32) {
        let u = u.normalize_or_zero();
        if u == Vec3::ZERO || angle == 0.0 {
            return;
        }
        let q = glam::Quat::from_axis_angle(u, angle);
        let rot = |p: &mut Pos| {
            let v = Vec3::new(p.x, p.y, p.z);
            let nv = c + q * (v - c);
            *p = Pos::new(nv.x, nv.y, nv.z);
        };
        if let Some(frame) = self.trajectory.frames.get_mut(self.trajectory.current) {
            for &a in atoms {
                if let Some(p) = frame.coords.get_mut(a) {
                    rot(p);
                }
            }
        } else if let Some(sys) = self.data.system_mut() {
            let mut bound = sys.select_all_bound_mut();
            for &a in atoms {
                if let Some(p) = bound.get_pos_mut(a) {
                    rot(p);
                }
            }
        }
        self.hover_grid = None;
        self.structure_version += 1;
    }

    /// The coordinate store a structural edit writes to *right now*: a specific trajectory
    /// frame (`Some(index)`) if one is loaded, else the owned `System` (`None`). Captured at
    /// edit time and carried on a [`StructEdit::Coords`](crate::history::StructEdit::Coords)
    /// so its undo/redo targets the **same** store even after the displayed frame or the
    /// trajectory itself has changed.
    pub fn coord_edit_target(&self) -> Option<usize> {
        (!self.trajectory.frames.is_empty()).then_some(self.trajectory.current)
    }

    /// Overwrite the positions of `atoms` (global indices) with `coords` (parallel, nm) in
    /// the coordinate store `frame` designates — a specific trajectory frame (`Some`) or the
    /// owned `System` (`None`), as captured by [`coord_edit_target`](Self::coord_edit_target)
    /// when the edit was recorded. Used by undo/redo of a coordinate delta
    /// ([`crate::history::StructEdit::Coords`]); a no-op if the target frame is gone or the
    /// molecule is shared.
    pub fn set_coords(&mut self, atoms: &[usize], coords: &[[f32; 3]], frame: Option<usize>) {
        match frame {
            Some(f) => {
                if let Some(fr) = self.trajectory.frames.get_mut(f) {
                    for (&a, c) in atoms.iter().zip(coords) {
                        if let Some(p) = fr.coords.get_mut(a) {
                            *p = Pos::new(c[0], c[1], c[2]);
                        }
                    }
                }
            }
            None => {
                if let Some(sys) = self.data.system_mut() {
                    let mut bound = sys.select_all_bound_mut();
                    for (&a, c) in atoms.iter().zip(coords) {
                        if let Some(p) = bound.get_pos_mut(a) {
                            *p = Pos::new(c[0], c[1], c[2]);
                        }
                    }
                }
            }
        }
        self.structure_version += 1;
        self.hover_grid = None;
    }

    /// Write per-atom partial charges (by global atom index) into the owned `System`.
    /// Used by the espaloma assignment and its undo/redo. Charges are per-atom topology
    /// data, not per-frame, so there is no coordinate store to choose.
    pub fn set_charges(&mut self, atoms: &[usize], charges: &[f32]) {
        if let Some(sys) = self.data.system_mut() {
            let mut bound = sys.select_all_bound_mut();
            for (&a, &q) in atoms.iter().zip(charges) {
                if let Some(mut at) = bound.get_atom_mut(a) {
                    at.set_charge(q);
                }
            }
        }
        // Colors are baked into the geometry, so a charge change needs a rebuild — not the
        // cheaper coords-only path, which reuses the existing per-vertex colors.
        for rep in &mut self.reps {
            rep.geom_dirty = true;
        }
    }

    /// Cycle the order of bond `k` (single→double→triple→single).
    pub fn cycle_bond_order(&mut self, k: usize) {
        use crate::minimize::BondOrderExt;
        if let Some(b) = self.bonds.get_mut(k) {
            b.order = b.order.cycle();
            self.structure_version += 1;
        }
    }

    /// Remove atom `i` and every bond incident to it, re-indexing the surviving
    /// bonds (endpoints `> i` shift down by one, mirroring molar's atom re-index).
    /// Returns `true` if the molecule is now **empty** (the caller should delete it).
    pub fn remove_atom(&mut self, i: usize) -> bool {
        if i >= self.n_atoms {
            return self.n_atoms == 0;
        }
        let shift = |x: usize| if x > i { x - 1 } else { x };
        self.bonds = self
            .bonds
            .iter()
            .filter(|b| !b.contains(i)) // drop incident bonds
            .map(|b| Bond::with_order(shift(b.i1), shift(b.i2), b.order))
            .collect();
        let _ = self.data.remove(std::iter::once(i));
        self.n_atoms = self.n_atoms.saturating_sub(1);
        self.hover_grid = None;
        self.structure_version += 1;
        // The atom columns shrank and the surviving bonds were renumbered above.
        self.sync_bonds_to_topology();
        self.refresh_bbox();
        self.n_atoms == 0
    }

    /// Remove several atoms (and their bonds). Returns `true` if the molecule is now
    /// empty. Removes in descending index order so earlier indices stay valid.
    pub fn remove_atoms(&mut self, indices: &[usize]) -> bool {
        let mut idx: Vec<usize> = indices.to_vec();
        idx.sort_unstable();
        idx.dedup();
        for &i in idx.iter().rev() {
            self.remove_atom(i);
        }
        self.n_atoms == 0
    }

    /// Every atom reachable from `seeds` through the bond graph — i.e. the **complete
    /// molecules** the seeds belong to.
    ///
    /// Needed wherever a per-molecule property is computed from a selection: partial charges
    /// are equilibrated over a whole connected graph, so `molar_ff` rejects a selection that
    /// cuts a bond. A selection like `not apolh` (a perfectly ordinary *viewing* selection,
    /// and what the docking loader sets) cuts every C–H, so the closure is what turns "these
    /// atoms" into "the molecules these atoms are part of".
    ///
    /// Returns sorted, de-duplicated global indices; a seed with no bonds comes back alone.
    pub fn connected_closure(&self, seeds: &[usize]) -> Vec<usize> {
        let adj = BondAdjacency::build(self.n_atoms, bond_storage(&self.bonds).iter_pairs());
        let mut seen = vec![false; self.n_atoms];
        let mut stack: Vec<usize> = Vec::new();
        for &s in seeds {
            if s < self.n_atoms && !seen[s] {
                seen[s] = true;
                stack.push(s);
            }
        }
        let mut out = Vec::with_capacity(seeds.len());
        while let Some(a) = stack.pop() {
            out.push(a);
            for nb in adj.neighbors(a) {
                let n = nb.atom();
                if n < self.n_atoms && !seen[n] {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }
        out.sort_unstable();
        out
    }

    // --- Molecular perception bridge ---------------------------------------
    // molar's perception routines all take a prebuilt `BondAdjacency`, and the owned
    // `System`'s topology already carries our resolved connectivity (see
    // `sync_bonds_to_topology`), so these run directly on it — no copy of the atoms.
    // A *shared* molecule's topology isn't ours to index, so it falls back to a
    // throwaway topology carrying its atoms and our bonds.

    /// Run `f` over a topology holding this molecule's atoms and its resolved bond graph,
    /// plus the matching bond adjacency.
    ///
    /// For an owned molecule that topology *is* the `System`'s — it already carries the
    /// resolved connectivity (see [`Molecule::sync_bonds_to_topology`]) — so nothing is
    /// copied. Only a shared molecule needs a scratch clone, since we can't publish bonds
    /// into a topology we don't own.
    ///
    /// The adjacency is built from whichever bond table `f` will read, so the bond indices
    /// it hands out always index that same table.
    fn with_perception_topology<R>(&self, f: impl FnOnce(&Topology, &BondAdjacency) -> R) -> R {
        if let Some(sys) = self.data.system() {
            let top = sys.topology();
            let adj = BondAdjacency::build(top.atoms.len(), top.bonds.iter_pairs());
            return f(top, &adj);
        }
        let mut top = self.data.topology().clone();
        top.bonds = bond_storage(&self.bonds);
        top.bonds.ensure_adjacency(top.atoms.len());
        let adj = top.bonds.get_adjacency().unwrap();
        f(&top, adj)
    }

    /// Perceive rings + aromaticity: write the perceived aromatic orders back into
    /// `self.bonds` and cache the aromatic rings (atom-index loops) for the ring-circle
    /// overlay. Coordinate-free; cheap for editor-scale molecules.
    pub fn perceive_aromaticity(&mut self) {
        // This one *does* aromatize, and the editor wants that (the orders it writes back
        // drive the double/triple-bond rendering), so it runs on a scratch topology and the
        // result is copied into the flat graph — `perceive` only rewrites the order column,
        // so the bond indices still line up.
        let mut top = self.data.topology().clone();
        top.bonds = bond_storage(&self.bonds);
        let perc = perceive(&mut top);
        for (b, r) in self.bonds.iter_mut().zip(top.bonds.iter()) {
            b.order = r.order();
        }
        self.aromatic_rings = perc.aromatic_rings().cloned().collect();
        self.aromatic_dirty = true; // the ring-circle geometry must rebuild
        self.sync_bonds_to_topology(); // the orders changed
    }

    /// Implicit-hydrogen count per atom, over the editor's connectivity.
    pub fn implicit_hydrogens(&self) -> Vec<u8> {
        self.with_perception_topology(|top, adj| implicit_hydrogens(top, adj))
    }

    /// Lazily compute + cache the molecule's aromatic rings (atom-index loops) for
    /// interaction detection. Uses molar's **non-mutating** `aromatic_rings`, so unlike
    /// `perceive_aromaticity` it leaves the bond orders alone — which matters because an
    /// aromatized bond can no longer be charged (espaloma needs a Kekulé structure). Rings
    /// are topology-derived, so this runs once and is reused across trajectory frames.
    pub fn ensure_interaction_rings(&mut self) {
        if self.interaction_rings.is_some() {
            return;
        }
        let rings = self.with_perception_topology(|top, adj| aromatic_rings(top, adj));
        self.interaction_rings = Some(rings);
    }
}

/// Outcome of a failed [`evaluate`]. molar treats a selection that matches zero
/// atoms as an error, but the GUI distinguishes it from a real (syntax) error: an
/// empty match is a non-destructive *warning* (the text stays, the field is flagged)
/// while an invalid selection keeps the prior geometry and shows the message.
#[derive(Debug)]
pub enum EvalError {
    /// Valid syntax, but the selection matched no atoms.
    Empty,
    /// Syntax (or other) error: a concise message to surface, plus — for a parse
    /// error — the byte range of the offending word in the (trimmed) selection text,
    /// so the UI can highlight the whole bad word. `None` for non-positional errors.
    Invalid {
        message: String,
        span: Option<std::ops::Range<usize>>,
    },
}

/// Parse a VMD-like selection string into a compiled `SelectionExpr` and evaluate
/// it against `system` to produce the current `Sel`. Returns both so the caller
/// can keep the compiled expression (for per-frame re-evaluation in trajectories)
/// alongside the evaluated index set. `Err(Empty)` = valid but zero atoms;
/// `Err(Invalid)` = a syntax/other error.
pub fn evaluate(system: &System, text: &str) -> Result<(SelectionExpr, Sel), EvalError> {
    let expr = SelectionExpr::new(text).map_err(|e| match e {
        // Structured parse error: build a concise message + keep the offending-word
        // span (relative to the trimmed text; the caller shifts it past any leading
        // whitespace to align with the field).
        SelectionParserError::SyntaxError(info) => EvalError::Invalid {
            message: crate::suggest::concise_message(&info),
            span: Some(info.span),
        },
        other => EvalError::Invalid { message: other.to_string(), span: None },
    })?;
    match system.select(&expr) {
        Ok(sel) => Ok((expr, sel)),
        Err(e) if is_empty_selection(&e) => Err(EvalError::Empty),
        Err(e) => Err(EvalError::Invalid { message: e.to_string(), span: None }),
    }
}

/// Whether a `SelectionError` just means "matched nothing" (vs a real error) — the
/// family of `Empty*` variants molar raises for a valid expression with no results.
fn is_empty_selection(e: &SelectionError) -> bool {
    matches!(
        e,
        SelectionError::EmptyExpr(_)
            | SelectionError::EmptySlice
            | SelectionError::EmptyRange
            | SelectionError::EmptySplit
            | SelectionError::EmptyIntersection
            | SelectionError::EmptyDifference
            | SelectionError::EmptyInvert
    )
}

#[derive(Default)]
pub struct Scene {
    pub molecules: Vec<Molecule>,
    /// Molecular groups (multi-molecule SDF). Members are ordinary entries in
    /// `molecules` tagged with [`Molecule::group`]; this is the grouping layer.
    pub groups: Vec<MolGroup>,
    pub selected_mol: Option<usize>,
    /// Molecules removed from the document but retained so a delete can be undone.
    pub trash: HashMap<MolId, Molecule>,
    /// Groups removed from the document but retained so undo/redo can restore them
    /// (mirrors [`trash`]; the metadata is tiny — members live in `trash`).
    pub group_trash: HashMap<GroupId, MolGroup>,
    next_id: u64,
    next_group_id: u64,
}

impl Scene {
    /// Load a molecule into the scene, assigning it a fresh [`MolId`].
    pub fn add(&mut self, raw: RawMolecule, rep_defaults: &crate::settings::RepDefaults) -> MolId {
        let id = MolId(self.next_id);
        self.next_id += 1;
        self.molecules.push(Molecule::new(id, raw, rep_defaults));
        id
    }

    /// Add a molecule backed by a **shared external source** (pymolar), rendered
    /// zero-copy. Returns the new molecule's `MolId`, or an error if the source is
    /// empty / its selection machinery rejects it.
    pub fn add_shared(
        &mut self,
        name: String,
        source: Box<dyn crate::moldata::SharedSource>,
        bond_params: &crate::data::bonds::BondParams,
        rep_defaults: &crate::settings::RepDefaults,
    ) -> Result<MolId, String> {
        let id = MolId(self.next_id);
        let mol = Molecule::new_shared(id, name, source, bond_params, rep_defaults)?;
        self.next_id += 1;
        self.molecules.push(mol);
        Ok(id)
    }

    /// Allocate the next fresh [`GroupId`].
    pub fn alloc_group_id(&mut self) -> GroupId {
        let id = GroupId(self.next_group_id);
        self.next_group_id += 1;
        id
    }

    /// Build a [`MolGroup`] from the records of a multi-molecule file: each record
    /// becomes a **hidden** member molecule, and the group gets one default
    /// **Licorice** shared rep materialized onto the first (shown) member. Returns
    /// the first member's id (for camera framing), or `None` if `records` is empty.
    /// The scene-side half of `App::add_group`, shared with the startup path.
    pub fn add_group(
        &mut self,
        records: Vec<RawMolecule>,
        source: MoleculeSource,
        name: String,
        rep_defaults: &crate::settings::RepDefaults,
    ) -> Option<MolId> {
        if records.is_empty() {
            return None;
        }
        let gid = self.alloc_group_id();
        let mut members = Vec::with_capacity(records.len());
        for raw in records {
            let id = self.add(raw, rep_defaults);
            let mi = self.mol_index(id).unwrap();
            let mol = &mut self.molecules[mi];
            // Members carry no own reps; the group's shared rep is what shows.
            mol.reps.clear();
            mol.selected_rep = None;
            mol.group = Some(gid);
            mol.visible = false;
            mol.n_shared = 0;
            // Collapse each member's own-rep block by default so an expanded group
            // shows a compact list of member names (a 20-record SDF would otherwise be
            // a wall of "(no own representations)").
            mol.reps_open = false;
            members.push(id);
        }
        // Default shared rep = Licorice (SDF records are small organics/ligands).
        let mut licorice = rep_defaults.clone();
        licorice.kind = RepKind::Licorice;
        let shared = Representation::from_defaults(&licorice);
        let first = members[0];
        if let Some(mi) = self.mol_index(first) {
            let mol = &mut self.molecules[mi];
            mol.reps = vec![shared];
            mol.n_shared = 1;
            mol.selected_rep = Some(0);
        }
        self.groups.push(MolGroup {
            id: gid,
            name,
            source,
            members,
            current: 0,
            visible: true,
            expanded: false,
            members_expanded: false,
                docking_sync: None,
        });
        let gi = self.groups.len() - 1;
        self.apply_group_visibility(gi);
        Some(first)
    }

    /// Index of a molecule by stable id (live scene only, not `trash`).
    pub fn mol_index(&self, id: MolId) -> Option<usize> {
        self.molecules.iter().position(|m| m.id == id)
    }

    /// Index of a group by stable id.
    pub fn group_index(&self, id: GroupId) -> Option<usize> {
        self.groups.iter().position(|g| g.id == id)
    }

    /// Enforce the group invariant: the shown member (`members[current]`) is visible
    /// iff `group.visible`; every other member is hidden. Call after any change to a
    /// group's membership / current / visibility.
    pub fn apply_group_visibility(&mut self, gi: usize) {
        let Some(g) = self.groups.get(gi) else { return };
        let cur_id = g.members.get(g.current).copied();
        let gvis = g.visible;
        let members = g.members.clone();
        for id in members {
            if let Some(mi) = self.mol_index(id) {
                self.molecules[mi].visible = Some(id) == cur_id && gvis;
            }
        }
    }

    /// Switch which member of group `gi` is shown to `new_current`: capture the shared
    /// reps off the old member, strip them, re-materialize them onto the new member
    /// (so the shared document follows the shown molecule, re-evaluated against its
    /// topology), and re-apply visibility. Returns `true` if the shown member changed.
    pub fn switch_group_member(&mut self, gi: usize, new_current: usize) -> bool {
        let Some(g) = self.groups.get(gi) else { return false };
        if new_current >= g.members.len() || new_current == g.current {
            return false;
        }
        let old_id = g.members[g.current];
        let new_id = g.members[new_current];
        // Capture the shared prefix from the old shown member (as a document), then
        // strip the live copies off it.
        let shared: Vec<RepState> = match self.mol_index(old_id) {
            Some(oi) => {
                let m = &mut self.molecules[oi];
                let ns = m.n_shared.min(m.reps.len());
                let caps: Vec<RepState> = m.reps[..ns].iter().map(RepState::capture).collect();
                m.reps.drain(0..ns);
                m.n_shared = 0;
                caps
            }
            None => Vec::new(),
        };
        // Re-materialize onto the new shown member (fresh, so they rebuild against it).
        if let Some(ni) = self.mol_index(new_id) {
            let m = &mut self.molecules[ni];
            let live: Vec<Representation> = shared.iter().map(|s| s.to_representation()).collect();
            let n = live.len();
            m.reps.splice(0..0, live);
            m.n_shared = n;
        }
        self.groups[gi].current = new_current;
        self.apply_group_visibility(gi);
        true
    }

    /// The shared reps of group `gi`, captured from the currently shown member's
    /// prefix (the single live source of truth). Empty if the group/member is gone.
    pub fn group_shared_reps(&self, gi: usize) -> Vec<RepState> {
        let Some(g) = self.groups.get(gi) else { return Vec::new() };
        let Some(cur_id) = g.members.get(g.current).copied() else { return Vec::new() };
        let Some(mi) = self.mol_index(cur_id) else { return Vec::new() };
        let m = &self.molecules[mi];
        let ns = m.n_shared.min(m.reps.len());
        m.reps[..ns].iter().map(RepState::capture).collect()
    }

    /// Remove a single member `mol_id` from its group and return it (for the trash).
    /// Captures the group's shared reps first and re-materializes them onto the new
    /// shown member, so deleting the *shown* member doesn't lose the shared document;
    /// clamps `current` and drops the group if it becomes empty. `None` if `mol_id`
    /// isn't a group member.
    pub fn remove_grouped_molecule(&mut self, mol_id: MolId) -> Option<Molecule> {
        let gid = self.molecules.iter().find(|m| m.id == mol_id)?.group?;
        let gi = self.group_index(gid)?;
        let shared = self.group_shared_reps(gi);
        self.groups[gi].members.retain(|&id| id != mol_id);
        let idx = self.mol_index(mol_id)?;
        let mol = self.molecules.remove(idx);
        if self.groups[gi].members.is_empty() {
            self.groups.remove(gi);
            return Some(mol);
        }
        if self.groups[gi].current >= self.groups[gi].members.len() {
            self.groups[gi].current = self.groups[gi].members.len() - 1;
        }
        // Re-materialize the shared reps onto the (possibly new) shown member.
        let cur = self.groups[gi].current;
        if let Some(&cur_id) = self.groups[gi].members.get(cur) {
            if let Some(mi) = self.mol_index(cur_id) {
                let m = &mut self.molecules[mi];
                let ns = m.n_shared.min(m.reps.len());
                m.reps.drain(0..ns);
                let live: Vec<Representation> =
                    shared.iter().map(|s| s.to_representation()).collect();
                m.n_shared = live.len();
                m.reps.splice(0..0, live);
            }
        }
        self.apply_group_visibility(gi);
        Some(mol)
    }

    /// Clamp `selected_mol`/`selected_rep` to valid ranges (after add/remove). Also
    /// prunes group membership of vanished molecules, clamps each group's `current`,
    /// drops empty groups, and re-applies the group visibility invariant.
    pub fn clamp_selection(&mut self) {
        // Prune dead members and empty groups.
        let live: std::collections::HashSet<MolId> =
            self.molecules.iter().map(|m| m.id).collect();
        for g in &mut self.groups {
            g.members.retain(|id| live.contains(id));
            if g.current >= g.members.len() {
                g.current = g.members.len().saturating_sub(1);
            }
        }
        self.groups.retain(|g| !g.members.is_empty());
        for gi in 0..self.groups.len() {
            self.apply_group_visibility(gi);
        }

        if self.molecules.is_empty() {
            self.selected_mol = None;
        } else {
            let m = self.selected_mol.unwrap_or(0).min(self.molecules.len() - 1);
            self.selected_mol = Some(m);
        }
        for mol in &mut self.molecules {
            if mol.reps.is_empty() {
                mol.selected_rep = None;
            } else {
                let r = mol.selected_rep.unwrap_or(0).min(mol.reps.len() - 1);
                mol.selected_rep = Some(r);
            }
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::bonds::BondParams;
    use crate::settings::RepDefaults;

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests")).join(name)
    }

    fn load_first_record(name: &str) -> Molecule {
        let mut recs = crate::data::load_records(&fixture(name), &BondParams::default())
            .expect("load records");
        Molecule::new(MolId(0), recs.remove(0), &RepDefaults::default())
    }

    /// The resolved bond graph reaches the topology, so molar's bond-graph selection
    /// keywords work. Aspirin's only polar hydrogen is its carboxylic acid O–H; every
    /// other H hangs off carbon.
    #[test]
    fn resolved_bonds_reach_the_selection_keywords() {
        let mol = load_first_record("ligands20.sdf");
        let polh = mol.data.evaluate("polh").expect("polh must match with bonds published");
        assert_eq!(polh.1.len(), 1, "aspirin has one acid O-H");
        let apolh = mol.data.evaluate("apolh").expect("apolh must match");
        assert_eq!(apolh.1.len(), 7, "the remaining hydrogens are on carbon");
    }

    /// A viewing selection that hides hydrogens cuts every C–H bond, so a per-molecule
    /// property computed from it (partial charges) has to be widened to whole molecules
    /// first. This is what the [Compute charges] button does with the rep's selection —
    /// without it, espaloma rejects the whole thing as "not bond-complete".
    #[test]
    fn connected_closure_completes_a_hydrogen_hiding_selection() {
        let mol = load_first_record("ligands20.sdf"); // aspirin: 21 atoms, 8 of them H
        let (_, visible) = mol.data.evaluate("not apolh").expect("not apolh");
        let seeds: Vec<usize> = visible.iter_index().collect();
        assert!(
            seeds.len() < mol.n_atoms,
            "the selection must actually hide something for this to test anything"
        );

        let closed = mol.connected_closure(&seeds);
        assert_eq!(closed.len(), mol.n_atoms, "closure must recover the whole molecule");
        assert_eq!(closed, (0..mol.n_atoms).collect::<Vec<_>>(), "sorted, complete, no dupes");
    }

    /// The closure follows bonds, so it takes the molecules the seeds touch — not everything.
    #[test]
    fn connected_closure_stops_at_molecule_boundaries() {
        // 2lao is a protein plus a few hundred unbonded crystal waters.
        let path = fixture("2lao.pdb");
        let raw = crate::data::load(&path).expect("load 2lao.pdb");
        let mol = Molecule::new(MolId(0), raw, &RepDefaults::default());

        // A single protein atom pulls in its whole chain, but no waters.
        let chain = mol.connected_closure(&[0]);
        assert!(chain.len() > 1000, "the protein chain, not one atom: {}", chain.len());
        assert!(chain.len() < mol.n_atoms, "but not the waters too: {}", chain.len());

        // An unbonded water oxygen comes back on its own.
        let last = mol.n_atoms - 1;
        assert_eq!(mol.connected_closure(&[last]), vec![last]);
    }

    /// Aromatic-ring perception needs the file's Kekulé orders — a benzene of
    /// order-less (distance-guessed) bonds reads as sp3 and yields no aromatic ring.
    /// This silently found nothing before the file's bond table was kept.
    #[test]
    fn aromatic_rings_are_found_on_an_sdf_ligand() {
        let mut mol = load_first_record("ligands20.sdf");
        mol.ensure_interaction_rings();
        let rings = mol.interaction_rings.as_ref().expect("rings cached");
        assert_eq!(rings.len(), 1, "aspirin has one aromatic ring");
        assert_eq!(rings[0].len(), 6, "...a six-membered one");
    }

    /// ...and finding them must not aromatize the molecule: espaloma charge assignment
    /// rejects `Aromatic`-order bonds, so the Kekulé structure has to survive.
    #[test]
    fn ring_perception_leaves_bond_orders_alone() {
        let mut mol = load_first_record("ligands20.sdf");
        let before: Vec<BondOrder> = mol.bonds.iter().map(|b| b.order).collect();
        mol.ensure_interaction_rings();
        let after: Vec<BondOrder> = mol.bonds.iter().map(|b| b.order).collect();
        assert_eq!(before, after, "interaction-ring perception must not rewrite orders");
        assert!(
            !after.contains(&BondOrder::Aromatic),
            "the SDF is Kekulé and must stay chargeable"
        );
        // The published topology must match the flat graph, order for order.
        let published: Vec<BondOrder> =
            mol.data.topology().bonds.iter().map(|b| b.order()).collect();
        assert_eq!(published, after);
    }
}
