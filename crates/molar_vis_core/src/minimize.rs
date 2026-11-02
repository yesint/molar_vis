//! GAFF2 + Espaloma geometry cleanup using molar_ff's cutoff nonbonded engine
//! and energy-decreasing L-BFGS minimizer. All preparation uses a private model;
//! only visible original atom coordinates are copied back after a successful run.
use molar::prelude::*;
use molar_ff::{
    amber::{AmberModel, EvaluationError, MinimizeOptions, NonbondedOptions, StopReason},
    ApplyCharges, ChargeModel, FFType,
};

pub use molar::prelude::{Bond, BondOrder};

/// Editor/force-field helpers on molar's foreign [`BondOrder`] (can't be inherent
/// methods on a foreign type, so they ride an extension trait).
pub trait BondOrderExt {
    /// Next order when the user clicks a bond to cycle it
    /// (unspecified/single → double → triple → single). Aromatic → single.
    fn cycle(self) -> Self;
    /// Short human label for tooltips.
    fn label(self) -> &'static str;
}

impl BondOrderExt for BondOrder {
    fn cycle(self) -> Self {
        match self {
            BondOrder::Unspecified | BondOrder::Single => BondOrder::Double,
            BondOrder::Double => BondOrder::Triple,
            BondOrder::Triple => BondOrder::Single,
            BondOrder::Aromatic => BondOrder::Single,
        }
    }

