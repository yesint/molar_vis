//! CPU geometry builders: turn a molecule's per-atom arrays + a selection +
//! representation into GPU instance/vertex data. Bonds are split at their
//! midpoint into two half-bonds, each colored by its endpoint atom (VMD-style
//! half-bond coloring). Only atoms in the selection (and bonds whose endpoints
//! are both selected) are emitted.

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
#[derive(Clone, Copy, Debug)]
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

/// Per-atom arrays of a molecule, borrowed for geometry building.
pub struct AtomArrays<'a> {
    pub positions: &'a [[f32; 3]],
    pub vdw: &'a [f32],
    pub colors: &'a [u32],
    pub bonds: &'a [[usize; 2]],
}

/// Build GPU geometry for one representation over the selected atom indices.
pub fn build(
    atoms: &AtomArrays,
    sel: &[usize],
    kind: RepKind,
    params: &RepParams,
) -> GeometryData {
    // Membership mask so bonds can be filtered to selected endpoints.
    let mut in_sel = vec![false; atoms.positions.len()];
    for &i in sel {
        in_sel[i] = true;
    }

    match kind {
        RepKind::Vdw => GeometryData {
            spheres: spheres(atoms, sel, |i| atoms.vdw[i]),
            ..Default::default()
        },
        RepKind::Licorice => GeometryData {
            spheres: spheres(atoms, sel, |_| params.bond_radius),
            cylinders: cylinders(atoms, &in_sel, params.bond_radius),
            ..Default::default()
        },
        RepKind::BallAndStick => GeometryData {
            spheres: spheres(atoms, sel, |i| atoms.vdw[i] * params.sphere_scale),
            cylinders: cylinders(atoms, &in_sel, params.bond_radius),
            ..Default::default()
        },
        RepKind::Lines => GeometryData {
            lines: lines(atoms, &in_sel),
            ..Default::default()
        },
    }
}

fn spheres(atoms: &AtomArrays, sel: &[usize], radius: impl Fn(usize) -> f32) -> Vec<SphereInstance> {
    sel.iter()
        .map(|&i| SphereInstance {
            center: atoms.positions[i],
            radius: radius(i),
            color: atoms.colors[i],
        })
        .collect()
}

fn midpoint(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5]
}

fn cylinders(atoms: &AtomArrays, in_sel: &[bool], radius: f32) -> Vec<CylinderInstance> {
    let mut v = Vec::new();
    for &[a, b] in atoms.bonds {
        if !(in_sel[a] && in_sel[b]) {
            continue;
        }
        let (pa, pb) = (atoms.positions[a], atoms.positions[b]);
        let m = midpoint(pa, pb);
        v.push(CylinderInstance { p0: pa, radius, p1: m, color: atoms.colors[a] });
        v.push(CylinderInstance { p0: m, radius, p1: pb, color: atoms.colors[b] });
    }
    v
}

fn lines(atoms: &AtomArrays, in_sel: &[bool]) -> Vec<LineVertex> {
    let mut v = Vec::new();
    for &[a, b] in atoms.bonds {
        if !(in_sel[a] && in_sel[b]) {
            continue;
        }
        let (pa, pb) = (atoms.positions[a], atoms.positions[b]);
        let m = midpoint(pa, pb);
        v.push(LineVertex { pos: pa, color: atoms.colors[a] });
        v.push(LineVertex { pos: m, color: atoms.colors[a] });
        v.push(LineVertex { pos: m, color: atoms.colors[b] });
        v.push(LineVertex { pos: pb, color: atoms.colors[b] });
    }
    v
}
