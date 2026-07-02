//! Atom picking by CPU ray-cast, for the **hover-info** pick mode.
//!
//! The ray is intersected against the atoms **as they are displayed** — i.e. at
//! their smoothed and/or periodic-replicated positions, so hovering matches what's
//! on screen — but the resulting [`PickHit`] reports the atom's **real** stored
//! coordinate (current frame, central image, un-smoothed), never a computed one.

use glam::{Mat4, Vec2, Vec3, Vec4Swizzles};
use molar::prelude::*;

use crate::geometry::{RepKind, RepParams};
use crate::scene::{Molecule, Scene};

/// What the picker does on hover / drag.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum PickMode {
    /// Picking disabled (no per-hover cost).
    #[default]
    Off,
    /// Hovering shows the atom's identity + glow (as before); **clicking** the hovered
    /// atom/residue adds it to the active selection (Shift = add, Ctrl/⌘ = subtract,
    /// plain = replace), expanded per the `Atoms`/`Residues` scope.
    Click,
    /// Drag a freehand lasso; atoms whose displayed positions fall inside it
    /// become a new selection (see [`lasso_select`]).
    Lasso,
}

impl PickMode {
    pub fn label(self) -> &'static str {
        match self {
            PickMode::Off => "Off",
            PickMode::Click => "Click",
            PickMode::Lasso => "Lasso",
        }
    }
}

/// How a raw set of hit atoms (from a lasso) is expanded into the final selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum SelectionMode {
    /// Exactly the atoms hit.
    #[default]
    Atoms,
    /// Every atom of any residue that has at least one hit atom.
    Residues,
    /// Hit **heavy** atoms plus the hydrogens bonded to them. A hit hydrogen whose
    /// bonded heavy atom isn't itself hit is dropped ("lone H are not selected").
    BoundH,
}

impl SelectionMode {
    pub fn label(self) -> &'static str {
        match self {
            SelectionMode::Atoms => "Atoms",
            SelectionMode::Residues => "Residues",
            SelectionMode::BoundH => "Bound H",
        }
    }
}

/// Expand a raw set of hit atom indices according to `mode`, using the molecule's
/// topology (`system`) for residue/element lookup and its guessed `bonds` for the
/// hydrogen attachment. Returns sorted, de-duplicated global atom indices; `Atoms`
/// is the identity. `Topology`'s index is the identity (atom `i` *is* global atom
/// `i`), so this never scans the whole system.
pub fn expand_selection(
    data: &crate::moldata::MolData,
    bonds: &[Bond],
    atoms: &[usize],
    mode: SelectionMode,
) -> Vec<usize> {
    use std::collections::BTreeSet;
    if atoms.is_empty() || mode == SelectionMode::Atoms {
        return atoms.to_vec();
    }
    let topo = data.topology();
    match mode {
        SelectionMode::Atoms => unreachable!(),
        SelectionMode::Residues => {
            // A residue is a *contiguous* run of atom indices (atoms are stored
            // grouped by residue), so grow each hit outward — down then up — while
            // the residue index holds, instead of scanning the whole system. That's
            // O(residue size) per residue. Skipping seeds already collected makes
            // each residue walked once even when a lasso hits many atoms in it.
            let mut out: BTreeSet<usize> = BTreeSet::new();
            for &seed in atoms {
                if out.contains(&seed) {
                    continue;
                }
                let Some(r) = topo.get_atom(seed).map(|a| a.resindex) else {
                    continue;
                };
                out.insert(seed);
                let mut i = seed; // walk down to the residue's first atom
                while i > 0 {
                    i -= 1;
                    match topo.get_atom(i) {
                        Some(a) if a.resindex == r => {
                            out.insert(i);
                        }
                        _ => break,
                    }
                }
                let mut i = seed + 1; // walk up to its last atom
                while let Some(a) = topo.get_atom(i) {
                    if a.resindex == r {
                        out.insert(i);
                        i += 1;
                    } else {
                        break;
                    }
                }
            }
            out.into_iter().collect()
        }
        SelectionMode::BoundH => {
            let is_h = |i: usize| topo.get_atom(i).is_some_and(|a| a.atomic_number == 1);
            // Hit heavy atoms; then the hydrogens bonded to them (the `heavy`
            // snapshot drives the bond test so added H can't chain further).
            let heavy: BTreeSet<usize> = atoms
                .iter()
                .copied()
                .filter(|&i| topo.get_atom(i).is_some_and(|a| a.atomic_number != 1))
                .collect();
            let mut out = heavy.clone();
            for bond in bonds {
                let (a, b) = (bond.i1, bond.i2);
                if heavy.contains(&a) && is_h(b) {
                    out.insert(b);
                }
                if heavy.contains(&b) && is_h(a) {
                    out.insert(a);
                }
            }
            out.into_iter().collect()
        }
    }
}

