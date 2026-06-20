//! CPU geometry builders: turn a molecule's atom data (read directly from its
//! molar `System` via a bound all-selection) + a selection + representation into
//! GPU instance/vertex data. Bonds are split at their midpoint into two
//! half-bonds, each colored by its endpoint atom (VMD-style half-bond coloring).
//! Only selected atoms (and bonds whose endpoints are both selected) are emitted.

use molar::prelude::*;

use crate::color::{ColorMethod, Colorizer};
use crate::material::Material;
use crate::render::{CylinderInstance, LineVertex, MeshVertex, SphereInstance};
use crate::secstruct::SsMap;

mod cartoon;
mod surface;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RepKind {
    Vdw,
    Licorice,
    BallAndStick,
    Lines,
    Cartoon,
    Surface,
}

impl RepKind {
    pub const ALL: [RepKind; 6] = [
        RepKind::Vdw,
        RepKind::Licorice,
        RepKind::BallAndStick,
        RepKind::Lines,
        RepKind::Cartoon,
        RepKind::Surface,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RepKind::Vdw => "VDW",
            RepKind::Licorice => "Licorice",
            RepKind::BallAndStick => "Ball and Stick",
            RepKind::Lines => "Lines",
            RepKind::Cartoon => "Cartoon",
            RepKind::Surface => "Surface",
        }
    }

    /// Parse a rep name (used by the `MOLAR_VIS_DEBUG_REP` verification hook).
    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "vdw" => Some(RepKind::Vdw),
            "licorice" => Some(RepKind::Licorice),
            "ballstick" | "ball_and_stick" | "ball-and-stick" => Some(RepKind::BallAndStick),
            "lines" => Some(RepKind::Lines),
            "cartoon" => Some(RepKind::Cartoon),
            "surface" | "surf" | "sas" => Some(RepKind::Surface),
            _ => None,
        }
    }
}

/// Tunable representation parameters (nm). Defaults follow VMD conventions
/// converted to nm (VMD's Å values / 10).
/// Tunable parameters for a representation (nm). Each style carries only the
/// knobs it actually uses; `for_kind` resets to that style's VMD-derived
/// defaults (VMD's Å values / 10). The variant always matches the rep's
/// `RepKind` — switching style replaces it wholesale.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum RepParams {
    Vdw {
        /// VDW radius multiplier (1.0 = true van der Waals radii).
        scale: f32,
    },
    Licorice {
        /// Cylinder + cap radius.
        bond_radius: f32,
    },
    BallAndStick {
        /// VDW radius multiplier for the balls.
        sphere_scale: f32,
        /// Stick radius.
        bond_radius: f32,
    },
    Lines {
        /// Line width in pixels (screen-space, constant at any zoom — like VMD's
        /// line thickness).
        width: f32,
    },
    Cartoon {
        /// Coil/turn tube radius.
        coil_radius: f32,
        /// Helix/sheet ribbon half-width.
        ribbon_width: f32,
        /// Helix/sheet ribbon half-thickness.
        ribbon_thickness: f32,
    },
    Surface {
        /// Probe radius added to each vdW radius (nm). 0 → vdW surface, 0.14 → SAS.
        probe: f32,
        /// Grid resolution level (0 = coarse/fast … 4 = fine/smooth).
        quality: u32,
        /// Distance-field blur passes before isosurfacing (0 = none, more = smoother).
        smoothing: u32,
    },
}

impl RepParams {
    pub fn for_kind(kind: RepKind) -> Self {
        match kind {
            RepKind::Vdw => RepParams::Vdw { scale: 1.0 },
            RepKind::Licorice => RepParams::Licorice { bond_radius: 0.03 },
            RepKind::BallAndStick => RepParams::BallAndStick {
                sphere_scale: 0.25,
                bond_radius: 0.015,
            },
            RepKind::Lines => RepParams::Lines { width: 1.0 },
            RepKind::Cartoon => RepParams::Cartoon {
                coil_radius: 0.03,
                ribbon_width: 0.15,
                ribbon_thickness: 0.03,
            },
            RepKind::Surface => RepParams::Surface {
                probe: 0.14,
                quality: 2,
                // Off by default: the Laplacian mesh pass smooths the normals, so
                // the distance-field blur is now opt-in (extra geometric smoothing).
                smoothing: 0,
            },
        }
    }
}

