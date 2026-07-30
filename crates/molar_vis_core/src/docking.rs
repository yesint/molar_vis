//! Loading a **docking result**: one receptor plus the set of ligand poses docked into it.
//!
//! A docking run produces N ligands and either one receptor structure (rigid docking — the
//! same protein for every pose) or N of them (flexible docking — one receptor conformation
//! per pose, as an ensemble/trajectory). This module holds the part of that with no UI and
//! no IO in it: what the file selection *means*, and whether it is consistent.
//!
//! The viewer then represents it as a [`crate::scene::MolGroup`] of ligands (one shown at a
//! time) with an `Interactions` rep pointed at the receptor, and — for the flexible case —
//! the group's shown member and the receptor's trajectory frame stepping together.

/// How the receptor's frames line up with the ligand poses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockingMode {
    /// One receptor structure shared by every pose (rigid-receptor docking).
    Rigid,
    /// One receptor conformation per pose (flexible-receptor docking): the receptor's
    /// frame `i` belongs with ligand `i`, so cycling either one drives the other.
    Flexible,
}

/// Decide the mode from what was actually loaded, or explain why the two don't go together.
///
/// A static structure reads as a single frame (`n_frames` 0 and 1 are the same thing here —
/// a molecule only grows a trajectory once frames are appended). Anything between the two
/// valid cases is rejected rather than guessed at: a receptor with, say, 5 frames for 26
/// ligands has no defensible interpretation, and silently showing frame 0 for all of them
/// would misrepresent the run.
pub fn docking_mode(receptor_frames: usize, ligands: usize) -> Result<DockingMode, String> {
    if ligands == 0 {
        return Err("no ligands were loaded".into());
    }
    match receptor_frames {
        0 | 1 => Ok(DockingMode::Rigid),
        n if n == ligands => Ok(DockingMode::Flexible),
        n => Err(format!(
            "the protein has {n} frames but there are {ligands} ligands — expected either 1 \
             frame (rigid docking: one receptor for every pose) or {ligands} (flexible \
             docking: one receptor conformation per pose)"
        )),
    }
}

/// Whether the topology file's **own** coordinates are one of the receptor's conformations.
///
/// A protein selection of several files can mean two different things, and they differ by
/// exactly this one frame:
///
/// * **one file per pose** (31 structures for 31 poses) — the first file is the receptor pose 0
///   was docked into, so it counts;
/// * **a structure plus a trajectory holding the whole ensemble** (`jak2.pdb` + a 26-frame
///   trajectory for 26 poses) — the structure is the reference conformation, not a pose, so it
///   does not; counting it would offset every pose/receptor pairing by one.
///
/// Nothing in the file list itself separates the two (a "trajectory" may be a multi-model PDB,
/// and 30 one-model files are a perfectly good ensemble), but **the pose count does**: exactly
/// one of the two readings can match it. So take the one that does, and otherwise count the
/// frame — the natural reading, the one a single file needs, and the one that makes the
/// rejection message report the honest total.
///
/// `appended_frames` is what the files *after* the first contribute.
pub fn structure_frame_counts(appended_frames: usize, ligands: usize) -> bool {
    appended_frames != ligands
}

/// What a flexible-docking reconcile should do this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sync {
    /// Nothing moved.
    Idle,
    /// Show this receptor frame (the pose moved, or this is the first reconcile).
    ShowFrame(usize),
    /// Show this pose (the receptor frame moved).
    ShowPose(usize),
}

