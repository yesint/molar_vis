//! The **Analysis ▸ Align…** dialog: superpose one selection onto another, and report RMSD.
//!
//! A **non-modal** `egui::Window`, unlike the app's other dialogs, for two reasons that both
//! matter here: each side can be filled by *clicking a representation* in the tree or in the
//! 3-D view, which a modal's backdrop would swallow; and the window grows when the RMSD
//! readout appears, which a centred `Modal` would answer by jumping its top edge (the same
//! reason the settings window is a `Window` — see `draw_settings_dialog`).

use super::*;
use super::rep_panel::spin_u32;
use super::widgets::{bold_name, max_label_width};
use crate::analysis;

/// Which side of the dialog a picked representation fills.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AlignSide {
    Source,
    Target,
}

/// One side's inputs: a molecule, a selection in it, and a frame.
///
/// The molecule is held as a [`MolId`], not an index, so a molecule deleted while the dialog
/// sits open is reported as gone instead of silently becoming its neighbour.
pub(super) struct SideState {
    pub(super) mol: Option<MolId>,
    pub(super) sel: String,
    pub(super) frame: u32,
}

impl SideState {
    fn new(mol: Option<MolId>, frame: u32) -> Self {
        Self { mol, sel: DEFAULT_SEL.to_string(), frame }
    }
}

/// Starting selection for both sides. Neutral on purpose: what to fit on is the one thing
/// nobody can guess (backbone? one chain? a ligand?), and `all` at least always resolves.
const DEFAULT_SEL: &str = "all";

/// The molecule a freshly opened dialog points at: the one last worked with — mapped to its
/// group's **shown** member when it belongs to a group, since a group's other members aren't
/// even on screen and pointing the dialog at one of them would be a surprise.
fn default_mol(scene: &Scene) -> Option<usize> {
    let mi = scene.selected_mol.filter(|&i| i < scene.molecules.len()).unwrap_or(0);
    let gid = scene.molecules.get(mi)?.group;
    let Some(gid) = gid else { return Some(mi) };
    let g = scene.groups.iter().find(|g| g.id == gid)?;
    g.members.get(g.current).and_then(|&id| scene.mol_index(id)).or(Some(mi))
}

/// Width of the label column, so both sides' controls line up under each other.
const LABEL_W: f32 = 58.0;

/// Width of the selection field. **Fixed**: a selection is the longest thing typed here, and it
/// used to be whatever was left over after the molecule dropdown — so choosing a group member
/// (whose label carries its group's name) squeezed it down to a few characters. Now the row keeps
/// its width and the *window* grows instead (see `set_min_width` in [`App::draw_align_dialog`]).
const SEL_FIELD_W: f32 = 240.0;

/// Minimum window width. The window auto-sizes to its content above this, so a long molecule
/// name widens the dialog rather than eating the field next to it.
const MIN_WIDTH: f32 = 520.0;

/// State of the alignment dialog.
pub(super) struct AlignDialog {
    pub(super) source: SideState,
    pub(super) target: SideState,
    /// Target = the source's molecule + selection, at the frame in the *source's* frame box.
    pub(super) same_as_source: bool,
    /// Fit every frame of the source molecule, not just one.
    pub(super) all_frames: bool,
    pub(super) common_subset: bool,
    /// Move the whole source molecule rather than only the atoms its selection matched.
    pub(super) move_whole: bool,
    /// Last result, shown until an input changes (a stale number must not read as current).
    pub(super) rmsd: Option<analysis::Rmsd>,
    pub(super) error: Option<String>,
}

impl AlignDialog {
    /// Open on the currently selected molecule, at its displayed frame — the molecule the
    /// user was last working with is the one they mean far more often than molecule 0.
    pub(super) fn new(scene: &Scene) -> Self {
        let (id, frame) = match default_mol(scene).and_then(|mi| scene.molecules.get(mi)) {
            Some(m) => (Some(m.id), m.trajectory.current as u32),
            None => (None, 0),
        };
        Self {
            source: SideState::new(id, frame),
            target: SideState::new(id, frame),
            same_as_source: false,
            all_frames: false,
            // Off: atom for atom is the honest comparison, and it *tells you* when the counts
            // don't match. Name-pairing guesses, so it is something to turn on deliberately
            // (and it can mispair a systematic difference — see [`analysis`]'s module docs).
            common_subset: false,
            move_whole: false,
            rmsd: None,
            error: None,
        }
    }

