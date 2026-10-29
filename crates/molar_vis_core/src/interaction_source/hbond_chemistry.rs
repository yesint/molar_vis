//! Mol* valence-model and hydrogen-bond role assignment (MIT; licenses/molstar.txt).
//! Copyright (c) 2019–2026 Mol* contributors.
//! Reference: fe1ac1555001d1acb023077cd4dc6ca1d06870f8, computed/chemistry/valence-model.ts
//! and computed/interactions/hydrogen-bonds.ts. Molar supplies missing bond orders.

use crate::hbond::{Chemistry, Geometry, Role};
use molar::prelude::*;

pub(crate) fn perceive(input: &Topology, state: &State) -> Result<Chemistry, String> {
    let has_h = input.atoms.iter().any(|a| a.get_atomic_number() == 1);
    let needs_connectivity = input
        .bonds
        .iter()
        .any(|b| b.order() == BondOrder::Unspecified);
    // PDB alternate sites share an atom name and residue and carry occupancies.
    // Prepare one occupied site per atom before chemical perception, as PLIP does
    // before assigning donors/acceptors. The displayed structure is not modified.
    let mut active = vec![true; input.atoms.len()];
    if needs_connectivity {
        let mut sites = std::collections::BTreeMap::<(usize, String), Vec<usize>>::new();
        for (i, a) in input.atoms.iter().enumerate() {
            if !a.get_name().is_empty() {
                sites
                    .entry((a.get_resindex(), a.get_name().to_string()))
                    .or_default()
                    .push(i);
            }
        }
        for ids in sites.values().filter(|ids| ids.len() > 1) {
            if !ids
                .iter()
                .any(|&i| input.atoms.get(i).unwrap().get_occupancy() < 1.0)
            {
                continue;
            }
            let mut best = ids[0];
            for &i in &ids[1..] {
                if input.atoms.get(i).unwrap().get_occupancy()
                    > input.atoms.get(best).unwrap().get_occupancy()
                {
                    best = i;
                }
            }
            for &i in ids {
                active[i] = i == best;
            }
        }
    }
    let source_ids: Vec<_> = (0..active.len()).filter(|&i| active[i]).collect();
    let mut local_ids = vec![usize::MAX; active.len()];
    let mut top = Topology::default();
    let mut local_state = State::default();
    for (local, &source) in source_ids.iter().enumerate() {
        local_ids[source] = local;
        top.atoms.push(&input.atoms.to_atom(source));
        local_state.coords.push(state.coords[source]);
    }
    for b in input.bonds.iter() {
        if active[b.i1()] && active[b.i2()] {
            top.bonds.push(&Bond::with_order(
                local_ids[b.i1()],
                local_ids[b.i2()],
                b.order(),
            ));
        }
    }
    if needs_connectivity {
        // Use Molar's covalent radii and coordination limits, not the display's
        // VDW-radius graph. Keep explicitly supplied chemical orders.
        let _timing = crate::performance::span("hbond-connectivity");
        let known: Vec<_> = top
            .bonds
            .iter()
            .filter(|b| b.order() != BondOrder::Unspecified)
            .map(|b| Bond::with_order(b.i1(), b.i2(), b.order()))
            .collect();
        let system = System::new(top.clone(), local_state.clone()).map_err(|e| crate::errors::error_chain(&e))?;
        let perceived =
            perceive_connectivity(&system.select_all_bound(), &ConnectivityOptions::default())
                .map_err(|e| crate::errors::error_chain(&e))?;
        let mut bonds = std::collections::BTreeMap::new();
        for b in perceived
            .iter()
            .map(|b| Bond::with_order(b.i1(), b.i2(), b.order()))
            .chain(known)
        {
            bonds.insert((b.i1.min(b.i2), b.i1.max(b.i2)), b.order);
        }
        top.bonds = BondStorage::default();
        for ((i, j), order) in bonds {
            top.bonds.push(&Bond::with_order(i, j, order));
        }
    }
    if top
        .bonds
        .iter()
        .any(|b| matches!(b.order(), BondOrder::Unspecified | BondOrder::Aromatic))
    {
        let _timing = crate::performance::span("hbond-bond-orders");
        let options = BondOrderOptions {
            input_orders: InputOrders::PreserveKnown,
            hydrogens: if has_h {
                HydrogenPolicy::AllExplicit
            } else {
                HydrogenPolicy::InferFromGeometry
            },
            // With geometric inference, this Molar version can pin both C–O
            // bonds double when a carboxyl template conflicts with ASP/GLU names.
            // Residue templates plus the valence solver avoid that conflict.
            use_functional_groups: has_h,
            ..Default::default()
        };
        let assignment = assign_bond_orders(&top, Some(&local_state.coords), &options)
            .map_err(|e| crate::errors::error_chain(&e))?;
        assignment.apply_to(&mut top).map_err(|e| crate::errors::error_chain(&e))?;
    }
    let mut chemistry = Chemistry {
        roles: vec![Role::default(); active.len()],
        neighbors: vec![Vec::new(); active.len()],
    };
    for (local, role) in classify(&top, has_h).into_iter().enumerate() {
        chemistry.roles[source_ids[local]] = role;
    }
    for [a, b] in top.bonds.iter_pairs() {
        let (a, b) = (source_ids[a], source_ids[b]);
        chemistry.neighbors[a].push(b as u32);
        chemistry.neighbors[b].push(a as u32);
    }
    Ok(chemistry)
}