#[derive(Default)]
pub struct GeometryData {
    pub spheres: Vec<SphereInstance>,
    pub cylinders: Vec<CylinderInstance>,
    pub lines: Vec<LineVertex>,
    pub mesh: MeshData,
}

impl GeometryData {
    /// Concatenate another geometry into this one (mesh indices are offset by the
    /// current vertex count). Used to merge several reps' geometry into a single
    /// buffer set — e.g. the active-selection glow, built per rep but drawn as one.
    pub fn append(&mut self, mut other: GeometryData) {
        let base = self.mesh.vertices.len() as u32;
        self.spheres.append(&mut other.spheres);
        self.cylinders.append(&mut other.cylinders);
        self.lines.append(&mut other.lines);
        self.mesh.vertices.append(&mut other.mesh.vertices);
        self.mesh.vert_res.append(&mut other.mesh.vert_res);
        self.mesh
            .indices
            .extend(other.mesh.indices.iter().map(|i| i + base));
    }
}

/// An indexed triangle mesh (Cartoon representation).
#[derive(Default, Clone)]
pub struct MeshData {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
    /// Per-vertex source residue index (`resindex`), parallel to `vertices`. Only
    /// the Cartoon builder fills this (for selecting the sub-ribbon of given
    /// residues when building the selection glow); empty for other meshes. Not
    /// uploaded to the GPU.
    pub vert_res: Vec<u32>,
}

/// Whether a representation needs secondary structure (for the Cartoon shape or
/// the SecStruct color scheme). The caller computes/caches the [`SsMap`] and
/// passes it to [`build`].
pub fn needs_ss(params: &RepParams, color: ColorMethod) -> bool {
    matches!(params, RepParams::Cartoon { .. }) || color.needs_ss()
}

/// Build GPU geometry for one representation from a `bound` selection (which may
/// be a [`molar::SelBoundParts`] reading coordinates from a trajectory frame held
/// apart from the topology — nothing is copied). `n_atoms` is the molecule's atom
/// count (for the per-atom bond lookup); `ss` is the precomputed secondary
/// structure (required for Cartoon / SecStruct color, else `None`). Spheres come
/// from the selected atoms; bonds are emitted only where both endpoints are selected.
pub fn build(
    bound: &(impl ParticleIterProvider + PosProvider + AtomProvider + BoxProvider),
    n_atoms: usize,
    bonds: &[Bond],
    params: &RepParams,
    color: ColorMethod,
    material: Material,
    ss: Option<&SsMap>,
    dashed_pbc: bool,
) -> GeometryData {
    let colorizer = Colorizer::new(color, bound, n_atoms, ss);
    // The periodic box lets bond rendering use the minimum image, so a bond crossing
    // a box face is drawn as two dashed half-bond stubs (one from each atom toward its
    // partner's nearest image) instead of a long line across the box, and a cartoon
    // ribbon is split at the boundary. Only consult it when the dashed-PBC setting is
    // on — otherwise bonds draw as plain solid half-bonds (the cheap path).
    let pbox = if dashed_pbc { bound.get_box() } else { None };
    let mut data = match *params {
        RepParams::Vdw { scale } => GeometryData {
            spheres: spheres(bound, &colorizer, |a| a.vdw() * scale),
            ..Default::default()
        },
        RepParams::Licorice { bond_radius } => {
            let lut = selected_lut(bound, &colorizer, n_atoms);
            GeometryData {
                spheres: spheres(bound, &colorizer, |_| bond_radius),
                cylinders: cylinders(&lut, bonds, bond_radius, pbox),
                ..Default::default()
            }
        }
        RepParams::BallAndStick { sphere_scale, bond_radius } => {
            let lut = selected_lut(bound, &colorizer, n_atoms);
            GeometryData {
                spheres: spheres(bound, &colorizer, |a| a.vdw() * sphere_scale),
                cylinders: cylinders(&lut, bonds, bond_radius, pbox),
                ..Default::default()
            }
        }
        RepParams::Lines { width } => {
            let lut = selected_lut(bound, &colorizer, n_atoms);
            let mut lines = lines(&lut, bonds, width, pbox);
            // Lines only draws bonds, so a selected atom with no drawn bond (an ion,
            // a lone water, …) would otherwise be invisible. VMD marks such atoms
            // with a tiny cross — emit one per bondless atom, at the same width.
            lines.extend(isolated_crosses(bound, &colorizer, &lut, bonds, width));
            GeometryData {
                lines,
                ..Default::default()
            }
        }
        RepParams::Cartoon { coil_radius, ribbon_width, ribbon_thickness } => {
            let ss = ss.expect("ss computed for cartoon");
            GeometryData {
                mesh: cartoon::build(
                    bound,
                    &colorizer,
                    ss,
                    coil_radius,
                    ribbon_width,
                    ribbon_thickness,
                    pbox,
                ),
                ..Default::default()
            }
        }
        RepParams::Surface { probe, quality, smoothing } => GeometryData {
            mesh: surface::build(bound, &colorizer, probe, quality, smoothing),
            ..Default::default()
        },
    };

    // Stamp the material onto every element: the packed lighting coefficients
    // (`mat`) drive the lit shaders (spheres/cylinders/mesh), and the opacity rides
    // in the color's alpha channel (read by all shaders; the renderer draws
    // transparent reps in a second, depth-write-off, blended pass). Lines are
    // unlit, so they carry opacity only.
    let lighting = material.pack_lighting();
    let a = material.opacity_u8();
    let with_opacity = |c: u32| (c & 0x00ff_ffff) | ((a as u32) << 24);
    for s in &mut data.spheres {
        s.color = with_opacity(s.color);
        s.mat = lighting;
    }
    for c in &mut data.cylinders {
        c.color = with_opacity(c.color);
        c.mat = lighting;
    }
    for l in &mut data.lines {
        l.color = with_opacity(l.color);
    }
    for v in &mut data.mesh.vertices {
        // Multiply (not overwrite) the alpha so a per-vertex fade set by the
        // builder survives — the Cartoon ribbon fades its alpha to 0 at a PBC
        // break (a bond/ribbon crossing the box face); see `cartoon.rs`.
        let va = (v.color >> 24) & 0xff;
        let blended = (va * a as u32) / 255;
        v.color = (v.color & 0x00ff_ffff) | (blended << 24);
        v.mat = lighting;
    }
    data
}

