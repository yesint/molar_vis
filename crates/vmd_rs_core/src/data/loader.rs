//! Load a structure file with molar and extract render-ready per-atom data.
//!
//! molar stores coordinates in nanometers and `atom.vdw()` returns the van der
//! Waals radius in nm, so everything downstream (geometry, camera, clip planes)
//! is in nm with no conversion.

use std::path::Path;

use glam::Vec3;
use molar::prelude::*;

use crate::color;
use crate::data::bonds;

// We bulk-copy molar positions (Point3<Float>) into f32 buffers, which is only
// valid when Float == f32. molar's `f64` feature is opt-in and disabled here
// (default-features = false); this guard fails the build loudly otherwise.
const _: () = assert!(std::mem::size_of::<molar::Float>() == 4);

/// Render-ready per-atom data plus guessed connectivity. Coordinates are in nm.
pub struct LoadedMolecule {
    pub name: String,
    pub n_atoms: usize,
    pub positions: Vec<[f32; 3]>,
    /// Van der Waals radius per atom (nm).
    pub vdw: Vec<f32>,
    /// Element color per atom (RGBA8 packed).
    pub colors: Vec<u32>,
    pub bonds: Vec<[u32; 2]>,
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
}

/// Load `path` (PDB/GRO/… anything molar reads), extract per-atom arrays, and
/// guess bonds.
pub fn load(path: &Path) -> Result<LoadedMolecule, String> {
    #[cfg(not(target_arch = "wasm32"))]
    let t0 = std::time::Instant::now();

    let system = System::from_file(path)
        .map_err(|e| format!("failed to load {}: {e}", path.display()))?;

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

    Ok(LoadedMolecule {
        name,
        n_atoms: n,
        positions,
        vdw,
        colors,
        bonds,
        bbox_min: Vec3::new(min.x, min.y, min.z),
        bbox_max: Vec3::new(max.x, max.y, max.z),
    })
}