/// Decide which side of a flexible-docking pair to drive, from the pair recorded at the
/// last reconcile and the current values.
///
/// Comparing against the last pair is what identifies *which* side the user moved, without
/// having to hook every control that can move either one (the pose cycle bar, the trajectory
/// bar, `Command::Frame`, the playback tick) — so receptor playback cycles the poses for
/// free and no control can be forgotten.
///
/// The pose wins if somehow both moved in one frame: it is the thing the user picks, and the
/// receptor conformation is a property of that choice.
pub fn sync_action(last: Option<(usize, usize)>, pose: usize, frame: usize) -> Sync {
    match last {
        // First reconcile (just loaded, or the pairing only now became valid): align the
        // receptor to the shown pose, not the other way round, so a freshly loaded docking
        // result shows pose 0 with the conformation it was docked into.
        None => Sync::ShowFrame(pose),
        Some((last_pose, last_frame)) => {
            if pose != last_pose {
                Sync::ShowFrame(pose)
            } else if frame != last_frame {
                Sync::ShowPose(frame)
            } else {
                Sync::Idle
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_receptor_frame_is_rigid_docking() {
        // A plain structure carries no trajectory at all until frames are appended.
        assert_eq!(docking_mode(0, 26), Ok(DockingMode::Rigid));
        assert_eq!(docking_mode(1, 26), Ok(DockingMode::Rigid));
        assert_eq!(docking_mode(1, 1), Ok(DockingMode::Rigid));
    }

    #[test]
    fn matching_frame_count_is_flexible_docking() {
        assert_eq!(docking_mode(26, 26), Ok(DockingMode::Flexible));
        // With a single ligand, one frame is rigid — there is nothing to step through.
        assert_eq!(docking_mode(1, 1), Ok(DockingMode::Rigid));
    }

    #[test]
    fn any_other_frame_count_is_an_error() {
        let e = docking_mode(5, 26).unwrap_err();
        assert!(e.contains("5 frames"), "{e}");
        assert!(e.contains("26 ligands"), "{e}");
        // More frames than ligands is just as wrong as fewer.
        assert!(docking_mode(27, 26).is_err());
        assert!(docking_mode(2, 1).is_err());
    }

    #[test]
    fn no_ligands_is_an_error() {
        assert!(docking_mode(1, 0).is_err());
    }

    /// One protein file per pose: the first file is pose 0's receptor, so its own coordinates
    /// are a conformation — dropping them made 31 files come out as 30 frames and the load was
    /// rejected against 31 poses.
    #[test]
    fn one_protein_file_per_pose_counts_the_first_file() {
        // 31 files, 30 of them after the first, 31 poses.
        assert!(structure_frame_counts(30, 31));
        assert_eq!(docking_mode(30 + 1, 31), Ok(DockingMode::Flexible));
    }

    /// A structure plus a trajectory that holds the whole ensemble: the structure is the
    /// reference conformation, not a pose.
    #[test]
    fn a_full_ensemble_trajectory_does_not_count_the_structure() {
        assert!(!structure_frame_counts(26, 26));
        assert_eq!(docking_mode(26, 26), Ok(DockingMode::Flexible));
    }

    /// A single file is its own ensemble — nothing follows it, so its coordinates always count
    /// (a 1-model PDB is rigid docking, a 26-model one flexible).
    #[test]
    fn a_lone_protein_file_always_counts() {
        assert!(structure_frame_counts(0, 26));
        assert!(structure_frame_counts(0, 1));
    }

    /// Neither reading matching is the real mismatch — reported as the natural (counting)
    /// total rather than one less, so the number in the message is the one the files hold.
    #[test]
    fn a_genuine_mismatch_reports_the_counting_total() {
        assert!(structure_frame_counts(4, 26));
        let e = docking_mode(4 + 1, 26).unwrap_err();
        assert!(e.contains("5 frames"), "{e}");
    }

    #[test]
    fn first_reconcile_aligns_the_receptor_to_the_shown_pose() {
        assert_eq!(sync_action(None, 0, 5), Sync::ShowFrame(0));
        assert_eq!(sync_action(None, 3, 0), Sync::ShowFrame(3));
    }

    #[test]
    fn moving_the_pose_drives_the_receptor_frame() {
        assert_eq!(sync_action(Some((0, 0)), 7, 0), Sync::ShowFrame(7));
    }

    #[test]
    fn moving_the_receptor_frame_drives_the_pose() {
        // Includes the playback case: the trajectory advanced on its own.
        assert_eq!(sync_action(Some((0, 0)), 0, 1), Sync::ShowPose(1));
        assert_eq!(sync_action(Some((4, 4)), 4, 9), Sync::ShowPose(9));
    }

    #[test]
    fn nothing_moved_means_idle() {
        assert_eq!(sync_action(Some((3, 3)), 3, 3), Sync::Idle);
    }

    #[test]
    fn the_pose_wins_if_both_moved() {
        // The pose is what the user picks; the conformation follows from it.
        assert_eq!(sync_action(Some((0, 0)), 2, 5), Sync::ShowFrame(2));
    }
}
