//! Saving and loading the **visualization state** (a "session"): which molecules
//! are loaded (by their source file), how each is represented (the full
//! representation document), and the global view (camera, projection, depth cue,
//! picking / axes toggles).
//!
//! ## Extensible without ceremony
//!
//! The per-representation document is serialized through
//! [`crate::history::RepState`] — the *same* struct undo/redo already uses. A
//! field added there for a new feature is therefore snapshotted **and** persisted
//! automatically, with no second place to remember. Global view settings live in
//! one [`ViewState`] struct, captured/applied through the single
//! `App::view_state` / `App::apply_view_state` seam.
//!
//! The file is JSON (human-readable, diffable) and **every field defaults**
//! (`#[serde(default)]`), so a session written by an older or newer build still
//! loads: unknown fields are ignored, missing fields fall back to their defaults.
//! New persisted fields should keep this property (give them a sensible default).
//!
//! Molecules are referenced by their **source path**, not embedded — embedding the
//! atoms is a separate "save molecules to file" feature. Reopening a session
//! reloads the structure (and any trajectories) from disk.
//!
//! This module is pure data + serialization (no filesystem, no `App`), so it
//! compiles for `wasm32` too; the native file IO that drives it lives in `app.rs`.
//! The browser build has no `App` save/load wiring yet (no filesystem to reload
//! molecule sources from), so the module is dead code there — silenced below.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use serde::{Deserialize, Serialize};

use crate::app::Corner;
use crate::camera::Camera;
use crate::history::RepState;
use crate::pick::{PickMode, SelectionMode};
use crate::scene::{Molecule, MoleculeSource, Scene, TrajLoad};

/// On-disk session format version. Additive changes (new defaulting fields) do
/// **not** need a bump; bump only on an incompatible change and migrate in
/// [`Session::from_json`].
pub const VERSION: u32 = 1;

/// Marker stored in the file so an unrelated JSON file is rejected with a clear
/// message rather than silently producing an empty scene.
const FORMAT_TAG: &str = "molar_vis.session";

/// A complete saved visualization state.
#[derive(Serialize, Deserialize)]
pub struct Session {
    /// Format discriminator (see [`FORMAT_TAG`]).
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub view: ViewState,
    /// Standalone (non-grouped) molecules. Group members are saved under [`groups`].
    #[serde(default)]
    pub molecules: Vec<MolSession>,
    /// Molecular groups (multi-molecule SDF), reloaded from their source file by
    /// record index. `#[serde(default)]` so older sessions (no groups) still load.
    #[serde(default)]
    pub groups: Vec<GroupSession>,
}

/// One molecular group's saved state: where its records came from, the shared
/// representation document, and each member's own reps + record index.
#[derive(Serialize, Deserialize)]
pub struct GroupSession {
    /// The multi-molecule file to re-open (members are picked out by record index).
    pub source: MoleculeSource,
    #[serde(default)]
    pub name: String,
    /// Shown member after load (index into `members`).
    #[serde(default)]
    pub current: usize,
    #[serde(default = "default_true")]
    pub visible: bool,
    /// The shared representation document (applies to every member).
    #[serde(default)]
    pub shared_reps: Vec<RepState>,
    #[serde(default)]
    pub members: Vec<MemberSession>,
}

/// One member of a saved group: which `$$$$` record it is, plus its own
/// (non-shared) representations. Member visibility is derived from the group's
/// `current`, so it isn't stored.
#[derive(Serialize, Deserialize)]
pub struct MemberSession {
    /// 0-based record index in the group's source file (the load order).
    #[serde(default)]
    pub record_index: usize,
    #[serde(default)]
    pub name: String,
    /// This member's own reps only (the shared ones live on the group).
    #[serde(default)]
    pub reps: Vec<RepState>,
}

/// One molecule's saved state: where it came from, the full representation
/// document, and the per-molecule view bits (visibility, box, trajectory).
#[derive(Serialize, Deserialize)]
pub struct MolSession {
    pub source: MoleculeSource,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default)]
    pub show_box: bool,
    /// The representation document — reused verbatim from undo/redo, so new rep
    /// features serialize for free (see [`RepState`]).
    #[serde(default)]
    pub reps: Vec<RepState>,
    /// Trajectory files to replay, in load order.
    #[serde(default)]
    pub traj_loads: Vec<TrajLoad>,
    /// The displayed frame after the trajectories are reloaded.
    #[serde(default)]
    pub current_frame: usize,
}

/// Global, non-undoable view state persisted with a session.
///
/// **Add new persisted global settings here**, then read/write them in
/// `App::view_state` / `App::apply_view_state` — those two functions are the only
/// manual plumbing the save/load framework needs.
#[derive(Serialize, Deserialize, Default)]
pub struct ViewState {
    /// The full camera pose (target, orientation, zoom, projection, depth cue).
    /// `None` keeps the current camera (e.g. a hand-written session that omits it).
    #[serde(default)]
    pub camera: Option<Camera>,
    #[serde(default)]
    pub pick_mode: PickMode,
    #[serde(default)]
    pub selection_mode: SelectionMode,
    #[serde(default)]
    pub axes_on: bool,
    #[serde(default)]
    pub axes_corner: Corner,
}

