//! Bond guessing. GRO files carry no bonds and PDB only partial ones, so we
//! infer connectivity from interatomic distances using molar's grid distance
//! search, then accept pairs closer than a fraction of the summed VDW radii.

use molar::prelude::*;

/// Distance-search cutoff (nm): an upper bound on any plausible covalent bond.
const SEARCH_CUTOFF: f32 = 0.25;
/// A pair is bonded if `dist < BOND_FACTOR * (vdw_i + vdw_j)`.
const BOND_FACTOR: f32 = 0.6;
/// Reject coincident atoms / duplicate sites below this distance (nm).
const MIN_DIST: f32 = 0.04;

/// Guess bonds for all atoms. `sel` is the bound all-selection (the position
/// source for the grid search); `positions`/`vdw` are the extracted per-atom
/// arrays (nm) used to score candidate pairs.
pub fn guess(sel: &impl PosProvider, positions: &[[f32; 3]], vdw: &[f32]) -> Vec<[usize; 2]> {
    let n = positions.len();
    if n < 2 {
        return Vec::new();
    }

    let candidates: Vec<(usize, usize)> =
        distance_search_single::<(usize, usize), Vec<_>>(SEARCH_CUTOFF, sel, 0..n);

    let min2 = MIN_DIST * MIN_DIST;
    let mut bonds: Vec<[usize; 2]> = Vec::new();
    for (i, j) in candidates {
        if i == j {
            continue;
        }
        let (a, b) = if i < j { (i, j) } else { (j, i) };
        let (pa, pb) = (positions[a], positions[b]);
        let d2 = {
            let dx = pa[0] - pb[0];
            let dy = pa[1] - pb[1];
            let dz = pa[2] - pb[2];
            dx * dx + dy * dy + dz * dz
        };
        let thresh = BOND_FACTOR * (vdw[a] + vdw[b]);
        if d2 > min2 && d2 < thresh * thresh {
            bonds.push([a, b]);
        }
    }

    // The search may report a pair from either cell ordering; dedup.
    bonds.sort_unstable();
    bonds.dedup();
    bonds
}
