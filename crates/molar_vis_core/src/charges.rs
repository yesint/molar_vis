//! Partial-charge assignment on a selection, via `molar_ff`'s espaloma-charge model.
//!
//! The `Charge` color scheme paints whatever charge a molecule already carries; this is
//! how a molecule that carries none gets one. The prediction is a graph neural network
//! (a bundled ONNX run through `tract`), so it needs **chemistry**, not coordinates:
//! explicit Kekulé bond orders and a bond-complete selection. That is exactly what an
//! SDF/MOL ligand provides and what a PDB/GRO does not — see [`compute_espaloma`]'s
//! error cases, which the `Color` tab surfaces verbatim.
//!
//! Native only: `tract` plus a ~600 kB model has no business in the wasm bundle, and the
//! browser build has no way to obtain charges anyway. The *coloring* works everywhere.

use molar::prelude::*;
use molar_ff::{ApplyCharges, ChargeError, ChargeModel};

use crate::scene::Molecule;

/// The per-atom charge change a successful [`compute_espaloma`] made: the selected atoms
/// and their charges before and after, ready to be recorded as an undo step.
pub struct ChargeEdit {
    pub atoms: Vec<usize>,
    pub before: Vec<f32>,
    pub after: Vec<f32>,
}

/// Predict espaloma partial charges for `sel`'s atoms and write them into the molecule,
/// returning the before/after delta so the caller can make it undoable.
///
/// Fails, with a message meant for the UI, when:
///
/// * the molecule is externally owned (a shared pymolar/JS `System` — charge it from the
///   host language instead, which owns the memory);
/// * a bond in the selection has no explicit single/double/triple order. Distance-guessed
///   connectivity (PDB/GRO) is order-less and aromatized bonds are rejected too, so this
///   is the common failure: the model needs a Kekulé graph and no amount of geometry
///   recovers one;
/// * the selection cuts through a bond, so it isn't a whole molecule (charges are
///   equilibrated over the whole graph, so a fragment would be meaningless);
/// * the molecule contains an element the model was never trained on.
pub fn compute_espaloma(mol: &mut Molecule, sel: &Sel) -> Result<ChargeEdit, String> {
    // Grow the selection to the **complete molecules** it touches. Charges are equilibrated
    // over a whole connected graph, so a selection that cuts a bond has no valid answer — and
    // ordinary *viewing* selections cut bonds all the time (`not apolh`, which the docking
    // loader sets, severs every C–H). Treating the selection as "which molecules to charge"
    // rather than "which atoms" is the only reading that both matches the button and produces
    // correct chemistry; the hidden atoms still get their charges, they just aren't painted.
    let seeds: Vec<usize> = sel.iter_index().collect();
    let atoms = mol.connected_closure(&seeds);
    let sel = &Sel::from_vec(atoms.clone())
        .map_err(|e| format!("can't select the complete molecule: {e}"))?;
    let sys = mol.data.system_mut().ok_or(
        "this molecule's data is owned by the host program; compute charges there instead",
    )?;
    let before: Vec<f32> = {
        let bound = sys.bind(sel);
        bound.iter_atoms().map(|a| a.get_charge()).collect()
    };

    sys.try_bind_mut(sel)
        .map_err(|e| format!("can't select those atoms: {e}"))?
        .apply_charges(ChargeModel::Espaloma)
        .map_err(explain)?;

    let after: Vec<f32> = {
        let bound = sys.bind(sel);
        bound.iter_atoms().map(|a| a.get_charge()).collect()
    };
    Ok(ChargeEdit { atoms, before, after })
}

/// Turn a [`ChargeError`] into advice. The raw messages name atom indices, which is fine,
/// but they don't say what the user should *do* — and for the overwhelmingly common case
/// (no bond orders) the answer is "load an SDF/MOL instead", which is worth spelling out.
fn explain(e: ChargeError) -> String {
    match e {
        ChargeError::MissingBondOrders(..) => format!(
            "{e}\n\nEspaloma needs explicit single/double/triple bonds. Structures whose \
             connectivity was guessed from distances (PDB, GRO, XYZ) don't have them — \
             load the molecule from an SDF/MOL file, or draw it in the structure editor."
        ),
        ChargeError::OpenSelection { .. } => format!(
            "{e}\n\nCharges are equilibrated over a whole molecule, so the selection has \
             to contain every atom of it. Widen the selection to the complete molecule."
        ),
        other => other.to_string(),
    }
}
