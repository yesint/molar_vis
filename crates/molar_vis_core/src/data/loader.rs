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

use crate::data::bonds::{self, BondParams};
use crate::scene::MoleculeSource;

// We read molar positions (Point3<Float>) as f32 into GPU buffers, which is only
// valid when Float == f32. molar's `f64` feature is opt-in and disabled here
// (default-features = false); this guard fails the build loudly otherwise.
const _: () = assert!(std::mem::size_of::<molar::Float>() == 4);

/// A loaded structure: the live molar `System` (the source of all per-atom data),
/// guessed connectivity, and a cached bounding box. Wrapped into a
/// `scene::Molecule` (with representations) by the caller.
pub struct RawMolecule {
    pub name: String,
    /// Where this structure came from, so a saved session can reload it.
    pub source: MoleculeSource,
    pub system: System,
    pub n_atoms: usize,
    pub bonds: Vec<Bond>,
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
}

/// Load `path` (PDB/GRO/… anything molar reads) and guess bonds with the default
/// thresholds. Convenience wrapper over [`load_with`]; production code threads the
/// settings-derived params through `load_with`, so this is currently test-only.
#[cfg(test)]
pub fn load(path: &Path) -> Result<RawMolecule, String> {
    load_with(path, &BondParams::default())
}

/// Like [`load`] but with caller-supplied bond-guessing thresholds (from the
/// program settings).
pub fn load_with(path: &Path, bonds: &BondParams) -> Result<RawMolecule, String> {
    let system = System::from_file(path)
        .map_err(|e| format!("failed to load {}: {e}", path.display()))?;
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "molecule".to_string());
    Ok(assemble(system, name, MoleculeSource::File(path.to_path_buf()), bonds))
}

/// Load **every** record of a multi-molecule file as a **distinct** molecule:
/// molar's `FileHandler::read()` returns a fresh `(Topology, State)` for each
/// `$$$$`-separated SDF record (each its own atoms + bonds), so we loop it to EOF.
/// Used to build a [`crate::scene::MolGroup`]. Member display names come from each
/// record's molfile title line when present, else `"{stem} #{n}"`. Errors if the
/// file can't be opened or a record is malformed; an empty file is an error.
pub fn load_records(path: &Path, bonds: &BondParams) -> Result<Vec<RawMolecule>, String> {
    let mut fh = FileHandler::open(path)
        .map_err(|e| format!("failed to load {}: {e}", path.display()))?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "molecule".to_string());
    // Titles are a display nicety; read failures just fall back to generated names.
    let titles = std::fs::read_to_string(path).ok().map(|t| sdf_titles(&t)).unwrap_or_default();
    let mut out = Vec::new();
    loop {
        match fh.read() {
            Ok((top, st)) => {
                let i = out.len();
                let system = System::new(top, st)
                    .map_err(|e| format!("invalid record {} in {}: {e}", i + 1, path.display()))?;
                let name = record_name(&titles, i, &stem);
                out.push(assemble(
                    system,
                    name,
                    MoleculeSource::SdfRecord { path: path.to_path_buf(), index: i },
                    bonds,
                ));
            }
            Err(e) if matches!(e.kind(), FileFormatError::Eof) => break,
            Err(e) => {
                return Err(format!(
                    "failed to read record {} in {}: {e}",
                    out.len() + 1,
                    path.display()
                ))
            }
        }
    }
    if out.is_empty() {
        return Err(format!("no molecules found in {}", path.display()));
    }
    Ok(out)
}

/// Multi-record loader over in-memory bytes (the browser path). Members carry a
/// `Bytes` source (no path to reload from), so a wasm-loaded group can't be saved
/// to a session — same limitation as any byte-sourced molecule.
#[cfg(target_arch = "wasm32")]
pub fn load_records_from_bytes(
    name: &str,
    bytes: Vec<u8>,
    bonds: &BondParams,
) -> Result<Vec<RawMolecule>, String> {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name).to_string();
    let titles = std::str::from_utf8(&bytes).ok().map(sdf_titles).unwrap_or_default();
    let mut fh = FileHandler::from_reader(&ext, std::io::Cursor::new(bytes))
        .map_err(|e| format!("can't read {name}: {e}"))?;
    let mut out = Vec::new();
    loop {
        match fh.read() {
            Ok((top, st)) => {
                let i = out.len();
                let system = System::new(top, st)
                    .map_err(|e| format!("invalid record {} in {name}: {e}", i + 1))?;
                let rname = record_name(&titles, i, &stem);
                out.push(assemble(
                    system,
                    rname.clone(),
                    MoleculeSource::Bytes { name: rname },
                    bonds,
                ));
            }
            Err(e) if matches!(e.kind(), FileFormatError::Eof) => break,
            Err(e) => return Err(format!("failed to read record {} in {name}: {e}", out.len() + 1)),
        }
    }
    if out.is_empty() {
        return Err(format!("no molecules found in {name}"));
    }
    Ok(out)
}