    fn label(self) -> &'static str {
        match self {
            BondOrder::Unspecified => "Unspecified",
            BondOrder::Single => "Single",
            BondOrder::Double => "Double",
            BondOrder::Triple => "Triple",
            BondOrder::Aromatic => "Aromatic",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum RelaxKind {
    Quick,
    Cleanup,
}

#[derive(Clone, Debug)]
pub struct MinimizeResult {
    pub steps: usize,
    pub final_force_norm: f64,
    pub converged: bool,
    pub initial_energy: f64,
    pub final_energy: f64,
    pub estimated_terms: usize,
    pub temporary_hydrogens: usize,
    pub stop_reason: StopReason,
}

/// Prepare complete chemistry in a temporary model. The source's atoms, charges,
/// types, bond orders, hydrogen cache, and fixed coordinates remain unchanged.
/// The input bond list is the viewer's current connectivity, including edits.
pub fn relax_in_system(
    system: &mut System,
    bonds: &[Bond],
    kind: RelaxKind,
    atoms: &[usize],
) -> Result<MinimizeResult, String> {
    let n = system.topology().atoms.len();
    if atoms.is_empty() {
        return Err("No visible atoms to optimize".into());
    }
    if atoms.iter().any(|&i| i >= n) {
        return Err("The editable atom list is out of date".into());
    }
    let mut topology = system.topology().clone();
    topology.bonds = crate::data::bond_storage(bonds);
    let mut prepared = System::new(topology, system.state().clone())
        .map_err(|e| format!("Cannot prepare optimization: {e}"))?;
    // Hydrogen placement needs a defined direction for each existing bond.
    // Separate coincident editable endpoints on the private model first.
    let mut original_movable = vec![false; n];
    for &i in atoms {
        original_movable[i] = true;
    }
    let mut seed = prepared.state().coords.clone();
    if seed.iter().any(|p| p.iter().any(|v| !v.is_finite())) {
        return Err("Cannot optimize non-finite coordinates".into());
    }
    for bond in bonds {
        let (a, b) = (bond.i1, bond.i2);
        if a >= n || b >= n || a == b {
            return Err("Invalid bond in optimization input".into());
        }
        if (seed[a] - seed[b]).norm_squared() < 1e-12 {
            let i = if original_movable[b] {
                b
            } else if original_movable[a] {
                a
            } else {
                return Err("Cannot optimize: a bond joins two coincident fixed atoms".into());
            };
            seed[i] += Vector3f::new(0.0001, 0.00017, 0.00013);
        }
    }
    for (p, value) in prepared.select_all_bound_mut().iter_pos_mut().zip(seed) {
        *p = value;
    }
    if bonds.iter().any(|b| b.order == BondOrder::Unspecified) {
        let assignment = prepared
            .assign_bond_orders(&BondOrderOptions {
                input_orders: InputOrders::PreserveKnown,
                hydrogens: HydrogenPolicy::InferFromGeometry,
                ..Default::default()
            })
            .map_err(|e| format!("Cannot determine bond orders for optimization: {e}"))?;
        prepared
            .apply_bond_assignment(&assignment)
            .map_err(|e| e.to_string())?;
    }
    let plan = plan_hydrogen_addition(&prepared, &HydrogenOptions::default());
    let temporary_hydrogens = plan.len();
    prepared
        .add_hydrogens(&plan)
        .map_err(|e| format!("Cannot complete hydrogens for optimization: {e}"))?;
    // Equilibrate each disconnected component separately so formal charges cannot
    // leak between separate fragments merely stored in the same viewer molecule.
    let mut neighbors = vec![vec![]; prepared.topology().atoms.len()];
    for b in prepared.iter_bonds() {
        let [a, c] = b.pair();
        neighbors[a].push(c);
        neighbors[c].push(a);
    }
    let mut seen = vec![false; neighbors.len()];
    for start in 0..neighbors.len() {
        if seen[start] {
            continue;
        }
        let mut stack = vec![start];
        let mut component = vec![];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            component.push(i);
            for &j in &neighbors[i] {
                if !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        let selection = Sel::from_vec(component).map_err(|e| e.to_string())?;
        prepared
            .try_bind_mut(&selection)
            .map_err(|e| e.to_string())?
            .apply_charges(ChargeModel::Espaloma)
            .map_err(|e| format!("Cannot assign optimization charges: {e}"))?;
    }
    let model = AmberModel::gaff(&prepared, FFType::Gaff2, NonbondedOptions::default())
        .map_err(|e| format!("Cannot prepare GAFF2 optimization: {e}"))?;
    let estimated_terms = model.bonded().estimated_terms().count();
    let mut coordinates: Vec<[f64; 3]> = prepared
        .state()
        .coords
        .iter()
        .map(|p| [p.x as f64, p.y as f64, p.z as f64])
        .collect();
    let mut movable = vec![false; coordinates.len()];
    for &i in atoms {
        movable[i] = true;
    }
    movable[n..].fill(true); // temporary hydrogens are internal degrees of freedom
    regularize_sketch(&model, &mut coordinates, &movable)?;
    let options = match kind {
        RelaxKind::Quick => MinimizeOptions {
            max_steps: 60,
            force_tolerance: 200.0,
            ..Default::default()
        },
        RelaxKind::Cleanup => MinimizeOptions::default(),
    };
    let result = model
        .minimize(&mut coordinates, &movable, options)
        .map_err(|e| format!("Optimization failed: {e}"))?;
    // Validate the entire result before any write. Do not silently clamp bad forces
    // or apply part of a failed optimization to the displayed molecule.
    if coordinates
        .iter()
        .flatten()
        .any(|&v| !(v as f32).is_finite())
    {
        return Err("Optimization produced invalid coordinates".into());
    }
    let mut target = system.select_all_bound_mut();
    for (i, position) in target.iter_pos_mut().enumerate() {
        if movable[i] {
            let c = coordinates[i];
            *position = Pos::new(c[0] as _, c[1] as _, c[2] as _);
        }
    }
    Ok(MinimizeResult {
        steps: result.steps,
        final_force_norm: result.max_force,
        converged: result.converged(),
        initial_energy: result.initial_energy.total(),
        final_energy: result.final_energy.total(),
        estimated_terms,
        temporary_hydrogens,
        stop_reason: result.reason,
    })
}

/// Exact collinearity/coincidence is common in 2-D sketches. Perturb only movable
/// atoms of the undefined interaction, deterministically, by at most 0.0001 nm per
/// attempt. This is preparation, not a change to the force-field equations.
fn regularize_sketch(
    model: &AmberModel,
    positions: &mut [[f64; 3]],
    movable: &[bool],
) -> Result<(), String> {
    for attempt in 0..32 {
        let atoms = match model.evaluate(positions) {
            Ok(_) => return Ok(()),
            Err(EvaluationError::Degenerate { atoms, .. }) => atoms,
            Err(EvaluationError::CoincidentNonbonded { atoms }) => atoms.to_vec(),
            Err(e) => return Err(format!("Cannot optimize these coordinates: {e}")),
        };
        let mut moved = false;
        for i in atoms {
            if movable[i] {
                for axis in 0..3 {
                    positions[i][axis] +=
                        0.0001 * ((i * 17 + axis * 7 + attempt * 11 + 1) as f64).sin();
                }
                moved = true;
            }
        }
        if !moved {
            return Err("Cannot optimize: undefined geometry involves only fixed atoms".into());
        }
    }
    Err("Cannot resolve degenerate sketch geometry for optimization".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sketch(z: &[u8], positions: &[[f32; 3]], bonds: &[Bond]) -> System {
        let mut top = Topology::default();
        for &z in z {
            let mut a = Atom::new().with_name("atom");
            a.atomic_number = z;
            top.atoms.push(&a);
        }
        top.bonds = crate::data::bond_storage(bonds);
        System::new(
            top,
            State {
                coords: positions
                    .iter()
                    .map(|p| Pos::new(p[0], p[1], p[2]))
                    .collect(),
                ..Default::default()
            },
        )
        .unwrap()
    }
    #[test]
    fn gaff_cleanup_completes_hydrogens_privately_and_keeps_hidden_atoms_fixed() {
        let bonds = [Bond::with_order(0, 1, BondOrder::Single)];
        let mut sys = sketch(&[6, 6], &[[0.0, 0.0, 0.0], [0.25, 0.0, 0.0]], &bonds);
        let fixed = sys.state().coords[1];
        let result = relax_in_system(&mut sys, &bonds, RelaxKind::Cleanup, &[0]).unwrap();
        assert!(result.final_energy < result.initial_energy);
        assert!(result.converged, "{result:?}");
        assert_eq!(result.temporary_hydrogens, 6);
        assert_eq!(sys.topology().atoms.len(), 2);
        assert_eq!(sys.topology().bonds.len(), 1);
        assert_eq!(sys.state().coords[1], fixed);
        assert!(sys
            .iter_atoms()
            .all(|a| a.get_type_name().is_none() && a.get_charge() == 0.0));
        let length = (sys.state().coords[0] - sys.state().coords[1]).norm();
        assert!((0.145..0.165).contains(&length), "C-C length {length}");
    }
    #[test]
    fn unknown_orders_are_prepared_without_changing_the_input_topology() {
        let bonds = [Bond::with_order(0, 1, BondOrder::Unspecified)];
        let mut sys = sketch(&[6, 6], &[[0.0, 0.0, 0.0], [0.154, 0.0, 0.0]], &bonds);
        relax_in_system(&mut sys, &bonds, RelaxKind::Cleanup, &[0, 1]).unwrap();
        assert_eq!(
            sys.topology().bonds.iter().next().unwrap().order(),
            BondOrder::Unspecified
        );
        assert_eq!(sys.topology().atoms.len(), 2);
    }
    #[test]
    fn failed_parameterization_never_moves_the_source() {
        let bonds = [Bond::with_order(0, 1, BondOrder::Single)];
        let mut sys = sketch(&[6, 26], &[[0.0, 0.0, 0.0], [0.2, 0.0, 0.0]], &bonds);
        let before = sys.state().coords.clone();
        assert!(relax_in_system(&mut sys, &bonds, RelaxKind::Cleanup, &[0, 1]).is_err());
        assert_eq!(sys.state().coords, before);
        assert_eq!(sys.topology().atoms.len(), 2);
    }
    #[test]
    fn coincident_sketch_atoms_are_regularized_before_minimization() {
        let bonds = [Bond::with_order(0, 1, BondOrder::Single)];
        let mut sys = sketch(&[6, 6], &[[0.0; 3]; 2], &bonds);
        let result = relax_in_system(&mut sys, &bonds, RelaxKind::Cleanup, &[0, 1]).unwrap();
        assert!(result.final_energy < result.initial_energy);
        assert!(sys
            .state()
            .coords
            .iter()
            .all(|p| p.iter().all(|v| v.is_finite())));
        assert!((sys.state().coords[0] - sys.state().coords[1]).norm() > 0.1);
    }
}
