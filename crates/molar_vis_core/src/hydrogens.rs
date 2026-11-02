//! Reversible hydrogen editing and geometry-aware completion of explicit hydrogens.
use crate::{history::AtomLite, scene::Molecule};
use glam::{Mat3, Quat, Vec3};
use molar::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CachedHydrogen {
    atom: AtomLite,
    parent: usize,
    parent_element: u8,
    site_count: usize,
    #[serde(default)]
    site_changed: bool,
    heavy_bonds: Vec<(usize, BondOrder)>,
    position: [f32; 3],
    parent_position: [f32; 3],
    // Up to two heavy-atom reference positions define the local orientation.
    anchors: Vec<(usize, [f32; 3])>,
    order: BondOrder,
}

impl CachedHydrogen {
    pub(crate) fn remap(&mut self, map: &[Option<usize>]) -> bool {
        let Some(parent) = map.get(self.parent).copied().flatten() else {
            return false;
        };
        self.parent = parent;
        self.site_changed |= self
            .heavy_bonds
            .iter()
            .any(|(i, _)| map.get(*i).copied().flatten().is_none());
        self.heavy_bonds = self
            .heavy_bonds
            .iter()
            .filter_map(|(i, order)| map.get(*i).copied().flatten().map(|j| (j, *order)))
            .collect();
        self.anchors = self
            .anchors
            .iter()
            .filter_map(|(i, pos)| map.get(*i).copied().flatten().map(|j| (j, *pos)))
            .collect();
        true
    }

    fn restored_position(&self, coords: &[Vec3]) -> Vec3 {
        let old_parent = Vec3::from(self.parent_position);
        let parent = coords[self.parent];
        if parent == old_parent
            && self
                .anchors
                .iter()
                .all(|(i, p)| coords[*i] == Vec3::from(*p))
        {
            return self.position.into(); // Preserve unchanged coordinates exactly.
        }
        let offset = Vec3::from(self.position) - old_parent;
        let pairs: Vec<_> = self
            .anchors
            .iter()
            .map(|(i, p)| (Vec3::from(*p) - old_parent, coords[*i] - parent))
            .collect();
        let rotation = match pairs.as_slice() {
            [(a, b), (c, d), ..] => match (basis(*a, *c), basis(*b, *d)) {
                (Some(old), Some(new)) => Quat::from_mat3(&(new * old.transpose())),
                _ => rotation_between(*a, *b),
            },
            [(a, b)] => rotation_between(*a, *b),
            _ => Quat::IDENTITY,
        };
        parent + rotation * offset
    }
}

fn rotation_between(a: Vec3, b: Vec3) -> Quat {
    if a.length_squared() < 1e-10 || b.length_squared() < 1e-10 {
        Quat::IDENTITY
    } else {
        Quat::from_rotation_arc(a.normalize(), b.normalize())
    }
}

fn basis(a: Vec3, b: Vec3) -> Option<Mat3> {
    let x = a.try_normalize()?;
    let y = (b - x * x.dot(b)).try_normalize()?;
    Some(Mat3::from_cols(x, y, x.cross(y)))
}

fn prepared_system(mol: &Molecule) -> Result<System, String> {
    let mut top = mol.data.topology().clone();
    top.bonds = crate::data::bond_storage(&mol.bonds);
    let mut system =
        System::new(top, mol.data.state().clone()).map_err(|e| crate::errors::error_chain(&e))?;
    if mol.bonds.iter().any(|b| b.order == BondOrder::Unspecified) {
        let assignment = system
            .assign_bond_orders(&BondOrderOptions {
                input_orders: InputOrders::PreserveKnown,
                hydrogens: HydrogenPolicy::InferFromGeometry,
                ..Default::default()
            })
            .map_err(|e| {
                format!(
                    "Cannot determine hydrogen valence: {}",
                    crate::errors::error_chain(&e)
                )
            })?;
        system
            .apply_bond_assignment(&assignment)
            .map_err(|e| crate::errors::error_chain(&e))?;
    }
    Ok(system)
}

fn coordinates(mol: &Molecule) -> Vec<Vec3> {
    mol.data
        .state()
        .coords
        .iter()
        .map(|p| Vec3::new(p.x, p.y, p.z))
        .collect()
}