/// A hovered atom. `real` is the atom's actual stored coordinate; `display` is where
/// it's drawn (smoothed + periodic shift) — used to place the glow.
pub struct PickHit {
    /// Index of the hit atom's molecule in `scene.molecules`.
    pub mol: usize,
    /// Index of the rep (within the molecule) whose geometry was hit — used by the
    /// partner-pick mode to identify which representation is under the cursor.
    pub rep: usize,
    /// Global atom index of the hit atom within its molecule's `System`.
    pub id: usize,
    pub name: String,
    pub resname: String,
    pub resid: i32,
    /// Real (stored, current-frame, central-image, un-smoothed) position, nm.
    pub real: Vec3,
    /// Displayed position (the picked image, smoothed if the rep smooths), nm.
    pub display: Vec3,
    /// Effective sphere radius of the rep at this atom, nm (drives the glow size).
    pub radius: f32,
}

/// The small Ball-and-Stick sphere fraction of the van der Waals radius (matches
/// the Ball-and-Stick default `sphere_scale`) — the marker size used for reps that
/// don't draw their own spheres.
const BALLSTICK_SPHERE_SCALE: f32 = 0.25;

/// The highlight/pick sphere radius for an atom in a given rep: the actual drawn
/// sphere for VDW / Ball-and-Stick, else the small Ball-and-Stick sphere size.
pub(crate) fn effective_radius(params: &RepParams, atom: &Atom) -> f32 {
    match params {
        RepParams::Vdw { scale } => atom.vdw() * scale,
        RepParams::BallAndStick { sphere_scale, .. } => atom.vdw() * sphere_scale,
        // Licorice / Lines / Cartoon / Surface → small Ball-and-Stick sphere size.
        _ => atom.vdw() * BALLSTICK_SPHERE_SCALE,
    }
}

/// Whether `name` is a protein **backbone** atom — the only atoms a Cartoon
/// ribbon is built from (Cα drives the spline path, the carbonyl O its
/// orientation; N/C round out the backbone). Side-chain atoms contribute nothing
/// to the drawn ribbon, so they aren't pickable on a Cartoon rep.
fn cartoon_atom(name: &str) -> bool {
    matches!(name, "N" | "CA" | "C" | "O" | "OT1" | "OT2" | "OXT")
}

/// Whether an atom named `name` contributes to the drawn geometry of a rep of
/// `kind` — the shared style-specific test used by both hover picking and lasso
/// selection. A Cartoon ribbon is built only from backbone atoms (side chains
/// aren't part of it); every other style draws something at each selected atom
/// (Lines included, via its isolated-atom dots).
pub(crate) fn atom_in_rep(kind: RepKind, name: &str) -> bool {
    match kind {
        // An Interactions rep draws only contact lines — no per-atom geometry to hit.
        RepKind::Interactions => false,
        RepKind::Cartoon => cartoon_atom(name),
        _ => true,
    }
}

/// World-space ray (origin, unit direction) through the normalized device coords
/// `(ndc_x, ndc_y)` (each in `[-1, 1]`, y up), for either projection. Unprojects the
/// near and far plane points and connects them.
pub fn cursor_ray(view: Mat4, proj: Mat4, ndc_x: f32, ndc_y: f32) -> (Vec3, Vec3) {
    let inv = (proj * view).inverse();
    let unproject = |z: f32| {
        let p = inv * glam::vec4(ndc_x, ndc_y, z, 1.0);
        p.xyz() / p.w
    };
    let near = unproject(0.0); // wgpu NDC depth is [0,1]
    let far = unproject(1.0);
    (near, (far - near).normalize_or_zero())
}

