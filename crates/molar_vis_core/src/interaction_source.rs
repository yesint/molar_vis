//! Shared chemical input for interaction rendering and foreign selections.
use crate::{interactions::InteractionSet, scene};
use molar::prelude::*;
mod hbond_chemistry;
fn v3(p: &Pos) -> glam::Vec3 {
    glam::Vec3::new(p.x, p.y, p.z)
}

/// Prepare chemical connectivity, bond orders and roles before gathering contact
/// coordinates. This cached stage uses the full structure, never a display selection.
fn prepare_hbond_chemistry(mol: &scene::Molecule) -> &crate::hbond::Chemistry {
    mol.interaction_hbond_chemistry.get_or_init(|| {
        let _timing = crate::performance::span("hbond-chemistry");
        let mut top = mol.data.topology().clone();
        top.bonds = crate::data::bonds::bond_storage(&mol.bonds);
        hbond_chemistry::perceive(&top, mol.data.state()).unwrap_or_else(|error| {
            log::warn!("Cannot assign hydrogen-bond chemistry for {}: {error}", mol.name);
            crate::hbond::Chemistry {
                roles: vec![crate::hbond::Role::default(); mol.n_atoms],
                neighbors: vec![Vec::new(); mol.n_atoms],
            }
        })
    })
}

/// Gather everything one rep's selection contributes to interaction detection: heavy
/// atoms (+ attached H / hydrophobic flag / halogen antecedent), aromatic rings within
/// the selection (centroid + normal from the displayed frame; ring atom sets come from
/// the molecule's cached `interaction_rings`), and charged groups. `res_key` is made
/// unique per (molecule, residue) via `mol_idx` so the detector's residue-level dedup
/// never merges same-index residues from two molecules.
pub(crate) fn gather_set(
    mol: &scene::Molecule,
    mol_idx: usize,
    sel: &molar::prelude::Sel,
    state: &State,
) -> InteractionSet {
    let chemistry = prepare_hbond_chemistry(mol);
    let topo = mol.data.topology();
    let n = state.coords.len();
    let res_base = (mol_idx as u64) << 40;
    let coord = |i: usize| state.coords.get(i).map(v3);

    // Adjacency over the whole molecule (bonds index the full topology).
    let mut neigh: Vec<Vec<u32>> = vec![Vec::new(); n];
    for bond in &mol.bonds {
        let [a, b] = bond.pair();
        if a < n && b < n {
            neigh[a].push(b as u32);
            neigh[b].push(a as u32);
        }
    }
    let anum_of = |i: usize| topo.get_atom(i).map(|a| a.get_atomic_number()).unwrap_or(0);

    // Selected atoms → heavy-atom AtomInfo + a membership mask (for rings/charges).
    let bound = mol.data.bind_with_state(sel, state);
    let mut in_sel = vec![false; n];
    let mut atoms = Vec::new();
    for p in bound.iter_particle() {
        in_sel[p.id] = true;
        let anum = p.atom.get_atomic_number();
        if anum == 1 {
            continue; // H rides in its heavy neighbour's `attached_h`
        }
        let only_ch = anum == 6 && neigh[p.id].iter().all(|&i| matches!(anum_of(i as usize), 1 | 6));
        let mut attached_h = Vec::new();
        let mut heavy_neighbors = Vec::new();
        let antecedent = neigh[p.id].iter().find(|&&i| anum_of(i as usize) != 1)
            .and_then(|&i| coord(i as usize));
        for &nb in &chemistry.neighbors[p.id] {
            let nb = nb as usize;
            let na = anum_of(nb);
            if na == 1 {
                if let Some(c) = coord(nb) {
                    attached_h.push(c);
                }
            } else if let Some(c) = coord(nb) {
                heavy_neighbors.push(c);
            }
        }
        let pos = v3(p.pos);
        let mut plane_points = heavy_neighbors.iter().take(2).copied().collect::<Vec<_>>();
        if plane_points.len() == 1 {
            if let Some(&nb) = chemistry.neighbors[p.id].iter().find(|&&i| anum_of(i as usize) != 1) {
                if let Some(c) = chemistry.neighbors[nb as usize].iter().copied()
                    .find(|&i| i as usize != p.id && anum_of(i as usize) != 1)
                    .and_then(|i| coord(i as usize)) {
                    plane_points.push(c);
                }
            }
        }
        let plane_normal = (plane_points.len() == 2)
            .then(|| (plane_points[0] - pos).cross(plane_points[1] - pos).try_normalize())
            .flatten();
        atoms.push(crate::interactions::AtomInfo {
            id: crate::interactions::AtomId {
                molecule: mol_idx,
                index: p.id,
            },
            attached_h_ids: chemistry.neighbors[p.id]
                .iter()
                .map(|&i| i as usize)
                .filter(|&i| anum_of(i) == 1)
                .map(|index| crate::interactions::AtomId {
                    molecule: mol_idx,
                    index,
                })
                .collect(),
            pos: glam::Vec3::new(p.pos.x, p.pos.y, p.pos.z),
            atomicnum: anum,
            res_key: res_base | (p.atom.get_resindex() as u64),
            only_ch_neighbors: only_ch,
            attached_h,
            hbond: crate::hbond::Site { role: chemistry.roles[p.id], heavy_neighbors, plane_normal },
            antecedent,
        });
    }

    // Aromatic rings fully inside the selection → centroid + plane normal.
    let mut rings = Vec::new();
    let computed_rings;
    let ring_sets = if let Some(rings) = &mol.interaction_rings {
        rings.as_slice()
    } else {
        computed_rings = mol.with_perception_topology(|top, adj| aromatic_rings(top, adj));
        computed_rings.as_slice()
    };
    for ring in ring_sets {
        if ring.len() < 3 || !ring.iter().all(|&i| i < n && in_sel[i]) {
            continue;
        }
        let pts: Vec<glam::Vec3> = ring.iter().filter_map(|&i| coord(i)).collect();
        if pts.len() < 3 {
            continue;
        }
        let center = pts.iter().copied().sum::<glam::Vec3>() / pts.len() as f32;
        rings.push(crate::interactions::RingInfo {
            atoms: ring
                .iter()
                .map(|&index| crate::interactions::AtomId {
                    molecule: mol_idx,
                    index,
                })
                .collect(),
            center,
            normal: crate::interactions::ring_normal(&pts),
            res_key: res_base
                | (topo
                    .get_atom(ring[0])
                    .map(|a| a.get_resindex() as u64)
                    .unwrap_or(0)),
        });
    }

    let (cations, anions) = charged_groups(topo, &neigh, state, &in_sel, res_base, n);
    InteractionSet {
        atoms,
        rings,
        cations,
        anions,
    }
}

