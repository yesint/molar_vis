//! The **Load docking data…** dialog and its load path.
//!
//! Loading a docking result by hand is a fiddly sequence — open the receptor, append its
//! ensemble as a trajectory, open the pose file as a group, add an `Interactions` rep, aim
//! it at the receptor with the partner picker — and every step is easy to get subtly wrong.
//! This does the whole thing from two file choices, and refuses the combinations that don't
//! describe a docking run (see [`crate::docking::docking_mode`]).
//!
//! Native only: it reads several files from disk and the browser build has no filesystem.

use super::*;
use super::widgets::{modal_shell, ModalBody, ModalSpec};
use crate::docking::{docking_mode, structure_frame_counts, sync_action, DockingMode, Sync};

/// State of the "Load docking data" modal.
///
/// Both selections are `Vec<PathBuf>` because both are legitimately multi-file:
///
/// * **Ligands** — either one multi-record SDF (each `$$$$` record a pose) or one file per
///   pose. Either way the poses become the members of one [`MolGroup`].
/// * **Protein** — one structure; or several files with the first as the structure and the
///   rest as trajectory frames; or a structure plus a trajectory file. These are the same
///   thing to the loader, so they need no separate controls: the first file always provides
///   the topology and everything after it contributes frames (the command line's
///   `-m a.pdb a.xtc` grouping, see `launch::parse_file_args`).
pub(super) struct DockingDialog {
    pub(super) protein: Vec<std::path::PathBuf>,
    pub(super) ligands: Vec<std::path::PathBuf>,
}

impl DockingDialog {
    pub(super) fn new() -> Self {
        Self { protein: Vec::new(), ligands: Vec::new() }
    }
}

/// Default selection for a docking view: everything but the apolar hydrogens.
///
/// Docked structures usually come fully protonated, and the C–H hydrogens are pure noise —
/// they hide the pose in a haze and contribute to nothing the view is for. The **polar** ones
/// are kept: they are what H-bonds are made of, and the `Interactions` detector uses them for
/// the explicit-H geometry test rather than falling back to the heavy-atom criterion. A
/// structure with no hydrogens at all matches everything, so this is safe either way.
const HEAVY_ATOMS: &str = "not apolh";

/// Margin (nm) added around a pose when framing it, so the binding site comes with it.
///
/// Sized to the interaction cutoffs (H-bond ~0.35 nm, hydrophobic ~0.4 nm, salt bridge
/// ~0.55 nm, π-cation ~0.6 nm), so every residue the `Interactions` rep can draw a line to is
/// on screen.
const POSE_VIEW_MARGIN: f32 = 0.6;

/// Line width (px) for the receptor's lines rep. Wider than the 1 px default because the
/// receptor here is a backdrop being read *through* — at 1 px it disappears against the pose.
const DOCKING_LINE_WIDTH: f32 = 3.0;

/// One line of the file summary: "3 files: jak2.pdb, jak2_traj.pdb, …" or the single name.
fn describe(paths: &[std::path::PathBuf]) -> String {
    let name = |p: &std::path::PathBuf| {
        p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    };
    match paths {
        [] => String::new(),
        [one] => name(one),
        many => format!("{} files: {}", many.len(), name(&many[0])),
    }
}

