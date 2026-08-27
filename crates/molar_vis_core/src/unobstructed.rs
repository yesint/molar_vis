//! Unobstructed-view geometry — pure, GPU-free, wasm-safe.
//!
//! Given a target representation's atoms and the atoms of every other rep that could
//! hide it, this picks the camera view direction that leaves the **most target atoms
//! directly visible** — front-most, hidden by neither another rep nor another atom of
//! the target rep itself ("most of the rep is on screen").
//!
//! Every atom is modeled as a van-der-Waals sphere and projected **orthographically**
//! (the viewer's default projection). A target atom counts as visible along a view
//! direction `d` when no other atom nearer the camera covers its projected centre. The
//! score is that visible count; the search maximizes it over a Fibonacci sphere of
//! directions plus a local refine.
//!
//! `d` is the world direction from the target toward the camera: the camera sits on the
//! `+d` side and looks back along `-d`, so a larger `dot(pos, d)` means nearer the
//! camera. [`look_along_quat`] turns `d` into the camera orientation used by
//! `App::unobstructed_view`.

use glam::{Mat3, Quat, Vec3};
use std::f32::consts::TAU;

/// Uniform directions on the unit sphere (Fibonacci lattice) for the coarse search.
fn fibonacci_sphere(n: usize) -> Vec<Vec3> {
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
fn tangent_basis(d: Vec3) -> (Vec3, Vec3) {
    let a = if d.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let t1 = a.cross(d).normalize();
    let t2 = d.cross(t1);
    (t1, t2)
}

/// Count TARGET atoms that are front-most (un-occluded) when the scene is viewed from
/// direction `d`. Atoms (target + occluders) are vdW spheres projected orthographically;
/// a target atom is hidden when any other atom nearer the camera covers its projected
/// centre. A target atom hidden by a nearer atom of the target rep is also counted as
/// hidden (only the front one of an overlapping pair shows).
fn visible_count(target: &[(Vec3, f32)], occluders: &[(Vec3, f32)], d: Vec3) -> u32 {
    let t = target.len();
    if t == 0 {
        return 0;
    }
    let (t1, t2) = tangent_basis(d);

    // Combined projection: target atoms first (0..t), then occluders. proj = [u, v,
    // depth, radius]; depth grows toward the camera.
    let mut proj: Vec<[f32; 4]> = Vec::with_capacity(t + occluders.len());
    let (mut umin, mut umax) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut vmin, mut vmax) = (f32::INFINITY, f32::NEG_INFINITY);
    let mut rmax = 1e-3_f32;
    for (p, r) in target.iter().chain(occluders.iter()) {
        let u = p.dot(t1);
        let v = p.dot(t2);
        proj.push([u, v, p.dot(d), *r]);
        umin = umin.min(u);
        umax = umax.max(u);
        vmin = vmin.min(v);
        vmax = vmax.max(v);
        rmax = rmax.max(*r);
    }

    // Uniform screen-space grid for neighbour queries (no brute-force pairwise). Cell =
    // 2·rmax, so every sphere that can cover a queried point sits in the 3×3 block
    // around that point's cell.
    let cell = (2.0 * rmax).max(1e-3);
    let inv = 1.0 / cell;
    let nx = (((umax - umin) * inv).ceil() as i32 + 1).max(1);
    let ny = (((vmax - vmin) * inv).ceil() as i32 + 1).max(1);
    let cell_of = |u: f32, v: f32| -> (i32, i32) {
        (
            (((u - umin) * inv).floor() as i32).clamp(0, nx - 1),
            (((v - vmin) * inv).floor() as i32).clamp(0, ny - 1),
        )
    };
    let mut grid: Vec<Vec<u32>> = vec![Vec::new(); (nx * ny) as usize];
    for (k, pr) in proj.iter().enumerate() {
        let (cx, cy) = cell_of(pr[0], pr[1]);
        grid[(cy * nx + cx) as usize].push(k as u32);
    }

    let eps = 1e-4_f32;
    let mut visible = 0u32;
    for i in 0..t {
        let [ui, vi, di, _ri] = proj[i];
        let (cx, cy) = cell_of(ui, vi);
        let mut hidden = false;
        'search: for gy in (cy - 1)..=(cy + 1) {
            if gy < 0 || gy >= ny {
                continue;
            }
            for gx in (cx - 1)..=(cx + 1) {
                if gx < 0 || gx >= nx {
                    continue;
                }
                for &j in &grid[(gy * nx + gx) as usize] {
                    let j = j as usize;
                    if j == i {
                        continue;
                    }
                    let [uj, vj, dj, rj] = proj[j];
                    if dj > di + eps {
                        let du = ui - uj;
                        let dv = vi - vj;
                        if du * du + dv * dv < rj * rj {
                            hidden = true;
                            break 'search;
                        }
                    }
                }
            }
        }
        if !hidden {
            visible += 1;
        }
    }
    visible
}

/// Pick the view direction that shows the most target atoms un-occluded. Returns a unit
/// vector `d` pointing from the target toward the camera (see the module docs). Atoms
/// are `(position_nm, vdw_radius_nm)`.
pub fn best_unobstructed_direction(target: &[(Vec3, f32)], occluders: &[(Vec3, f32)]) -> Vec3 {
    if target.is_empty() {
        return Vec3::Z;
    }
    // Coarse pass over a uniform sphere.
    let mut best = Vec3::Z;
    let mut best_score = 0u32;
    let mut seen = false;
    for d in fibonacci_sphere(256) {
        let s = visible_count(target, occluders, d);
        if !seen || s > best_score {
            best_score = s;
            best = d;
            seen = true;
        }
    }
    // Local refine: tilt the current best by shrinking cone angles.
    for &ring_deg in &[10.0_f32, 4.0, 1.5] {
        let base = best;
        let (t1, t2) = tangent_basis(base);
        let tilt = ring_deg.to_radians();
        let (c, s) = (tilt.cos(), tilt.sin());
        let around = 16;
        for k in 0..around {
            let az = k as f32 / around as f32 * TAU;
            let tdir = t1 * az.cos() + t2 * az.sin();
            let dir = (base * c + tdir * s).normalize();
            let sc = visible_count(target, occluders, dir);
            if sc > best_score {
                best_score = sc;
                best = dir;
            }
        }
    }
    best.normalize_or_zero()
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
    fn look_along_maps_local_z_to_direction() {
        let d = Vec3::new(0.3, -0.7, 0.65).normalize();
        let q = look_along_quat(d);
        let z = q * Vec3::Z;
        assert!((z - d).length() < 1e-4, "orientation*Z should equal d, got {z:?}");
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
        let d = best_unobstructed_direction(&group, &none);
        assert_eq!(visible_count(&group, &none, d), 2, "best view should reveal both");
        assert!(d.z.abs() < 0.6, "best view should not look down the stack, got {d:?}");
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

        let d = best_unobstructed_direction(&target, &occ);
        // The best view must look through the hole, i.e. from the -X side.
        assert!(d.x < -0.6, "expected a view from the -X opening, got {d:?}");
        // Sanity: that direction really is much clearer than the blocked +X side.
        assert!(
            visible_count(&target, &occ, d) > visible_count(&target, &occ, Vec3::X),
            "the opening must beat the blocked side"
        );
    }
}