    /// Drop the last result. Called whenever an input changes: the readout is a measurement
    /// of a specific pair of selections, so it stops being true the moment either changes.
    fn invalidate(&mut self) {
        self.rmsd = None;
        self.error = None;
    }

    fn side(&mut self, side: AlignSide) -> &mut SideState {
        match side {
            AlignSide::Source => &mut self.source,
            AlignSide::Target => &mut self.target,
        }
    }
}

impl App {
    /// Fill one side of the dialog from a representation the user picked in the tree or the
    /// 3-D view: its molecule, its selection text, and that molecule's displayed frame.
    ///
    /// The reason the "Existing rep" *type* the first sketch of this dialog had is gone: a rep
    /// is nothing but a molecule + a selection, so picking one just writes those in, and the
    /// text stays editable afterwards.
    pub(super) fn align_take_rep(&mut self, side: AlignSide, mi: usize, rep: usize) {
        let Some(mol) = self.scene.molecules.get(mi) else { return };
        let (id, frame) = (mol.id, mol.trajectory.current as u32);
        let sel = mol.reps.get(rep).map(|r| r.sel_text.clone());
        if let Some(dialog) = self.align_dialog.as_mut() {
            let s = dialog.side(side);
            s.mol = Some(id);
            s.frame = frame;
            if let Some(sel) = sel {
                s.sel = sel;
            }
            dialog.invalidate();
        }
        self.rep_pick = None;
        self.scene.clear_hover();
    }

    /// Draw the alignment window, and run whichever button was pressed.
    pub(super) fn draw_align_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.align_dialog.take() else {
            return;
        };
        let mut action = None;
        let mut close = false;
        // Which side (if any) is currently waiting for a rep click, so the row can say so.
        let picking = match self.rep_pick {
            Some(RepPick::Align(side)) => Some(side),
            _ => None,
        };
        // Molecule list for the dropdowns, read before the closure borrows `dialog`.
        let mols = mol_entries(&self.scene);