/// The 12 edges of the periodic box as a line list (24 vertices). The box is the
/// parallelepiped spanned by its three lattice vectors from the origin (GROMACS
/// convention). Drawn in a neutral grey.
pub fn box_wireframe(pbox: &PeriodicBox) -> Vec<LineVertex> {
    let m = pbox.get_matrix();
    // Columns of the box matrix are the three lattice vectors a, b, c.
    let a = [m[(0, 0)], m[(1, 0)], m[(2, 0)]];
    let b = [m[(0, 1)], m[(1, 1)], m[(2, 1)]];
    let c = [m[(0, 2)], m[(1, 2)], m[(2, 2)]];
    let corner = |i: f32, j: f32, k: f32| {
        [
            i * a[0] + j * b[0] + k * c[0],
            i * a[1] + j * b[1] + k * c[1],
            i * a[2] + j * b[2] + k * c[2],
        ]
    };
    let color = crate::color::pack_rgba8([170, 170, 170, 255]);
    // 12 edges as corner (i,j,k) pairs: bottom face, top face, then verticals.
    const EDGES: [((f32, f32, f32), (f32, f32, f32)); 12] = [
        ((0., 0., 0.), (1., 0., 0.)),
        ((1., 0., 0.), (1., 1., 0.)),
        ((1., 1., 0.), (0., 1., 0.)),
        ((0., 1., 0.), (0., 0., 0.)),
        ((0., 0., 1.), (1., 0., 1.)),
        ((1., 0., 1.), (1., 1., 1.)),
        ((1., 1., 1.), (0., 1., 1.)),
        ((0., 1., 1.), (0., 0., 1.)),
        ((0., 0., 0.), (0., 0., 1.)),
        ((1., 0., 0.), (1., 0., 1.)),
        ((1., 1., 0.), (1., 1., 1.)),
        ((0., 1., 0.), (0., 1., 1.)),
    ];
    let mut v = Vec::with_capacity(24);
    for ((i0, j0, k0), (i1, j1, k1)) in EDGES {
        v.push(LineVertex { pos: corner(i0, j0, k0), color, width: 1.0 });
        v.push(LineVertex { pos: corner(i1, j1, k1), color, width: 1.0 });
    }
    v
}

/// Half-length (nm) of each arm of the tiny "+" cross drawn for a Lines isolated
/// atom — much lighter than the old sphere dot. (Hover/lasso still treat such an
/// atom with the small Ball-and-Stick sphere radius; see `pick`.)
const LINES_CROSS_HALF_LEN: f32 = 0.025;

