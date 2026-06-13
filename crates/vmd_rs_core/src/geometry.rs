//! CPU geometry builders: turn a molecule's atom data (read directly from its
//! molar `System` via a bound all-selection) + a selection + representation into
//! GPU instance/vertex data. Bonds are split at their midpoint into two
//! half-bonds, each colored by its endpoint atom (VMD-style half-bond coloring).
//! Only selected atoms (and bonds whose endpoints are both selected) are emitted.

use molar::prelude::*;

use crate::color::{ColorMethod, Colorizer};
use crate::render::{CylinderInstance, LineVertex, MeshVertex, SphereInstance};
use crate::secstruct::SsMap;

mod cartoon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepKind {
    Vdw,
    Licorice,
    BallAndStick,
    Lines,
    Cartoon,
}

impl RepKind {
    pub const ALL: [RepKind; 5] = [
        RepKind::Vdw,
        RepKind::Licorice,
        RepKind::BallAndStick,
        RepKind::Lines,
        RepKind::Cartoon,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RepKind::Vdw => "VDW",
            RepKind::Licorice => "Licorice",
            RepKind::BallAndStick => "Ball and Stick",
            RepKind::Lines => "Lines",
            RepKind::Cartoon => "Cartoon",
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
    }
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