        let screen = ctx.content_rect();
        egui::Window::new(format!("{}  Align", icon::ARROWS_IN))
            .id(egui::Id::new("align_window"))
            .collapsible(false)
            .resizable(false)
            .movable(true)
            .pivot(egui::Align2::CENTER_TOP)
            .default_pos(egui::pos2(screen.center().x, screen.top() + 64.0))
            .show(ctx, |ui| {
                ui.set_min_width(MIN_WIDTH);

                // Width reserved for **both** molecule dropdowns: the wider of the two current
                // labels. That keeps the two rows' columns lined up with each other, and lets a
                // long name push the window wider instead of squeezing the field beside it.
                let mols = MolList { entries: &mols, width: {
                    let label = |m: Option<MolId>| {
                        m.and_then(|id| chosen(&mols, id))
                            .map(|(l, _)| l)
                            .unwrap_or_else(|| NO_MOLECULE.to_string())
                    };
                    let (a, b) = (label(dialog.source.mol), label(dialog.target.mol));
                    max_label_width(ui, [a.as_str(), b.as_str()].into_iter()) + CARET_W
                }};

                // — Source —
                let (src_frames, src_changed) = side_row(
                    ui, "Source", &mut dialog.source, &mols, picking, AlignSide::Source,
                    &mut action,
                );
                if src_changed {
                    dialog.invalidate();
                }
                ui.horizontal(|ui| {
                    ui.add_space(LABEL_W);
                    // "All frames" belongs to what *moves*, so it applies to every kind of
                    // target: fit a whole trajectory onto a reference structure, or onto one
                    // of its own frames.
                    let mut all = dialog.all_frames;
                    if ui
                        .add_enabled(
                            src_frames > 1,
                            egui::Checkbox::new(&mut all, "All frames"),
                        )
                        .on_hover_text(
                            "Fit every frame of the source molecule, not just the one above",
                        )
                        .changed()
                    {
                        dialog.all_frames = all;
                        dialog.invalidate();
                    }
                });

                ui.add_space(4.0);

                // — Target —
                ui.horizontal(|ui| {
                    ui.add_sized([LABEL_W, 18.0], egui::Label::new("Target").selectable(false));
                    let mut same = dialog.same_as_source;
                    if ui
                        .checkbox(&mut same, "Same as source")
                        .on_hover_text(
                            "Use the source's own selection as the reference, at the frame set \
                             above — with “All frames” this is the usual trajectory fit",
                        )
                        .changed()
                    {
                        dialog.same_as_source = same;
                        dialog.invalidate();
                    }
                });
                if !dialog.same_as_source {
                    let (_, changed) = side_row(
                        ui, "", &mut dialog.target, &mols, picking, AlignSide::Target,
                        &mut action,
                    );
                    if changed {
                        dialog.invalidate();
                    }
                }

                ui.add_space(4.0);
                ui.separator();

                let mut common = dialog.common_subset;
                if ui
                    .checkbox(&mut common, "Common subset")
                    .on_hover_text(
                        "Compare only the atoms whose names match (molar's sequence alignment), \
                         so selections of unequal size can still be fitted. Off: the two \
                         selections must correspond atom for atom.",
                    )
                    .changed()
                {
                    dialog.common_subset = common;
                    dialog.invalidate();
                }
                let mut whole = dialog.move_whole;
                if ui
                    .checkbox(&mut whole, "Move whole molecule")
                    .on_hover_text(
                        "Apply the fit to every atom of the source molecule. Off: only the \
                         atoms the source selection matched are moved.",
                    )
                    .changed()
                {
                    dialog.move_whole = whole;
                    dialog.invalidate();
                }

                // The readout: only once something has been computed.
                if let Some(r) = dialog.rmsd {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("RMSD:");
                        ui.label(bold_name(ui, &r.label()));
                    });
                }
                if let Some(e) = dialog.error.as_deref() {
                    ui.add_space(4.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(e).color(ui.visuals().error_fg_color),
                        )
                        .wrap(),
                    );
                }
                if picking.is_some() {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(
                            "Click a representation in the tree or in the 3-D view (Esc to \
                             cancel)",
                        )
                        .weak(),
                    );
                }

                ui.separator();
                ui.horizontal(|ui| {
                    let ready = dialog.source.mol.is_some()
                        && (dialog.same_as_source || dialog.target.mol.is_some());
                    if ui
                        .add_enabled(ready, egui::Button::new("Align"))
                        .on_hover_text("Fit the source onto the target, then measure")
                        .clicked()
                    {
                        action = Some(AlignAction::Align);
                    }
                    if ui
                        .add_enabled(ready, egui::Button::new("RMSD"))
                        .on_hover_text("Measure only — move nothing")
                        .clicked()
                    {
                        action = Some(AlignAction::Rmsd);
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });

        // Escape closes the window — unless it is being used to cancel a rep pick, which the
        // viewport consumes first (so one Escape does one thing).
        if self.rep_pick.is_none() && ctx.input_mut(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }

        match action {
            Some(AlignAction::Pick(side)) => self.rep_pick = Some(RepPick::Align(side)),
            Some(AlignAction::Align) => self.run_align(&mut dialog, true),
            Some(AlignAction::Rmsd) => self.run_align(&mut dialog, false),
            None => {}
        }
        if close {
            // Leaving the dialog must not leave the app stuck in pick mode.
            if picking.is_some() {
                self.rep_pick = None;
                self.scene.clear_hover();
            }
        } else {
            self.align_dialog = Some(dialog);
        }
    }

    /// Run the dialog's request: fit + measure, or measure only. The outcome (a value or the
    /// reason there isn't one) lands in the dialog.
    fn run_align(&mut self, dialog: &mut AlignDialog, apply: bool) {
        dialog.invalidate();
        let req = match self.align_request(dialog) {
            Ok(req) => req,
            Err(e) => {
                dialog.error = Some(e);
                return;
            }
        };
        let outcome = match apply {
            false => analysis::rmsd(&self.scene, &req).map(|r| (r, Vec::new())),
            true => analysis::align(&mut self.scene, &req),
        };
        match outcome {
            Ok((rmsd, edits)) => {
                if !edits.is_empty() {
                    // One step, however many frames moved: it was one press of one button.
                    self.history.record_structs(edits, "align".into());
                    self.view_dirty = true;
                }
                dialog.rmsd = Some(rmsd);
            }
            Err(e) => dialog.error = Some(e),
        }
    }

    /// Turn the dialog's inputs into an [`analysis::Request`], resolving molecule ids to
    /// indices and deciding the frames.
    fn align_request(&self, dialog: &AlignDialog) -> Result<analysis::Request, String> {
        let index = |id: Option<MolId>, what: &str| -> Result<usize, String> {
            id.and_then(|id| self.scene.mol_index(id))
                .ok_or_else(|| format!("choose a {what} molecule"))
        };
        let src_mol = index(dialog.source.mol, "source")?;

        // With "Same as source", the reference is the source's own selection at the frame in
        // the source's frame box — so with "All frames" off the frame that *moves* is the
        // molecule's displayed one, otherwise the operation would compare a frame with itself
        // and do nothing.
        let (target, src_frame) = match dialog.same_as_source {
            true => {
                let target = analysis::Side {
                    mol: src_mol,
                    sel: dialog.source.sel.clone(),
                    frame: dialog.source.frame as usize,
                };
                // The source's box is spoken for (it is the reference), so the single frame
                // this moves is the displayed one. With "All frames" the plan covers every
                // frame and this value is unused.
                (target, self.scene.molecules[src_mol].trajectory.current)
            }
            false => {
                let tgt_mol = index(dialog.target.mol, "target")?;
                let target = analysis::Side {
                    mol: tgt_mol,
                    sel: dialog.target.sel.clone(),
                    frame: dialog.target.frame as usize,
                };
                (target, dialog.source.frame as usize)
            }
        };
        Ok(analysis::Request {
            source: analysis::Side {
                mol: src_mol,
                sel: dialog.source.sel.clone(),
                frame: src_frame,
            },
            target,
            all_frames: dialog.all_frames,
            common_subset: dialog.common_subset,
            move_whole: dialog.move_whole,
        })
    }
}