fn adjacency(mol: &Molecule) -> Vec<Vec<usize>> {
    let mut adj = vec![Vec::new(); mol.n_atoms];
    for b in &mol.bonds {
        adj[b.i1].push(b.i2);
        adj[b.i2].push(b.i1);
    }
    adj
}

impl Molecule {
    /// Cache and remove H atoms attached to the editable heavy atoms (or directly selected).
    pub(crate) fn remove_edit_hydrogens(&mut self, scope: &[usize]) -> usize {
        if self.data.is_shared() || !self.trajectory.frames.is_empty() {
            return 0;
        }
        let scope: HashSet<_> = scope.iter().copied().collect();
        let coords = coordinates(self);
        let adj = adjacency(self);
        let top = self.data.topology();
        let z: Vec<_> = top.iter_atoms().map(|a| a.get_atomic_number()).collect();
        let mut removed = Vec::new();
        let mut cached = Vec::new();
        for i in 0..self.n_atoms {
            if z[i] != 1 {
                continue;
            }
            // Only attached H can be restored against an unambiguous heavy parent.
            let Some(&parent) = adj[i].first().filter(|&&p| adj[i].len() == 1 && z[p] != 1) else {
                continue;
            };
            if !scope.contains(&i) && !scope.contains(&parent) {
                continue;
            }
            let mut refs: Vec<_> = adj[parent].iter().copied().filter(|&j| z[j] != 1).collect();
            if refs.len() == 1 {
                let j = refs[0];
                refs.extend(
                    adj[j]
                        .iter()
                        .copied()
                        .filter(|&k| k != parent && z[k] != 1)
                        .take(1),
                );
            }
            refs.truncate(2);
            cached.push(CachedHydrogen {
                atom: AtomLite::capture(&top.get_atom(i).unwrap()),
                parent,
                parent_element: z[parent],
                site_count: adj[parent].iter().filter(|&&j| z[j] == 1).count(),
                site_changed: false,
                heavy_bonds: adj[parent]
                    .iter()
                    .copied()
                    .filter(|&j| z[j] != 1)
                    .map(|j| (j, self.bonds[self.bond_between(parent, j).unwrap()].order))
                    .collect(),
                position: coords[i].to_array(),
                parent_position: coords[parent].to_array(),
                anchors: refs
                    .into_iter()
                    .map(|j| (j, coords[j].to_array()))
                    .collect(),
                order: self.bonds[self.bond_between(i, parent).unwrap()].order,
            });
            removed.push(i);
        }
        let count = removed.len();
        self.hydrogen_cache.extend(cached);
        self.remove_atoms(&removed);
        count
    }

