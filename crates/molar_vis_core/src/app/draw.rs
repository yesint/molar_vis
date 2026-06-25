//! Draw-mode types + tool palette UI.
use super::*;
use super::widgets::*;


// ===========================================================================
// Draw mode — interactive molecule sketching
// ===========================================================================
//
// Draw mode lets the user build a small molecule by hand: place atoms in the
// viewport, draw bonds between them, cycle bond orders, and erase, with the
// force-field cleanup minimizer (`crate::minimize`) relaxing the strained
// hand-drawn geometry into sensible bond lengths/angles. State lives in an
// `App::draw: Option<DrawSession>`; it's mutually exclusive with the pick modes
// (turning one on clears the other). Cross-platform (no native-only deps in the
// pointer/UI path), so it works in the browser too.

/// An element on the drawing palette. Atoms are built via
/// `Atom::new().with_name(symbol).guess()`, which fills element/mass/vdw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Element {
    H,
    C,
    N,
    O,
    S,
    P,
    F,
    Cl,
    Br,
    I,
}

impl Element {
    /// The full palette, in display order (Carbon-first, the common organics, then
    /// the heteroatoms and halogens).
    const ALL: [Element; 10] = [
        Element::C,
        Element::H,
        Element::N,
        Element::O,
        Element::S,
        Element::P,
        Element::F,
        Element::Cl,
        Element::Br,
        Element::I,
    ];

    /// The atomic symbol — the name fed to molar's `Atom::with_name(..).guess()`,
    /// and the palette button label.
    pub(super) fn symbol(self) -> &'static str {
        match self {
            Element::H => "H",
            Element::C => "C",
            Element::N => "N",
            Element::O => "O",
            Element::S => "S",
            Element::P => "P",
            Element::F => "F",
            Element::Cl => "Cl",
            Element::Br => "Br",
            Element::I => "I",
        }
    }

    /// Atomic number (for the CPK palette color).
    pub(super) fn atomic_number(self) -> u8 {
        match self {
            Element::H => 1,
            Element::C => 6,
            Element::N => 7,
            Element::O => 8,
            Element::F => 9,
            Element::P => 15,
            Element::S => 16,
            Element::Cl => 17,
            Element::Br => 35,
            Element::I => 53,
        }
    }

    /// Build a fresh molar `Atom` of this element (element/mass/vdw guessed from the
    /// name). The residue is a generic `DRG` so a drawn molecule reads as one ligand.
    pub(super) fn make_atom(self) -> molar::prelude::Atom {
        molar::prelude::Atom::new()
            .with_name(self.symbol())
            .with_resname("DRG")
            .guess()
    }
}

/// Which drawing tool the pointer acts as. There is no separate atom/bond mode —
/// the single **Draw** tool infers the action from the gesture: click empty space →
/// place an atom; drag from an existing atom → grow a bond; drag from empty space →
/// two bonded atoms; click a bond → cycle its order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DrawTool {
    /// Click/drag to add atoms and bonds (the default; see the type doc).
    Draw,
    /// Click an atom to delete it (+ its bonds), or a bond to delete the bond.
    Erase,
}

/// What the cursor is over in the drawing editor (used by hover-highlight + click).
#[derive(Clone, Copy)]
pub(super) enum HitTarget {
    Atom(usize),
    Bond(usize),
}

/// In-progress pointer gesture for the Draw tool. `current` is the live cursor
/// position (viewport pixels) for the rubber-band line.
pub(super) enum DrawDrag {
    /// No drag in progress.
    Idle,
    /// Dragging a bond out of an existing atom `from` (global index).
    FromAtom { from: usize, current: egui::Pos2 },
    /// Dragging from empty space (`start`) → will create two bonded atoms on release.
    FromEmpty { start: egui::Pos2, current: egui::Pos2 },
}

