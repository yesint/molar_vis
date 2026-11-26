//! Editable molecule loading and ligand export for embedding clients.
use super::App;
use crate::data;
use molar::prelude::{AtomLike, AtomProvider, BondOrder};
use std::fmt::Write;

const ELEMENTS: [&str; 119] = [
    "X", "H", "He", "Li", "Be", "B", "C", "N", "O", "F", "Ne", "Na", "Mg", "Al", "Si", "P", "S",
    "Cl", "Ar", "K", "Ca", "Sc", "Ti", "V", "Cr", "Mn", "Fe", "Co", "Ni", "Cu", "Zn", "Ga", "Ge",
    "As", "Se", "Br", "Kr", "Rb", "Sr", "Y", "Zr", "Nb", "Mo", "Tc", "Ru", "Rh", "Pd", "Ag", "Cd",
    "In", "Sn", "Sb", "Te", "I", "Xe", "Cs", "Ba", "La", "Ce", "Pr", "Nd", "Pm", "Sm", "Eu", "Gd",
    "Tb", "Dy", "Ho", "Er", "Tm", "Yb", "Lu", "Hf", "Ta", "W", "Re", "Os", "Ir", "Pt", "Au", "Hg",
    "Tl", "Pb", "Bi", "Po", "At", "Rn", "Fr", "Ra", "Ac", "Th", "Pa", "U", "Np", "Pu", "Am", "Cm",
    "Bk", "Cf", "Es", "Fm", "Md", "No", "Lr", "Rf", "Db", "Sg", "Bh", "Hs", "Mt", "Ds", "Rg", "Cn",
    "Nh", "Fl", "Mc", "Lv", "Ts", "Og",
];

impl App {
    /// Load an owned molecule, so the drawing editor can change its topology.
    pub fn add_editable_bytes(&mut self, name: &str, bytes: Vec<u8>) -> Result<u64, String> {
        let raw = data::load_from_bytes(name, bytes, &self.settings.behavior.bond_params())?;
        self.add_loaded(raw);
        Ok(self
            .scene
            .molecules
            .last()
            .ok_or("No molecule was loaded")?
            .id
            .0)
    }

    /// Enter the native drawing editor or accept its edits for this molecule.
    pub fn set_molecule_editing(&mut self, id: u64, enabled: bool) -> Result<(), String> {
        let index = self.resolve_molecule(id)?;
        if enabled {
            if self.scene.molecules[index].data.is_shared() {
                return Err("The drawing editor requires an owned molecule".into());
            }
            if !self.molecule_editing(id)? {
                self.open_molecule_in_editor(index);
            }
        } else if self.molecule_editing(id)? {
            self.end_draw();
        }
        Ok(())
    }

    pub fn molecule_editing(&self, id: u64) -> Result<bool, String> {
        self.resolve_molecule(id)?;
        Ok(self
            .draw
            .as_ref()
            .is_some_and(|s| s.target.is_some_and(|target| target.0 == id)))
    }

    /// Export the ligand graph in mmCIF, preserving unknown charges and bond orders.
    pub fn molecule_cif(&self, id: u64) -> Result<String, String> {
        let mol = &self.scene.molecules[self.resolve_molecule(id)?];
        if mol.n_atoms == 0 {
            return Err("The ligand has no atoms".into());
        }
        let mut out = String::from(concat!(
            "data_ligand\n",
            "#\n",
            "loop_\n",
            "_atom_site.group_PDB\n",
            "_atom_site.id\n",
            "_atom_site.type_symbol\n",
            "_atom_site.label_atom_id\n",
            "_atom_site.label_comp_id\n",
            "_atom_site.label_asym_id\n",
            "_atom_site.label_seq_id\n",
            "_atom_site.Cartn_x\n",
            "_atom_site.Cartn_y\n",
            "_atom_site.Cartn_z\n",
            "_atom_site.pdbx_formal_charge\n",
        ));
        for (i, pos) in mol.render_state().coords.iter().enumerate() {
            let atom = mol
                .data
                .topology()
                .get_atom(i)
                .ok_or("Missing ligand atom")?;
            let z = atom.get_atomic_number() as usize;
            let symbol = ELEMENTS
                .get(z)
                .filter(|_| z > 0)
                .ok_or_else(|| format!("Atom {} has no chemical element", i + 1))?;
            if !pos.iter().all(|v| v.is_finite()) {
                return Err(format!("Atom {} has invalid coordinates", i + 1));
            }
            let charge = atom
                .get_formal_charge()
                .map_or_else(|| "?".into(), |c| c.to_string());
            writeln!(
                out,
                "HETATM {} {} A{} LIG L 1 {:.6} {:.6} {:.6} {}",
                i + 1,
                symbol,
                i + 1,
                pos.x * 10.0,
                pos.y * 10.0,
                pos.z * 10.0,
                charge
            )
            .unwrap();
        }
        if !mol.bonds.is_empty() {
            out.push_str(concat!(
                "#\n",
                "loop_\n",
                "_struct_conn.id\n",
                "_struct_conn.conn_type_id\n",
                "_struct_conn.ptnr1_label_comp_id\n",
                "_struct_conn.ptnr1_label_asym_id\n",
                "_struct_conn.ptnr1_label_seq_id\n",
                "_struct_conn.ptnr1_label_atom_id\n",
                "_struct_conn.ptnr2_label_comp_id\n",
                "_struct_conn.ptnr2_label_asym_id\n",
                "_struct_conn.ptnr2_label_seq_id\n",
                "_struct_conn.ptnr2_label_atom_id\n",
                "_struct_conn.pdbx_value_order\n",
            ));
            for (i, bond) in mol.bonds.iter().enumerate() {
                let order = match bond.order {
                    BondOrder::Single => "sing",
                    BondOrder::Double => "doub",
                    BondOrder::Triple => "trip",
                    BondOrder::Aromatic => "arom",
                    BondOrder::Unspecified => "?",
                };
                writeln!(
                    out,
                    "b{} covale LIG L 1 A{} LIG L 1 A{} {}",
                    i + 1,
                    bond.i1 + 1,
                    bond.i2 + 1,
                    order
                )
                .unwrap();
            }
        }
        out.push_str("#\n");
        Ok(out)
    }
}