impl App {
    /// Render the "Load docking data" modal: a receptor chooser, a ligand chooser, and
    /// Load/Cancel. Mirrors `draw_load_dialog`'s shape (a centered `egui::Modal`, errors
    /// shown in place so a rejected combination can be corrected without reopening).
    pub(super) fn draw_docking_dialog(&mut self, ctx: &egui::Context) {
        modal_shell(
            self,
            ctx,
            |a| &mut a.docking_dialog,
            ModalSpec {
                id: "docking_modal",
                width: 420.0,
                heading: "Load docking data",
                commit: "Load",
            },
            |ui, dialog, err, _app| {
            ui.label(
                egui::RichText::new(
                    "A receptor plus the ligand poses docked into it. One receptor frame is \
                     rigid docking; one frame per pose is flexible docking.",
                )
                .weak(),
            );
            ui.separator();

            egui::Grid::new("docking_files")
                .num_columns(3)
                .spacing(egui::vec2(8.0, 6.0))
                .show(ui, |ui| {
                    ui.label("Protein");
                    if ui
                        .button(format!("{}  Choose…", icon::FOLDER_OPEN))
                        .on_hover_text(
                            "A structure; or a structure + trajectory; or several files, the \
                             first as the structure and the rest as frames (multi-select)",
                        )
                        .clicked()
                    {
                        if let Some(mut ps) = rfd::FileDialog::new()
                            .add_filter(
                                "Structures & trajectories",
                                &["pdb", "ent", "gro", "xyz", "tpr", "xtc", "trr", "dcd", "nc", "ncdf"],
                            )
                            .pick_files()
                        {
                            // rfd hands back selection order, which is unspecified across
                            // platforms; sort so "the first file is the structure" is
                            // predictable (jak2.pdb before jak2_traj.pdb).
                            ps.sort();
                            dialog.protein = ps;
                            *err = None;
                        }
                    }
                    match dialog.protein.is_empty() {
                        true => {
                            ui.weak("no file selected");
                        }
                        false => {
                            ui.monospace(describe(&dialog.protein))
                                .on_hover_text(dialog.protein.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n"));
                        }
                    }
                    ui.end_row();

                    ui.label("Ligands");
                    if ui
                        .button(format!("{}  Choose…", icon::FOLDER_OPEN))
                        .on_hover_text(
                            "One multi-molecule SDF, or one file per pose (multi-select)",
                        )
                        .clicked()
                    {
                        if let Some(mut ps) = rfd::FileDialog::new()
                            .add_filter("Ligands", &["sdf", "sd", "mol", "mol2", "pdb", "xyz"])
                            .pick_files()
                        {
                            ps.sort();
                            dialog.ligands = ps;
                            *err = None;
                        }
                    }
                    match dialog.ligands.is_empty() {
                        true => {
                            ui.weak("no file selected");
                        }
                        false => {
                            ui.monospace(describe(&dialog.ligands))
                                .on_hover_text(dialog.ligands.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n"));
                        }
                    }
                    ui.end_row();
                });

            ModalBody::enabled(!dialog.protein.is_empty() && !dialog.ligands.is_empty())
            },
            // A rejected combination reopens the dialog with the reason (the shell's fourth
            // state), so the selection can be fixed without starting over.
            |app, dialog| match app.load_docking(&dialog.protein, &dialog.ligands) {
                Ok(msg) => {
                    app.status = msg;
                    Ok(())
                }
                Err(e) => {
                    log::error!("docking load: {e}");
                    Err(e)
                }
            },
        );
    }

    /// Load a docking result: the receptor (+ its ensemble as trajectory frames) and the
    /// ligand poses as a [`MolGroup`], with an `Interactions` rep on the group aimed at the
    /// receptor. Returns the status line, or the reason it isn't a docking result.
    ///
    /// Validated **before** anything is added to the scene, so a rejected combination
    /// leaves the document untouched rather than half-loaded.
    pub(super) fn load_docking(
        &mut self,
        protein: &[std::path::PathBuf],
        ligands: &[std::path::PathBuf],
    ) -> Result<String, String> {
        let bonds = self.settings.behavior.bond_params();

        // --- ligands: one multi-record file, or one file per pose --------------------
        let mut poses: Vec<data::RawMolecule> = Vec::new();
        for path in ligands {
            let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
            // A record-structured file may hold many poses; anything else is one.
            if matches!(ext.as_deref(), Some("sdf") | Some("sd") | Some("mol")) {
                poses.extend(data::load_records(path, &bonds)?);
            } else {
                poses.push(data::load_with(path, &bonds)?);
            }
        }
        if poses.is_empty() {
            return Err("no ligand poses were loaded".into());
        }

        // --- receptor: the first file is the topology, the rest are its ensemble -----
        //
        // The **first file supplies the topology**; the receptor's conformations are its own
        // coordinates plus the frames of every file after it — except when those files already
        // hold one conformation per pose, in which case the structure is the reference
        // conformation rather than a pose. Which of the two a file list means is decided by
        // [`structure_frame_counts`] (from the pose count, the only thing that can tell them
        // apart); with nothing after the first file it is always its own ensemble, so a
        // one-model PDB is rigid docking and a 26-model one flexible.
        let (structure, extra) = protein.split_first().ok_or("no protein file selected")?;
        let receptor = data::load_with(structure, &bonds)?;
        let n_atoms = receptor.n_atoms;
        // Read the frames *before* anything is added to the scene, so a count mismatch
        // aborts with the document untouched.
        let mut frames: Vec<(std::path::PathBuf, usize, Vec<State>)> = Vec::new();
        for path in extra {
            let opts = LoadOptions { from: 0, to: None, stride: 1 };
            let read = data::traj_loader::read_frames_sync(path, &opts, n_atoms)?;
            if !read.is_empty() {
                frames.push((path.clone(), 0, read));
            }
        }
        let appended: usize = frames.iter().map(|(_, _, s)| s.len()).sum();
        let count_structure = structure_frame_counts(appended, poses.len());
        // Nothing after the first file: fall back to that file's own extra models (read from
        // frame 1, since frame 0 is the structure we already loaded) — seeded as frame 0 below.
        if frames.is_empty() {
            let opts = LoadOptions { from: 1, to: None, stride: 1 };
            let own = data::traj_loader::read_frames_sync(structure, &opts, n_atoms)?;
            if !own.is_empty() {
                frames.push((structure.clone(), 1, own));
            }
        }
        let receptor_frames =
            frames.iter().map(|(_, _, s)| s.len()).sum::<usize>() + count_structure as usize;
        let mode = docking_mode(receptor_frames, poses.len())?;

        // --- commit: receptor, then the pose group, then the link ------------------
        let mut receptor = receptor;
        receptor.source = MoleculeSource::File(structure.clone());
        self.add_loaded(receptor);
        let protein_mi = self.scene.molecules.len() - 1;
        let protein_src = self.scene.molecules[protein_mi].source.clone();
        self.scene.style_receptor(protein_mi);
        {
            let mol = &mut self.scene.molecules[protein_mi];
            // Frame 0 is the structure's own coordinates whenever they count as a conformation
            // (one file per pose, or a lone multi-model file); when the later files already
            // hold the whole ensemble, the frames are exactly theirs. Skipped with nothing to
            // append, so rigid docking keeps a plain static receptor and no trajectory bar.
            if count_structure && !frames.is_empty() {
                mol.seed_frame0();
            }
            for (path, from, states) in frames {
                mol.append_frames(states);
                mol.traj_loads.push(crate::scene::TrajLoad { path, from, to: None, stride: 1 });
            }
            mol.apply_current_frame();
        }

        let n_poses = poses.len();
        let group_name = ligands
            .first()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "ligands".to_string());
        let group_source = match ligands {
            // One record-structured file: the group reloads from it by record index.
            [one] => MoleculeSource::File(one.clone()),
            // Several files: there is no single file to reload the group from, so the
            // members keep their own per-file sources (set by the loader) and the group
            // is byte-sourced — the same limitation a browser-loaded group has.
            _ => MoleculeSource::Bytes { name: group_name.clone() },
        };
        self.add_group(poses, group_source, group_name);
        let Some(gi) = self.scene.groups.len().checked_sub(1) else {
            return Err("failed to create the ligand group".into());
        };

        // The Interactions rep goes on the *group* (a shared rep, so it survives member
        // switching and applies to whichever pose is shown) and points at the receptor's
        // first rep — exactly the state the partner picker would leave behind.
        self.scene.add_docking_interactions(gi, protein_src, 0);
        self.scene.style_poses(gi);
        // Flexible docking: line the receptor's frame up with the shown pose from the
        // start, and let the two step together from then on (`sync_docking_frames`).
        if mode == DockingMode::Flexible {
            self.scene.groups[gi].docking_sync = None; // force the first reconcile
        }
        // Frame the **ligand**, not the whole receptor: the pose and its contacts are what
        // you opened a docking result to look at, and a 5000-atom receptor fitted to the
        // viewport leaves the pose a few pixels across. `add_loaded` framed the receptor
        // because it was the first molecule into an empty scene, so this overrides it.
        self.focus_shown_pose(gi);
        self.view_dirty = true;
        Ok(match mode {
            DockingMode::Rigid => format!("Loaded {n_poses} ligand poses (rigid receptor)"),
            DockingMode::Flexible => {
                format!("Loaded {n_poses} ligand poses (flexible receptor, {receptor_frames} frames)")
            }
        })
    }

    /// Zoom to the currently shown pose **plus its contact shell**.
    ///
    /// Fitting a ~1 nm ligand to the viewport on its own is uselessly tight — the pose fills
    /// the screen with none of the site it sits in, and the interaction lines run off the
    /// edge to partners you cannot see. Padding by [`POSE_VIEW_MARGIN`] brings in exactly the
    /// residues those lines reach.
    fn focus_shown_pose(&mut self, gi: usize) {
        let Some(&member_id) = self.scene.groups.get(gi).and_then(|g| g.members.get(g.current))
        else {
            return;
        };
        if let Some(mi) = self.scene.mol_index(member_id) {
            let (min, max) = self.scene.molecules[mi].current_bbox();
            let pad = glam::Vec3::splat(POSE_VIEW_MARGIN);
            self.camera.focus_bbox(min - pad, max + pad);
        }
    }

}

/// The scene-only half of loading a docking result: styling the two sides and wiring the
/// pose group's `Interactions` rep to the receptor. These read and write nothing but the
/// scene, so they sit on [`Scene`].
impl Scene {
    /// The receptor's docking view: **lines** over the heavy atoms (wider than the default,
    /// since it is a backdrop being read *through*) plus a **cartoon coloured by secondary
    /// structure**, which is what makes a binding site legible as part of a fold.
    pub(super) fn style_receptor(&mut self, mi: usize) {
        let mol = &mut self.molecules[mi];
        if let Some(rep) = mol.reps.first_mut() {
            rep.kind = RepKind::Lines;
            rep.params = RepParams::Lines { width: DOCKING_LINE_WIDTH };
            rep.sel_text = HEAVY_ATOMS.to_string();
            rep.sel_dirty = true;
        }
        let mut cartoon = Representation::new(RepKind::Cartoon);
        cartoon.color = ColorMethod::SecStruct;
        cartoon.sel_text = HEAVY_ATOMS.to_string();
        mol.reps.push(cartoon);
        mol.selected_rep = Some(0);
    }