/// An in-flight relaxation. The minimizer is cheap for a few-atom molecule, so a job
/// is a one-shot — run a single `relax_in_system` call next eligible frame, then
/// clear. `to_convergence` picks the profile (Cleanup vs Quick).
pub(super) struct RelaxJob {
    /// Steps budget left (unused in the one-shot model, kept for the job shape /
    /// future incremental stepping per the Draw-mode contract). 0 ⇒ done.
    #[allow(dead_code)]
    pub(super) remaining: u32,
    /// Whether to run the full Cleanup profile (the "Clean up" button) vs Quick.
    pub(super) to_convergence: bool,
}

/// State of an active drawing session.
pub(super) struct DrawSession {
    /// The molecule edits land in. `None` until the first atom is placed (which
    /// *creates* the molecule, since molar can't append to a 0-atom system).
    pub(super) target: Option<MolId>,
    /// The active tool.
    pub(super) tool: DrawTool,
    /// The element placed by the Atom tool / a bonded end atom.
    pub(super) element: Element,
    /// Default order for newly drawn bonds.
    pub(super) bond_order: crate::minimize::BondOrder,
    /// The Bond tool's in-progress drag (rubber band).
    pub(super) drag: DrawDrag,
    /// A committed edit happened; relax once the pointer settles (debounced).
    pub(super) minimize_pending: bool,
    /// Active relaxation job, if any (drives the per-frame `relax_in_system` call).
    pub(super) relax: Option<RelaxJob>,
    /// A world point the drawing plane passes through (updated to the last-touched
    /// atom). `None` ⇒ use the camera target.
    pub(super) plane_depth: Option<glam::Vec3>,
}

impl Default for DrawSession {
    fn default() -> Self {
        Self {
            target: None,
            tool: DrawTool::Draw,
            element: Element::C,
            bond_order: crate::minimize::BondOrder::Single,
            drag: DrawDrag::Idle,
            minimize_pending: false,
            relax: None,
            plane_depth: None,
        }
    }
}
impl App {
    /// Turn Draw mode on (a fresh session) or off. Mutually exclusive with picking:
    /// entering Draw forces `pick_mode = Off` (and clears any in-progress lasso).
    pub(super) fn toggle_draw(&mut self) {
        if self.draw.is_some() {
            self.draw = None;
        } else {
            self.draw = Some(DrawSession::default());
            self.pick_mode = PickMode::Off;
            self.lasso_path.clear();
        }
    }

    /// Create a new editable molecule from a freshly seeded single-atom `RawMolecule`,
    /// give it a Ball-and-Stick rep (Element color), mark it `editable`, select it,
    /// and record its `MolId` on `session.target`. Shared by the first Atom-tool click
    /// and the headless preset hook.
    pub(super) fn start_drawn_molecule(&mut self, raw: data::RawMolecule, session: &mut DrawSession) {
        let rep_defaults = self.rep_defaults.clone();
        let id = self.scene.add(raw, &rep_defaults);
        let mol = self.scene.molecules.last_mut().unwrap();
        mol.editable = true;
        mol.trajectory.speed_fps = self.settings.behavior.traj_fps;
        mol.trajectory.loop_mode = self.settings.behavior.loop_mode;
        // A drawn molecule reads best as Ball-and-Stick / Element color.
        if let Some(rep) = mol.reps.first_mut() {
            rep.kind = RepKind::BallAndStick;
            rep.params = RepParams::for_kind(RepKind::BallAndStick);
            rep.color = ColorMethod::Element;
            rep.sel_text = "all".to_string();
            rep.sel_dirty = true;
            rep.geom_dirty = true;
        }
        self.scene.selected_mol = Some(self.scene.molecules.len() - 1);
        session.target = Some(id);
        self.view_dirty = true;
    }

