//! Secondary-structure assignment shared by the Cartoon representation and the
//! "Secondary Structure" coloring scheme. Wraps molar's DSSP.

use std::collections::BTreeMap;

use molar::prelude::*;

/// A coarse secondary-structure family — what the cartoon cross-section and the
/// SS color scheme actually switch on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SsClass {
    Helix,
    Sheet,
    Coil,
}

/// Collapse molar's 10-variant `SS` into helix / sheet / coil. Isolated β-bridges
/// (`B`) are treated as coil — drawn as sheet they become spurious single-residue
/// arrows (VMD does the same: only extended `E` strands get the arrow ribbon).
pub fn classify(ss: SS) -> SsClass {
    match ss {
        SS::AlphaHelix | SS::Helix310 | SS::PiHelix => SsClass::Helix,
        SS::BetaSheet => SsClass::Sheet,
        _ => SsClass::Coil,
    }
}

/// Per-residue secondary structure, keyed by molar `resindex`.
pub struct SsMap {
    map: std::collections::HashMap<usize, SS>,
}

impl SsMap {
    /// Assign secondary structure on `bound` (PyMOL `dss` algorithm) and key the
    /// per-residue result by `resindex`.
    ///
    /// We use molar's `PymolSS` rather than `Dssp`: DSSP (Kabsch–Sander) over-
    /// extends β-strands relative to VMD/PyMOL (e.g. 2lao 178–185 vs the real
    /// 178–180), which looked wrong in the cartoon. `PymolSS::ss()` is ordered by
    /// ascending residue index, one entry per distinct `resindex`, so it zips 1:1
    /// against the sorted distinct resindices (both from a `BTreeMap` over the
    /// same particles).
    pub fn compute(bound: &(impl ParticleIterProvider + PosProvider), algo: SsAlgorithm) -> Self {
        let mut resindices: BTreeMap<usize, ()> = BTreeMap::new();
        // Coarse-grained (Martini) backbone beads, for the CG SS path. DSSP needs the
        // all-atom backbone (N/CA/C/O), which CG doesn't have, so when the residues are
        // CG `BB` beads (and there's no atomistic `CA`) we infer SS from the BB trace
        // geometry instead (see `assign_cg_ss`).
        let mut bb: BTreeMap<usize, Vector3f> = BTreeMap::new();
        let mut has_ca = false;
        for p in bound.iter_particle() {
            resindices.insert(p.atom.resindex, ());
            match p.atom.name.as_str() {
                "CA" => has_ca = true,
                "BB" => {
                    bb.entry(p.atom.resindex).or_insert(p.pos.coords);
                }
                _ => {}
            }
        }
        if !has_ca && !bb.is_empty() {
            let trace: Vec<(usize, Vector3f)> = bb.into_iter().collect();
            return Self { map: assign_cg_ss(&trace) };
        }
        let ss = match algo {
            SsAlgorithm::Dssp => Dssp::new(bound).ss().to_vec(),
            SsAlgorithm::DsspGmx => Dssp::new_gmx(bound).ss().to_vec(),
            SsAlgorithm::Dss => Dss::new(bound).ss().to_vec(),
        };
        let map = resindices.into_keys().zip(ss).collect();
        Self { map }
    }

    /// Secondary structure of a residue (`Coil` if unknown).
    pub fn get(&self, resindex: usize) -> SS {
        self.map.get(&resindex).copied().unwrap_or(SS::Coil)
    }

    pub fn class(&self, resindex: usize) -> SsClass {
        classify(self.get(resindex))
    }

    /// Iterate `(resindex, ss)` pairs (for building a per-residue color table).
    pub fn entries(&self) -> impl Iterator<Item = (usize, SS)> + '_ {
        self.map.iter().map(|(&k, &v)| (k, v))
    }
}