/// Nearest non-negative ray–sphere intersection distance, if any.
pub(crate) fn ray_sphere(ro: Vec3, rd: Vec3, center: Vec3, r: f32) -> Option<f32> {
    let oc = ro - center;
    let b = oc.dot(rd);
    let c = oc.dot(oc) - r * r;
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let s = disc.sqrt();
    let t0 = -b - s;
    if t0 >= 0.0 {
        Some(t0)
    } else {
        let t1 = -b + s; // origin inside the sphere
        (t1 >= 0.0).then_some(t1)
    }
}

/// Nearest atom of `mol` hit by the world ray `(ro, rd)`, at its currently displayed
/// position. Atoms are hit-tested as spheres of half their vdW radius, floored at
/// `min_radius` (nm) so they stay easy to grab while drawing. Returns the global atom
/// index + ray distance. Used by the drawing tool for snap-to-atom and erase.
pub(crate) fn nearest_atom(
    mol: &Molecule,
    ro: Vec3,
    rd: Vec3,
    min_radius: f32,
) -> Option<(usize, f32)> {
    let state = mol.render_state();
    let topo = mol.data.topology();
    let mut best: Option<(usize, f32)> = None;
    for (i, p) in state.coords.iter().enumerate() {
        let r = topo
            .get_atom(i)
            .map(|a| a.vdw() * 0.5)
            .unwrap_or(min_radius)
            .max(min_radius);
        let center = Vec3::new(p.x, p.y, p.z);
        if let Some(t) = ray_sphere(ro, rd, center, r) {
            if best.is_none_or(|(_, bt)| t < bt) {
                best = Some((i, t));
            }
        }
    }
    best
}

/// Distance from point `p` to segment `a–b` (all in the same 2-D space).
fn point_segment_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 1.0e-12 {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).length()
}

/// Index of the bond of `mol` closest to the cursor (in screen NDC), within
/// `max_dist` NDC units, hit-testing the projected bond segment. Used by the drawing
/// tool to cycle a bond's order or erase it. `ndc` is the cursor in `[-1,1]` (y up).
pub(crate) fn nearest_bond(
    mol: &Molecule,
    view: Mat4,
    proj: Mat4,
    ndc: Vec2,
    max_dist: f32,
) -> Option<usize> {
    let mvp = proj * view;
    let state = mol.render_state();
    let project = |i: usize| -> Option<Vec2> {
        let p = state.coords.get(i)?;
        let clip = mvp * glam::vec4(p.x, p.y, p.z, 1.0);
        if clip.w.abs() < 1.0e-6 {
            return None;
        }
        Some(Vec2::new(clip.x / clip.w, clip.y / clip.w))
    };
    let mut best: Option<(usize, f32)> = None;
    for (k, bond) in mol.bonds.iter().enumerate() {
        let (Some(pa), Some(pb)) = (project(bond.i1), project(bond.i2)) else {
            continue;
        };
        let d = point_segment_dist(ndc, pa, pb);
        if d <= max_dist && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((k, d));
        }
    }
    best.map(|(k, _)| k)
}

