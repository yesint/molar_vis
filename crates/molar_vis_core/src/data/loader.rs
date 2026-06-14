//! Load a structure file with molar and prepare it for the scene.
//!
//! molar stores coordinates in nanometers and `atom.vdw()` returns the van der
//! Waals radius in nm, so everything downstream (geometry, camera, clip planes)
//! is in nm with no conversion. The molar `System` is retained and is the single
//! source of per-atom data (positions, elements, radii) — we cache only the
//! guessed bonds and the bounding box, never a copy of the coordinates.

use std::path::Path;

use glam::Vec3;
use molar::prelude::*;

use crate::data::bonds;

// We read molar positions (Point3<Float>) as f32 into GPU buffers, which is only
// valid when Float == f32. molar's `f64` feature is opt-in and disabled here
// (default-features = false); this guard fails the build loudly otherwise.
const _: () = assert!(std::mem::size_of::<molar::Float>() == 4);

/// A loaded structure: the live molar `System` (the source of all per-atom data),
/// guessed connectivity, and a cached bounding box. Wrapped into a
/// `scene::Molecule` (with representations) by the caller.
pub struct RawMolecule {
    pub name: String,
    pub system: System,
    pub n_atoms: usize,
    pub bonds: Vec<[usize; 2]>,
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
}

/// Load `path` (PDB/GRO/… anything molar reads) and guess bonds.
pub fn load(path: &Path) -> Result<RawMolecule, String> {
    #[cfg(not(target_arch = "wasm32"))]
    let t0 = std::time::Instant::now();

    let system = System::from_file(path)
        .map_err(|e| format!("failed to load {}: {e}", path.display()))?;

    // Guess bonds and the bounding box while borrowing the system, using only
    // transient arrays (positions/radii) that are dropped here — the System
    // remains the single source of coordinates.
    let (bonds, bbox_min, bbox_max, n) = {
        let all = system.select_all_bound();
        let (min, max) = all.min_max();
        let n = all.len();

        let mut positions = Vec::with_capacity(n);
        let mut vdw = Vec::with_capacity(n);
        for (pos, atom) in all.iter_pos().zip(all.iter_atoms()) {
            positions.push([pos.x, pos.y, pos.z]);
            vdw.push(atom.vdw());
        }
        let bonds = bonds::guess(&all, &positions, &vdw);
        (
            bonds,
            Vec3::new(min.x, min.y, min.z),
            Vec3::new(max.x, max.y, max.z),
            n,
        )
    };

    #[cfg(not(target_arch = "wasm32"))]
    log::info!(
        "loaded {} atoms, {} bonds from {} in {:.2?}",
        n,
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
        system,
        n_atoms: n,
        bonds,
        bbox_min,
        bbox_max,
    })
}