    /// Restore cached H first, then generate only the remaining valence shortfall.
    /// Returns (new atom, parent) pairs for updating visible representations.
    pub(crate) fn add_edit_hydrogens(
        &mut self,
        scope: &[usize],
    ) -> Result<(Vec<(usize, usize)>, Option<String>), String> {
        if scope.is_empty() || self.data.is_shared() || !self.trajectory.frames.is_empty() {
            return Ok((Vec::new(), None));
        }
        let scope: HashSet<_> = scope.iter().copied().collect();
        let (mut prepared, warning) = match prepared_system(self) {
            Ok(system) => (system, None),
            Err(error) => {
                // Reusing known hydrogens does not require a new chemical assignment.
                let mut top = self.data.topology().clone();
                top.bonds = crate::data::bond_storage(&self.bonds);
                (
                    System::new(top, self.data.state().clone()).expect("valid source molecule"),
                    Some(error),
                )
            }
        };
        let adj = BondAdjacency::build(self.n_atoms, prepared.topology().bonds.iter_pairs());
        let mut missing = implicit_hydrogens(&prepared, &adj);
        let coords = coordinates(self);
        let z: Vec<_> = self
            .data
            .topology()
            .iter_atoms()
            .map(|a| a.get_atomic_number())
            .collect();
        let mut occupied = vec![Vec::new(); self.n_atoms];
        for b in &self.bonds {
            if z[b.i1] == 1 && z[b.i2] != 1 {
                occupied[b.i2].push(coords[b.i1]);
            }
            if z[b.i2] == 1 && z[b.i1] != 1 {
                occupied[b.i1].push(coords[b.i2]);
            }
        }
        let original_bonds = self.bonds.clone();
        let mut added = Vec::new();
        let cache = std::mem::take(&mut self.hydrogen_cache);
        let cached_count = cache.len();
        for h in cache {
            if h.parent >= z.len() || z[h.parent] != h.parent_element {
                continue;
            }
            let heavy: Vec<_> = original_bonds
                .iter()
                .filter_map(|b| {
                    let j = if b.i1 == h.parent {
                        b.i2
                    } else if b.i2 == h.parent {
                        b.i1
                    } else {
                        return None;
                    };
                    (z[j] != 1).then_some((j, b.order))
                })
                .collect();
            let same_site = !h.site_changed
                && heavy.len() == h.heavy_bonds.len()
                && heavy.iter().all(|b| h.heavy_bonds.contains(b));
            if warning.is_some() && !same_site {
                self.hydrogen_cache.push(h);
                continue; // A changed site needs a valid valence assignment before reuse.
            }
            let needed = if same_site {
                occupied[h.parent].len() < h.site_count
            } else {
                missing[h.parent] > 0
            };
            if !scope.contains(&h.parent) || !needed {
                self.hydrogen_cache.push(h);
                continue;
            }
            let pos = h.restored_position(&coords);
            if occupied[h.parent].iter().any(|p| p.distance(pos) < 0.02) {
                continue; // This cached site was already filled by an intervening edit.
            }
            if let Some(i) = self.add_atom(&h.atom.to_atom(), pos) {
                self.add_bond(h.parent, i, h.order);
                prepared
                    .append_atom(&h.atom.to_atom(), &Pos::new(pos.x, pos.y, pos.z))
                    .expect("validated cached hydrogen");
                let mut bonds = crate::data::bonds::bond_vec(&prepared.topology().bonds);
                bonds.push(Bond::with_order(h.parent, i, BondOrder::Single));
                prepared
                    .set_bonds(crate::data::bond_storage(&bonds))
                    .expect("valid hydrogen bond");
                occupied[h.parent].push(pos);
                missing[h.parent] = missing[h.parent].saturating_sub(1);
                added.push((i, h.parent));
            } else {
                self.hydrogen_cache.push(h);
            }
        }
        if self.hydrogen_cache.len() != cached_count {
            self.structure_version += 1;
        }
        if let Some(error) = warning {
            if added.is_empty() {
                return Err(error);
            }
            return Ok((
                added,
                Some(format!(
                    "Restored cached hydrogens. {error}; no new hydrogens generated."
                )),
            ));
        }
        // Molar provides valence counts, element-specific bond lengths, and ideal
        // linear/trigonal/tetrahedral directions fitted to existing bonds.
        let plan = plan_hydrogen_addition(&prepared, &HydrogenOptions::default());
        let mut by_parent: BTreeMap<usize, Vec<Vec3>> = BTreeMap::new();
        for (&p, pos) in plan.parents().iter().zip(plan.positions()) {
            if scope.contains(&p) {
                by_parent
                    .entry(p)
                    .or_default()
                    .push(Vec3::new(pos.x, pos.y, pos.z));
            }
        }
        let coords = coordinates(self);
        let adj = adjacency(self);
        let prepared_bonds = crate::data::bonds::bond_vec(&prepared.topology().bonds);
        for (parent, positions) in by_parent {
            let positions = orient_hydrogens(
                prepared.topology(),
                &prepared_bonds,
                parent,
                positions,
                &coords,
                &adj,
            );
            let src = self.data.topology().get_atom(parent).unwrap();
            let atom = Atom::new()
                .with_name("H")
                .guess()
                .with_resname(src.get_resname())
                .with_resid(src.get_resid() as i32)
                .with_resindex(src.get_resindex())
                .with_chain(src.get_chain());
            for pos in positions {
                if !pos.is_finite() {
                    continue;
                }
                if let Some(i) = self.add_atom(&atom, pos) {
                    self.add_bond(parent, i, BondOrder::Single);
                    added.push((i, parent));
                }
            }
        }
        Ok((added, None))
    }
}

