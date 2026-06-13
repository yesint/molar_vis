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
    /// Run DSSP on `bound` and key the per-residue result by `resindex`.
    ///
    /// molar's `Dssp::ss()` is ordered by ascending residue index, one entry
    /// per distinct `resindex` present in the selection, so it zips 1:1 against
    /// the sorted distinct resindices (both come from a `BTreeMap` over the same
    /// particles).
    pub fn compute(bound: &(impl ParticleIterProvider + PosProvider)) -> Self {
        let mut resindices: BTreeMap<usize, ()> = BTreeMap::new();
        for p in bound.iter_particle() {
            resindices.insert(p.atom.resindex, ());
        }
        let dssp = Dssp::new(bound);
        let map = resindices
            .into_keys()
            .zip(dssp.ss().iter().copied())
            .collect();
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