fn classify(top: &Topology, has_h: bool) -> Vec<Role> {
    let n = top.atoms.len();
    let z: Vec<_> = top.atoms.iter().map(|a| a.get_atomic_number()).collect();
    let mut bonds = vec![Vec::<(usize, i32)>::new(); n];
    for b in top.bonds.iter() {
        let order = match b.order() {
            BondOrder::Single => 1,
            BondOrder::Double => 2,
            BondOrder::Triple => 3,
            _ => continue,
        };
        bonds[b.i1()].push((b.i2(), order));
        bonds[b.i2()].push((b.i1(), order));
    }
    let conjugated = |i: usize| {
        let hetero = matches!(z[i], 7 | 8);
        if hetero && bonds[i].len() == 4 {
            return false;
        }
        bonds[i].iter().any(|&(j, order)| {
            order > 1
                || (hetero
                    && bonds[j]
                        .iter()
                        .any(|&(k, o)| o > 1 && !(matches!(z[j], 15 | 16) && z[k] == 8)))
        })
    };
    top.atoms
        .iter()
        .enumerate()
        .map(|(i, atom)| {
            let degree = bonds[i].len() as i32;
            let valence: i32 = bonds[i].iter().map(|b| b.1).sum();
            let hydrogen_count = bonds[i].iter().filter(|b| z[b.0] == 1).count() as i32;
            let mut charge = atom.get_formal_charge().unwrap_or(0);
            let assign_charge = charge == 0;
            let assign_h = !has_h && hydrogen_count == 0;
            let conjugated = conjugated(i);
            let multi_bond = valence > degree;
            let mut implicit_h = 0;
            let mut geometry = Geometry::Unknown;
            match z[i] {
                7 => {
                    if assign_charge {
                        if !assign_h {
                            charge = valence - 3;
                        } else if conjugated && valence < 4 {
                            charge = i32::from(
                                degree - hydrogen_count == 1 && valence - hydrogen_count == 2,
                            );
                        } else {
                            // Mol*: amines are protonated when H and charge must both
                            // be assigned; sulfonamides and metal-bound N stay neutral.
                            for &(j, _) in &bonds[i] {
                                if z[j] == 16 || is_metal(z[j]) {
                                    charge = 0;
                                    break;
                                }
                                charge = 1;
                            }
                        }
                    }
                    if assign_h {
                        implicit_h = (3 - valence + charge).max(0);
                    }
                    geometry = Geometry::from_coordination(
                        degree + implicit_h - charge + i32::from(!conjugated || multi_bond),
                    );
                }
                8 => {
                    if assign_charge {
                        if !assign_h {
                            charge = valence - 2;
                        }
                        if valence == 1
                            && bonds[i].iter().any(|&(j, _)| {
                                bonds[j].iter().any(|&(k, o)| k != i && z[k] == 8 && o == 2)
                            })
                        {
                            charge = -1;
                        }
                    }
                    if assign_h {
                        implicit_h = (2 - valence + charge).max(0);
                    }
                    geometry = Geometry::from_coordination(
                        degree + implicit_h - charge
                            + if conjugated && !multi_bond { 1 } else { 2 },
                    );
                }
                16 => {
                    if assign_charge && !assign_h {
                        charge = if valence <= 3 && !bonds[i].iter().any(|b| z[b.0] == 8) {
                            valence - 2
                        } else {
                            0
                        };
                    }
                    if assign_h && valence < 2 {
                        implicit_h = (2 - valence + charge).max(0);
                    }
                    if valence <= 3 {
                        geometry = Geometry::from_coordination(degree + implicit_h - charge + 2);
                    }
                }
                _ => {}
            }
            let donor = matches!(z[i], 7 | 8 | 16) && hydrogen_count + implicit_h > 0;
            let acceptor = match z[i] {
                8 => true,
                7 if charge < 1 => {
                    let total_bonds = degree + implicit_h;
                    match geometry {
                        Geometry::Tetrahedral => total_bonds < 4,
                        Geometry::Trigonal => total_bonds < 3,
                        Geometry::Linear => total_bonds < 2,
                        Geometry::Unknown => false,
                    }
                }
                16 => {
                    matches!(atom.get_resname(), "CYS" | "MET")
                        || atom.get_formal_charge() == Some(-1)
                }
                _ => false,
            };
            // Unlike Mol*'s ambiguous-HIS override, use the input protonation state
            // consistently: an explicit pyrrole-like N–H is not also an acceptor.
            Role {
                donor,
                acceptor,
                geometry,
            }
        })
        .collect()
}

