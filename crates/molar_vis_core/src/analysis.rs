//! Structural analysis: least-squares **superposition** (align) and **RMSD**.
//!
//! Both operations compare two sides — a selection of a molecule at a frame — so this module
//! is written around one [`Request`] describing the pair, and the two entry points
//! [`rmsd`] (measure only) and [`align`] (fit the source onto the target, then measure).
//!
//! The maths is molar's ([`molar::prelude::fit_transform`] / [`molar::prelude::rmsd`], and
//! `get_matching_atoms_by_name` for the common-subset pairing); what lives here is the part
//! that has to be right *around* it: which atoms pair with which, which frames take part,
//! which atoms move, and producing the undo deltas rather than mutating behind the caller's
//! back. It touches only [`Scene`], so it is WASM-safe and unit-testable without a GPU.
//!
//! **What the common subset can and cannot do.** It aligns the two **atom-name sequences**,
//! so it recovers from a few missing atoms — an unresolved side chain, a different
//! protonation, a truncated terminus — which is what it is for. It is not a structural
//! matcher: atom names repeat down a chain, so a *systematic* difference (every `CA` absent
//! from one side, say) leaves the alignment free to pair one residue's atoms with the next
//! one's and the resulting RMSD is meaningless. When the two selections do correspond atom
//! for atom, leave it off — then nothing is guessed at.

use crate::history::StructEdit;
use crate::scene::{MolId, Scene};
use molar::prelude::*;

/// One side of a comparison: a selection of a molecule, at one frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Side {
    /// Index into [`Scene::molecules`].
    pub mol: usize,
    /// molar selection text.
    pub sel: String,
    /// Frame index. Clamped to the trajectory; ignored for a molecule that has none.
    pub frame: usize,
}

/// What to compare, and (for [`align`]) what to move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The side that moves.
    pub source: Side,
    /// The side that stays put — the reference.
    pub target: Side,
    /// Fit **every** frame of the source molecule onto the target instead of just
    /// `source.frame`. Ignored when the source has no trajectory.
    pub all_frames: bool,
    /// Pair the two selections by atom **name** (molar's Needleman–Wunsch sequence
    /// alignment) instead of atom for atom, so selections of unequal size can be compared.
    pub common_subset: bool,
    /// Move every atom of the source molecule, not just the atoms its selection matched.
    /// The transform is computed from the selection either way.
    pub move_whole: bool,
}

/// RMSD over the frames a request compared. `min`/`max`/`mean` coincide for a single frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rmsd {
    pub frames: usize,
    pub mean: f32,
    pub min: f32,
    pub max: f32,
}

impl Rmsd {
    /// One line for the dialog's readout, in **nm** (molar's unit throughout the viewer).
    ///
    /// A single frame is just the value — the spread of one number would be noise. Several
    /// frames report the mean plus the range, which is what says whether a fitted trajectory
    /// is uniformly close or has outlier frames.
    pub fn label(&self) -> String {
        if self.frames <= 1 {
            format!("{:.3} nm", self.mean)
        } else {
            format!(
                "{:.3} nm — mean of {} frames (min {:.3}, max {:.3})",
                self.mean, self.frames, self.min, self.max
            )
        }
    }

    fn from_values(v: &[f32]) -> Result<Self, String> {
        if v.is_empty() {
            return Err("nothing to compare".into());
        }
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        Ok(Self {
            frames: v.len(),
            mean,
            min: v.iter().copied().fold(f32::INFINITY, f32::min),
            max: v.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        })
    }
}

/// Which atoms of the two sides are compared, as **parallel global-index lists**.
///
/// Both lists are ascending: an atom-for-atom pairing is the selections' own index slices,
/// and the name-matched one walks molar's alignment left to right over those same ascending
/// slices. That is what lets each list go back through [`Sel::from_vec`] (which sorts) with
/// the correspondence intact.
pub struct Pairing {
    pub src: Vec<usize>,
    pub tgt: Vec<usize>,
}

