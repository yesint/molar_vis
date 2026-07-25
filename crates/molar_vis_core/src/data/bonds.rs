//! Where a molecule's connectivity comes from.
//!
//! Some formats record bonds and some don't, so [`resolve`] owns the policy: trust a
//! chemistry-complete table (SDF/MOL — it carries bond **orders**), union a partial one
//! (PDB `CONECT`) with a distance guess, and fall back to guessing alone (GRO/XYZ).
//! [`guess`] is the distance part: molar's grid search, accepting pairs closer than a
//! fraction of the summed VDW radii. molar has no coordinate-based bond perception of its
//! own, so that part stays here.

use molar::prelude::*;

/// Tunable thresholds for bond guessing, surfaced in the program settings so the
/// user can loosen/tighten connectivity inference. The defaults reproduce the
/// previous hardcoded constants exactly.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BondParams {
    /// A pair is bonded if `dist < factor * (vdw_i + vdw_j)`.
    pub factor: f32,
    /// Distance-search cutoff (nm): an upper bound on any plausible covalent bond.
    pub search_cutoff: f32,
    /// Reject coincident atoms / duplicate sites below this distance (nm).
    pub min_dist: f32,
    /// **Periodic** bond search: when true *and* the structure has a box, use the
    /// minimum-image distance search so covalent bonds crossing a box face (in a
    /// wrapped structure) are found. Off by default — the periodic search is much
    /// slower, and most structures don't need cross-face bonds.
    pub periodic: bool,
}

impl Default for BondParams {
    fn default() -> Self {
        Self { factor: 0.6, search_cutoff: 0.25, min_dist: 0.04, periodic: false }
    }
}

/// The molecule's connectivity, combining what the file recorded with what geometry
/// implies. `file_bonds` is the freshly loaded topology's own bond table; the remaining
/// arguments are [`guess`]'s.
///
/// Three cases, keyed off molar's own signal for "did this format record chemistry":
///
/// * **`file_bonds.has_orders()`** — the source carried explicit bond *orders* (an SDF/MOL
///   bond block), so it is a complete chemistry table. Taken verbatim: distance guessing
///   could only lose the orders, and those orders are what aromatic-ring perception and
///   espaloma charge assignment need (a Kekulé structure can't be recovered from
///   distances).
/// * **bonds but no orders** — a connectivity-only table. In a PDB that's `CONECT`, which
///   records the *exceptions* (ligands, disulfides, metal links) and may or may not cover
///   the whole system — 2 records in `2lao.pdb`, but a complete 32716 in the Martini
///   `cg.pdb`. So it's **unioned** with the distance guess rather than replacing it or
///   being replaced: neither source is trusted to be complete. This is also what finally
///   gives coarse-grained structures their real bonds — CG bead spacing (~0.32 nm) exceeds
///   `search_cutoff`, so distance guessing finds almost none of them.
/// * **no bonds** — GRO/XYZ; the distance guess is all there is.
pub fn resolve(
    file_bonds: &BondStorage,
    sel: &impl PosProvider,
    positions: &[[f32; 3]],
    vdw: &[f32],
    pbox: Option<&PeriodicBox>,
    params: &BondParams,
) -> Vec<Bond> {
    let n = positions.len();
    let from_file = || {
        // Defensive: never let a malformed record index past the atoms we loaded.
        bond_vec(file_bonds).into_iter().filter(|b| b.i1 < n && b.i2 < n && b.i1 != b.i2)
    };
    if file_bonds.has_orders() {
        let mut bonds: Vec<Bond> = from_file().map(normalized).collect();
        dedup_keeping_order(&mut bonds);
        return bonds;
    }
    let mut bonds = guess(sel, positions, vdw, pbox, params);
    if !file_bonds.is_empty() {
        bonds.extend(from_file().map(normalized));
        dedup_keeping_order(&mut bonds);
    }
    bonds
}

