//! CPU geometry builders: turn a [`LoadedMolecule`] + representation into GPU
//! instance/vertex data. Bonds are split at their midpoint into two half-bonds,
//! each colored by its endpoint atom (VMD-style half-bond coloring).

use crate::data::LoadedMolecule;
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

/// Build GPU geometry for one representation of `mol`.
pub fn build(mol: &LoadedMolecule, kind: RepKind, params: &RepParams) -> GeometryData {
    match kind {
        RepKind::Vdw => GeometryData {
            spheres: spheres(mol, |i| mol.vdw[i]),
            ..Default::default()
        },
        RepKind::Licorice => GeometryData {
            // Sphere caps at the joints share the cylinder radius.
            spheres: spheres(mol, |_| params.bond_radius),
            cylinders: cylinders(mol, params.bond_radius),
            ..Default::default()
        },
        RepKind::BallAndStick => GeometryData {
            spheres: spheres(mol, |i| mol.vdw[i] * params.sphere_scale),
            cylinders: cylinders(mol, params.bond_radius),
            ..Default::default()
        },
        RepKind::Lines => GeometryData {
            lines: lines(mol),
            ..Default::default()
        },
    }
}

fn spheres(mol: &LoadedMolecule, radius: impl Fn(usize) -> f32) -> Vec<SphereInstance> {
    (0..mol.n_atoms)
        .map(|i| SphereInstance {
            center: mol.positions[i],
            radius: radius(i),
            color: mol.colors[i],
        })
        .collect()
}

fn midpoint(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        (a[0] + b[0]) * 0.5,
        (a[1] + b[1]) * 0.5,
        (a[2] + b[2]) * 0.5,
    ]
}

fn cylinders(mol: &LoadedMolecule, radius: f32) -> Vec<CylinderInstance> {
    let mut v = Vec::with_capacity(mol.bonds.len() * 2);
    for &[a, b] in &mol.bonds {
        let (a, b) = (a as usize, b as usize);
        let (pa, pb) = (mol.positions[a], mol.positions[b]);
        let m = midpoint(pa, pb);
        v.push(CylinderInstance { p0: pa, radius, p1: m, color: mol.colors[a] });
        v.push(CylinderInstance { p0: m, radius, p1: pb, color: mol.colors[b] });
    }
    v
}

fn lines(mol: &LoadedMolecule) -> Vec<LineVertex> {
    let mut v = Vec::with_capacity(mol.bonds.len() * 4);
    for &[a, b] in &mol.bonds {
        let (a, b) = (a as usize, b as usize);
        let (pa, pb) = (mol.positions[a], mol.positions[b]);
        let m = midpoint(pa, pb);
        v.push(LineVertex { pos: pa, color: mol.colors[a] });
        v.push(LineVertex { pos: m, color: mol.colors[a] });
        v.push(LineVertex { pos: m, color: mol.colors[b] });
        v.push(LineVertex { pos: pb, color: mol.colors[b] });
    }
    v
}