/// Pair the atoms of two bound selections.
///
/// Atom for atom by default — which requires equal counts, and says so plainly when they
/// differ, since that is the whole reason the *Common subset* option exists. With
/// `common_subset`, molar aligns the two **atom-name sequences** (Needleman–Wunsch) and only
/// the matched atoms are compared, so a structure missing a few atoms — a different
/// protonation, an unresolved side chain, a truncated terminus — still measures against its
/// reference.
pub fn pair_atoms(
    src: &(impl AtomProvider + IndexSliceProvider),
    tgt: &(impl AtomProvider + IndexSliceProvider),
    common_subset: bool,
) -> Result<Pairing, String> {
    let (s, t) = (src.get_index_slice(), tgt.get_index_slice());
    if !common_subset {
        if s.len() != t.len() {
            return Err(format!(
                "the selections have different atom counts ({} and {}) — turn on \
                 “Common subset” to compare the atoms whose names match",
                s.len(),
                t.len()
            ));
        }
        return Ok(Pairing { src: s.to_vec(), tgt: t.to_vec() });
    }
    // molar returns *local* (within-selection) indices, so map them back through each
    // selection's own index slice to get global atom indices.
    let (li, lj) = get_matching_atoms_by_name(src, tgt);
    if li.is_empty() {
        return Err("no atoms with matching names in the two selections".into());
    }
    Ok(Pairing {
        src: li.iter().filter_map(|&i| s.get(i).copied()).collect(),
        tgt: lj.iter().filter_map(|&j| t.get(j).copied()).collect(),
    })
}

/// Measure the RMSD of a request without moving anything.
pub fn rmsd(scene: &Scene, req: &Request) -> Result<Rmsd, String> {
    let plan = Plan::resolve(scene, req)?;
    Rmsd::from_values(&plan.rmsds(scene)?)
}

/// Fit the source onto the target and measure what is left.
///
/// Returns the post-fit RMSD plus the coordinate deltas, for the caller to push onto the
/// undo timeline as **one** step (see [`crate::history::History::record_structs`]) — an
/// alignment moves atoms, so Ctrl+Z has to take it back.
///
/// Every transform is computed **before** the first coordinate is written, so a failure
/// (an unresolvable selection, a count mismatch, a degenerate fit) leaves the scene
/// untouched rather than partly moved.
pub fn align(
    scene: &mut Scene,
    req: &Request,
) -> Result<(Rmsd, Vec<(MolId, StructEdit)>), String> {
    let plan = Plan::resolve(scene, req)?;
    if scene.molecules[plan.src_mol].data.is_shared() {
        return Err(
            "this molecule's coordinates are owned by the host program (Python/JavaScript), \
             so the viewer cannot move them"
                .into(),
        );
    }
    // Phase 1 — where each frame's atoms land, read-only. The target may live in the *same*
    // molecule (aligning a trajectory onto one of its own frames), which is exactly why
    // nothing may be written while positions are still being read.
    let planned = plan
        .frames
        .iter()
        .map(|&f| plan.fitted_coords(scene, f).map(|c| (f, c)))
        .collect::<Result<Vec<_>, String>>()?;

    // Phase 2 — write, capturing a before/after delta per frame.
    let mol_id = scene.molecules[plan.src_mol].id;
    let mut edits = Vec::with_capacity(planned.len());
    for (f, after) in planned {
        let mol = &mut scene.molecules[plan.src_mol];
        // The store this frame writes to: a trajectory frame, or the owned System for a
        // static molecule — the same distinction `StructEdit::Coords` carries so undo hits
        // the same store later (see `Molecule::coord_edit_target`).
        let store = (!mol.trajectory.frames.is_empty()).then_some(f);
        let before: Vec<[f32; 3]> = {
            let st = mol.trajectory.frames.get(f).unwrap_or_else(|| mol.data.state());
            plan.moving
                .iter()
                .map(|&a| st.coords.get(a).map(|p| [p.x, p.y, p.z]).unwrap_or([0.0; 3]))
                .collect()
        };
        mol.set_coords(&plan.moving, &after, store);
        edits.push((
            mol_id,
            StructEdit::Coords { atoms: plan.moving.clone(), before, after, frame: store },
        ));
    }
    scene.molecules[plan.src_mol].mark_coords_dirty(true);

    // Phase 3 — measure what the fit left behind, from the coordinates now stored.
    let stats = Rmsd::from_values(&plan.rmsds(scene)?)?;
    Ok((stats, edits))
}

