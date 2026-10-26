//! Unobstructed-view geometry — pure, GPU-free, wasm-safe.
//!
//! Small solid targets use rendered spheres, capsules and mesh triangles in a
//! fixed-scale depth grid (`pixels`): maximize visible area, including self-overlap.
//! Large targets retain the accelerated atom-centre search below.
//!
//! Given a target representation's atoms and the atoms of every other rep that could
//! hide it, this picks the camera view direction that leaves the **most target atoms
//! directly visible** — front-most, hidden by neither another rep nor another atom of
//! the target rep itself ("most of the rep is on screen").
//!
//! The fallback atom search uses representation radii. Mesh representations supply
//! actual triangles, accelerated with a BVH.
//! Projection is orthographic (the viewer's default). A target atom counts as visible
//! when its projected centre is covered by neither nearer spheres nor mesh triangles.
//! The search maximizes that count over Fibonacci directions and a local refinement.
//!
//! `d` is the world direction from the target toward the camera: the camera sits on the
//! `+d` side and looks back along `-d`, so a larger `dot(pos, d)` means nearer the
//! camera. [`look_along_quat`] turns `d` into the camera orientation used by
//! `App::unobstructed_view`.

mod pixels;
pub(crate) use pixels::{PixelScene, PixelView};

use glam::{Mat3, Quat, Vec3};
use std::f32::consts::TAU;

/// Uniform directions on the unit sphere (Fibonacci lattice) for the coarse search.
pub(crate) fn fibonacci_sphere(n: usize) -> Vec<Vec3> {
    let mut dirs = Vec::with_capacity(n);
    // Golden-angle increment.
    let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    for i in 0..n {
        let y = 1.0 - (i as f32 + 0.5) / n as f32 * 2.0; // in (1, -1)
        let r = (1.0 - y * y).max(0.0).sqrt();
        let theta = golden * i as f32;
        dirs.push(Vec3::new(theta.cos() * r, y, theta.sin() * r));
    }
    dirs
}

/// Two orthonormal vectors spanning the plane perpendicular to unit `d`.
pub(crate) fn tangent_basis(d: Vec3) -> (Vec3, Vec3) {
    let a = if d.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let t1 = a.cross(d).normalize();
    let t2 = d.cross(t1);
    (t1, t2)
}

/// Projection and grid buffers retained across direction evaluations.
#[derive(Default)]
struct VisibilityScratch {
    proj: Vec<[f32; 4]>,
    atom_cells: Vec<usize>,
    offsets: Vec<usize>,
    cursors: Vec<usize>,
    indices: Vec<usize>,
    max_depth: Vec<f32>,
}

impl VisibilityScratch {
    /// Count TARGET atoms that are front-most (un-occluded) when the scene is viewed from
    /// direction `d`. Atoms (target + occluders) are vdW spheres projected orthographically;
    /// a target atom is hidden when any other atom nearer the camera covers its projected
    /// centre. A target atom hidden by a nearer atom of the target rep is also counted as
    /// hidden (only the front one of an overlapping pair shows).
    #[cfg(test)]
    fn visible_count(&mut self, target: &[(Vec3, f32)], occluders: &[(Vec3, f32)], d: Vec3) -> u32 {
        self.visible_count_cancellable(target, occluders, d, None, &|| false)
            .unwrap()
    }