/// Tiny "+" crosses for selected atoms that take part in **no drawn bond** (a bond
/// with both endpoints selected). The Lines representation renders only bonds, so
/// without these an isolated atom (ion, lone water, …) would be invisible; VMD
/// marks the same atoms with a small cross. Each cross is three short segments
/// along the x/y/z axes through the atom, drawn at the rep's line `width` and the
/// atom's color. Bonded atoms are already marked by their meeting line segments,
/// so they get no cross.
fn isolated_crosses(
    bound: &impl ParticleIterProvider,
    colorizer: &Colorizer,
    lut: &[Option<([f32; 3], u32)>],
    bonds: &[Bond],
    width: f32,
) -> Vec<LineVertex> {
    // `lut[i].is_some()` == atom i is selected; mark every selected endpoint of a
    // drawn bond as bonded.
    let mut bonded = vec![false; lut.len()];
    for bond in bonds {
        let [a, b] = bond.pair();
        if lut[a].is_some() && lut[b].is_some() {
            bonded[a] = true;
            bonded[b] = true;
        }
    }
    let h = LINES_CROSS_HALF_LEN;
    let mut v = Vec::new();
    for p in bound.iter_particle() {
        if bonded[p.id] {
            continue;
        }
        let c = [p.pos.x, p.pos.y, p.pos.z];
        let color = colorizer.color(p.atom, p.id);
        // One short segment per axis (a 3-D "+" that reads as a cross from any view).
        for axis in 0..3 {
            let (mut lo, mut hi) = (c, c);
            lo[axis] -= h;
            hi[axis] += h;
            v.push(LineVertex { pos: lo, color, width });
            v.push(LineVertex { pos: hi, color, width });
        }
    }
    v
}

fn spheres(
    bound: &impl ParticleIterProvider,
    colorizer: &Colorizer,
    radius: impl Fn(&Atom) -> f32,
) -> Vec<SphereInstance> {
    bound
        .iter_particle()
        .map(|p| SphereInstance {
            center: [p.pos.x, p.pos.y, p.pos.z],
            radius: radius(p.atom),
            color: colorizer.color(p.atom, p.id),
            mat: 0,
            pick: [0, 0],
        })
        .collect()
}

/// Per-atom (position, color) lookup keyed by global atom index, populated only
/// for the selected atoms. `None` doubles as the "not selected" test for bonds.
fn selected_lut(
    bound: &impl ParticleIterProvider,
    colorizer: &Colorizer,
    n_atoms: usize,
) -> Vec<Option<([f32; 3], u32)>> {
    let mut lut = vec![None; n_atoms];
    for p in bound.iter_particle() {
        lut[p.id] = Some(([p.pos.x, p.pos.y, p.pos.z], colorizer.color(p.atom, p.id)));
    }
    lut
}

fn midpoint(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5]
}

/// Dash geometry (nm): a wrapping half-bond stub is split into short solid pieces
/// with gaps so it reads as dashed.
const DASH_LEN: f32 = 0.02;
const DASH_GAP: f32 = 0.015;

/// The two half-bond endpoints for a bond `a→b` plus whether it **crosses a box
/// face** (wraps). A normal bond uses the usual midpoint split (`a → mid`, `b →
/// mid`), drawn solid. A **wrapping** bond is instead drawn as two stubs that run
/// from each atom **to its partner's nearest periodic image** (the full bond
/// toward the image, not beyond it): `a → b_image` and `b → a_image`. These cross
/// opposite box faces, reach where the partner actually is in the nearest cell,
/// never cross the box interior, and are dashed.
fn half_bond_ends(
    pa: [f32; 3],
    pb: [f32; 3],
    pbox: Option<&PeriodicBox>,
    wrap_thresh2: f32,
) -> ([f32; 3], [f32; 3], bool) {
    if let Some(b) = pbox {
        // Fast pre-test: a real covalent bond is short (< the search cutoff), so it
        // can only wrap if the two atoms sit far apart in raw coords. Skip the two
        // `closest_image` calls unless the raw separation exceeds half the box — the
        // overwhelming majority of bonds are non-wrapping, so this is the hot path.
        let (rx, ry, rz) = (pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]);
        if rx * rx + ry * ry + rz * rz > wrap_thresh2 {
            let pa_p = Pos::new(pa[0], pa[1], pa[2]);
            let pb_p = Pos::new(pb[0], pb[1], pb[2]);
            // `closest_image(point, target)` = the image of `point` nearest `target`.
            let b_img = b.closest_image(&pb_p, &pa_p); // image of b nearest a
            let a_img = b.closest_image(&pa_p, &pb_p); // image of a nearest b
            // Wraps iff b's nearest image to a isn't b's real position.
            let (dx, dy, dz) = (b_img.x - pb[0], b_img.y - pb[1], b_img.z - pb[2]);
            if dx * dx + dy * dy + dz * dz > 1e-8 {
                let a_end = [b_img.x, b_img.y, b_img.z];
                let b_end = [a_img.x, a_img.y, a_img.z];
                return (a_end, b_end, true);
            }
        }
    }
    (midpoint(pa, pb), midpoint(pa, pb), false)
}

