//! CPU geometry builders: turn a molecule's atom data (read directly from its
//! molar `System` via a bound all-selection) + a selection + representation into
//! GPU instance/vertex data. Bonds are split at their midpoint into two
//! half-bonds, each colored by its endpoint atom (VMD-style half-bond coloring).
//! Only selected atoms (and bonds whose endpoints are both selected) are emitted.

use molar::prelude::*;

use crate::color::{ColorMethod, Colorizer};
use crate::render::{CylinderInstance, LineVertex, SphereInstance};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepKind {
    Vdw,
    Licorice,
    BallAndStick,
    Lines,
}

impl RepKind {
    pub const ALL: [RepKind; 4] = [
        RepKind::Vdw,
        RepKind::Licorice,
        RepKind::BallAndStick,
        RepKind::Lines,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RepKind::Vdw => "VDW",
            RepKind::Licorice => "Licorice",
            RepKind::BallAndStick => "Ball and Stick",
            RepKind::Lines => "Lines",
        }
    }

    /// Parse a rep name (used by the `VMD_RS_DEBUG_REP` verification hook).
    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "vdw" => Some(RepKind::Vdw),
            "licorice" => Some(RepKind::Licorice),
            "ballstick" | "ball_and_stick" | "ball-and-stick" => Some(RepKind::BallAndStick),
            "lines" => Some(RepKind::Lines),
            _ => None,
        }
    }
}

/// Tunable representation parameters (nm). Defaults follow VMD conventions
/// converted to nm (VMD's Å values / 10).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RepParams {
    /// VDW radius multiplier for sphere reps.
    pub sphere_scale: f32,
    /// Cylinder (and licorice cap) radius, nm.
    pub bond_radius: f32,
}

impl RepParams {
    pub fn for_kind(kind: RepKind) -> Self {
        match kind {
            RepKind::Vdw => Self { sphere_scale: 1.0, bond_radius: 0.0 },
            RepKind::Licorice => Self { sphere_scale: 1.0, bond_radius: 0.03 },
            RepKind::BallAndStick => Self { sphere_scale: 0.25, bond_radius: 0.015 },
            RepKind::Lines => Self { sphere_scale: 1.0, bond_radius: 0.0 },
        }
    }
}

#[derive(Default)]
pub struct GeometryData {
    pub spheres: Vec<SphereInstance>,
    pub cylinders: Vec<CylinderInstance>,
    pub lines: Vec<LineVertex>,
}

/// Build GPU geometry for one representation. `sel` is bound to its `System` to
/// read positions/atoms straight from the live `State`/`Topology` (nothing is
/// cached). Spheres come from the selected atoms; bonds are emitted only where
/// both endpoints are selected.
pub fn build(
    system: &System,
    sel: &Sel,
    bonds: &[[usize; 2]],
    kind: RepKind,
    params: &RepParams,
    color: ColorMethod,
) -> GeometryData {
    let bound = system.bind(sel);
    let colorizer = Colorizer::new(color, &bound, system.len());
    match kind {
        RepKind::Vdw => GeometryData {
            spheres: spheres(&bound, &colorizer, |a| a.vdw()),
            ..Default::default()
        },
        RepKind::Licorice => {
            let lut = selected_lut(&bound, &colorizer, system.len());
            GeometryData {
                spheres: spheres(&bound, &colorizer, |_| params.bond_radius),
                cylinders: cylinders(&lut, bonds, params.bond_radius),
                ..Default::default()
            }
        }
        RepKind::BallAndStick => {
            let lut = selected_lut(&bound, &colorizer, system.len());
            GeometryData {
                spheres: spheres(&bound, &colorizer, |a| a.vdw() * params.sphere_scale),
                cylinders: cylinders(&lut, bonds, params.bond_radius),
                ..Default::default()
            }
        }
        RepKind::Lines => {
            let lut = selected_lut(&bound, &colorizer, system.len());
            GeometryData {
                lines: lines(&lut, bonds),
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