impl App {
    /// Verification hook (`MOLAR_VIS_DEBUG_ALIGN`): fill the dialog from a compact spec and
    /// press **Align** — or **RMSD** with the `rmsd` flag — logging the outcome.
    ///
    /// `"<src mol>,<sel>,<frame>;<tgt mol>,<sel>,<frame>;<flags>"`, flags from
    /// `common` / `all` / `whole` / `same` / `rmsd`; the target half may be empty with `same`.
    /// It goes through [`Self::run_align`], the same path the buttons take (request building,
    /// the same-as-source frame rule, the undo step), and leaves the dialog open so a
    /// `_SAVE_UI` capture shows the readout it produced.
    pub(super) fn debug_align(&mut self, spec: &str) {
        let mut parts = spec.split(';');
        let mut dialog = AlignDialog::new(&self.scene);
        let side = |s: &str, out: &mut SideState, scene: &Scene| {
            let f: Vec<&str> = s.split(',').map(str::trim).collect();
            if let Some(mi) = f.first().and_then(|v| v.parse::<usize>().ok()) {
                out.mol = scene.molecules.get(mi).map(|m| m.id);
            }
            if let Some(sel) = f.get(1).filter(|s| !s.is_empty()) {
                out.sel = (*sel).to_string();
            }
            if let Some(fr) = f.get(2).and_then(|v| v.parse::<u32>().ok()) {
                out.frame = fr;
            }
        };
        side(parts.next().unwrap_or_default(), &mut dialog.source, &self.scene);
        side(parts.next().unwrap_or_default(), &mut dialog.target, &self.scene);
        let flags = parts.next().unwrap_or_default();
        let has = |f: &str| flags.split(',').any(|v| v.trim() == f);
        dialog.common_subset = has("common");
        dialog.all_frames = has("all");
        dialog.move_whole = has("whole");
        dialog.same_as_source = has("same");

        self.run_align(&mut dialog, !has("rmsd"));
        match (&dialog.rmsd, &dialog.error) {
            (Some(r), _) => log::info!("debug align: RMSD {}", r.label()),
            (None, Some(e)) => log::error!("debug align: {e}"),
            (None, None) => log::error!("debug align: nothing computed"),
        }
        self.align_dialog = Some(dialog);
    }
}

/// What the dialog asked for this frame.
enum AlignAction {
    /// Start choosing a rep for this side.
    Pick(AlignSide),
    Align,
    Rmsd,
}

/// One molecule the dropdown can offer.
struct MolChoice {
    id: MolId,
    /// How it reads in the list — `"3: 2lao.pdb"`, or a group member's own name.
    label: String,
    frames: usize,
}

/// A row of the molecule dropdown.
enum MolEntry {
    /// An ordinary molecule.
    Single(MolChoice),
    /// A [`MolGroup`]: **one** row that expands to its members. Listing members flat is what
    /// broke this dropdown — a 20-pose SDF buried every other molecule in the scene — and it
    /// also misrepresented the group, which the rest of the UI treats as one thing showing one
    /// member at a time.
    Group {
        name: String,
        /// The member shown in the viewport: the group's default here, as everywhere else.
        shown: Option<MolId>,
        members: Vec<MolChoice>,
    },
}