/// Squared half-box threshold for [`half_bond_ends`]: a bond whose raw separation
/// stays within `½·(shortest lattice vector)` cannot wrap (minimum image == raw),
/// so it skips the `closest_image` work. Computed once per build.
fn wrap_thresh2(b: &PeriodicBox) -> f32 {
    let m = b.get_matrix();
    let len = |j: usize| (m[(0, j)].powi(2) + m[(1, j)].powi(2) + m[(2, j)].powi(2)).sqrt();
    let l_min = len(0).min(len(1)).min(len(2));
    let half = 0.5 * l_min;
    half * half
}

/// Split `p0 → p1` into dash segments (`DASH_LEN` on, `DASH_GAP` off). Always
/// emits at least one segment.
fn dashes(p0: [f32; 3], p1: [f32; 3]) -> Vec<([f32; 3], [f32; 3])> {
    let d = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if len < 1e-6 {
        return Vec::new();
    }
    let dir = [d[0] / len, d[1] / len, d[2] / len];
    let at = |t: f32| [p0[0] + dir[0] * t, p0[1] + dir[1] * t, p0[2] + dir[2] * t];
    let period = DASH_LEN + DASH_GAP;
    let mut out = Vec::new();
    let mut t = 0.0;
    while t < len {
        out.push((at(t), at((t + DASH_LEN).min(len))));
        t += period;
    }
    if out.is_empty() {
        out.push((p0, p1));
    }
    out
}

fn cylinders(
    lut: &[Option<([f32; 3], u32)>],
    bonds: &[Bond],
    radius: f32,
    pbox: Option<&PeriodicBox>,
) -> Vec<CylinderInstance> {
    let wrap2 = pbox.map_or(f32::INFINITY, wrap_thresh2);
    let mut v = Vec::new();
    let mut push = |p0, p1, color, dashed: bool| {
        if dashed {
            for (s, e) in dashes(p0, p1) {
                v.push(CylinderInstance { p0: s, radius, p1: e, color, mat: 0 });
            }
        } else {
            v.push(CylinderInstance { p0, radius, p1, color, mat: 0 });
        }
    };
    for bond in bonds {
        let [a, b] = bond.pair();
        if let (Some((pa, ca)), Some((pb, cb))) = (lut[a], lut[b]) {
            let (a_end, b_end, wrapped) = half_bond_ends(pa, pb, pbox, wrap2);
            push(pa, a_end, ca, wrapped);
            push(pb, b_end, cb, wrapped);
        }
    }
    v
}

fn lines(
    lut: &[Option<([f32; 3], u32)>],
    bonds: &[Bond],
    width: f32,
    pbox: Option<&PeriodicBox>,
) -> Vec<LineVertex> {
    let wrap2 = pbox.map_or(f32::INFINITY, wrap_thresh2);
    let mut v = Vec::new();
    let mut push = |p0: [f32; 3], p1: [f32; 3], color, dashed: bool| {
        if dashed {
            for (s, e) in dashes(p0, p1) {
                v.push(LineVertex { pos: s, color, width });
                v.push(LineVertex { pos: e, color, width });
            }
        } else {
            v.push(LineVertex { pos: p0, color, width });
            v.push(LineVertex { pos: p1, color, width });
        }
    };
    for bond in bonds {
        let [a, b] = bond.pair();
        if let (Some((pa, ca)), Some((pb, cb))) = (lut[a], lut[b]) {
            let (a_end, b_end, wrapped) = half_bond_ends(pa, pb, pbox, wrap2);
            push(pa, a_end, ca, wrapped);
            push(pb, b_end, cb, wrapped);
        }
    }
    v
}