    /// The poses' docking view: the group's shared reps over the heavy atoms only.
    pub(super) fn style_poses(&mut self, gi: usize) {
        let Some(&member_id) = self.groups.get(gi).and_then(|g| g.members.get(g.current))
        else {
            return;
        };
        let Some(mi) = self.mol_index(member_id) else { return };
        let mol = &mut self.molecules[mi];
        let n_shared = mol.n_shared.min(mol.reps.len());
        for rep in &mut mol.reps[..n_shared] {
            // The Interactions rep detects contacts *from* this selection, so it wants the
            // same heavy-atom scope as the geometry reps.
            rep.sel_text = HEAVY_ATOMS.to_string();
            rep.sel_dirty = true;
        }
    }

    /// Give the ligand group an `Interactions` shared rep whose partner is the receptor's
    /// rep `protein_rep`, as if it had been assigned with the partner picker.
    ///
    /// The group's existing shared rep (Licorice, from `add_group`) is kept — an
    /// `Interactions` rep draws only contact lines, so on its own the poses would vanish.
    /// That is the same reasoning as the style picker's clone-on-switch.
    pub(super) fn add_docking_interactions(
        &mut self,
        gi: usize,
        protein_src: MoleculeSource,
        protein_rep: usize,
    ) {
        let Some(&member_id) = self.groups.get(gi).and_then(|g| g.members.get(g.current))
        else {
            return;
        };
        let Some(mi) = self.mol_index(member_id) else { return };
        let mut rep = Representation::new(RepKind::Interactions);
        rep.partner = Some((protein_src, protein_rep));
        let mol = &mut self.molecules[mi];
        // Appended to the shared prefix, so it is shared across members like the Licorice.
        let at = mol.n_shared.min(mol.reps.len());
        mol.reps.insert(at, rep);
        mol.n_shared = at + 1;
        mol.selected_rep = Some(at);
    }