/// Extract each record's title line (the molfile's first line) from raw SDF/MOL
/// text: the file's first line, plus the first line after every `$$$$` separator.
/// Blank titles come back empty (the caller then generates a name).
fn sdf_titles(text: &str) -> Vec<String> {
    let mut titles = Vec::new();
    let mut at_title = true; // the very first line is a title
    for line in text.lines() {
        if at_title {
            titles.push(line.trim().to_string());
            at_title = false;
        }
        if line.trim_end() == "$$$$" {
            at_title = true; // the next line begins the next record
        }
    }
    titles
}

/// A member's display name: its molfile title if non-empty, else `"{stem} #{n}"`.
fn record_name(titles: &[String], i: usize, stem: &str) -> String {
    match titles.get(i) {
        Some(t) if !t.is_empty() => t.clone(),
        _ => format!("{stem} #{}", i + 1),
    }
}

/// Load a structure from in-memory bytes (the browser path: the file picker reads
/// a `File`/`Blob` into a `Vec<u8>`). The format is taken from `name`'s extension.
/// Uses molar's `FileHandler::from_reader`, so no filesystem access is needed.
#[cfg(target_arch = "wasm32")]
pub fn load_from_bytes(name: &str, bytes: Vec<u8>, bonds: &BondParams) -> Result<RawMolecule, String> {
    let ext = name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut fh = FileHandler::from_reader(&ext, std::io::Cursor::new(bytes))
        .map_err(|e| format!("can't read {name}: {e}"))?;
    let (top, st) = fh
        .read()
        .map_err(|e| format!("failed to parse {name}: {e}"))?;
    let system = System::new(top, st).map_err(|e| format!("invalid structure in {name}: {e}"))?;
    Ok(assemble(
        system,
        name.to_string(),
        MoleculeSource::Bytes { name: name.to_string() },
        bonds,
    ))
}

/// Shared tail of [`load`]/[`load_from_bytes`]: guess bonds and the bounding box
/// from the freshly loaded `system`, using only transient arrays (positions/radii)
/// that are dropped here — the `System` stays the single source of coordinates.
fn assemble(
    system: System,
    name: String,
    source: MoleculeSource,
    bond_params: &BondParams,
) -> RawMolecule {
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
        // PBC-aware bond guessing when the structure has a box (finds bonds that
        // cross a box face in a wrapped structure; rendered as dashed half-bonds).
        let pbox = system.state().pbox.clone();
        let bonds = bonds::guess(&all, &positions, &vdw, pbox.as_ref(), bond_params);
        (
            bonds,
            Vec3::new(min.x, min.y, min.z),
            Vec3::new(max.x, max.y, max.z),
            n,
        )
    };

    log::info!("loaded {} atoms, {} bonds from {}", n, bonds.len(), name);

    RawMolecule {
        name,
        source,
        system,
        n_atoms: n,
        bonds,
        bbox_min,
        bbox_max,
    }
}

impl RawMolecule {
    /// Build a fresh in-memory molecule holding a single `atom` at `pos` (nm), for
    /// the drawing tool. molar's `append_atom` underflows on a 0-atom system, so a
    /// drawable molecule must never be empty: the first viewport click *creates* it
    /// here via `System::new`, and every later atom is appended. `mol_name` is the
    /// display name; the source is `Bytes` (no file to reload from).
    pub fn single_atom(mol_name: &str, atom: Atom, pos: Vec3) -> Result<RawMolecule, String> {
        let mut top = Topology::default();
        top.atoms.push(atom);
        top.assign_resindex();
        let st = State {
            coords: vec![Pos::new(pos.x, pos.y, pos.z)],
            ..Default::default()
        };
        let system =
            System::new(top, st).map_err(|e| format!("can't create drawn molecule: {e}"))?;
        Ok(RawMolecule {
            name: mol_name.to_string(),
            source: MoleculeSource::Bytes { name: mol_name.to_string() },
            system,
            n_atoms: 1,
            bonds: Vec::new(),
            bbox_min: pos,
            bbox_max: pos,
        })
    }
}
