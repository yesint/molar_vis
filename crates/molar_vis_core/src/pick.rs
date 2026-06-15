//! Atom picking by CPU ray-cast, for the **hover-info** pick mode.
//!
//! The ray is intersected against the atoms **as they are displayed** — i.e. at
//! their smoothed and/or periodic-replicated positions, so hovering matches what's
//! on screen — but the resulting [`PickHit`] reports the atom's **real** stored
//! coordinate (current frame, central image, un-smoothed), never a computed one.

use glam::{Mat4, Vec2, Vec3, Vec4Swizzles};
use molar::prelude::*;

use crate::geometry::{RepKind, RepParams};
use crate::scene::Scene;

/// What the picker does on hover / drag.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PickMode {
    /// Picking disabled (no per-hover cost).
    #[default]
    Off,
    /// Show the hovered atom's identity + real coordinates and glow its outline.
    HoverInfo,
    /// Drag a freehand lasso; atoms whose displayed positions fall inside it
    /// become a new selection (see [`lasso_select`]).
    Lasso,
}

impl PickMode {
    pub fn label(self) -> &'static str {
        match self {
            PickMode::Off => "Off",
            PickMode::HoverInfo => "Hover info",
            PickMode::Lasso => "Lasso select",
        }
    }
}

/// A hovered atom. `real` is the atom's actual stored coordinate; `display` is where
/// it's drawn (smoothed + periodic shift) — used to place the glow.
pub struct PickHit {
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
fn effective_radius(params: &RepParams, atom: &Atom) -> f32 {
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
fn atom_in_rep(kind: RepKind, name: &str) -> bool {
    !matches!(kind, RepKind::Cartoon) || cartoon_atom(name)
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
fn ray_sphere(ro: Vec3, rd: Vec3, center: Vec3, r: f32) -> Option<f32> {
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

    for mol in &scene.molecules {
        if !mol.visible {
            continue;
        }
        // Real coordinates source: the current trajectory frame, or the structure.
        let frame: &State = mol
            .trajectory
            .frames
            .get(mol.trajectory.current)
            .unwrap_or_else(|| mol.system.state());
        // Box lattice vectors (columns of the box matrix), for periodic offsets.
        let box_vecs = frame.pbox.as_ref().map(|pb| {
            let m = pb.get_matrix();
            [
                Vec3::new(m[(0, 0)], m[(1, 0)], m[(2, 0)]),
                Vec3::new(m[(0, 1)], m[(1, 1)], m[(2, 1)]),
                Vec3::new(m[(0, 2)], m[(1, 2)], m[(2, 2)]),
            ]
        });

        for rep in &mol.reps {
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

            let bound = mol.system.bind_with_state(sel, disp_state);
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

    let mut out = Vec::new();
    for (mi, mol) in scene.molecules.iter().enumerate() {
        if !mol.visible {
            continue;
        }
        let frame: &State = mol
            .trajectory
            .frames
            .get(mol.trajectory.current)
            .unwrap_or_else(|| mol.system.state());
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

            let bound = mol.system.bind_with_state(sel, disp_state);
            for p in bound.iter_particle() {
                // Same style filter as picking, and skip atoms already selected.
                if !atom_in_rep(rep.kind, p.atom.name.as_str()) || picked.contains(&p.id) {
                    continue;
                }
                let base = Vec3::new(p.pos.x, p.pos.y, p.pos.z);
                let inside = offsets
                    .iter()
                    .any(|&off| project(base + off).is_some_and(|ndc| point_in_polygon(ndc, polygon)));
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
        scene.add(raw, kind);
        let mol = &mut scene.molecules[0];
        let rep = &mut mol.reps[0];
        rep.kind = kind;
        rep.sel_text = sel_text.to_string();
        let (expr, sel) = crate::scene::evaluate(&mol.system, sel_text).expect("eval selection");
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
        let cam = Camera::frame_bbox(mol.bbox_min, mol.bbox_max);
        let hits = lasso_select(&scene, cam.view(), cam.proj(1.0), &full_screen_polygon());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].mol, 0);
        // A VDW rep draws every selected atom, so a full-screen lasso gets them all.
        assert_eq!(hits[0].atoms.len(), mol.n_atoms);
    }

    #[test]
    fn lasso_cartoon_selects_only_backbone() {
        let scene = scene_with_rep(RepKind::Cartoon, "protein");
        let mol = &scene.molecules[0];

        // Atom name by global index, for checking the selected set.
        let all = mol.system.select_all();
        let bound = mol.system.bind(&all);
        let mut name_by_id = vec![String::new(); mol.n_atoms];
        let mut n_protein = 0usize;
        for p in bound.iter_particle() {
            name_by_id[p.id] = p.atom.name.as_str().to_string();
        }
        // Protein atom count (the rep's selection size) for the "fewer" assertion.
        if let Some(sel) = &mol.reps[0].sel {
            n_protein = mol.system.bind(sel).iter_particle().count();
        }

        let cam = Camera::frame_bbox(mol.bbox_min, mol.bbox_max);
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
}