    fn visible_count_cancellable(
        &mut self,
        target: &[(Vec3, f32)],
        occluders: &[(Vec3, f32)],
        d: Vec3,
        mesh: Option<&TriangleOccluders>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<u32, String> {
        let t = target.len();
        if t == 0 {
            return Ok(0);
        }
        let (t1, t2) = tangent_basis(d);
        self.proj.clear();
        let (mut umin, mut umax) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut vmin, mut vmax) = (f32::INFINITY, f32::NEG_INFINITY);
        let mut rmax = 1e-3_f32;
        for (i, (p, r)) in target.iter().chain(occluders.iter()).enumerate() {
            if i % 1024 == 0 && cancelled() {
                return Err("cancelled".into());
            }
            let u = p.dot(t1);
            let v = p.dot(t2);
            self.proj.push([u, v, p.dot(d), *r]);
            umin = umin.min(u);
            umax = umax.max(u);
            vmin = vmin.min(v);
            vmax = vmax.max(v);
            rmax = rmax.max(*r);
        }

        // Cell = 2*rmax: a blocker lies in the query's cell or one of its eight
        // neighbours. Store buckets as contiguous slices instead of separate Vecs.
        let inv = 1.0 / (2.0 * rmax).max(1e-3);
        let nx = (((umax - umin) * inv).ceil() as usize + 1).max(1);
        let ny = (((vmax - vmin) * inv).ceil() as usize + 1).max(1);
        let cells = nx * ny;
        let cell_of = |u: f32, v: f32| -> usize {
            let cx = (((u - umin) * inv).floor() as usize).min(nx - 1);
            let cy = (((v - vmin) * inv).floor() as usize).min(ny - 1);
            cy * nx + cx
        };
        self.offsets.resize(cells + 1, 0);
        self.offsets.fill(0);
        self.max_depth.resize(cells, f32::NEG_INFINITY);
        self.max_depth.fill(f32::NEG_INFINITY);
        self.atom_cells.clear();
        for pr in &self.proj {
            let cell = cell_of(pr[0], pr[1]);
            self.atom_cells.push(cell);
            self.offsets[cell + 1] += 1;
            self.max_depth[cell] = self.max_depth[cell].max(pr[2]);
        }
        for cell in 0..cells {
            self.offsets[cell + 1] += self.offsets[cell];
        }
        self.cursors.clear();
        self.cursors.extend_from_slice(&self.offsets[..cells]);
        self.indices.resize(self.proj.len(), 0);
        for (i, &cell) in self.atom_cells.iter().enumerate() {
            self.indices[self.cursors[cell]] = i;
            self.cursors[cell] += 1;
        }

        // Sorting amortizes over many target queries, but costs more than it saves
        // for a small ligand against a large background. Use it only for large targets
        // that constitute at least one eighth of the combined atom set.
        let sorted = t >= 1024 && t >= self.proj.len().div_ceil(8);
        if sorted {
            for cell in 0..cells {
                self.indices[self.offsets[cell]..self.offsets[cell + 1]]
                    .sort_unstable_by(|&a, &b| self.proj[b][2].total_cmp(&self.proj[a][2]));
            }
        }

        let eps = 1e-4_f32;
        let mut visible = 0u32;
        // The same cell is the most likely place to find a blocker.
        const NEIGHBOURS: [(isize, isize); 9] = [
            (0, 0),
            (-1, -1),
            (0, -1),
            (1, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ];
        for i in 0..t {
            if i % 256 == 0 && cancelled() {
                return Err("cancelled".into());
            }
            let [ui, vi, di, _] = self.proj[i];
            let cell = self.atom_cells[i];
            let cx = (cell % nx) as isize;
            let cy = (cell / nx) as isize;
            let mut hidden = false;
            'search: for (dx, dy) in NEIGHBOURS {
                let gx = cx + dx;
                let gy = cy + dy;
                if gx < 0 || gy < 0 || gx >= nx as isize || gy >= ny as isize {
                    continue;
                }
                let cell = gy as usize * nx + gx as usize;
                if self.max_depth[cell] <= di + eps {
                    continue;
                }
                for &j in &self.indices[self.offsets[cell]..self.offsets[cell + 1]] {
                    let [uj, vj, dj, rj] = self.proj[j];
                    if dj <= di + eps {
                        if sorted {
                            break;
                        }
                        continue;
                    }
                    // The depth condition excludes the query atom itself.
                    let du = ui - uj;
                    let dv = vi - vj;
                    if du * du + dv * dv < rj * rj {
                        hidden = true;
                        break 'search;
                    }
                }
            }
            if !hidden && !mesh.is_some_and(|mesh| mesh.blocks(target[i].0, d)) {
                visible += 1;
            }
        }
        Ok(visible)
    }
}

#[cfg(test)]
pub(crate) fn visible_count(target: &[(Vec3, f32)], occluders: &[(Vec3, f32)], d: Vec3) -> u32 {
    VisibilityScratch::default().visible_count(target, occluders, d)
}