/// A bond with its endpoints in ascending order, so the two sources' pairs compare equal.
fn normalized(b: Bond) -> Bond {
    if b.i1 <= b.i2 { b } else { Bond::with_order(b.i2, b.i1, b.order) }
}

/// Sort + drop duplicate pairs, keeping the most informative order of each duplicate group
/// (a file's `Single`/`Double`/… beats a guess's `Unspecified`).
fn dedup_keeping_order(bonds: &mut Vec<Bond>) {
    bonds.sort_unstable_by_key(|b| (b.i1, b.i2, b.order == BondOrder::Unspecified));
    bonds.dedup_by_key(|b| (b.i1, b.i2));
}

/// Guess bonds for all atoms. `sel` is the bound all-selection (the position
/// source for the grid search); `positions`/`vdw` are the extracted per-atom
/// arrays (nm) used to score candidate pairs. The **PBC-aware** (minimum-image)
/// search + scoring — which finds bonds crossing a box face in a wrapped structure,
/// drawn as dashed half-bonds — is used only when **`params.periodic` is on and the
/// structure has a box**. The periodic search is much slower (it scans the
/// neighbouring cells), so it's opt-in; the default non-periodic path is the fast
/// one for large structures.
pub fn guess(
    sel: &impl PosProvider,
    positions: &[[f32; 3]],
    vdw: &[f32],
    pbox: Option<&PeriodicBox>,
    params: &BondParams,
) -> Vec<Bond> {
    let n = positions.len();
    if n < 2 {
        return Vec::new();
    }

    // Only honor the box when periodic search is requested.
    let pbc = if params.periodic { pbox } else { None };
    let candidates: Vec<(usize, usize)> = match pbc {
        Some(b) => distance_search_single_pbc::<(usize, usize), Vec<_>>(
            params.search_cutoff,
            sel.iter_pos(),
            0..n,
            b,
            PBC_FULL,
        ),
        None => distance_search_single::<(usize, usize), Vec<_>>(params.search_cutoff, sel, 0..n),
    };

    let min2 = params.min_dist * params.min_dist;
    let mut bonds: Vec<[usize; 2]> = Vec::new();
    for (i, j) in candidates {
        if i == j {
            continue;
        }
        let (a, b) = if i < j { (i, j) } else { (j, i) };
        let (pa, pb) = (positions[a], positions[b]);
        let d2 = match pbc {
            // Minimum-image distance, so a covalent bond whose atoms sit on
            // opposite faces of the box still scores as short.
            Some(b) => b.distance_squared(
                &Pos::new(pa[0], pa[1], pa[2]),
                &Pos::new(pb[0], pb[1], pb[2]),
                PBC_FULL,
            ),
            None => {
                let dx = pa[0] - pb[0];
                let dy = pa[1] - pb[1];
                let dz = pa[2] - pb[2];
                dx * dx + dy * dy + dz * dz
            }
        };
        let thresh = params.factor * (vdw[a] + vdw[b]);
        if d2 > min2 && d2 < thresh * thresh {
            bonds.push([a, b]);
        }
    }

    // The search may report a pair from either cell ordering; dedup.
    bonds.sort_unstable();
    bonds.dedup();
    // Guessed bonds carry no chemical order (distances don't tell us one).
    bonds.into_iter().map(|[a, b]| Bond::new(a, b)).collect()
}

/// Collect a columnar [`BondStorage`] back into the flat list the viewer keeps.
pub fn bond_vec(st: &BondStorage) -> Vec<Bond> {
    st.iter().map(|b| Bond::with_order(b.i1(), b.i2(), b.order())).collect()
}