/// Respect conjugated nitrogen planes and the orientation of a neighboring group.
/// Molar's one-neighbor construction leaves this free rotation unspecified.
fn orient_hydrogens(
    top: &Topology,
    bonds: &[Bond],
    parent: usize,
    mut positions: Vec<Vec3>,
    coords: &[Vec3],
    adj: &[Vec<usize>],
) -> Vec<Vec3> {
    let p = coords[parent];
    let neighbors = &adj[parent];
    let atom = top.get_atom(parent).unwrap();
    let conjugated_n = atom.get_atomic_number() == 7
        && atom.get_formal_charge().unwrap_or(0) <= 0
        && neighbors.iter().any(|&j| {
            bonds.iter().any(|b| {
                b.contains(j)
                    && !b.contains(parent)
                    && matches!(b.order, BondOrder::Double | BondOrder::Aromatic)
            })
        });
    if conjugated_n && neighbors.len() + positions.len() == 3 && neighbors.len() == 2 {
        let a = (coords[neighbors[0]] - p).normalize_or_zero();
        let b = (coords[neighbors[1]] - p).normalize_or_zero();
        if let Some(d) = (-(a + b)).try_normalize() {
            positions[0] = p + d * 0.101;
        }
    }
    if neighbors.len() != 1 {
        return positions;
    }
    let j = neighbors[0];
    let Some(axis) = (coords[j] - p).try_normalize() else {
        return positions;
    };
    let Some(reference) = adj[j].iter().filter(|&&k| k != parent).find_map(|&k| {
        (coords[k] - coords[j] - axis * axis.dot(coords[k] - coords[j])).try_normalize()
    }) else {
        return positions;
    };
    let bond = bonds
        .iter()
        .find(|b| b.contains(parent) && b.contains(j))
        .unwrap();
    let planar = conjugated_n || matches!(bond.order, BondOrder::Double | BondOrder::Aromatic);
    if conjugated_n && positions.len() == 2 {
        return [1.0, -1.0]
            .into_iter()
            .map(|sign| p + 0.101 * (-0.5 * axis + sign * (3.0_f32.sqrt() / 2.0) * reference))
            .collect();
    }
    if planar {
        if let Some(current) =
            (positions[0] - p - axis * axis.dot(positions[0] - p)).try_normalize()
        {
            let angle = axis
                .dot(current.cross(reference))
                .atan2(current.dot(reference));
            let rotation = Quat::from_axis_angle(axis, angle);
            for h in &mut positions {
                *h = p + rotation * (*h - p);
            }
        }
    } else {
        // Choose a sterically open torsion without moving any existing atom.
        let mut best = positions.clone();
        let mut best_score = f32::INFINITY;
        for step in 0..12 {
            let rot = Quat::from_axis_angle(axis, step as f32 * std::f32::consts::TAU / 12.0);
            let trial: Vec<_> = positions.iter().map(|h| p + rot * (*h - p)).collect();
            let score: f32 = trial
                .iter()
                .flat_map(|h| {
                    coords
                        .iter()
                        .enumerate()
                        .filter(move |(k, _)| *k != parent && *k != j)
                        .map(move |(_, q)| (0.15 / h.distance(*q).max(0.01)).powi(6))
                })
                .sum();
            if score < best_score {
                best_score = score;
                best = trial;
            }
        }
        positions = best;
    }
    positions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        data::RawMolecule,
        scene::{reserve_mol_id, Scene},
        settings::RepDefaults,
    };

    fn molecule(
        elements: &[&str],
        coords: &[[f32; 3]],
        bonds: &[(usize, usize, BondOrder)],
    ) -> Molecule {
        let atom = |el: &str| {
            Atom::new()
                .with_name(el)
                .with_resname("LIG")
                .with_resid(7)
                .with_chain('B')
                .guess()
        };
        let raw = RawMolecule::single_atom("test", atom(elements[0]), coords[0].into()).unwrap();
        let mut mol = Molecule::new(reserve_mol_id(), raw, &RepDefaults::default());
        for (&el, &pos) in elements.iter().zip(coords).skip(1) {
            mol.add_atom(&atom(el), pos.into());
        }
        for &(a, b, order) in bonds {
            mol.add_bond(a, b, order);
        }
        mol
    }

    #[test]
    fn methane_is_tetrahedral_and_add_is_idempotent() {
        let mut mol = molecule(&["C"], &[[0.0; 3]], &[]);
        assert_eq!(mol.add_edit_hydrogens(&[0]).unwrap().0.len(), 4);
        let p = coordinates(&mol);
        for h in &p[1..] {
            assert!((h.length() - 0.109).abs() < 1e-5);
        }
        for i in 1..5 {
            for j in i + 1..5 {
                assert!((p[i].normalize().dot(p[j].normalize()) + 1.0 / 3.0).abs() < 1e-4);
            }
        }
        assert!(mol.add_edit_hydrogens(&[0]).unwrap().0.is_empty());
        assert_eq!(mol.n_atoms, 5);
    }

    #[test]
    fn partial_hydrogens_keep_positions_and_metadata() {
        let mut mol = molecule(
            &["C", "H"],
            &[[0.0; 3], [0.109, 0.0, 0.0]],
            &[(0, 1, BondOrder::Single)],
        );
        let original = coordinates(&mol);
        assert_eq!(mol.add_edit_hydrogens(&[0]).unwrap().0.len(), 3);
        assert_eq!(&coordinates(&mol)[..2], &original);
        for i in 2..5 {
            let a = mol.data.topology().get_atom(i).unwrap();
            assert_eq!(a.get_chain(), 'B');
            assert_eq!(a.get_resid(), 7);
            assert_eq!(a.get_resname(), "LIG");
            assert!((coordinates(&mol)[i].normalize().dot(Vec3::X) + 1.0 / 3.0).abs() < 1e-4);
        }
    }

    #[test]
    fn cache_restores_exact_hydrogens_through_undo_redo() {
        let mut mol = molecule(&["C"], &[[0.0; 3]], &[]);
        mol.add_edit_hydrogens(&[0]).unwrap().0;
        {
            let sys = mol.data.system_mut().unwrap();
            let mut all = sys.select_all_bound_mut();
            for (i, mut a) in all.iter_atoms_mut().enumerate() {
                a.set_charge(i as f32 * 0.03);
                a.set_bfactor(19.0);
                a.set_occupancy(0.6);
                a.set_formal_charge(0);
                if i > 0 {
                    a.set_name(&format!("H{i}"));
                }
            }
        }
        let original = crate::history::StructureSnapshot::capture(&mol);
        mol.reps[0].sel_text = "index 0:4".into();
        let id = mol.id;
        let mut scene = Scene::default();
        scene.molecules.push(mol);
        let mut history = crate::history::History::new(crate::history::EditState::capture(&scene));
        let before = scene.molecules[0].structure_snapshot().unwrap();
        assert_eq!(scene.molecules[0].remove_edit_hydrogens(&[0]), 4);
        let after = scene.molecules[0].structure_snapshot().unwrap();
        assert_eq!(after.hydrogen_cache.len(), 4);
        // The cache also survives session-style serialization.
        let decoded: crate::history::StructureSnapshot =
            serde_json::from_str(&serde_json::to_string(&after).unwrap()).unwrap();
        assert!(decoded.hydrogen_cache == after.hydrogen_cache);
        history.record_struct(
            id,
            crate::history::StructEdit::Topology { before, after },
            "remove H".into(),
        );
        history.maybe_record(crate::history::EditState::capture(&scene));
        assert_eq!(
            history.undo_len(),
            1,
            "index remapping is part of the hydrogen edit"
        );
        history.undo(&mut scene);
        assert_eq!(scene.molecules[0].n_atoms, 5);
        assert_eq!(scene.molecules[0].reps[0].sel_text, "index 0:4");
        history.maybe_record(crate::history::EditState::capture(&scene));
        assert_eq!(history.undo_len(), 0);
        history.redo(&mut scene);
        assert_eq!(scene.molecules[0].n_atoms, 1);
        assert_eq!(scene.molecules[0].visible_edit_atoms(), vec![0]);
        assert_eq!(
            scene.molecules[0].add_edit_hydrogens(&[0]).unwrap().0.len(),
            4
        );
        let restored = crate::history::StructureSnapshot::capture(&scene.molecules[0]);
        assert!(original.atoms == restored.atoms);
        assert_eq!(original.coords, restored.coords);
        assert_eq!(original.bonds, restored.bonds);
        assert!(restored.hydrogen_cache.is_empty());
    }

    #[test]
    fn cache_fills_only_missing_sites_after_partial_manual_restoration() {
        let mut mol = molecule(&["C"], &[[0.0; 3]], &[]);
        mol.add_edit_hydrogens(&[0]).unwrap().0;
        let original = coordinates(&mol);
        mol.remove_edit_hydrogens(&[0]);
        let h = mol
            .add_atom(&Atom::new().with_name("H").guess(), original[1])
            .unwrap();
        mol.add_bond(0, h, BondOrder::Single);
        assert_eq!(mol.add_edit_hydrogens(&[0]).unwrap().0.len(), 3);
        let positions = coordinates(&mol);
        for i in 1..5 {
            for j in i + 1..5 {
                assert!(positions[i].distance(positions[j]) > 0.1);
            }
        }
    }

    #[test]
    fn cached_hydrogens_follow_parent_translation_and_deleted_indices() {
        let mut mol = molecule(
            &["O", "C", "C"],
            &[[2.0, 0.0, 0.0], [0.0; 3], [0.154, 0.0, 0.0]],
            &[(1, 2, BondOrder::Single)],
        );
        mol.add_edit_hydrogens(&[1]).unwrap().0;
        let original = coordinates(&mol);
        assert_eq!(mol.remove_edit_hydrogens(&[1]), 3);
        mol.remove_atom(0); // cached parent shifts from 1 to 0
        let shift = Vec3::new(0.2, 0.3, 0.4);
        let positions: Vec<_> = coordinates(&mol)
            .iter()
            .map(|p| (*p + shift).to_array())
            .collect();
        mol.set_coords(&[0, 1], &positions, None);
        assert_eq!(mol.add_edit_hydrogens(&[0]).unwrap().0.len(), 3);
        for (a, b) in coordinates(&mol)[2..].iter().zip(&original[3..]) {
            assert!(a.distance(*b + shift) < 1e-6);
        }
        assert_eq!(mol.remove_edit_hydrogens(&[0]), 3);
        mol.remove_atom(0); // deleting the parent drops its cache
        assert!(mol.hydrogen_cache.is_empty());
    }

    #[test]
    fn cached_sites_follow_a_rigid_rotation() {
        let mut mol = molecule(
            &["C", "C", "O"],
            &[[0.0; 3], [0.154, 0.0, 0.0], [0.20, 0.13, 0.0]],
            &[(0, 1, BondOrder::Single), (1, 2, BondOrder::Single)],
        );
        mol.add_edit_hydrogens(&[0]).unwrap();
        let before = coordinates(&mol);
        mol.remove_edit_hydrogens(&[0]);
        let rotation = Quat::from_rotation_z(1.2) * Quat::from_rotation_x(0.7);
        let shift = Vec3::new(0.4, -0.2, 0.1);
        let moved: Vec<_> = before[..3]
            .iter()
            .map(|p| (rotation * *p + shift).to_array())
            .collect();
        mol.set_coords(&[0, 1, 2], &moved, None);
        let added = mol.add_edit_hydrogens(&[0]).unwrap().0;
        assert_eq!(added.len(), 3);
        for ((i, _), old) in added.iter().zip(&before[3..]) {
            assert!(coordinates(&mol)[*i].distance(rotation * *old + shift) < 1e-6);
        }
    }

    #[test]
    fn hydrogen_edits_leave_other_fragments_unchanged() {
        let mut mol = molecule(
            &["C", "C", "H"],
            &[[0.0; 3], [1.0, 0.0, 0.0], [1.109, 0.0, 0.0]],
            &[(1, 2, BondOrder::Single)],
        );
        mol.reps[0].sel_text = "index 0".into();
        mol.reps[0].sel_dirty = true;
        assert_eq!(mol.visible_edit_atoms(), vec![0]);
        let original = coordinates(&mol);
        let scope = mol.visible_edit_atoms();
        assert_eq!(mol.add_edit_hydrogens(&scope).unwrap().0.len(), 4);
        assert_eq!(&coordinates(&mol)[..3], &original);
        assert_eq!(mol.remove_edit_hydrogens(&scope), 4);
        assert_eq!(coordinates(&mol), original);
    }

    #[test]
    fn jak2_residue_restores_original_hydrogens_without_extra_carbonyl_h() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/jak2.pdb");
        let raw = crate::data::load(&path).unwrap();
        let mut mol = Molecule::new(reserve_mol_id(), raw, &RepDefaults::default());
        mol.reps[0].sel_text = "(chain A) and (resid 930)".into();
        let scope = mol.visible_edit_atoms();
        let original: Vec<_> = scope
            .iter()
            .filter_map(|&i| {
                let a = mol.data.topology().get_atom(i).unwrap();
                (a.get_atomic_number() == 1).then(|| (AtomLite::capture(&a), coordinates(&mol)[i]))
            })
            .collect();
        assert!(
            !original.is_empty(),
            "scope={scope:?}, atoms={:?}, error={:?}",
            mol.data
                .topology()
                .iter_atoms()
                .filter(|a| a.get_resid() == 930)
                .map(|a| (
                    a.get_name().to_owned(),
                    a.get_chain(),
                    a.get_atomic_number()
                ))
                .collect::<Vec<_>>(),
            mol.data.evaluate("(chain A) and (resid 930)").err()
        );
        assert_eq!(mol.remove_edit_hydrogens(&scope), original.len());
        let scope = mol.visible_edit_atoms();
        let added = mol.add_edit_hydrogens(&scope).unwrap().0;
        assert_eq!(added.len(), original.len());
        for ((i, _), (atom, pos)) in added.iter().zip(original) {
            assert!(AtomLite::capture(&mol.data.topology().get_atom(*i).unwrap()) == atom);
            assert_eq!(coordinates(&mol)[*i], pos);
        }
    }

    #[test]
    fn visible_scope_is_union_and_survives_interleaved_hydrogen_removal() {
        let mut mol = molecule(
            &["H", "C", "H", "C", "O"],
            &[
                [-0.109, 0.0, 0.0],
                [0.0; 3],
                [0.0, 0.109, 0.0],
                [0.154, 0.0, 0.0],
                [1.0, 0.0, 0.0],
            ],
            &[
                (0, 1, BondOrder::Single),
                (1, 2, BondOrder::Single),
                (1, 3, BondOrder::Single),
            ],
        );
        mol.reps[0].sel_text = "index 1".into();
        let mut second = crate::scene::Representation::from_defaults(&RepDefaults::default());
        second.sel_text = "index 3".into();
        mol.reps.push(second);
        let mut hidden = crate::scene::Representation::from_defaults(&RepDefaults::default());
        hidden.sel_text = "index 4".into();
        hidden.visible = false;
        mol.reps.push(hidden);
        assert_eq!(mol.visible_edit_atoms(), vec![1, 3]);
        mol.remove_edit_hydrogens(&mol.visible_edit_atoms());
        assert_eq!(mol.visible_edit_atoms(), vec![0, 1]);
        assert_eq!(mol.n_atoms, 3);
        assert!(mol.reps[2].sel_text.contains("index 2"));
        mol.reps[0].visible = false;
        mol.reps[1].visible = false;
        assert!(mol.visible_edit_atoms().is_empty());
    }

    #[test]
    fn amide_and_alkene_hydrogens_follow_the_molecular_plane() {
        // O=C-N, with the carbonyl in XY. The N-H bonds must share that plane.
        let mut mol = molecule(
            &["O", "C", "N", "C"],
            &[
                [0.0, 0.12, 0.0],
                [0.0; 3],
                [0.135, 0.0, 0.0],
                [-0.1, -0.1, 0.0],
            ],
            &[
                (0, 1, BondOrder::Double),
                (1, 2, BondOrder::Single),
                (1, 3, BondOrder::Single),
            ],
        );
        assert_eq!(mol.add_edit_hydrogens(&[2]).unwrap().0.len(), 2);
        let coords = coordinates(&mol);
        let nh: Vec<_> = coords[4..].iter().map(|h| *h - coords[2]).collect();
        for h in &nh {
            assert!(h.z.abs() < 1e-6);
            assert!((h.length() - 0.101).abs() < 1e-5);
        }
        assert!((nh[0].normalize().dot(nh[1].normalize()) + 0.5).abs() < 1e-4);
        let mut alkene = molecule(
            &["C", "C", "C"],
            &[[0.0; 3], [0.134, 0.0, 0.0], [0.2, 0.13, 0.0]],
            &[(0, 1, BondOrder::Double), (1, 2, BondOrder::Single)],
        );
        assert_eq!(alkene.add_edit_hydrogens(&[0]).unwrap().0.len(), 2);
        for h in &coordinates(&alkene)[3..] {
            assert!(h.z.abs() < 1e-6);
        }
    }
}