    /// The receptor molecule of a flexible-docking pair: the molecule targeted by the
    /// group's `Interactions` rep. `None` when the group has no such rep, its partner is
    /// unresolvable, or the partner is a member of this very group.
    pub(super) fn docking_receptor(&self, gi: usize) -> Option<usize> {
        let g = self.groups.get(gi)?;
        let mi = self.mol_index(*g.members.get(g.current)?)?;
        let mol = &self.molecules[mi];
        let n_shared = mol.n_shared.min(mol.reps.len());
        for rep in &mol.reps[..n_shared] {
            if !matches!(rep.kind, RepKind::Interactions) {
                continue;
            }
            if let Some((pmi, _)) = super::build::partner_index(self, rep) {
                // A partner inside this group would make the coupling self-referential.
                if self.molecules[pmi].group != Some(g.id) {
                    return Some(pmi);
                }
            }
        }
        None
    }

    /// Show receptor frame `frame`, going through the molecule's own frame-change path so
    /// the geometry/coords dirty flags are set exactly as the trajectory bar would. Returns
    /// whether the frame moved (the caller flags the re-render).
    pub(super) fn set_receptor_frame(&mut self, pmi: usize, frame: usize) -> bool {
        let mol = &mut self.molecules[pmi];
        if mol.trajectory.current == frame || frame >= mol.trajectory.n_frames() {
            return false;
        }
        mol.trajectory.set_current(frame);
        mol.apply_current_frame();
        true
    }
}

impl App {
    /// Keep a flexible-docking group and its receptor in step: moving to another pose shows
    /// the receptor conformation it was docked into, and scrubbing (or playing) the receptor
    /// trajectory shows the matching pose.
    ///
    /// Applies to any group whose `Interactions` partner resolves to a molecule with exactly
    /// one frame per member — which is what makes them a flexible-docking pair, whether they
    /// came from the docking dialog or were wired up by hand. Rigid docking (one receptor
    /// frame) has nothing to step, and a mismatched count is left alone.
    ///
    /// Called once per frame after the panels have drawn. Rather than hooking every control
    /// that can move either side (the pose cycle bar, the trajectory bar, `Command::Frame`,
    /// the playback tick), it compares both values against the pair recorded last frame and
    /// propagates whichever moved — so playback works for free, and there is no risk of a
    /// control being missed. The member takes precedence if somehow both moved at once.
    pub(super) fn sync_docking_frames(&mut self) {
        for gi in 0..self.scene.groups.len() {
            let Some(pmi) = self.scene.docking_receptor(gi) else {
                self.scene.groups[gi].docking_sync = None; // not (or no longer) a pair
                continue;
            };
            let members = self.scene.groups[gi].members.len();
            if self.scene.molecules[pmi].trajectory.n_frames() != members {
                self.scene.groups[gi].docking_sync = None;
                continue;
            }
            let member = self.scene.groups[gi].current;
            let frame = self.scene.molecules[pmi].trajectory.current;
            match sync_action(self.scene.groups[gi].docking_sync, member, frame) {
                Sync::ShowFrame(f) => {
                    if self.scene.set_receptor_frame(pmi, f) {
                        self.view_dirty = true;
                    }
                }
                Sync::ShowPose(p) => self.switch_group_member_synced(gi, p),
                Sync::Idle => {}
            }
            // Re-read: either branch may have moved one of them.
            let member = self.scene.groups[gi].current;
            let frame = self.scene.molecules[pmi].trajectory.current;
            self.scene.groups[gi].docking_sync = Some((member, frame));
        }
    }

    /// Show pose `member`, re-centering the camera on it exactly as the cycle bar does
    /// (partial focus: pan the target, keep the zoom).
    fn switch_group_member_synced(&mut self, gi: usize, member: usize) {
        if !self.scene.switch_group_member(gi, member) {
            return;
        }
        if let Some(&id) = self.scene.groups[gi].members.get(member) {
            if let Some(mi) = self.scene.mol_index(id) {
                let (min, max) = self.scene.molecules[mi].current_bbox();
                self.camera.target = 0.5 * (min + max);
            }
        }
        self.view_dirty = true;
    }
}