/// The dropdown's contents, in panel order: each ungrouped molecule, and each group once, at
/// the position of its first member.
fn mol_entries(scene: &Scene) -> Vec<MolEntry> {
    let choice = |m: &crate::scene::Molecule, label: String| MolChoice {
        id: m.id,
        label,
        frames: m.trajectory.n_frames(),
    };
    let mut out = Vec::new();
    let mut done: Vec<GroupId> = Vec::new();
    for (i, m) in scene.molecules.iter().enumerate() {
        let Some(gid) = m.group else {
            out.push(MolEntry::Single(choice(m, format!("{}: {}", i + 1, m.name))));
            continue;
        };
        if done.contains(&gid) {
            continue;
        }
        done.push(gid);
        let Some(g) = scene.groups.iter().find(|g| g.id == gid) else { continue };
        out.push(MolEntry::Group {
            name: g.name.clone(),
            shown: g.members.get(g.current).copied(),
            members: g
                .members
                .iter()
                .filter_map(|&id| scene.mol_index(id))
                .map(|mi| {
                    let m = &scene.molecules[mi];
                    choice(m, m.name.clone())
                })
                .collect(),
        });
    }
    out
}

/// The molecule dropdown's contents plus the width both rows reserve for it — they belong
/// together (the width is measured from these entries' labels), and keeping them one value is
/// what keeps [`side_row`]'s parameter list readable.
struct MolList<'a> {
    entries: &'a [MolEntry],
    width: f32,
}

/// The chosen molecule as the closed dropdown shows it, plus its frame count. A group member
/// is qualified by its group (`ligands20.sdf: aspirin`), since its own name says nothing about
/// where it came from.
fn chosen(entries: &[MolEntry], id: MolId) -> Option<(String, usize)> {
    entries.iter().find_map(|e| match e {
        MolEntry::Single(c) if c.id == id => Some((c.label.clone(), c.frames)),
        MolEntry::Group { name, members, .. } => members
            .iter()
            .find(|c| c.id == id)
            .map(|c| (format!("{name}: {}", c.label), c.frames)),
        MolEntry::Single(_) => None,
    })
}

/// Height at which the dropdown starts scrolling instead of growing. A scene can hold any
/// number of molecules, and a menu taller than the screen cannot be used at all.
const LIST_MAX_H: f32 = 320.0;

/// Placeholder in a dropdown with nothing chosen (an empty scene).
const NO_MOLECULE: &str = "(no molecule)";

/// Slack added to a measured dropdown label for the caret and the button's own padding.
const CARET_W: f32 = 32.0;

