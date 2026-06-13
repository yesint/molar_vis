//! CPU geometry builders: turn a molecule's atom data (read directly from its
//! molar `System` via a bound all-selection) + a selection + representation into
//! GPU instance/vertex data. Bonds are split at their midpoint into two
//! half-bonds, each colored by its endpoint atom (VMD-style half-bond coloring).
//! Only selected atoms (and bonds whose endpoints are both selected) are emitted.

use molar::prelude::*;

use crate::color;
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

/// Source of atom data for geometry building — a molar all-selection bound to the
/// molecule's `System` (so atom index == global index). `get_pos`/`get_atom`
/// read straight from the live `State`/`Topology`; nothing is cached.
pub trait AtomSource: PosProvider + AtomProvider + LenProvider {}
impl<T: PosProvider + AtomProvider + LenProvider> AtomSource for T {}

fn pos3(src: &impl PosProvider, i: usize) -> [f32; 3] {
    let p = src.get_pos(i).expect("atom index in range");
    [p.x, p.y, p.z]
}

fn atom_color(src: &impl AtomProvider, i: usize) -> u32 {
    let a = src.get_atom(i).expect("atom index in range");
    color::pack_rgba8(color::element_color(a.atomic_number))
}

/// Build GPU geometry for one representation over the selected atom indices.
pub fn build(
    src: &impl AtomSource,
    bonds: &[[usize; 2]],
    sel: &[usize],
    kind: RepKind,
    params: &RepParams,
) -> GeometryData {
    // Membership mask so bonds can be filtered to selected endpoints.
    let mut in_sel = vec![false; src.len()];
    for &i in sel {
        in_sel[i] = true;
    }

    match kind {
        RepKind::Vdw => GeometryData {
            spheres: spheres(src, sel, |a| a.vdw()),
            ..Default::default()
        },
        RepKind::Licorice => GeometryData {
            spheres: spheres(src, sel, |_| params.bond_radius),
            cylinders: cylinders(src, bonds, &in_sel, params.bond_radius),
            ..Default::default()
        },
        RepKind::BallAndStick => GeometryData {
            spheres: spheres(src, sel, |a| a.vdw() * params.sphere_scale),
            cylinders: cylinders(src, bonds, &in_sel, params.bond_radius),
            ..Default::default()
        },
        RepKind::Lines => GeometryData {
            lines: lines(src, bonds, &in_sel),
            ..Default::default()
        },
    }
}

fn spheres(
    src: &impl AtomSource,
    sel: &[usize],
    radius: impl Fn(&Atom) -> f32,
) -> Vec<SphereInstance> {
    sel.iter()
        .map(|&i| {
            let a = src.get_atom(i).expect("atom index in range");
            SphereInstance {
                center: pos3(src, i),
                radius: radius(a),
                color: color::pack_rgba8(color::element_color(a.atomic_number)),
            }
        })
        .collect()
}

fn midpoint(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5]
}

fn cylinders(
    src: &impl AtomSource,
    bonds: &[[usize; 2]],
    in_sel: &[bool],
    radius: f32,
) -> Vec<CylinderInstance> {
    let mut v = Vec::new();
    for &[a, b] in bonds {
        if !(in_sel[a] && in_sel[b]) {
            continue;
        }
        let (pa, pb) = (pos3(src, a), pos3(src, b));
        let m = midpoint(pa, pb);
        v.push(CylinderInstance { p0: pa, radius, p1: m, color: atom_color(src, a) });
        v.push(CylinderInstance { p0: m, radius, p1: pb, color: atom_color(src, b) });
    }
    v
}

fn lines(src: &impl AtomSource, bonds: &[[usize; 2]], in_sel: &[bool]) -> Vec<LineVertex> {
    let mut v = Vec::new();
    for &[a, b] in bonds {
        if !(in_sel[a] && in_sel[b]) {
            continue;
        }
        let (pa, pb) = (pos3(src, a), pos3(src, b));
        let m = midpoint(pa, pb);
        v.push(LineVertex { pos: pa, color: atom_color(src, a) });
        v.push(LineVertex { pos: m, color: atom_color(src, a) });
        v.push(LineVertex { pos: m, color: atom_color(src, b) });
        v.push(LineVertex { pos: pb, color: atom_color(src, b) });
    }
    v
}
