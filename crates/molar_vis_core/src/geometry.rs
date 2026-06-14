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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RepParams {
    Vdw,
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
    Lines,
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
            RepKind::Vdw => RepParams::Vdw,
            RepKind::Licorice => RepParams::Licorice { bond_radius: 0.03 },
            RepKind::BallAndStick => RepParams::BallAndStick {
                sphere_scale: 0.25,
                bond_radius: 0.015,
            },
            RepKind::Lines => RepParams::Lines,
            RepKind::Cartoon => RepParams::Cartoon {
                coil_radius: 0.03,
                ribbon_width: 0.15,
                ribbon_thickness: 0.03,
            },
            RepKind::Surface => RepParams::Surface {
                probe: 0.14,
                quality: 2,
                smoothing: 2,
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

/// An indexed triangle mesh (Cartoon representation).
#[derive(Default)]
pub struct MeshData {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
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
    bound: &(impl ParticleIterProvider + PosProvider + AtomProvider),
    n_atoms: usize,
    bonds: &[[usize; 2]],
    params: &RepParams,
    color: ColorMethod,
    material: Material,
    ss: Option<&SsMap>,
) -> GeometryData {
    let colorizer = Colorizer::new(color, bound, n_atoms, ss);
    let mut data = match *params {
        RepParams::Vdw => GeometryData {
            spheres: spheres(bound, &colorizer, |a| a.vdw()),
            ..Default::default()
        },
        RepParams::Licorice { bond_radius } => {
            let lut = selected_lut(bound, &colorizer, n_atoms);
            GeometryData {
                spheres: spheres(bound, &colorizer, |_| bond_radius),
                cylinders: cylinders(&lut, bonds, bond_radius),
                ..Default::default()
            }
        }
        RepParams::BallAndStick { sphere_scale, bond_radius } => {
            let lut = selected_lut(bound, &colorizer, n_atoms);
            GeometryData {
                spheres: spheres(bound, &colorizer, |a| a.vdw() * sphere_scale),
                cylinders: cylinders(&lut, bonds, bond_radius),
                ..Default::default()
            }
        }
        RepParams::Lines => {
            let lut = selected_lut(bound, &colorizer, n_atoms);
            GeometryData {
                lines: lines(&lut, bonds),
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
        v.color = with_opacity(v.color);
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
        v.push(LineVertex { pos: corner(i0, j0, k0), color });
        v.push(LineVertex { pos: corner(i1, j1, k1), color });
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

fn cylinders(
    lut: &[Option<([f32; 3], u32)>],
    bonds: &[[usize; 2]],
    radius: f32,
) -> Vec<CylinderInstance> {
    let mut v = Vec::new();
    for &[a, b] in bonds {
        if let (Some((pa, ca)), Some((pb, cb))) = (lut[a], lut[b]) {
            let m = midpoint(pa, pb);
            v.push(CylinderInstance { p0: pa, radius, p1: m, color: ca, mat: 0 });
            v.push(CylinderInstance { p0: m, radius, p1: pb, color: cb, mat: 0 });
        }
    }
    v
}

fn lines(lut: &[Option<([f32; 3], u32)>], bonds: &[[usize; 2]]) -> Vec<LineVertex> {
    let mut v = Vec::new();
    for &[a, b] in bonds {
        if let (Some((pa, ca)), Some((pb, cb))) = (lut[a], lut[b]) {
            let m = midpoint(pa, pb);
            v.push(LineVertex { pos: pa, color: ca });
            v.push(LineVertex { pos: m, color: ca });
            v.push(LineVertex { pos: m, color: cb });
            v.push(LineVertex { pos: pb, color: cb });
        }
    }
    v
}
