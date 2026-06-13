//! Load a structure file with molar and extract render-ready per-atom data.
//!
//! molar stores coordinates in nanometers and `atom.vdw()` returns the van der
//! Waals radius in nm, so everything downstream (geometry, camera, clip planes)
//! is in nm with no conversion. The molar `System` is retained so each
//! representation can evaluate its own selection string.

use std::path::Path;

use glam::Vec3;
use molar::prelude::*;

use crate::color;
use crate::data::bonds;

// We bulk-copy molar positions (Point3<Float>) into f32 buffers, which is only
// valid when Float == f32. molar's `f64` feature is opt-in and disabled here
// (default-features = false); this guard fails the build loudly otherwise.
const _: () = assert!(std::mem::size_of::<molar::Float>() == 4);

/// Raw per-atom data + connectivity + the live molar `System`, as loaded from a
/// file. Wrapped into a `scene::Molecule` (with representations) by the caller.
pub struct RawMolecule {
    pub name: String,
    pub system: System,
    pub n_atoms: usize,
    pub positions: Vec<[f32; 3]>,
    pub vdw: Vec<f32>,
    pub colors: Vec<u32>,
    pub bonds: Vec<[usize; 2]>,
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
}

/// Load `path` (PDB/GRO/… anything molar reads), extract per-atom arrays, and
/// guess bonds. The `System` is returned for later selection evaluation.
pub fn load(path: &Path) -> Result<RawMolecule, String> {
    #[cfg(not(target_arch = "wasm32"))]
    let t0 = std::time::Instant::now();

    let system = System::from_file(path)
        .map_err(|e| format!("failed to load {}: {e}", path.display()))?;

    // Extract arrays and guess bonds while borrowing the system, then release the
    // borrow so the system can be moved into the returned struct.
    let (positions, vdw, colors, bonds, bbox_min, bbox_max) = {
        let all = system.select_all_bound();
        let (min, max) = all.min_max();
        let n = all.len();

        let mut positions = Vec::with_capacity(n);
        let mut vdw = Vec::with_capacity(n);
        let mut colors = Vec::with_capacity(n);
        for (pos, atom) in all.iter_pos().zip(all.iter_atoms()) {
            positions.push([pos.x, pos.y, pos.z]);
            vdw.push(atom.vdw());
            colors.push(color::pack_rgba8(color::element_color(atom.atomic_number)));
        }
        let bonds = bonds::guess(&all, &positions, &vdw);
        (
            positions,
            vdw,
            colors,
            bonds,
            Vec3::new(min.x, min.y, min.z),
            Vec3::new(max.x, max.y, max.z),
        )
    };

    #[cfg(not(target_arch = "wasm32"))]
    log::info!(
        "loaded {} atoms, {} bonds from {} in {:.2?}",
        positions.len(),
        bonds.len(),
        path.display(),
        t0.elapsed()
    );

    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "molecule".to_string());

    Ok(RawMolecule {
        name,
        n_atoms: positions.len(),
        system,
        positions,
        vdw,
        colors,
        bonds,
        bbox_min,
        bbox_max,
    })
}