/// Scatter a flat bond list into molar's columnar [`BondStorage`].
///
/// The viewer keeps connectivity as a flat `Vec<Bond>` (indexable, `Copy` rows — what
/// the geometry/pick hot paths want), while molar's topology-side machinery — selection
/// keywords (`polh`/`apolh`), [`perceive`](molar::prelude::perceive), force-field typing
/// and charge assignment — reads a `BondStorage`. This is the bridge between the two.
pub fn bond_storage(bonds: &[Bond]) -> BondStorage {
    let mut st = BondStorage::default();
    st.reserve(bonds.len());
    for b in bonds {
        st.push(b);
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests")).join(name)
    }

    /// Bond count from distance guessing alone — the pre-`resolve` behavior, for
    /// comparison.
    fn guessed_only(path: &Path) -> Vec<Bond> {
        let system = System::from_file(path.to_str().unwrap()).expect("load");
        let all = system.select_all_bound();
        let (positions, vdw): (Vec<[f32; 3]>, Vec<f32>) = all
            .iter_pos()
            .zip(all.iter_atoms())
            .map(|(p, a)| ([p.x, p.y, p.z], a.vdw()))
            .unzip();
        let pbox = system.state().pbox.clone();
        guess(&all, &positions, &vdw, pbox.as_ref(), &BondParams::default())
    }

    /// An SDF carries a complete bond block *with orders*, so it is taken verbatim —
    /// distance guessing can't recover a Kekulé structure, which is what aromatic
    /// perception and espaloma charges need.
    #[test]
    fn sdf_bonds_are_taken_verbatim_with_orders() {
        let recs = crate::data::load_records(&fixture("ligands20.sdf"), &BondParams::default())
            .expect("load ligands20.sdf");
        let aspirin = &recs[0];
        // The molfile counts line says "21 21": 21 atoms, 21 bonds.
        assert_eq!(aspirin.n_atoms, 21);
        assert_eq!(aspirin.bonds.len(), 21, "the file's own bond table, not a guess");
        // Aspirin has an aromatic ring + two carbonyls, so orders must have survived.
        assert!(
            aspirin.bonds.iter().any(|b| b.order == BondOrder::Double),
            "SDF bond orders must survive loading"
        );
        assert!(aspirin.bonds.iter().all(|b| b.order != BondOrder::Unspecified));
    }

    /// A PDB's `CONECT` block is only the exceptions, so it is unioned with the guess —
    /// every guessed bond is kept and the recorded pairs are added on top.
    #[test]
    fn pdb_conect_unions_with_the_distance_guess() {
        let path = fixture("2lao.pdb");
        let guessed = guessed_only(&path);
        let raw = crate::data::load(&path).expect("load 2lao.pdb");
        assert!(
            raw.bonds.len() >= guessed.len(),
            "union can only add: {} < {}",
            raw.bonds.len(),
            guessed.len()
        );
        let resolved: std::collections::BTreeSet<[usize; 2]> =
            raw.bonds.iter().map(|b| [b.i1, b.i2]).collect();
        for g in &guessed {
            assert!(resolved.contains(&[g.i1, g.i2]), "guessed bond {g:?} was dropped");
        }
    }

    /// Coarse-grained beads sit ~0.32 nm apart, past `search_cutoff`, so distance
    /// guessing misses most CG bonds. Reading the `CONECT` table is what recovers them.
    #[test]
    fn cg_conect_recovers_bonds_the_guess_misses() {
        let path = fixture("2lao_cg.pdb");
        let guessed = guessed_only(&path).len();
        let resolved = crate::data::load(&path).expect("load 2lao_cg.pdb").bonds.len();
        assert!(
            resolved > guessed,
            "CONECT must add CG bonds the guess can't see: {resolved} vs {guessed}"
        );
    }

    /// A duplicated pair keeps the informative order (the file's) over a guess's
    /// `Unspecified`, whichever way round the endpoints were recorded.
    #[test]
    fn dedup_prefers_the_known_order() {
        let mut bonds = vec![
            Bond::new(0, 1),
            normalized(Bond::with_order(1, 0, BondOrder::Double)),
            Bond::new(2, 3),
        ];
        dedup_keeping_order(&mut bonds);
        assert_eq!(bonds.len(), 2);
        assert_eq!(bonds[0].order, BondOrder::Double);
        assert_eq!(bonds[1].order, BondOrder::Unspecified);
    }
}