/// Build a [`PickHit`] for a specific atom, given the molecule + rep that drew it
/// (as decoded from the GPU id-buffer). O(1): the real coord comes from the current
/// frame, the displayed coord + glow radius from the named rep. Returns `None` if
/// any index is stale/out of range. Native only (drives the GPU pick path).
#[cfg(not(target_arch = "wasm32"))]
pub fn hit_for_atom(scene: &Scene, mi: usize, rep_idx: usize, aid: usize) -> Option<PickHit> {
    let mol = scene.molecules.get(mi)?;
    let atom = mol.data.topology().get_atom(aid)?;
    let frame: &State = mol
        .trajectory
        .frames
        .get(mol.trajectory.current)
        .unwrap_or_else(|| mol.data.state());
    let realp = *frame.coords.get(aid)?;
    let real = Vec3::new(realp.x, realp.y, realp.z);
    // Displayed position (smoothed if the rep smooths) + glow radius, from the rep
    // that produced the hit sphere. Central image only (GPU pick draws the central).
    let (display, radius) = match mol.reps.get(rep_idx) {
        Some(rep) => {
            let r = effective_radius(&rep.params, atom);
            let disp = (rep.smooth_window > 1)
                .then(|| mol.trajectory.smoothed_state(rep.smooth_window))
                .flatten()
                .and_then(|s| s.coords.get(aid).copied())
                .unwrap_or(realp);
            (Vec3::new(disp.x, disp.y, disp.z), r)
        }
        None => (real, atom.vdw() * BALLSTICK_SPHERE_SCALE),
    };
    Some(PickHit {
        mol: mi,
        rep: rep_idx,
        id: aid,
        name: atom.name.as_str().to_string(),
        resname: atom.resname.as_str().to_string(),
        resid: atom.resid,
        real,
        display,
        radius,
    })
}

/// Ray-cast the cursor against every visible atom of every visible rep, at its
/// displayed position, and return the nearest hit (or `None`). `ndc_x`/`ndc_y` are
/// the cursor in normalized device coords (y up).
pub fn pick(scene: &Scene, view: Mat4, proj: Mat4, ndc_x: f32, ndc_y: f32) -> Option<PickHit> {
    let (ro, rd) = cursor_ray(view, proj, ndc_x, ndc_y);
    if rd == Vec3::ZERO {
        return None;
    }
    let mut best_t = f32::INFINITY;
    let mut best: Option<PickHit> = None;

    for (mi, mol) in scene.molecules.iter().enumerate() {
        if !mol.visible {
            continue;
        }
        // Real coordinates source: the current trajectory frame, or the structure.
        let frame: &State = mol
            .trajectory
            .frames
            .get(mol.trajectory.current)
            .unwrap_or_else(|| mol.data.state());
        // Box lattice vectors (columns of the box matrix), for periodic offsets.
        let box_vecs = frame.pbox.as_ref().map(|pb| {
            let m = pb.get_matrix();
            [
                Vec3::new(m[(0, 0)], m[(1, 0)], m[(2, 0)]),
                Vec3::new(m[(0, 1)], m[(1, 1)], m[(2, 1)]),
                Vec3::new(m[(0, 2)], m[(1, 2)], m[(2, 2)]),
            ]
        });

        for (rep_idx, rep) in mol.reps.iter().enumerate() {
            if !rep.visible {
                continue;
            }
            let Some(sel) = &rep.sel else {
                continue;
            };
            // Displayed coordinates: the same smoothed blend the renderer uses (if
            // any), else the raw frame. Held for the duration of the bind below.
            let smoothed = (rep.smooth_window > 1)
                .then(|| mol.trajectory.smoothed_state(rep.smooth_window))
                .flatten();
            let disp_state: &State = smoothed.as_ref().unwrap_or(frame);
            let offsets = match box_vecs {
                Some([a, b, c]) => rep.periodic.offsets(a, b, c),
                None => vec![Vec3::ZERO],
            };

            let bound = mol.data.bind_with_state(sel, disp_state);
            for p in bound.iter_particle() {
                // Only hit atoms that form part of this rep's visible geometry
                // (Cartoon → backbone only; everything else → all selected atoms).
                if !atom_in_rep(rep.kind, p.atom.name.as_str()) {
                    continue;
                }
                let base = Vec3::new(p.pos.x, p.pos.y, p.pos.z);
                let r = effective_radius(&rep.params, p.atom);
                for &off in &offsets {
                    let center = base + off;
                    if let Some(t) = ray_sphere(ro, rd, center, r) {
                        if t < best_t {
                            best_t = t;
                            let real = frame.coords[p.id];
                            best = Some(PickHit {
                                mol: mi,
                                rep: rep_idx,
                                id: p.id,
                                name: p.atom.name.as_str().to_string(),
                                resname: p.atom.resname.as_str().to_string(),
                                resid: p.atom.resid,
                                real: Vec3::new(real.x, real.y, real.z),
                                display: center,
                                radius: r,
                            });
                        }
                    }
                }
            }
        }
    }
    best
}