/// Geometric secondary-structure assignment for a **coarse-grained (Martini)**
/// backbone, where DSSP can't run. `trace` is the per-residue `BB` bead position
/// keyed by `resindex`, ascending. Classification is from the BB trace's *virtual
/// bond angle* θ (∠ BBᵢ₋₁,BBᵢ,BBᵢ₊₁) and *virtual dihedral* τ (over four
/// consecutive BB) — both scale-invariant, so they transfer despite BB spacing
/// (~0.32 nm) differing from Cα (~0.38 nm). The windows were calibrated on a
/// Martini membrane protein (α-helix clusters at θ≈90–110°, τ≈−90…−30°; an
/// extended β-strand at θ≈120–140°, τ≈±180°). Short runs are demoted to coil. The
/// 10-variant `SS` only carries the coarse class here (helix→AlphaHelix,
/// sheet→BetaSheet), which is all the cartoon + SS coloring need.
fn assign_cg_ss(trace: &[(usize, Vector3f)]) -> std::collections::HashMap<usize, SS> {
    let n = trace.len();
    let mut cls = vec![SsClass::Coil; n];
    // Two BB beads belong to the same chain segment iff their residues are
    // consecutive and they're within a plausible bonded distance (a larger gap is a
    // chain break, where the virtual angle/dihedral would be meaningless).
    let contig = |i: usize, j: usize| {
        trace[j].0 == trace[i].0 + 1 && (trace[j].1 - trace[i].1).norm() < 0.6
    };
    for i in 1..n.saturating_sub(2) {
        if !(contig(i - 1, i) && contig(i, i + 1) && contig(i + 1, i + 2)) {
            continue;
        }
        let theta = vangle(trace[i - 1].1, trace[i].1, trace[i + 1].1);
        let tau = vdihedral(trace[i - 1].1, trace[i].1, trace[i + 1].1, trace[i + 2].1);
        // Windows calibrated against mdtraj-DSSP ground truth on a martinized α/β
        // protein (helix clusters at θ≈97°, τ≈−57°; extended β-strand at θ≈137°,
        // τ≈+142° wrapping to −180°).
        if (80.0..=118.0).contains(&theta) && (-100.0..=-20.0).contains(&tau) {
            cls[i] = SsClass::Helix;
        } else if theta >= 122.0 && (tau >= 120.0 || tau <= -150.0) {
            cls[i] = SsClass::Sheet;
        }
    }
    // β-strands pair into sheets: a real strand residue has a **non-sequential
    // partner BB nearby** (the H-bonded neighbour strand). Local geometry alone
    // can't tell an isolated strand from extended coil/a turn (DSSP uses H-bonds,
    // which CG lacks), and those false strands are exactly what we must NOT invent —
    // so drop any extended residue with no partner. (On the ground-truth α/β test
    // this lifts strand precision 0.59 → 0.92.) Antiparallel/parallel sheet BB pairs
    // sit ~0.5 nm apart; 0.6 nm allows for the coarse trace.
    let strand: Vec<usize> = (0..n).filter(|&i| cls[i] == SsClass::Sheet).collect();
    for &i in &strand {
        let paired = strand.iter().any(|&j| {
            (j as isize - i as isize).abs() > 2 && (trace[j].1 - trace[i].1).norm() < 0.6
        });
        if !paired {
            cls[i] = SsClass::Coil;
        }
    }
    // Fill isolated single-residue gaps flanked by the same class (a β-bulge or
    // helix kink shouldn't fragment a strand/helix). Only across contiguous beads.
    for i in 1..n.saturating_sub(1) {
        if cls[i] != cls[i - 1]
            && cls[i - 1] == cls[i + 1]
            && cls[i - 1] != SsClass::Coil
            && contig(i - 1, i)
            && contig(i, i + 1)
        {
            cls[i] = cls[i - 1];
        }
    }
    // A helix needs ≥4 residues; drop shorter helix/strand runs (single stray
    // residues read as spurious stubs/arrows). Strands only need ≥2 here — the
    // pairing filter above already removed the spurious ones.
    let mut i = 0;
    while i < n {
        let c = cls[i];
        let mut j = i + 1;
        while j < n && cls[j] == c {
            j += 1;
        }
        let min = match c {
            SsClass::Helix => 4,
            SsClass::Sheet => 2,
            SsClass::Coil => 0,
        };
        if j - i < min {
            cls[i..j].fill(SsClass::Coil);
        }
        i = j;
    }
    trace
        .iter()
        .zip(cls)
        .map(|(&(ri, _), c)| {
            let ss = match c {
                SsClass::Helix => SS::AlphaHelix,
                SsClass::Sheet => SS::BetaSheet,
                SsClass::Coil => SS::Coil,
            };
            (ri, ss)
        })
        .collect()
}

/// Virtual bond angle (degrees) at `p1`: ∠ p0,p1,p2.
fn vangle(p0: Vector3f, p1: Vector3f, p2: Vector3f) -> f32 {
    let a = p0 - p1;
    let b = p2 - p1;
    let cos = a.dot(&b) / (a.norm() * b.norm()).max(1e-9);
    cos.clamp(-1.0, 1.0).acos().to_degrees()
}

/// Signed virtual dihedral (degrees) over four points p0→p1→p2→p3.
fn vdihedral(p0: Vector3f, p1: Vector3f, p2: Vector3f, p3: Vector3f) -> f32 {
    let b0 = p1 - p0;
    let b1 = p2 - p1;
    let b2 = p3 - p2;
    let n1 = b0.cross(&b1);
    let n2 = b1.cross(&b2);
    let b1n = b1 / b1.norm().max(1e-9);
    let m = n1.cross(&b1n);
    n2.dot(&m).atan2(n2.dot(&n1)).to_degrees()
}

/// VMD-style "Structure" colors as RGBA8 (alpha = purple, 3-10 = blue, pi = red,
/// extended = yellow, bridge = tan, turn = cyan, coil = white).
pub fn ss_color(ss: SS) -> [u8; 4] {
    let rgb = match ss {
        SS::AlphaHelix => [200, 80, 220],  // purple
        SS::Helix310 => [40, 90, 240],     // blue
        SS::PiHelix => [225, 50, 70],      // red
        SS::BetaSheet => [255, 200, 40],   // yellow
        SS::BetaBridge => [180, 150, 60],  // tan
        SS::Turn => [70, 190, 220],        // cyan
        SS::Bend => [60, 200, 120],        // green
        SS::PolyProline => [120, 170, 110],
        SS::Coil | SS::Break => [235, 235, 235], // white
    };
    [rgb[0], rgb[1], rgb[2], 255]
}