impl MolSession {
    /// Capture a live molecule's persistable state.
    pub fn capture(mol: &Molecule) -> Self {
        Self {
            source: mol.source.clone(),
            name: mol.name.clone(),
            visible: mol.visible,
            show_box: mol.show_box,
            reps: mol.reps.iter().map(RepState::capture).collect(),
            traj_loads: mol.traj_loads.clone(),
            current_frame: mol.trajectory.current,
        }
    }

    /// Build the saved representations as fresh, dirty [`crate::scene::Representation`]s.
    /// Empty `reps` (a hand-written session) falls back to a single default rep so
    /// the molecule isn't invisible.
    pub fn build_reps(&self, default_rep: crate::geometry::RepKind) -> Vec<crate::scene::Representation> {
        if self.reps.is_empty() {
            vec![crate::scene::Representation::new(default_rep)]
        } else {
            self.reps.iter().map(RepState::to_representation).collect()
        }
    }
}

impl GroupSession {
    /// Capture group `gi` of `scene` (shared reps from the shown member's prefix,
    /// each member's own reps + record index).
    pub fn capture(scene: &Scene, gi: usize) -> Self {
        let g = &scene.groups[gi];
        let members = g
            .members
            .iter()
            .enumerate()
            .filter_map(|(pos, &id)| {
                let mi = scene.mol_index(id)?;
                let m = &scene.molecules[mi];
                let record_index = match &m.source {
                    MoleculeSource::SdfRecord { index, .. } => *index,
                    _ => pos,
                };
                let ns = m.n_shared.min(m.reps.len());
                Some(MemberSession {
                    record_index,
                    name: m.name.clone(),
                    reps: m.reps[ns..].iter().map(RepState::capture).collect(),
                })
            })
            .collect();
        GroupSession {
            source: g.source.clone(),
            name: g.name.clone(),
            current: g.current,
            visible: g.visible,
            shared_reps: scene.group_shared_reps(gi),
            members,
        }
    }
}

impl Session {
    /// Capture the document side (the molecules) from a scene; the caller supplies
    /// the global [`ViewState`] (it lives on `App`, not the scene).
    pub fn capture(scene: &Scene, view: ViewState) -> Self {
        Session {
            format: FORMAT_TAG.to_string(),
            version: VERSION,
            view,
            // Standalone molecules only; group members are saved under `groups`.
            molecules: scene
                .molecules
                .iter()
                .filter(|m| m.group.is_none())
                .map(MolSession::capture)
                .collect(),
            groups: (0..scene.groups.len()).map(|gi| GroupSession::capture(scene, gi)).collect(),
        }
    }