/// Detect charged groups in the selection: standard amino-acid sidechains / termini by
/// residue+atom name, plus ligand functional groups (carboxylate, phosphate/sulfate,
/// guanidinium) by connectivity. Heuristic — real formal charges aren't available; this
/// covers the common protein–ligand salt-bridge / π-cation cases.
fn charged_groups(
    topo: &molar::prelude::Topology,
    neigh: &[Vec<u32>],
    state: &State,
    in_sel: &[bool],
    res_base: u64,
    n: usize,
) -> (
    Vec<crate::interactions::ChargeGroup>,
    Vec<crate::interactions::ChargeGroup>,
) {
    use crate::interactions::ChargeGroup;
    use std::collections::HashMap;
    let coord = |i: usize| state.coords.get(i).map(v3);
    let name = |i: usize| {
        topo.get_atom(i)
            .map(|a| a.get_name().to_string())
            .unwrap_or_default()
    };
    let anum = |i: usize| topo.get_atom(i).map(|a| a.get_atomic_number()).unwrap_or(0);

    // Group selected atoms by residue.
    let mut byres: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, &sel) in in_sel.iter().enumerate().take(n) {
        if sel {
            if let Some(a) = topo.get_atom(i) {
                byres.entry(a.get_resindex()).or_default().push(i);
            }
        }
    }
    let centroid = |ids: &[usize]| -> Option<glam::Vec3> {
        let mut sum = glam::Vec3::ZERO;
        let mut k = 0;
        for &i in ids {
            if let Some(c) = coord(i) {
                sum += c;
                k += 1;
            }
        }
        (k > 0).then(|| sum / k as f32)
    };

    let mut cations = Vec::new();
    let mut anions = Vec::new();
    for (ridx, ids) in &byres {
        let rk = res_base | (*ridx as u64);
        let resname = topo
            .get_atom(ids[0])
            .map(|a| a.get_resname().to_ascii_uppercase())
            .unwrap_or_default();
        let pick = |names: &[&str]| -> Vec<usize> {
            ids.iter()
                .copied()
                .filter(|&i| names.contains(&name(i).as_str()))
                .collect()
        };
        let mut push = |grp: Vec<usize>, positive: bool| {
            if let Some(c) = centroid(&grp) {
                let cg = ChargeGroup {
                    atoms: grp
                        .iter()
                        .map(|&index| crate::interactions::AtomId {
                            molecule: (res_base >> 40) as usize,
                            index,
                        })
                        .collect(),
                    center: c,
                    res_key: rk,
                };
                if positive {
                    cations.push(cg);
                } else {
                    anions.push(cg);
                }
            }
        };
        match resname.as_str() {
            "ARG" => push(pick(&["CZ", "NH1", "NH2", "NE"]), true),
            "LYS" => push(pick(&["NZ"]), true),
            "HIS" | "HID" | "HIE" | "HIP" | "HSD" | "HSE" | "HSP" => {
                push(pick(&["ND1", "NE2", "CE1", "CG", "CD2"]), true)
            }
            "ASP" => push(pick(&["CG", "OD1", "OD2"]), false),
            "GLU" => push(pick(&["CD", "OE1", "OE2"]), false),
            _ => {
                // Ligand / non-standard residue: functional groups by connectivity.
                for &i in ids {
                    let deg_heavy =
                        |j: usize| neigh[j].iter().filter(|&&k| anum(k as usize) > 1).count();
                    if anum(i) == 6 {
                        // Carboxylate: C bonded to ≥2 terminal O.
                        let os: Vec<usize> = neigh[i]
                            .iter()
                            .map(|&k| k as usize)
                            .filter(|&k| anum(k) == 8 && deg_heavy(k) <= 1)
                            .collect();
                        if os.len() >= 2 {
                            let mut g = os;
                            g.push(i);
                            push(g, false);
                        }
                        // Guanidinium / amidinium: C bonded to ≥3 N.
                        let ns = neigh[i].iter().filter(|&&k| anum(k as usize) == 7).count();
                        if ns >= 3 {
                            let mut g: Vec<usize> = neigh[i].iter().map(|&k| k as usize).collect();
                            g.push(i);
                            push(g, true);
                        }
                    } else if matches!(anum(i), 15 | 16) {
                        // Phosphate / sulfate: P/S bonded to ≥3 O.
                        let os: Vec<usize> = neigh[i]
                            .iter()
                            .map(|&k| k as usize)
                            .filter(|&k| anum(k) == 8)
                            .collect();
                        if os.len() >= 3 {
                            let mut g = os;
                            g.push(i);
                            push(g, false);
                        }
                    }
                }
            }
        }
        // C-terminal carboxylate (any residue carrying a terminal-oxygen name).
        let oxt = pick(&["OXT", "OT1", "OT2", "OT"]);
        if !oxt.is_empty() {
            let mut g = oxt;
            g.extend(pick(&["C", "O"]));
            push(g, false);
        }
    }
    (cations, anions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactions::{detect, InteractionKind, InteractionSettings};

    #[test]
    fn hbond_preparation_preserves_source_and_invalidates_after_edits() {
        let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/jak2_inhs.sd"));
        let mut scene = scene::Scene::default();
        scene.add(crate::data::load(path).unwrap(), &crate::settings::RepDefaults::default());
        let mol = &mut scene.molecules[0];
        let before: Vec<_> = mol.bonds.iter().map(|b| (b.pair(), b.order)).collect();
        let charges: Vec<_> = mol.data.topology().atoms.iter().map(|a| a.get_formal_charge()).collect();
        let prepared = prepare_hbond_chemistry(mol) as *const _;
        assert_eq!(prepared, prepare_hbond_chemistry(mol) as *const _);
        assert_eq!(before, mol.bonds.iter().map(|b| (b.pair(), b.order)).collect::<Vec<_>>());
        assert_eq!(charges, mol.data.topology().atoms.iter().map(|a| a.get_formal_charge()).collect::<Vec<_>>());
        mol.cycle_bond_order(0);
        assert!(mol.interaction_hbond_chemistry.get().is_none());
        assert_eq!(mol.data.topology().bonds.iter().next().unwrap().order(), mol.bonds[0].order);
        prepare_hbond_chemistry(mol);
        mol.set_atom_element(0, &Atom::new().with_atomic_number(6));
        assert!(mol.interaction_hbond_chemistry.get().is_none());
        prepare_hbond_chemistry(mol);
        mol.add_atom(&Atom::new().with_atomic_number(8), glam::Vec3::splat(100.0)).unwrap();
        assert!(mol.interaction_hbond_chemistry.get().is_none());
    }

    #[test]
    fn jak2_water_hbonds_end_at_explicit_hydrogens() {
        let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests"));
        let mut scene = scene::Scene::default();
        let defaults = crate::settings::RepDefaults::default();
        scene.add(crate::data::load(&root.join("jak2.pdb")).unwrap(), &defaults);
        scene.add(crate::data::load(&root.join("jak2_inhs.sd")).unwrap(), &defaults);
        for mol in &scene.molecules {
            mol.with_perception_topology(|top, _| hbond_chemistry::perceive(top, mol.data.state()))
                .expect("JAK2 chemistry must be assigned");
        }
        let gather = |mi, selection: &str| {
            let mol = &scene.molecules[mi];
            gather_set(mol, mi, &scene.select(mi, selection).unwrap(), mol.data.state())
        };
        // Deliberately omit H from the selections; donor eligibility must still
        // reflect the hydrogens present in the underlying structures.
        let waters = gather(0, "resname HOH and not name H*");
        let receptor = gather(0, "not name H*");
        let ligand = gather(1, "not name H*");
        assert!(!waters.atoms.is_empty());
        assert!(waters.atoms.iter().all(|a| a.attached_h.len() == 2));
        assert!(waters.atoms.iter().all(|a| a.hbond.role.donor && a.hbond.role.acceptor));
        let mut water_donations = 0;
        let mut ligand_contacts = 0;
        for partner in [&receptor, &ligand] {
            for contact in detect(&waters, partner, &InteractionSettings::default().detect()) {
                if contact.kind != InteractionKind::HBond {
                    continue;
                }
                assert!(contact.angle.is_some(), "explicit-H JAK2 contacts must not use the heavy-atom fallback: {:?}", contact.atoms);
                let endpoints = [contact.a, contact.b];
                let water = waters.atoms.iter().find(|a| a.id == contact.atoms[0][0]).unwrap();
                let other = partner.atoms.iter().find(|a| a.id == contact.atoms[1][0]).unwrap();
                let water_donates = water.attached_h.iter().any(|h| endpoints.contains(h))
                    && endpoints.contains(&other.pos);
                let other_donates = other.attached_h.iter().any(|h| endpoints.contains(h))
                    && endpoints.contains(&water.pos);
                assert!(water_donates || other_donates, "H-bond must connect H to its acceptor");
                water_donations += usize::from(water_donates);
                ligand_contacts += usize::from(other.id.molecule == 1);
            }
        }
        assert!(water_donations > 0, "fixture must exercise water as donor");
        // The seven chemically possible water–ligand directions in pose 0 all
        // fail the pinned Mol* geometry test (frozen in the reference fixture).
        assert_eq!(ligand_contacts, 0);
        assert!(detect(&receptor, &ligand, &InteractionSettings::default().detect()).iter()
            .any(|c| c.kind == InteractionKind::HBond), "fixture must retain protein–ligand H-bonds");
    }
}