/// One molecule's lasso result: the global atom indices to turn into a selection.
pub struct LassoSelection {
    /// Index of the molecule in `scene.molecules`.
    pub mol: usize,
    /// Selected atoms' global indices, sorted ascending and de-duplicated.
    pub atoms: Vec<usize>,
}

/// Select every atom whose **displayed** position projects inside the screen
/// `polygon` (clip-space NDC, each coord in `[-1, 1]`, y up), grouped per
/// molecule. Honors the same per-rep style logic as hover [`pick`]: only atoms
/// that contribute to a *visible* rep's geometry are eligible (so a Cartoon rep
/// contributes only its backbone atoms, never side chains). An atom counts if any
/// of its drawn periodic images falls inside the polygon, matching what's on
/// screen. Returns one [`LassoSelection`] per molecule that had hits.
pub fn lasso_select(scene: &Scene, view: Mat4, proj: Mat4, polygon: &[Vec2]) -> Vec<LassoSelection> {
    if polygon.len() < 3 {
        return Vec::new();
    }
    let vp = proj * view;
    // World → NDC (xy); `None` when behind the camera (w ≤ 0).
    let project = |w: Vec3| -> Option<Vec2> {
        let c = vp * w.extend(1.0);
        (c.w > 0.0).then(|| Vec2::new(c.x / c.w, c.y / c.w))
    };
    // Screen-space (NDC) bounding box of the polygon: a cheap 4-compare reject so the
    // O(vertices) even-odd `point_in_polygon` only runs for atoms projecting into the
    // lasso's rect — the rest (the vast majority) are dropped in a few ops.
    let (mut pmin, mut pmax) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
    for v in polygon {
        pmin = pmin.min(*v);
        pmax = pmax.max(*v);
    }
    let in_polygon = |ndc: Vec2| -> bool {
        ndc.x >= pmin.x
            && ndc.x <= pmax.x
            && ndc.y >= pmin.y
            && ndc.y <= pmax.y
            && point_in_polygon(ndc, polygon)
    };

    let mut out = Vec::new();
    for (mi, mol) in scene.molecules.iter().enumerate() {
        if !mol.visible {
            continue;
        }
        let frame: &State = mol
            .trajectory
            .frames
            .get(mol.trajectory.current)
            .unwrap_or_else(|| mol.data.state());
        let box_vecs = frame.pbox.as_ref().map(|pb| {
            let m = pb.get_matrix();
            [
                Vec3::new(m[(0, 0)], m[(1, 0)], m[(2, 0)]),
                Vec3::new(m[(0, 1)], m[(1, 1)], m[(2, 1)]),
                Vec3::new(m[(0, 2)], m[(1, 2)], m[(2, 2)]),
            ]
        });

        // Dedupe atoms hit through multiple reps/images; BTreeSet keeps them sorted.
        let mut picked: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        for rep in &mol.reps {
            if !rep.visible {
                continue;
            }
            let Some(sel) = &rep.sel else {
                continue;
            };
            let smoothed = (rep.smooth_window > 1)
                .then(|| mol.trajectory.smoothed_state(rep.smooth_window))
                .flatten();
            let disp_state: &State = smoothed.as_ref().unwrap_or(frame);
            let offsets = match box_vecs {
                Some([a, b, c]) => rep.periodic.offsets(a, b, c),
                None => vec![Vec3::ZERO],
            };

            let bound = mol.data.bind_with_state(sel, disp_state);
            for p in bound.iter_particle() {
                // Same style filter as picking, and skip atoms already selected.
                if !atom_in_rep(rep.kind, p.atom.name.as_str()) || picked.contains(&p.id) {
                    continue;
                }
                let base = Vec3::new(p.pos.x, p.pos.y, p.pos.z);
                let inside = offsets
                    .iter()
                    .any(|&off| project(base + off).is_some_and(in_polygon));
                if inside {
                    picked.insert(p.id);
                }
            }
        }
        if !picked.is_empty() {
            out.push(LassoSelection { mol: mi, atoms: picked.into_iter().collect() });
        }
    }
    out
}