/// A resolved request: selections compiled, frames chosen, atoms paired.
///
/// Resolving up front is what makes both entry points cheap per frame and keeps every
/// failure mode in one place. The paired sub-selections are held as `Sel`s so molar's
/// `fit_transform`/`rmsd` can be used directly on them at any frame.
struct Plan {
    src_mol: usize,
    tgt_mol: usize,
    /// The paired atoms of each side, as selections (equal length, corresponding order).
    src_pair: Sel,
    tgt_pair: Sel,
    /// The frame of the target — the reference — held fixed.
    tgt_frame: usize,
    /// Source frames to fit/measure.
    frames: Vec<usize>,
    /// Global indices of the atoms an [`align`] moves.
    moving: Vec<usize>,
}

impl Plan {
    fn resolve(scene: &Scene, req: &Request) -> Result<Self, String> {
        let (src_mol, src_sel, src_frame) = resolve_side(scene, &req.source, "source")?;
        let (tgt_mol, tgt_sel, tgt_frame) = resolve_side(scene, &req.target, "target")?;

        let src = &scene.molecules[src_mol];
        let pairing = {
            let sb = src.data.bind_with_state(&src_sel, state_at(src, src_frame));
            let tgt = &scene.molecules[tgt_mol];
            let tb = tgt.data.bind_with_state(&tgt_sel, state_at(tgt, tgt_frame));
            pair_atoms(&sb, &tb, req.common_subset)?
        };
        // Under three atoms a least-squares fit is not determined (a single pair is a pure
        // translation, two leave a free rotation about their axis).
        if pairing.src.len() < 3 {
            return Err(format!(
                "only {} atom(s) pair up — a fit needs at least 3",
                pairing.src.len()
            ));
        }

        let n_frames = src.trajectory.n_frames();
        let frames: Vec<usize> = match req.all_frames && n_frames > 1 {
            true => (0..n_frames).collect(),
            false => vec![src_frame],
        };
        let moving = match req.move_whole {
            true => (0..src.n_atoms).collect(),
            false => src_sel.get_index_slice().to_vec(),
        };
        Ok(Self {
            src_mol,
            tgt_mol,
            src_pair: Sel::from_vec(pairing.src).map_err(|e| e.to_string())?,
            tgt_pair: Sel::from_vec(pairing.tgt).map_err(|e| e.to_string())?,
            tgt_frame,
            frames,
            moving,
        })
    }

    /// Where the moving atoms land: the best fit of the source's paired atoms at `frame` onto
    /// the target's, applied to [`Self::moving`].
    ///
    /// Returns the finished coordinates rather than the transform, so the fit is done and
    /// forgotten inside one read-only step — the caller can compute every frame before
    /// writing any of them, and nothing outside this function has to name molar's nalgebra
    /// isometry type (the crate is not a direct dependency; the whole molar boundary goes
    /// through molar's own aliases).
    fn fitted_coords(&self, scene: &Scene, frame: usize) -> Result<Vec<[f32; 3]>, String> {
        let src = &scene.molecules[self.src_mol];
        let tgt = &scene.molecules[self.tgt_mol];
        let tr = {
            let sb = src.data.bind_with_state(&self.src_pair, state_at(src, frame));
            let tb = tgt.data.bind_with_state(&self.tgt_pair, state_at(tgt, self.tgt_frame));
            molar::prelude::fit_transform(&sb, &tb).map_err(|e| e.to_string())?
        };
        let st = state_at(src, frame);
        Ok(self
            .moving
            .iter()
            .map(|&a| {
                let q = tr * st.coords.get(a).copied().unwrap_or(Pos::origin());
                [q.x, q.y, q.z]
            })
            .collect())
    }

    /// RMSD of the paired atoms, per frame of [`Self::frames`].
    fn rmsds(&self, scene: &Scene) -> Result<Vec<f32>, String> {
        let src = &scene.molecules[self.src_mol];
        let tgt = &scene.molecules[self.tgt_mol];
        let tb = tgt.data.bind_with_state(&self.tgt_pair, state_at(tgt, self.tgt_frame));
        self.frames
            .iter()
            .map(|&f| {
                let sb = src.data.bind_with_state(&self.src_pair, state_at(src, f));
                molar::prelude::rmsd(&sb, &tb).map_err(|e| e.to_string())
            })
            .collect()
    }
}