/// One side's row: `label | [molecule ▾] [selection] [⌖ pick] [frame]`.
///
/// Returns the chosen molecule's frame count (what decides whether the frame controls do
/// anything) and whether an input changed (so the caller can drop a stale result).
fn side_row(
    ui: &mut egui::Ui,
    label: &str,
    side: &mut SideState,
    mols: &MolList,
    picking: Option<AlignSide>,
    which: AlignSide,
    action: &mut Option<AlignAction>,
) -> (usize, bool) {
    let mut changed = false;
    let current = side.mol.and_then(|id| chosen(mols.entries, id));
    let n_frames = current.as_ref().map(|(_, n)| *n).unwrap_or(0);
    ui.horizontal(|ui| {
        ui.add_sized([LABEL_W, 18.0], egui::Label::new(label).selectable(false));

        // Molecule dropdown. Click-to-open `Popup::menu`, like every other dropdown here; at
        // the width the caller reserved for both rows.
        let name = current.map(|(l, _)| l).unwrap_or_else(|| NO_MOLECULE.to_string());
        let resp = ui.add_sized(
            [mols.width, 20.0],
            egui::Button::new(format!("{name}  {}", icon::CARET_DOWN)),
        );
        egui::Popup::menu(&resp).show(|ui| {
            egui::ScrollArea::vertical().max_height(LIST_MAX_H).show(ui, |ui| {
                for entry in mols.entries {
                    match entry {
                        MolEntry::Single(c) => {
                            if ui.button(&c.label).clicked() {
                                side.mol = Some(c.id);
                                changed = true;
                                ui.close();
                            }
                        }
                        MolEntry::Group { name, shown, members } => {
                            changed |= group_entry(ui, side, name, *shown, members);
                        }
                    }
                }
            });
        });

        // Selection text — the same thing a rep holds, so a picked rep just writes it here.
        changed |= ui
            .add_sized([SEL_FIELD_W, 20.0], egui::TextEdit::singleline(&mut side.sel))
            .on_hover_text("molar selection text")
            .changed();

        // Pick a rep: fills the molecule, the selection and the frame from it.
        if ui
            .selectable_label(picking == Some(which), icon::CROSSHAIR)
            .on_hover_text("Take the molecule and selection from a representation — click one \
                            in the tree or in the 3-D view")
            .clicked()
        {
            *action = Some(AlignAction::Pick(which));
        }

        // Frame: only meaningful for a molecule that has a trajectory.
        ui.add_enabled_ui(n_frames > 1, |ui| {
            let max = n_frames.saturating_sub(1) as u32;
            side.frame = side.frame.min(max);
            let mut f = side.frame;
            if spin_u32(ui, &mut f, 0..=max) {
                side.frame = f;
                changed = true;
            }
        });
    });
    (n_frames, changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::bonds::BondParams;
    use crate::settings::RepDefaults;

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests")).join(name)
    }

    /// A scene holding one ordinary molecule and one multi-record SDF group.
    fn scene_with_group() -> Scene {
        let mut scene = Scene::default();
        let raw = crate::data::load_with(&fixture("2lao.pdb"), &BondParams::default())
            .expect("load 2lao");
        scene.add(raw, &RepDefaults::default());
        let recs = crate::data::load_records(&fixture("ligands20.sdf"), &BondParams::default())
            .expect("load ligands");
        scene.add_group(
            recs,
            crate::scene::MoleculeSource::Bytes { name: "ligands20.sdf".into() },
            "ligands20.sdf".into(),
            &RepDefaults::default(),
        );
        scene
    }

    /// The dropdown lists a group as **one** expandable row, not twenty — that flat list is
    /// what made this control unusable with an SDF loaded — and a chosen member reads with its
    /// group's name in front of it.
    #[test]
    fn a_group_is_one_row_with_its_members_inside() {
        let scene = scene_with_group();
        let entries = mol_entries(&scene);
        assert_eq!(entries.len(), 2, "one molecule + one group, whatever the member count");

        let (name, members, shown) = match &entries[1] {
            MolEntry::Group { name, members, shown } => (name, members, shown),
            MolEntry::Single(_) => panic!("the SDF must come through as a group"),
        };
        assert_eq!(name, "ligands20.sdf");
        assert_eq!(members.len(), 20, "every member is reachable inside the row");
        // The group's shown member is its default here, as everywhere else in the UI.
        assert_eq!(*shown, Some(members[0].id));

        let (label, frames) = chosen(&entries, members[0].id).expect("member label");
        assert!(
            label.starts_with("ligands20.sdf: "),
            "a member must be qualified by its group: {label}"
        );
        assert_eq!(frames, 0, "a static member has no trajectory");

        // An ungrouped molecule keeps its numbered panel label.
        let first = match &entries[0] {
            MolEntry::Single(c) => c.id,
            MolEntry::Group { .. } => panic!("2lao is not grouped"),
        };
        assert_eq!(chosen(&entries, first).expect("label").0, "1: 2lao.pdb");
    }
}

/// A group's row in the molecule dropdown: the group name plus the member it currently shows,
/// expanding to the full member list. Returns whether a member was chosen.
///
/// The header row **is** a choice — clicking it takes the shown member, so the common case (the
/// group's active pose) is one click and the submenu is only for reaching a different member.
/// The shown member is marked in the list the way the panel marks it, so which one the header
/// stands for is visible from inside too.
fn group_entry(
    ui: &mut egui::Ui,
    side: &mut SideState,
    name: &str,
    shown: Option<MolId>,
    members: &[MolChoice],
) -> bool {
    use egui::containers::menu::SubMenu;
    let shown_label = shown
        .and_then(|id| members.iter().find(|c| c.id == id))
        .map(|c| format!(" — {}", c.label))
        .unwrap_or_default();
    let header = ui.button(format!("{}  {name}{shown_label}", icon::STACK));
    let mut picked = None;
    if header.clicked() {
        picked = shown;
    }
    SubMenu::new().show(ui, &header, |ui| {
        // Long groups (a 20-pose SDF, a docking run) scroll rather than run off the screen.
        egui::ScrollArea::vertical().max_height(LIST_MAX_H).show(ui, |ui| {
            for c in members {
                let is_shown = shown == Some(c.id);
                let label = match is_shown {
                    true => bold_name(ui, &c.label).underline(),
                    false => egui::RichText::new(&c.label),
                };
                if ui.button(label).clicked() {
                    picked = Some(c.id);
                }
            }
        });
    });
    match picked {
        Some(id) => {
            side.mol = Some(id);
            ui.close();
            true
        }
        None => false,
    }
}
