//! Atom picking by CPU ray-cast, for the **hover-info** pick mode.
//!
//! The ray is intersected against the atoms **as they are displayed** — i.e. at
//! their smoothed and/or periodic-replicated positions, so hovering matches what's
//! on screen — but the resulting [`PickHit`] reports the atom's **real** stored
//! coordinate (current frame, central image, un-smoothed), never a computed one.

use glam::{Mat4, Vec3, Vec4Swizzles};
use molar::prelude::*;

use crate::geometry::RepParams;
use crate::scene::Scene;

/// What the picker does on hover. The dropdown currently exposes only `HoverInfo`
/// (besides `Off`); lasso/selection modes will be added here later.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PickMode {
    /// Picking disabled (no per-hover cost).
    #[default]
    Off,
    /// Show the hovered atom's identity + real coordinates and glow its outline.
    HoverInfo,
}

impl PickMode {
    pub fn label(self) -> &'static str {
        match self {
            PickMode::Off => "Off",
            PickMode::HoverInfo => "Hover info",
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