/// Pick the view direction that shows the most target atoms un-occluded. Returns a unit
/// vector `d` pointing from the target toward the camera (see the module docs). Atoms
/// are `(position_nm, vdw_radius_nm)`.
///
/// `resolution` is the number of directions in the coarse Fibonacci-sphere pass (clamped
/// to at least 8); higher is more thorough but slower. A local refine follows regardless.
pub fn best_unobstructed_direction(
    target: &[(Vec3, f32)],
    occluders: &[(Vec3, f32)],
    resolution: usize,
) -> Vec3 {
    best_unobstructed_direction_cancellable(target, occluders, resolution, &|| false).unwrap()
}

/// Cancellation is checked between directions and within long scoring loops.
pub(crate) fn best_unobstructed_direction_cancellable(
    target: &[(Vec3, f32)],
    occluders: &[(Vec3, f32)],
    resolution: usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec3, String> {
    best_direction_with_triangles_cancellable(target, occluders, &[], resolution, cancelled)
}

/// Mesh reps occlude with their actual triangles rather than their atom spheres.
pub(crate) fn best_direction_with_triangles_cancellable(
    target: &[(Vec3, f32)],
    occluders: &[(Vec3, f32)],
    triangles: &[[Vec3; 3]],
    resolution: usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec3, String> {
    if cancelled() {
        return Err("cancelled".into());
    }
    let mesh = TriangleOccluders::new(triangles);
    if target.is_empty() {
        return Ok(Vec3::Z);
    }
    let mut scratch = VisibilityScratch::default();
    search_directions(resolution, |dirs| {
        dirs.iter()
            .map(|&d| {
                if cancelled() {
                    return Err("cancelled".into());
                }
                scratch.visible_count_cancellable(target, occluders, d, Some(&mesh), cancelled)
            })
            .collect()
    })
}

/// Both scorers use the same directions, tie order and three refinement rings.
/// A batch scorer allows the GPU to evaluate each pass with one score readback.
pub(crate) fn search_directions<E, S: Copy + Ord>(
    resolution: usize,
    score: impl FnMut(&[Vec3]) -> Result<Vec<S>, E>,
) -> Result<Vec3, E> {
    search_directions_with_seeds(resolution, 1, score)
}

fn search_directions_with_seeds<E, S: Copy + Ord>(
    resolution: usize,
    seeds: usize,
    mut score: impl FnMut(&[Vec3]) -> Result<Vec<S>, E>,
) -> Result<Vec3, E> {
    let dirs = fibonacci_sphere(resolution.max(8));
    let scores = score(&dirs)?;
    debug_assert_eq!(scores.len(), dirs.len());
    let mut ranked = dirs.into_iter().zip(scores).collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    let mut candidates: Vec<(Vec3, S)> = Vec::new();
    for candidate in ranked {
        if candidates.iter().all(|(d, _)| d.dot(candidate.0) < 15_f32.to_radians().cos()) {
            candidates.push(candidate);
            if candidates.len() >= seeds { break; }
        }
    }
    for &ring_deg in &[10.0_f32, 4.0, 1.5] {
        let tilt = ring_deg.to_radians();
        let (c, s) = (tilt.cos(), tilt.sin());
        let dirs: Vec<_> = candidates.iter().flat_map(|&(base, _)| {
            let (t1, t2) = tangent_basis(base);
            (0..16).map(move |k| {
                let az = k as f32 / 16.0 * TAU;
                let tdir = t1 * az.cos() + t2 * az.sin();
                (base * c + tdir * s).normalize()
            }) })
            .collect();
        let scores = score(&dirs)?;
        debug_assert_eq!(scores.len(), dirs.len());
        for (candidate, (dirs, scores)) in candidates.iter_mut().zip(dirs.chunks(16).zip(scores.chunks(16))) {
            for (&d, &s) in dirs.iter().zip(scores) {
                if s > candidate.1 { *candidate = (d, s); }
            }
        }
    }
    let mut best = candidates[0];
    for candidate in candidates.into_iter().skip(1) {
        if candidate.1 > best.1 { best = candidate; }
    }
    Ok(best.0.normalize_or_zero())
}

/// Camera orientation (a `Quat`) that looks at the target from direction `d`: the eye
/// sits on the `+d` side and looks back along `-d`. Matches the viewer's convention
/// `eye = target + orientation * (Z * distance)` (so `orientation * Z == d`), with the
/// world up axis kept upright (falling back to world Z when `d` is near-vertical).
pub fn look_along_quat(d: Vec3) -> Quat {
    let d = d.normalize_or_zero();
    if d == Vec3::ZERO {
        return Quat::IDENTITY;
    }
    let up0 = if d.y.abs() < 0.95 { Vec3::Y } else { Vec3::Z };
    let right = up0.cross(d).normalize();
    let up = d.cross(right);
    Quat::from_mat3(&Mat3::from_cols(right, up, d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_discards_partial_direction_search() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let target = vec![(Vec3::ZERO, 0.2); 4096];
        let cancelled = || {
            calls.set(calls.get() + 1);
            calls.get() >= 4
        };
        assert!(best_unobstructed_direction_cancellable(&target, &[], 256, &cancelled).is_err());
        assert_eq!(calls.get(), 4);
    }

    #[test]
    fn cancellable_search_preserves_direction() {
        let target = [(Vec3::ZERO, 0.2), (Vec3::Z, 0.2)];
        let expected = search_directions(32, |dirs| {
            Ok::<_, String>(
                dirs.iter()
                    .map(|&d| brute_visible(&target, &[], d))
                    .collect(),
            )
        })
        .unwrap();
        assert_eq!(
            best_unobstructed_direction_cancellable(&target, &[], 32, &|| false).unwrap(),
            expected
        );
    }

    // Independent pairwise oracle for the current centre/depth visibility rule.
    fn brute_visible(target: &[(Vec3, f32)], occluders: &[(Vec3, f32)], d: Vec3) -> u32 {
        let (u, v) = tangent_basis(d);
        target
            .iter()
            .filter(|&&(p, _)| {
                !target.iter().chain(occluders).any(|&(q, radius)| {
                    let du = p.dot(u) - q.dot(u);
                    let dv = p.dot(v) - q.dot(v);
                    q.dot(d) > p.dot(d) + 1e-4 && du * du + dv * dv < radius * radius
                })
            })
            .count() as u32
    }

    #[test]
    fn reused_grid_matches_pairwise_visibility_for_large_and_small_targets() {
        let mut scratch = VisibilityScratch::default();
        let mut seed = 17u64;
        let mut random = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 40) as f32 / (1u32 << 24) as f32
        };
        // Exercise sorted and unsorted buckets, varying radii and scratch resizing.
        for (t, o) in [(1050, 200), (32, 1200), (1050, 0), (1, 0), (0, 0)] {
            let atoms: Vec<_> = (0..t + o)
                .map(|_| {
                    (
                        Vec3::new(random(), random(), random()) * 3.0 - Vec3::ONE,
                        0.05 + random() * 0.25,
                    )
                })
                .collect();
            let (target, occluders) = atoms.split_at(t);
            for d in [
                Vec3::X,
                Vec3::Y,
                Vec3::NEG_Z,
                Vec3::new(0.3, -0.7, 0.65).normalize(),
            ] {
                assert_eq!(
                    scratch.visible_count(target, occluders, d),
                    brute_visible(target, occluders, d),
                    "t={t}, o={o}, d={d:?}"
                );
            }
        }
    }

    #[test]
    fn equal_depths_and_visibility_boundaries_match_pairwise_rule() {
        let target = [
            (Vec3::ZERO, 0.2),
            (Vec3::ZERO, 0.1),
            (Vec3::new(0.2, 0.0, 0.0), 0.1),
        ];
        for depth in [0.0, 1e-4, 1.01e-4, 0.1] {
            let occluders = [(Vec3::new(0.0, 0.0, depth), 0.2)];
            assert_eq!(
                visible_count(&target, &occluders, Vec3::Z),
                brute_visible(&target, &occluders, Vec3::Z)
            );
        }
    }

    #[test]
    fn look_along_maps_local_z_to_direction() {
        let d = Vec3::new(0.3, -0.7, 0.65).normalize();
        let q = look_along_quat(d);
        let z = q * Vec3::Z;
        assert!(
            (z - d).length() < 1e-4,
            "orientation*Z should equal d, got {z:?}"
        );
        // Right-handed orthonormal frame: X × Y == Z.
        let x = q * Vec3::X;
        let y = q * Vec3::Y;
        assert!((x.cross(y) - z).length() < 1e-4);
    }

    #[test]
    fn passed_reps_occlude_within_the_combined_target() {
        // Two one-atom "reps" stacked along Z (this is what `unobstructed_view_multi`
        // hands the scorer: the union of the passed reps as one target, no occluders).
        let group = vec![(Vec3::ZERO, 0.5), (Vec3::new(0.0, 0.0, 2.0), 0.5)];
        let none: Vec<(Vec3, f32)> = Vec::new();

        // Looking along the stack, the front atom hides the back one -> only 1 visible.
        assert_eq!(visible_count(&group, &none, Vec3::Z), 1);
        // Perpendicular, they sit side by side -> both visible.
        assert_eq!(visible_count(&group, &none, Vec3::X), 2);

        // So the search must avoid the stacking axis and reveal both.
        let d = best_unobstructed_direction(&group, &none, 256);
        assert_eq!(
            visible_count(&group, &none, d),
            2,
            "best view should reveal both"
        );
        assert!(
            d.z.abs() < 0.6,
            "best view should not look down the stack, got {d:?}"
        );
    }

    #[test]
    fn finds_the_opening_in_a_shell() {
        // Target: a small 3×3×3 cluster near the origin.
        let mut target = Vec::new();
        for xi in -1..=1 {
            for yi in -1..=1 {
                for zi in -1..=1 {
                    let p = Vec3::new(xi as f32, yi as f32, zi as f32) * 0.15;
                    target.push((p, 0.1));
                }
            }
        }
        // Occluders: a dense sphere shell (radius 1.2) around the target, with a hole —
        // shell points within 30° of -X are removed. Only a view from -X sees the
        // cluster through the hole.
        let hole = Vec3::NEG_X;
        let cos_hole = 30_f32.to_radians().cos();
        let occ: Vec<(Vec3, f32)> = fibonacci_sphere(320)
            .into_iter()
            .filter(|u| u.dot(hole) < cos_hole) // keep everything except the hole cap
            .map(|u| (u * 1.2, 0.25))
            .collect();

        let d = best_unobstructed_direction(&target, &occ, 256);
        // The best view must look through the hole, i.e. from the -X side.
        assert!(d.x < -0.6, "expected a view from the -X opening, got {d:?}");
        // Sanity: that direction really is much clearer than the blocked +X side.
        assert!(
            visible_count(&target, &occ, d) > visible_count(&target, &occ, Vec3::X),
            "the opening must beat the blocked side"
        );
    }
}

struct TriangleOccluders {
    triangles: Vec<[Vec3; 3]>,
    nodes: Vec<TriangleNode>,
}
struct TriangleNode {
    min: Vec3,
    max: Vec3,
    children: Option<[usize; 2]>,
    range: std::ops::Range<usize>,
}
impl TriangleOccluders {
    fn new(triangles: &[[Vec3; 3]]) -> Self {
        let mut mesh = Self {
            triangles: triangles.to_vec(),
            nodes: Vec::new(),
        };
        if !triangles.is_empty() {
            mesh.build(0, triangles.len());
        }
        mesh
    }
    fn build(&mut self, start: usize, end: usize) -> usize {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for triangle in &self.triangles[start..end] {
            for &p in triangle {
                min = min.min(p);
                max = max.max(p);
            }
        }
        let index = self.nodes.len();
        self.nodes.push(TriangleNode {
            min,
            max,
            children: None,
            range: start..end,
        });
        if end - start > 8 {
            let extent = max - min;
            let axis = if extent.x >= extent.y && extent.x >= extent.z {
                0
            } else if extent.y >= extent.z {
                1
            } else {
                2
            };
            let mid = start + (end - start) / 2;
            self.triangles[start..end].select_nth_unstable_by(mid - start, |a, b| {
                let center = |t: &[Vec3; 3]| (t[0][axis] + t[1][axis] + t[2][axis]) / 3.0;
                center(a).total_cmp(&center(b))
            });
            let left = self.build(start, mid);
            let right = self.build(mid, end);
            self.nodes[index].children = Some([left, right]);
        }
        index
    }
    fn blocks(&self, origin: Vec3, direction: Vec3) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        fn box_hit(min: Vec3, max: Vec3, origin: Vec3, direction: Vec3) -> bool {
            let mut near = 0.0_f32;
            let mut far = f32::INFINITY;
            for axis in 0..3 {
                if direction[axis].abs() < 1e-8 {
                    if origin[axis] < min[axis] - 1e-6 || origin[axis] > max[axis] + 1e-6 {
                        return false;
                    }
                } else {
                    let a = (min[axis] - origin[axis]) / direction[axis];
                    let b = (max[axis] - origin[axis]) / direction[axis];
                    near = near.max(a.min(b));
                    far = far.min(a.max(b));
                    if near > far + 1e-6 {
                        return false;
                    }
                }
            }
            far > 1e-5
        }
        fn triangle_hit(t: &[Vec3; 3], o: Vec3, d: Vec3) -> bool {
            let e1 = t[1] - t[0];
            let e2 = t[2] - t[0];
            let p = d.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1e-9 {
                return false;
            }
            let v = o - t[0];
            let u = v.dot(p) / det;
            if !(-1e-6..=1.0 + 1e-6).contains(&u) {
                return false;
            }
            let q = v.cross(e1);
            let w = d.dot(q) / det;
            w >= -1e-6 && u + w <= 1.0 + 1e-6 && e2.dot(q) / det > 1e-5
        }
        let mut stack = vec![0];
        while let Some(index) = stack.pop() {
            let node = &self.nodes[index];
            if !box_hit(node.min, node.max, origin, direction) {
                continue;
            }
            if let Some([left, right]) = node.children {
                stack.push(left);
                stack.push(right);
            } else if self.triangles[node.range.clone()]
                .iter()
                .any(|t| triangle_hit(t, origin, direction))
            {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod triangle_tests {
    use super::*;

    #[test]
    fn mesh_silhouettes_occlude_even_when_no_atom_proxy_is_near_the_ray() {
        let triangles = [[
            Vec3::new(-100.0, -100.0, 1.0),
            Vec3::new(100.0, -100.0, 1.0),
            Vec3::new(0.0, 100.0, 1.0),
        ]];
        let mesh = TriangleOccluders::new(&triangles);
        assert!(mesh.blocks(Vec3::ZERO, Vec3::Z));
        assert!(!mesh.blocks(Vec3::ZERO, -Vec3::Z));
        let target = [(Vec3::ZERO, 0.1)];
        assert_eq!(
            VisibilityScratch::default()
                .visible_count_cancellable(&target, &[], Vec3::Z, Some(&mesh), &|| false)
                .unwrap(),
            0
        );
        let direction =
            best_direction_with_triangles_cancellable(&target, &[], &triangles, 256, &|| false)
                .unwrap();
        assert!(!mesh.blocks(Vec3::ZERO, direction));
    }
    #[test]
    fn triangle_bvh_matches_brute_force_hits_in_different_regions() {
        let triangles = (0..30)
            .map(|i| {
                let x = i as f32 * 3.0;
                [
                    Vec3::new(x - 1.0, -1.0, 1.0),
                    Vec3::new(x + 1.0, -1.0, 1.0),
                    Vec3::new(x, 1.0, 1.0),
                ]
            })
            .collect::<Vec<_>>();
        let mesh = TriangleOccluders::new(&triangles);
        for x in 0..90 {
            let origin = Vec3::new(x as f32, 0.0, 0.0);
            let expected = triangles
                .iter()
                .any(|t| TriangleOccluders::new(&[*t]).blocks(origin, Vec3::Z));
            assert_eq!(mesh.blocks(origin, Vec3::Z), expected);
        }
    }
}