    /// The Draw-mode toggle button + (when active) a second toolbar row with the
    /// tool selector, element palette, bond-order selector, and the Clean up / New /
    /// Finish actions. Drawn inside `draw_view_toolbar`'s panel. Returns nothing; all
    /// state lives in `self.draw`.
    /// The vertical drawing-tools palette, shown as a narrow right-side panel only
    /// while Draw mode is active. Icon-only buttons (labels in hover tooltips): tool
    /// selector, CPK-colored element chips, bond-order line icons, then Clean-up /
    /// New / Finish actions. Scrolls if the window is short.
    pub(super) fn draw_tools_panel(&mut self, ui: &mut egui::Ui) {
        if self.draw.is_none() {
            return;
        }
        egui::Panel::right("draw_tools_panel")
            .resizable(false)
            .default_size(54.0)
            .show_inside(ui, |ui| {
                ui.add_space(6.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(4.0, 5.0);

                        // — Tools (Draw / Erase) —
                        let cur_tool = self.draw.as_ref().map(|d| d.tool);
                        for tool in [DrawTool::Draw, DrawTool::Erase] {
                            let glyph = match tool {
                                DrawTool::Draw => icon::PENCIL_SIMPLE,
                                DrawTool::Erase => icon::ERASER,
                            };
                            let tip = match tool {
                                DrawTool::Draw => "Draw — click to add an atom, drag to add a bond; click a bond to cycle its order",
                                DrawTool::Erase => "Erase — click an atom or bond to delete it",
                            };
                            if overlay_button(ui, glyph, cur_tool == Some(tool))
                                .on_hover_text(tip)
                                .clicked()
                            {
                                if let Some(d) = self.draw.as_mut() {
                                    d.tool = tool;
                                    d.drag = DrawDrag::Idle;
                                }
                            }
                        }

                        ui.separator();

                        // — Element palette — CPK-colored chips, two per row;
                        // picking one also switches to the Draw tool.
                        let cur_el = self.draw.as_ref().map(|d| d.element);
                        let mut pick_el = None;
                        for pair in Element::ALL.chunks(2) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
                                for &el in pair {
                                    let rgba = crate::color::element_color(el.atomic_number());
                                    if Self::element_chip(ui, el.symbol(), rgba, cur_el == Some(el))
                                        .on_hover_text(format!("Draw {} atoms", el.symbol()))
                                        .clicked()
                                    {
                                        pick_el = Some(el);
                                    }
                                }
                            });
                        }
                        if let (Some(el), Some(d)) = (pick_el, self.draw.as_mut()) {
                            d.element = el;
                            d.tool = DrawTool::Draw;
                        }

                        ui.separator();

                        // — Bond order (1 / 2 / 3 lines) for new bonds.
                        let cur_ord = self.draw.as_ref().map(|d| d.bond_order);
                        for (order, n) in [
                            (crate::minimize::BondOrder::Single, 1u8),
                            (crate::minimize::BondOrder::Double, 2),
                            (crate::minimize::BondOrder::Triple, 3),
                        ] {
                            if Self::bond_order_icon(ui, n, cur_ord == Some(order))
                                .on_hover_text(format!("{} bond", order.label()))
                                .clicked()
                            {
                                if let Some(d) = self.draw.as_mut() {
                                    d.bond_order = order;
                                }
                            }
                        }

                        ui.separator();

                        // — Clean up (relax to convergence) — disabled until a molecule exists.
                        let has_target = self.draw.as_ref().and_then(|d| d.target).is_some();
                        let cleanup = ui
                            .add_enabled_ui(has_target, |ui| {
                                overlay_button(ui, icon::SPARKLE, false)
                                    .on_hover_text("Clean up — relax the geometry to convergence")
                                    .clicked()
                            })
                            .inner;
                        if cleanup {
                            if let Some(d) = self.draw.as_mut() {
                                d.relax = Some(RelaxJob { remaining: 1, to_convergence: true });
                            }
                        }
                        // — H+ : add explicit hydrogens to satisfy valence, or remove
                        // them if the molecule already has any (toggle).
                        let toggle_h = ui
                            .add_enabled_ui(has_target, |ui| {
                                overlay_button(ui, "H+", false)
                                    .on_hover_text("Add/remove explicit hydrogens")
                                    .clicked()
                            })
                            .inner;
                        if toggle_h {
                            if let Some(mi) =
                                self.draw.as_ref().and_then(|d| d.target).and_then(|id| {
                                    self.scene.molecules.iter().position(|m| m.id == id)
                                })
                            {
                                self.toggle_hydrogens(mi);
                            }
                        }
                        // — New: the next click starts a fresh molecule.
                        if overlay_button(ui, icon::FILE_PLUS, false)
                            .on_hover_text("New — start a fresh molecule on the next click")
                            .clicked()
                        {
                            if let Some(d) = self.draw.as_mut() {
                                d.target = None;
                                d.drag = DrawDrag::Idle;
                            }
                        }
                        // — Finish: leave Draw mode.
                        if overlay_button(ui, icon::CHECK, false)
                            .on_hover_text("Finish — leave Draw mode")
                            .clicked()
                        {
                            self.draw = None;
                        }

                        // Alt-to-rotate hint (icon only; tooltip explains).
                        if ui.input(|i| i.modifiers.alt) {
                            ui.separator();
                            ui.colored_label(
                                egui::Color32::from_rgb(150, 190, 230),
                                egui::RichText::new(icon::ARROWS_CLOCKWISE).size(16.0),
                            )
                            .on_hover_text("Alt: rotate the view");
                        }
                    });
                });
            });
    }

    /// A small CPK-colored element chip (icon-only palette button): a rounded square
    /// filled with the element's color, the symbol drawn in contrasting ink, a ring
    /// when active/hovered.
    pub(super) fn element_chip(ui: &mut egui::Ui, symbol: &str, rgba: [u8; 4], active: bool) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
        let fill = egui::Color32::from_rgb(rgba[0], rgba[1], rgba[2]);
        let chip = rect.shrink(1.0);
        ui.painter().rect_filled(chip, 4.0, fill);
        if active {
            ui.painter().rect_stroke(
                chip,
                4.0,
                egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Inside,
            );
        } else if resp.hovered() {
            ui.painter().rect_stroke(
                chip,
                4.0,
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(180)),
                egui::StrokeKind::Inside,
            );
        }
        // Contrasting label by luminance.
        let lum = 0.299 * rgba[0] as f32 + 0.587 * rgba[1] as f32 + 0.114 * rgba[2] as f32;
        let txt = if lum > 140.0 { egui::Color32::BLACK } else { egui::Color32::WHITE };
        let font = egui::TextStyle::Button.resolve(ui.style());
        let galley = ui.painter().layout_no_wrap(symbol.to_owned(), font, txt);
        let ink = galley.mesh_bounds;
        ui.painter().galley(rect.center() - ink.center().to_vec2(), galley, txt);
        resp
    }

    /// A bond-order icon button: `n` stacked horizontal lines on the toolbar-button
    /// frame (1 = single, 2 = double, 3 = triple).
    pub(super) fn bond_order_icon(ui: &mut egui::Ui, n: u8, active: bool) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 26.0), egui::Sense::click());
        let vis = ui.style().interact_selectable(&resp, active);
        let fill = if active {
            ui.visuals().selection.bg_fill
        } else {
            vis.weak_bg_fill
        };
        ui.painter().rect_filled(rect, 4.0, fill);
        let col = ui.visuals().text_color();
        let c = rect.center();
        let half_w = 7.0;
        let spacing = 4.0;
        let total = (n as f32 - 1.0) * spacing;
        for i in 0..n {
            let y = c.y - total / 2.0 + i as f32 * spacing;
            ui.painter().line_segment(
                [egui::pos2(c.x - half_w, y), egui::pos2(c.x + half_w, y)],
                egui::Stroke::new(1.8, col),
            );
        }
        resp
    }
}