fn is_metal(z: u8) -> bool {
    matches!(z, 3 | 4 | 11 | 12 | 13 | 19..=31 | 37..=50 | 55..=84 | 87..=116)
}

#[cfg(test)]
mod tests {
    use super::*;
    use BondOrder::{Double as D, Single as S, Triple as T};

    fn topology(z: &[u8], bonds: &[(usize, usize, BondOrder)]) -> Topology {
        let mut top = Topology::default();
        for &z in z {
            top.atoms.push(&Atom::new().with_atomic_number(z));
        }
        for &(i, j, order) in bonds {
            top.bonds.push(&Bond::with_order(i, j, order));
        }
        top
    }

    #[test]
    fn chemical_roles_distinguish_donors_acceptors_and_resonance() {
        // Atom 0 is the tested site. Expected roles follow the Mol* valence model.
        let cases = [
            (
                "water",
                topology(&[8, 1, 1], &[(0, 1, S), (0, 2, S)]),
                true,
                true,
                Geometry::Tetrahedral,
            ),
            (
                "carbonyl",
                topology(&[8, 6, 6, 6], &[(0, 1, D), (1, 2, S), (1, 3, S)]),
                false,
                true,
                Geometry::Trigonal,
            ),
            (
                "amide NH",
                topology(
                    &[7, 6, 8, 6, 1],
                    &[(0, 1, S), (1, 2, D), (0, 3, S), (0, 4, S)],
                ),
                true,
                false,
                Geometry::Trigonal,
            ),
            (
                "tertiary amine",
                topology(&[7, 6, 6, 6], &[(0, 1, S), (0, 2, S), (0, 3, S)]),
                false,
                true,
                Geometry::Tetrahedral,
            ),
            (
                "ammonium",
                topology(
                    &[7, 1, 1, 1, 1],
                    &[(0, 1, S), (0, 2, S), (0, 3, S), (0, 4, S)],
                ),
                true,
                false,
                Geometry::Tetrahedral,
            ),
            (
                "quaternary ammonium",
                topology(
                    &[7, 6, 6, 6, 6],
                    &[(0, 1, S), (0, 2, S), (0, 3, S), (0, 4, S)],
                ),
                false,
                false,
                Geometry::Tetrahedral,
            ),
            (
                "pyridine",
                topology(
                    &[7, 6, 6, 6, 6, 6],
                    &[
                        (0, 1, D),
                        (1, 2, S),
                        (2, 3, D),
                        (3, 4, S),
                        (4, 5, D),
                        (5, 0, S),
                    ],
                ),
                false,
                true,
                Geometry::Trigonal,
            ),
            (
                "pyrrole NH",
                topology(
                    &[7, 6, 6, 6, 6, 1],
                    &[
                        (0, 1, S),
                        (1, 2, D),
                        (2, 3, S),
                        (3, 4, D),
                        (4, 0, S),
                        (0, 5, S),
                    ],
                ),
                true,
                false,
                Geometry::Trigonal,
            ),
            (
                "nitrile",
                topology(&[7, 6, 6], &[(0, 1, T), (1, 2, S)]),
                false,
                true,
                Geometry::Linear,
            ),
        ];
        for (name, top, donor, acceptor, geometry) in cases {
            let role = classify(&top, true)[0];
            assert_eq!(
                (role.donor, role.acceptor, role.geometry),
                (donor, acceptor, geometry),
                "{name}"
            );
        }
    }

    #[test]
    fn implicit_hydrogens_follow_valence_instead_of_proximity() {
        // H-free formamide: the amide N can donate; its carbonyl O cannot.
        let top = topology(&[7, 6, 8], &[(0, 1, S), (1, 2, D)]);
        let roles = classify(&top, false);
        assert!(roles[0].donor && !roles[0].acceptor);
        assert!(!roles[2].donor && roles[2].acceptor);
    }
}