/// Even-odd (ray-casting) point-in-polygon test. `poly` is a closed polygon given
/// by its vertices in order; `p` and `poly` share one 2-D space.
fn point_in_polygon(p: Vec2, poly: &[Vec2]) -> bool {
    let n = poly.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) {
            let x_cross = (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x;
            if p.x < x_cross {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Build a compact molar `index …` selection string from `atoms` (which must be
/// sorted ascending and unique), compressing runs of consecutive indices into
/// molar's inclusive `lo:hi` ranges — e.g. `[1,2,3,7,9,10]` → `index 1:3 7 9:10`.
/// Returns `None` for an empty set.
pub fn index_selection_string(atoms: &[usize]) -> Option<String> {
    use std::fmt::Write;
    if atoms.is_empty() {
        return None;
    }
    let mut s = String::from("index");
    let mut i = 0;
    while i < atoms.len() {
        let start = atoms[i];
        let mut end = start;
        while i + 1 < atoms.len() && atoms[i + 1] == end + 1 {
            end += 1;
            i += 1;
        }
        if end == start {
            let _ = write!(s, " {start}");
        } else {
            let _ = write!(s, " {start}:{end}");
        }
        i += 1;
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_string_compresses_runs() {
        assert_eq!(index_selection_string(&[]), None);
        assert_eq!(index_selection_string(&[5]).unwrap(), "index 5");
        assert_eq!(
            index_selection_string(&[1, 2, 3, 7, 9, 10]).unwrap(),
            "index 1:3 7 9:10"
        );
    }

    #[test]
    fn point_in_polygon_square() {
        // Unit square [0,1]².
        let sq = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ];
        assert!(point_in_polygon(Vec2::new(0.5, 0.5), &sq));
        assert!(!point_in_polygon(Vec2::new(1.5, 0.5), &sq));
        assert!(!point_in_polygon(Vec2::new(-0.1, 0.5), &sq));
    }

    use crate::camera::Camera;

    /// Load 2lao, set its first rep to `kind` over `sel_text`, and evaluate the
    /// selection so `lasso_select` sees an atom set (the renderer is bypassed).
    fn scene_with_rep(kind: RepKind, sel_text: &str) -> Scene {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb");
        let raw = crate::data::load(std::path::Path::new(path)).expect("load 2lao.pdb");
        let mut scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults { kind, ..Default::default() });
        let mol = &mut scene.molecules[0];
        let rep = &mut mol.reps[0];
        rep.kind = kind;
        rep.sel_text = sel_text.to_string();
        let (expr, sel) = mol.data.evaluate(sel_text).expect("eval selection");
        rep.expr = Some(expr);
        rep.sel = Some(sel);
        scene
    }

    /// A clip-space polygon larger than the [-1,1] NDC viewport, so a framed
    /// molecule's every in-front atom is enclosed.
    fn full_screen_polygon() -> Vec<Vec2> {
        vec![
            Vec2::new(-2.0, -2.0),
            Vec2::new(2.0, -2.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(-2.0, 2.0),
        ]
    }

    #[test]
    fn lasso_full_screen_selects_all_for_vdw() {
        let scene = scene_with_rep(RepKind::Vdw, "all");
        let mol = &scene.molecules[0];
        let cam = Camera::frame_bbox(mol.bbox_min, mol.bbox_max, 0.9);
        let hits = lasso_select(&scene, cam.view(), cam.proj(1.0), &full_screen_polygon());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].mol, 0);
        // A VDW rep draws every selected atom, so a full-screen lasso gets them all.
        assert_eq!(hits[0].atoms.len(), mol.n_atoms);
    }

    #[test]
    fn lasso_selects_in_every_visible_molecule() {
        // Two overlapping molecules (same structure, same coords): a full-screen
        // lasso must return one result per molecule, each with its own atom set.
        let mut scene = scene_with_rep(RepKind::Vdw, "all");
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb");
        let raw = crate::data::load(std::path::Path::new(path)).expect("load 2lao.pdb");
        scene.add(raw, &crate::settings::RepDefaults { kind: RepKind::Vdw, ..Default::default() });
        // Evaluate the second molecule's default rep so it has an atom set too.
        {
            let mol = &mut scene.molecules[1];
            let (expr, sel) = mol.data.evaluate("all").expect("eval");
            mol.reps[0].kind = RepKind::Vdw;
            mol.reps[0].sel_text = "all".to_string();
            mol.reps[0].expr = Some(expr);
            mol.reps[0].sel = Some(sel);
        }
        let (min, max) = scene.bbox().unwrap();
        let cam = Camera::frame_bbox(min, max, 0.9);
        let hits = lasso_select(&scene, cam.view(), cam.proj(1.0), &full_screen_polygon());
        assert_eq!(hits.len(), 2, "lasso should hit both visible molecules");
        assert_eq!(hits[0].mol, 0);
        assert_eq!(hits[1].mol, 1);
        assert_eq!(hits[0].atoms.len(), scene.molecules[0].n_atoms);
        assert_eq!(hits[1].atoms.len(), scene.molecules[1].n_atoms);

        // A hidden molecule is skipped entirely.
        scene.molecules[0].visible = false;
        let hits = lasso_select(&scene, cam.view(), cam.proj(1.0), &full_screen_polygon());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].mol, 1);
    }

    #[test]
    fn lasso_cartoon_selects_only_backbone() {
        let scene = scene_with_rep(RepKind::Cartoon, "protein");
        let mol = &scene.molecules[0];

        // Atom name by global index, for checking the selected set.
        let all = mol.data.select_all();
        let bound = mol.data.bind(&all);
        let mut name_by_id = vec![String::new(); mol.n_atoms];
        let mut n_protein = 0usize;
        for p in bound.iter_particle() {
            name_by_id[p.id] = p.atom.name.as_str().to_string();
        }
        // Protein atom count (the rep's selection size) for the "fewer" assertion.
        if let Some(sel) = &mol.reps[0].sel {
            n_protein = mol.data.bind(sel).iter_particle().count();
        }

        let cam = Camera::frame_bbox(mol.bbox_min, mol.bbox_max, 0.9);
        let hits = lasso_select(&scene, cam.view(), cam.proj(1.0), &full_screen_polygon());
        assert_eq!(hits.len(), 1);
        let atoms = &hits[0].atoms;
        assert!(!atoms.is_empty(), "cartoon lasso selected nothing");
        // Every selected atom must be a backbone atom — no side chains.
        for &id in atoms {
            assert!(
                cartoon_atom(&name_by_id[id]),
                "non-backbone atom {} ({}) selected on a Cartoon rep",
                id,
                name_by_id[id]
            );
        }
        // And the backbone is a strict subset of the protein selection.
        assert!(atoms.len() < n_protein, "expected fewer than all protein atoms");
    }

    #[test]
    fn expand_atoms_is_identity() {
        let scene = scene_with_rep(RepKind::Vdw, "all");
        let mol = &scene.molecules[0];
        let atoms = vec![5usize, 1, 9, 1]; // unsorted, with a dup
        let out = expand_selection(&mol.data, &mol.bonds, &atoms, SelectionMode::Atoms);
        assert_eq!(out, atoms, "Atoms mode must be the identity (no reorder/dedup)");
    }

    #[test]
    fn expand_residues_selects_whole_residue() {
        let scene = scene_with_rep(RepKind::Vdw, "all");
        let mol = &scene.molecules[0];

        // resindex of every atom, and all atoms grouped by resindex.
        let all = mol.data.select_all();
        let bound = mol.data.bind(&all);
        let mut resindex_by_id = vec![0usize; mol.n_atoms];
        let mut atoms_in_res: std::collections::HashMap<usize, Vec<usize>> = Default::default();
        for p in bound.iter_particle() {
            resindex_by_id[p.id] = p.atom.resindex;
            atoms_in_res.entry(p.atom.resindex).or_default().push(p.id);
        }

        // Seed with one atom from residue 0 and a *middle* atom of a later residue,
        // so the outward walk is exercised in both directions (down and up).
        let r0 = 0usize;
        let later = resindex_by_id[mol.n_atoms - 1];
        let mid = atoms_in_res[&later][atoms_in_res[&later].len() / 2];
        let seed = vec![atoms_in_res[&r0][0], mid];
        let out = expand_selection(&mol.data, &mol.bonds, &seed, SelectionMode::Residues);

        // Output is exactly the union of the two residues' atoms, sorted.
        let mut expected: Vec<usize> = atoms_in_res[&r0]
            .iter()
            .chain(atoms_in_res[&later].iter())
            .copied()
            .collect();
        expected.sort_unstable();
        assert_eq!(out, expected);
        // Every output atom shares a hit residue; nothing leaks in.
        for &id in &out {
            assert!(resindex_by_id[id] == r0 || resindex_by_id[id] == later);
        }
        assert!(out.len() > seed.len(), "residue expansion should grow the set");
    }

    /// Build a tiny methane (C + 4 bonded H) plus one far-away "lone" H, written as
    /// a PDB and loaded through `data::load` so bonds are guessed exactly as in the
    /// app. Atom indices: 0=C, 1..=4=bonded H, 5=lone H.
    fn methane_with_lone_h() -> crate::data::RawMolecule {
        // Tetrahedral C-H ≈ 1.09 Å (bonded); lone H 8.7 Å away (unbonded).
        let coords = [
            ("C", 0.0, 0.0, 0.0),
            ("H1", 0.629, 0.629, 0.629),
            ("H2", -0.629, -0.629, 0.629),
            ("H3", -0.629, 0.629, -0.629),
            ("H4", 0.629, -0.629, -0.629),
            ("H5", 5.0, 5.0, 5.0),
        ];
        let mut pdb = String::new();
        for (i, (name, x, y, z)) in coords.iter().enumerate() {
            let elem = if name.starts_with('C') { "C" } else { "H" };
            // Columns: name 13-16, x/y/z 31-54 (8.3), element 77-78.
            pdb.push_str(&format!(
                "ATOM  {:>5} {:^4} MOL A   1    {:>8.3}{:>8.3}{:>8.3}  1.00  0.00          {:>2}\n",
                i + 1, name, x, y, z, elem
            ));
        }
        pdb.push_str("END\n");
        let path = std::env::temp_dir().join("molar_vis_methane_test.pdb");
        std::fs::write(&path, pdb).expect("write methane pdb");
        let raw = crate::data::load(&path).expect("load methane pdb");
        let _ = std::fs::remove_file(&path);
        raw
    }

    #[test]
    fn expand_bound_h() {
        let raw = methane_with_lone_h();
        let bonds = raw.bonds.clone();
        let data = crate::moldata::MolData::Owned(raw.system);
        // Sanity: the loader read the elements and guessed the 4 C–H bonds.
        let all = data.select_all();
        let bound = data.bind(&all);
        let z: Vec<u8> = bound.iter_particle().map(|p| p.atom.atomic_number).collect();
        assert_eq!(z, vec![6, 1, 1, 1, 1, 1], "expected C + 5 H");
        let c_bonds = bonds.iter().filter(|b| b.contains(0)).count();
        assert_eq!(c_bonds, 4, "carbon should have 4 bonded H (lone H unbonded)");

        let exp = |atoms: &[usize]| {
            expand_selection(&data, &bonds, atoms, SelectionMode::BoundH)
        };

        // Heavy atom hit → it plus all 4 bonded H.
        assert_eq!(exp(&[0]), vec![0, 1, 2, 3, 4]);
        // Heavy + a far lone H → lone H dropped, bonded H added.
        assert_eq!(exp(&[0, 5]), vec![0, 1, 2, 3, 4]);
        // Only a bonded H hit (its carbon not selected) → dropped.
        assert_eq!(exp(&[1]), Vec::<usize>::new());
        // Only the lone H → dropped.
        assert_eq!(exp(&[5]), Vec::<usize>::new());
    }
}