/// Compile one side: check the molecule, evaluate the selection, clamp the frame.
fn resolve_side(
    scene: &Scene,
    side: &Side,
    what: &str,
) -> Result<(usize, Sel, usize), String> {
    let mol = scene
        .molecules
        .get(side.mol)
        .ok_or_else(|| format!("the {what} molecule is no longer loaded"))?;
    let (_, sel) = mol.data.evaluate(&side.sel).map_err(|e| match e {
        crate::scene::EvalError::Empty => format!("the {what} selection matches no atoms"),
        crate::scene::EvalError::Invalid { message, .. } => {
            format!("{what} selection: {message}")
        }
    })?;
    let frame = match mol.trajectory.n_frames() {
        0 => 0,
        n => side.frame.min(n - 1),
    };
    Ok((side.mol, sel, frame))
}

/// The coordinates of `mol` at `frame` — a trajectory frame, or its own state when it has
/// no trajectory (where the frame index is meaningless).
fn state_at(mol: &crate::scene::Molecule, frame: usize) -> &State {
    mol.trajectory.frames.get(frame).unwrap_or_else(|| mol.data.state())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::bonds::BondParams;
    use crate::settings::RepDefaults;

    fn scene_with(n: usize) -> Scene {
        let path =
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb"));
        let mut scene = Scene::default();
        for _ in 0..n {
            let raw = crate::data::load_with(path, &BondParams::default()).expect("load 2lao");
            scene.add(raw, &RepDefaults::default());
        }
        scene
    }

    fn side(mol: usize, sel: &str) -> Side {
        Side { mol, sel: sel.into(), frame: 0 }
    }

    fn request(src: Side, tgt: Side) -> Request {
        Request {
            source: src,
            target: tgt,
            all_frames: false,
            common_subset: false,
            move_whole: false,
        }
    }

    /// Two copies of the same file, untouched: zero deviation, and every atom pairs.
    #[test]
    fn identical_copies_have_zero_rmsd() {
        let scene = scene_with(2);
        let r = rmsd(&scene, &request(side(0, "protein"), side(1, "protein"))).expect("rmsd");
        assert_eq!(r.frames, 1);
        assert!(r.mean < 1e-6, "{r:?}");
        assert_eq!(r.label(), "0.000 nm");
    }

    /// The point of the whole feature: a rotated + translated copy is put back on top of the
    /// original, so what remains is numerical noise. Also checks that the fit is computed
    /// from the *selection* but that the RMSD reported afterwards is the real, stored one.
    #[test]
    fn align_recovers_a_rigidly_moved_copy() {
        let mut scene = scene_with(2);
        // Move molecule 1 bodily: rotate every atom about a point well off its centre.
        let all: Vec<usize> = (0..scene.molecules[1].n_atoms).collect();
        scene.molecules[1].rotate_fragment(&all, glam::Vec3::new(3.0, -1.0, 2.0), glam::Vec3::new(0.3, 0.5, -0.8), 0.7);
        let req = Request { move_whole: true, ..request(side(1, "protein"), side(0, "protein")) };

        let before = rmsd(&scene, &req).expect("rmsd before");
        assert!(before.mean > 0.5, "the copy must actually be displaced: {before:?}");

        let (after, edits) = align(&mut scene, &req).expect("align");
        assert!(after.mean < 1e-3, "the fit must recover the original: {after:?}");
        assert_eq!(edits.len(), 1, "one static molecule → one coordinate delta");
        // Measuring again reads the stored coordinates, so it must agree with the report.
        let again = rmsd(&scene, &req).expect("rmsd after");
        assert!((again.mean - after.mean).abs() < 1e-6);
    }

    /// Unequal selections are refused atom-for-atom — with the advice that fixes it — and
    /// go through once paired by name.
    ///
    /// The target here is the case the option exists for: a structure missing a few atoms
    /// (unresolved side chain, different protonation), not one missing a whole atom *type*.
    /// Atom names repeat down a chain, so a systematic removal leaves molar's name-sequence
    /// alignment free to pair one residue's atoms with the next one's — see the module docs.
    #[test]
    fn unequal_selections_need_the_common_subset() {
        let scene = scene_with(2);
        let gaps = "protein and not ((resid 5 and name CA) or (resid 9 and name N) \
                    or (resid 12 and name CB))";
        let mut req = request(side(0, "protein"), side(1, gaps));
        let e = rmsd(&scene, &req).unwrap_err();
        assert!(e.contains("different atom counts"), "{e}");
        assert!(e.contains("Common subset"), "{e}");

        req.common_subset = true;
        let r = rmsd(&scene, &req).expect("name-paired rmsd");
        assert!(r.mean < 1e-6, "the matched atoms are the same coordinates: {r:?}");
    }

    /// With `move_whole` off — the default — only the atoms the source selection matched are
    /// moved, so the rest of the molecule stays where it was (the selection is taken *out* of
    /// its molecule; that is what the option is choosing between).
    #[test]
    fn without_move_whole_only_the_selection_moves() {
        let mut scene = scene_with(2);
        let all: Vec<usize> = (0..scene.molecules[1].n_atoms).collect();
        scene.molecules[1].rotate_fragment(&all, glam::Vec3::ZERO, glam::Vec3::Z, 0.5);
        let (inside, outside) = {
            let mol = &scene.molecules[1];
            let (_, sel) = mol.data.evaluate("resid 1:50").expect("resid 1:50");
            let ids = sel.get_index_slice().to_vec();
            let last = *ids.last().expect("non-empty");
            (ids[0], last + 1) // an atom in the selection, and the next one outside it
        };
        let before = scene.molecules[1].render_state().coords.clone();

        let req = request(side(1, "resid 1:50"), side(0, "resid 1:50"));
        align(&mut scene, &req).expect("align");

        let after = scene.molecules[1].render_state();
        assert_ne!(after.coords[inside], before[inside], "the selection must move");
        assert_eq!(after.coords[outside], before[outside], "the rest must not");
    }

    /// An alignment moves atoms, so it has to be undoable — and the delta has to name the
    /// right coordinate store, which is what `StructEdit::Coords` carries.
    #[test]
    fn an_alignment_is_undoable() {
        use crate::history::{EditState, History};
        let mut scene = scene_with(2);
        let all: Vec<usize> = (0..scene.molecules[1].n_atoms).collect();
        scene.molecules[1].rotate_fragment(&all, glam::Vec3::ZERO, glam::Vec3::Z, 0.5);
        let displaced = scene.molecules[1].render_state().coords[0];

        let req = Request { move_whole: true, ..request(side(1, "protein"), side(0, "protein")) };
        let (_, edits) = align(&mut scene, &req).expect("align");
        assert_ne!(scene.molecules[1].render_state().coords[0], displaced, "it must move");

        let mut history = History::new(EditState::capture(&scene));
        history.record_structs(edits, "align".into());
        assert_eq!(history.undo(&mut scene).as_deref(), Some("align"));
        assert_eq!(
            scene.molecules[1].render_state().coords[0], displaced,
            "undo must put the coordinates back where the alignment found them"
        );
    }

    /// A pairing too small to determine a rotation is refused rather than producing an
    /// arbitrary one.
    #[test]
    fn a_fit_needs_three_atoms() {
        let mut scene = scene_with(2);
        let req = request(side(0, "index 0 1"), side(1, "index 0 1"));
        // `.err()` rather than `unwrap_err()`: the Ok side carries `StructEdit`s, which are
        // deliberately not `Debug` (they hold whole structure snapshots).
        let e = align(&mut scene, &req).err().expect("a 2-atom fit must be refused");
        assert!(e.contains("at least 3"), "{e}");
    }

    /// The label is the readout the dialog shows, so its two shapes are part of the contract.
    #[test]
    fn rmsd_label_summarises_several_frames() {
        let one = Rmsd { frames: 1, mean: 0.1234, min: 0.1234, max: 0.1234 };
        assert_eq!(one.label(), "0.123 nm");
        let many = Rmsd { frames: 26, mean: 0.184, min: 0.121, max: 0.245 };
        assert_eq!(many.label(), "0.184 nm — mean of 26 frames (min 0.121, max 0.245)");
    }
}
