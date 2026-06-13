//! CPU geometry builders: turn a molecule's atom data (read directly from its
//! molar `System` via a bound all-selection) + a selection + representation into
//! GPU instance/vertex data. Bonds are split at their midpoint into two
//! half-bonds, each colored by its endpoint atom (VMD-style half-bond coloring).
//! Only selected atoms (and bonds whose endpoints are both selected) are emitted.

use molar::prelude::*;

use crate::color::{ColorMethod, Colorizer};
use crate::render::{CylinderInstance, LineVertex, MeshVertex, MetaballAtom, SphereInstance};
use crate::secstruct::SsMap;

mod cartoon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepKind {
    Vdw,
    Licorice,
    BallAndStick,
    Lines,
    Cartoon,
    Metaball,
}

impl RepKind {
    pub const ALL: [RepKind; 6] = [
        RepKind::Vdw,
        RepKind::Licorice,
        RepKind::BallAndStick,
        RepKind::Lines,
        RepKind::Cartoon,
        RepKind::Metaball,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RepKind::Vdw => "VDW",
            RepKind::Licorice => "Licorice",
            RepKind::BallAndStick => "Ball and Stick",
            RepKind::Lines => "Lines",
            RepKind::Cartoon => "Cartoon",
            RepKind::Metaball => "Metaball",
        }
    }

    /// Parse a rep name (used by the `VMD_RS_DEBUG_REP` verification hook).
    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "vdw" => Some(RepKind::Vdw),
            "licorice" => Some(RepKind::Licorice),
            "ballstick" | "ball_and_stick" | "ball-and-stick" => Some(RepKind::BallAndStick),
            "lines" => Some(RepKind::Lines),
            "cartoon" => Some(RepKind::Cartoon),
            "metaball" => Some(RepKind::Metaball),
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
    Metaball {
        /// VDW radius multiplier for each atom's kernel (fused blobs; <1 shrinks).
        radius_scale: f32,
        /// Density-grid voxel edge (nm); smaller = smoother but heavier.
        resolution: f32,
        /// Isosurface density threshold (lower = fatter, more fused).
        isovalue: f32,
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
            RepKind::Metaball => RepParams::Metaball {
                radius_scale: 0.8,
                resolution: 0.08,
                isovalue: 0.1,
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
    pub metaball: Option<MetaballData>,
}

/// An indexed triangle mesh (Cartoon representation).
#[derive(Default)]
pub struct MeshData {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
}

/// Inputs for the GPU metaball pass: the selected atoms (center, kernel radius,
/// color) plus the density-volume grid covering them. The volume is baked and
/// ray-marched entirely on the GPU; the CPU only fills this.
#[derive(Default)]
pub struct MetaballData {
    pub atoms: Vec<MetaballAtom>,
    /// World-space min corner of the density volume.
    pub origin: [f32; 3],
    /// Voxel edge length (nm).
    pub voxel: f32,
    /// Grid dimensions (voxels per axis).
    pub dims: [u32; 3],
    /// Isosurface density threshold.
    pub isovalue: f32,
}

/// Build GPU geometry for one representation. `sel` is bound to its `System` to
/// read positions/atoms straight from the live `State`/`Topology` (nothing is
/// cached). Spheres come from the selected atoms; bonds are emitted only where
/// both endpoints are selected.
pub fn build(
    system: &System,
    sel: &Sel,
    bonds: &[[usize; 2]],
    params: &RepParams,
    color: ColorMethod,
    ss_algo: SsAlgorithm,
) -> GeometryData {
    let bound = system.bind(sel);
    // Secondary structure is needed for the Cartoon shape and the SecStruct
    // color scheme; the algorithm is chosen per-rep (`ss_algo`).
    let ss = (matches!(params, RepParams::Cartoon { .. }) || color.needs_ss())
        .then(|| SsMap::compute(&bound, ss_algo));
    let colorizer = Colorizer::new(color, &bound, system.len(), ss.as_ref());
    match *params {
        RepParams::Vdw => GeometryData {
            spheres: spheres(&bound, &colorizer, |a| a.vdw()),
            ..Default::default()
        },
        RepParams::Licorice { bond_radius } => {
            let lut = selected_lut(&bound, &colorizer, system.len());
            GeometryData {
                spheres: spheres(&bound, &colorizer, |_| bond_radius),
                cylinders: cylinders(&lut, bonds, bond_radius),
                ..Default::default()
            }
        }
        RepParams::BallAndStick { sphere_scale, bond_radius } => {
            let lut = selected_lut(&bound, &colorizer, system.len());
            GeometryData {
                spheres: spheres(&bound, &colorizer, |a| a.vdw() * sphere_scale),
                cylinders: cylinders(&lut, bonds, bond_radius),
                ..Default::default()
            }
        }
        RepParams::Lines => {
            let lut = selected_lut(&bound, &colorizer, system.len());
            GeometryData {
                lines: lines(&lut, bonds),
                ..Default::default()
            }
        }
        RepParams::Cartoon { coil_radius, ribbon_width, ribbon_thickness } => {
            let ss = ss.as_ref().expect("ss computed for cartoon");
            GeometryData {
                mesh: cartoon::build(
                    &bound,
                    &colorizer,
                    ss,
                    coil_radius,
                    ribbon_width,
                    ribbon_thickness,
                ),
                ..Default::default()
            }
        }
        RepParams::Metaball { radius_scale, resolution, isovalue } => GeometryData {
            metaball: build_metaball(&bound, &colorizer, radius_scale, resolution, isovalue),
            ..Default::default()
        },
    }
}

/// Kernel support radius as a multiple of each atom's kernel radius `R` (the
/// Gaussian `exp(-K·(d/R)²)` is ~0 beyond this). Must match `metaball_bake.wgsl`.
const METABALL_CUTOFF: f32 = 2.5;
/// Cap on total voxels (≈96 MB at 16 B/voxel) — keeps the volume buffer within
/// the default `max_storage_buffer_binding_size`; resolution is coarsened to fit.
const METABALL_MAX_VOXELS: f32 = 6_000_000.0;

/// Collect the selected atoms (center, kernel radius = VDW·`radius_scale`, color)
/// and size a density grid around them. The volume is baked + ray-marched on the
/// GPU; this only assembles the inputs.
fn build_metaball(
    bound: &impl ParticleIterProvider,
    colorizer: &Colorizer,
    radius_scale: f32,
    resolution: f32,
    isovalue: f32,
) -> Option<MetaballData> {
    let mut atoms = Vec::new();
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    let mut max_support = 0.0f32;
    for p in bound.iter_particle() {
        let r = (p.atom.vdw() * radius_scale).max(1e-3);
        let c = [p.pos.x, p.pos.y, p.pos.z];
        let col = colorizer.color(p.atom, p.id);
        let rgb = [
            (col & 0xff) as f32 / 255.0,
            ((col >> 8) & 0xff) as f32 / 255.0,
            ((col >> 16) & 0xff) as f32 / 255.0,
        ];
        atoms.push(MetaballAtom {
            center_radius: [c[0], c[1], c[2], r],
            color: [rgb[0], rgb[1], rgb[2], 1.0],
        });
        max_support = max_support.max(r * METABALL_CUTOFF);
        for k in 0..3 {
            lo[k] = lo[k].min(c[k]);
            hi[k] = hi[k].max(c[k]);
        }
    }
    if atoms.is_empty() {
        return None;
    }

    // Pad the box so every kernel's support fits inside the volume.
    let pad = max_support;
    let origin = [lo[0] - pad, lo[1] - pad, lo[2] - pad];
    let extent = [
        hi[0] - lo[0] + 2.0 * pad,
        hi[1] - lo[1] + 2.0 * pad,
        hi[2] - lo[2] + 2.0 * pad,
    ];

    // Choose a voxel size, coarsening if the grid would be too large.
    let mut voxel = resolution.max(0.01);
    let dims_at = |v: f32| {
        [
            (extent[0] / v).ceil() as u32 + 1,
            (extent[1] / v).ceil() as u32 + 1,
            (extent[2] / v).ceil() as u32 + 1,
        ]
    };
    let mut dims = dims_at(voxel);
    let total = dims[0] as f32 * dims[1] as f32 * dims[2] as f32;
    if total > METABALL_MAX_VOXELS {
        voxel *= (total / METABALL_MAX_VOXELS).cbrt();
        dims = dims_at(voxel);
    }

    Some(MetaballData { atoms, origin, voxel, dims, isovalue })
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
            v.push(CylinderInstance { p0: pa, radius, p1: m, color: ca });
            v.push(CylinderInstance { p0: m, radius, p1: pb, color: cb });
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