    /// Serialize to pretty JSON.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| format!("failed to serialize session: {e}"))
    }

    /// Parse a session from JSON, validating the format marker.
    pub fn from_json(s: &str) -> Result<Session, String> {
        let session: Session =
            serde_json::from_str(s).map_err(|e| format!("not a valid session file: {e}"))?;
        // An empty marker is tolerated (hand-written / very old files); a wrong
        // non-empty marker means this isn't a molar_vis session.
        if !session.format.is_empty() && session.format != FORMAT_TAG {
            return Err(format!(
                "not a molar_vis session (format = \"{}\")",
                session.format
            ));
        }
        if session.version > VERSION {
            log::warn!(
                "session was written by a newer build (v{} > v{VERSION}); loading best-effort",
                session.version
            );
        }
        Ok(session)
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::ColorMethod;
    use crate::geometry::RepKind;
    use crate::scene::Representation;
    use std::path::Path;

    fn scene_2lao() -> Scene {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb");
        let raw = crate::data::load(Path::new(path)).expect("load 2lao.pdb");
        let mut scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        scene
    }

    /// Capture → JSON → parse preserves the molecule source and the full rep
    /// document (the round-trip the save/load feature relies on).
    #[test]
    fn session_round_trips_reps_and_source() {
        let mut scene = scene_2lao();
        // A second rep with non-default style/color/selection.
        let mut rep = Representation::new(RepKind::Cartoon);
        rep.color = ColorMethod::SecStruct;
        rep.sel_text = "protein".to_string();
        rep.visible = false;
        scene.molecules[0].reps.push(rep);

        let session = Session::capture(&scene, ViewState::default());
        let json = session.to_json().unwrap();
        let back = Session::from_json(&json).unwrap();

        assert_eq!(back.format, FORMAT_TAG);
        assert_eq!(back.version, VERSION);
        assert_eq!(back.molecules.len(), 1);
        let m = &back.molecules[0];
        assert!(matches!(&m.source, MoleculeSource::File(p) if p.ends_with("2lao.pdb")));
        assert_eq!(m.reps.len(), 2);
        assert_eq!(m.reps[0].kind, RepKind::Lines);
        assert_eq!(m.reps[1].kind, RepKind::Cartoon);
        assert_eq!(m.reps[1].color, ColorMethod::SecStruct);
        assert_eq!(m.reps[1].sel_text, "protein");
        assert!(!m.reps[1].visible);

        // `build_reps` reconstructs live representations from the document.
        let reps = m.build_reps(RepKind::Lines);
        assert_eq!(reps.len(), 2);
        assert_eq!(reps[1].kind, RepKind::Cartoon);
    }

    /// The global view state (camera incl. glam quaternion/vector fields)
    /// survives the JSON round-trip.
    #[test]
    fn session_round_trips_camera() {
        let scene = scene_2lao();
        let mut view = ViewState::default();
        let mut cam = scene
            .bbox()
            .map(|(lo, hi)| crate::camera::Camera::frame_bbox(lo, hi, 0.9))
            .unwrap();
        cam.orbit(40.0, 15.0, 1.0);
        view.camera = Some(cam);
        view.pick_mode = PickMode::Lasso;

        let json = Session::capture(&scene, view).to_json().unwrap();
        let back = Session::from_json(&json).unwrap();
        let restored = back.view.camera.expect("camera persisted");
        assert!((restored.distance - cam.distance).abs() < 1e-6);
        assert!((restored.orientation.dot(cam.orientation).abs() - 1.0).abs() < 1e-6);
        assert_eq!(back.view.pick_mode, PickMode::Lasso);
    }

    /// Forward/back-compat: a minimal hand-written session (only a source) loads,
    /// with every other field defaulting — the property that lets new fields be
    /// added without breaking old files.
    #[test]
    fn minimal_session_fills_defaults() {
        let json = r#"{ "molecules": [ { "source": { "File": "/tmp/x.pdb" } } ] }"#;
        let s = Session::from_json(json).unwrap();
        assert_eq!(s.molecules.len(), 1);
        let m = &s.molecules[0];
        assert!(m.visible, "visible defaults to true");
        assert!(!m.show_box);
        assert!(m.reps.is_empty());
        assert_eq!(m.current_frame, 0);
        // Empty reps fall back to one default rep so the molecule renders.
        assert_eq!(m.build_reps(RepKind::Lines).len(), 1);
    }

    /// A JSON that isn't a molar_vis session is rejected with a clear error.
    #[test]
    fn wrong_format_is_rejected() {
        let json = r#"{ "format": "something.else", "molecules": [] }"#;
        assert!(Session::from_json(json).is_err());
    }

    /// A molecular group round-trips: members are saved under `groups` (not
    /// `molecules`), with the shared rep document, member record indices, and the
    /// shown-member index preserved.
    #[test]
    fn session_round_trips_group() {
        use crate::scene::{MolGroup, MoleculeSource};
        let sdf = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/ligands20.sdf");
        let records =
            crate::data::load_records(Path::new(sdf), &crate::data::BondParams::default())
                .expect("load ligands20.sdf");
        assert_eq!(records.len(), 20, "20 records expected");

        // Build a group the way `App::add_group` does (hidden members, one shared
        // Licorice rep on the shown member).
        let mut scene = Scene::default();
        let gid = scene.alloc_group_id();
        let mut members = Vec::new();
        for raw in records {
            let id = scene.add(raw, &crate::settings::RepDefaults::default());
            let mi = scene.mol_index(id).unwrap();
            let mol = &mut scene.molecules[mi];
            mol.reps.clear();
            mol.group = Some(gid);
            mol.visible = false;
            mol.n_shared = 0;
            members.push(id);
        }
        // The shared prefix lives on the shown member (here member 3).
        let shown = 3;
        let mi = scene.mol_index(members[shown]).unwrap();
        scene.molecules[mi].reps = vec![Representation::new(RepKind::Licorice)];
        scene.molecules[mi].n_shared = 1;
        scene.groups.push(MolGroup {
            id: gid,
            name: "ligands20.sdf".to_string(),
            source: MoleculeSource::File(Path::new(sdf).to_path_buf()),
            members,
            current: shown,
            visible: true,
            expanded: false,
        });
        scene.apply_group_visibility(0);

        let json = Session::capture(&scene, ViewState::default()).to_json().unwrap();
        let back = Session::from_json(&json).unwrap();

        assert!(back.molecules.is_empty(), "members are saved under groups, not molecules");
        assert_eq!(back.groups.len(), 1);
        let g = &back.groups[0];
        assert_eq!(g.members.len(), 20);
        assert_eq!(g.current, 3);
        assert!(g.visible);
        assert_eq!(g.shared_reps.len(), 1);
        assert_eq!(g.shared_reps[0].kind, RepKind::Licorice);
        // Member record indices are the load order; own reps are empty.
        assert_eq!(g.members[5].record_index, 5);
        assert!(g.members[5].reps.is_empty());
    }
}
