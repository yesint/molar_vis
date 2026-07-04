//! Dihedral rotation — the **DihedralRotate** tool of edit (Draw) mode.
//!
//! Pick a rotatable bond (the rotation **axis**), then drag one of the handles drawn on
//! its neighbouring bonds to swing that side of the molecule about the bond. State lives
//! in [`DihedralState`] on the active [`DrawSession`](super::draw::DrawSession); the
//! pointer path is driven from `draw_input` (which dispatches the DihedralRotate tool to
//! [`App::dihedral_input`]). A plain LMB click selects a bond; a plain LMB drag that
//! starts on a **handle** rotates that handle's side; a plain LMB drag anywhere else (or
//! Alt+LMB) orbits the camera as usual (nothing is grabbed).
//!
//! The rotation is a rigid transform of one half of the molecule about the bond line
//! (the axis atoms stay put), editing the *displayed* coordinates in place (see
//! [`crate::scene::Molecule::rotate_fragment`]). Every owned molecule is editable and its
//! coordinate edits are undoable (see [`crate::history`]); a shared (pymolar/JS) molecule
//! can't be mutated in place and is skipped.
use super::*;
use super::overlay::*;
use super::draw::DrawTool;

use glam::Vec3;
use std::f32::consts::{PI, TAU};

/// Screen-space grab radius for a handle (points): click/drag within this of a handle's
/// drawn dot to grab it.
const HANDLE_GRAB_PX: f32 = 14.0;
/// Handle dot radius (points); the grabbed/hovered one is drawn a touch larger.
const HANDLE_R: f32 = 6.0;
/// The selected bond (rotation axis) highlight colour (amber).
const AXIS_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 205, 90);

/// Which side of the axis bond a handle — and the rotation it drives — belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DihedralSide {
    /// The `i` endpoint's side (atoms reachable from `i` without crossing the bond).
    I,
    /// The `j` endpoint's side.
    J,
}

/// A grabbable handle: a neighbour atom of an axis endpoint (the far end of a bond
/// adjacent to the axis), plus the axis endpoint it hangs off and the side it rotates.
#[derive(Clone, Copy, Debug)]
pub(super) struct DihedralHandle {
    /// The neighbour atom (global index) — where the handle dot is drawn.
    pub(super) atom: usize,
    /// The axis endpoint this neighbour bonds to (`i` for side I, `j` for side J) —
    /// the near end of the grip line drawn from the axis to the handle.
    pub(super) anchor: usize,
    pub(super) side: DihedralSide,
}

/// A selected rotatable bond: its two endpoints (the rotation axis), the atom set on
/// each side, and the handles drawn on the neighbouring bonds.
pub(super) struct DihedralAxis {
    /// The molecule the bond belongs to (resolved to an index by id each frame).
    pub(super) mol: MolId,
    pub(super) i: usize,
    pub(super) j: usize,
    /// Atoms reachable from `i` without crossing the `i–j` bond (includes `i`).
    pub(super) i_side: Vec<usize>,
    /// Atoms reachable from `j` without crossing the `i–j` bond (includes `j`).
    pub(super) j_side: Vec<usize>,
    pub(super) handles: Vec<DihedralHandle>,
}

/// An in-progress handle drag: the fragment side being rotated, plus the axis frame
/// and last-measured drag angle used to compute the per-frame rotation delta.
pub(super) struct DihedralDrag {
    pub(super) side: DihedralSide,
    /// Index of the grabbed handle (for the emphasized overlay).
    pub(super) handle: usize,
    /// A point on the rotation line (world nm) and its unit direction (`i → j`).
    pub(super) axis_point: Vec3,
    pub(super) axis_dir: Vec3,
    /// Orthonormal basis ⟂ the axis for measuring the drag angle in the rotation plane.
    pub(super) e1: Vec3,
    pub(super) e2: Vec3,
    /// Last cursor angle in the rotation plane (rad); the per-frame delta drives the
    /// rotation, so there's no jump on grab and no accumulated float drift beyond a
    /// single small step. `None` when the axis is edge-on this frame (the angle can't be
    /// measured) — the next measurable frame re-baselines to it instead of applying all the
    /// motion accumulated during the edge-on span as one jump.
    pub(super) last_angle: Option<f32>,
    /// The rotated side's atoms + their coordinates captured at grab time, so the whole
    /// drag records as one undoable coordinate delta at release.
    pub(super) edit_atoms: Vec<usize>,
    pub(super) before: Vec<[f32; 3]>,
    /// The coordinate store the twist targets (owned System / a trajectory frame), captured
    /// at grab time so its undo hits the same store — see [`Molecule::coord_edit_target`].
    pub(super) frame: Option<usize>,
}

/// State for the DihedralRotate tool, held on the active [`DrawSession`]: the selected
/// bond (axis) + its sides/handles, and any in-progress handle drag.
#[derive(Default)]
pub(super) struct DihedralState {
    /// The currently selected bond (axis) + its sides/handles, or `None` before the
    /// first bond is picked.
    pub(super) axis: Option<DihedralAxis>,
    /// The active handle drag, if any.
    pub(super) drag: Option<DihedralDrag>,
}

/// Viewport pixel → clip-space NDC (each in `[-1, 1]`, y up).
fn px_to_ndc(px: egui::Pos2, rect: egui::Rect) -> (f32, f32) {
    (
        ((px.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0,
        1.0 - ((px.y - rect.top()) / rect.height().max(1.0)) * 2.0,
    )
}

/// Adjacency list (neighbour atoms per atom index) from a molecule's bond graph.
fn adjacency(mol: &scene::Molecule) -> Vec<Vec<usize>> {
    let mut adj = vec![Vec::new(); mol.n_atoms];
    for b in &mol.bonds {
        if b.i1 < mol.n_atoms && b.i2 < mol.n_atoms {
            adj[b.i1].push(b.i2);
            adj[b.i2].push(b.i1);
        }
    }
    adj
}

/// Atoms reachable from `start` over `adj` **without** ever traversing the `i–j` edge
/// (the rotation axis), i.e. the connected component of `start` once that one bond is
/// cut. Includes `start`.
fn side_atoms(adj: &[Vec<usize>], start: usize, i: usize, j: usize) -> Vec<usize> {
    let mut visited = vec![false; adj.len()];
    let mut stack = vec![start];
    if start < visited.len() {
        visited[start] = true;
    }
    while let Some(u) = stack.pop() {
        for &v in &adj[u] {
            // Cut exactly the axis bond (there is at most one i–j bond).
            if (u == i && v == j) || (u == j && v == i) {
                continue;
            }
            if !visited[v] {
                visited[v] = true;
                stack.push(v);
            }
        }
    }
    visited
        .iter()
        .enumerate()
        .filter_map(|(k, &b)| b.then_some(k))
        .collect()
}

/// Build a [`DihedralAxis`] for bond `k` of `mol`, or `None` if that bond is **not
/// rotatable**: in a ring (cutting it doesn't split the molecule), or terminal (an
/// endpoint has no other neighbour, so one side is a lone atom with nothing to swing).
/// Bond order is not considered — a guessed PDB bond has no order, and the tool is a
/// geometric aid, so any non-ring, non-terminal bond may be twisted.
fn build_axis(mol: &scene::Molecule, k: usize) -> Option<DihedralAxis> {
    let bond = mol.bonds.get(k)?;
    let (i, j) = (bond.i1, bond.i2);
    if i >= mol.n_atoms || j >= mol.n_atoms || i == j {
        return None;
    }
    let adj = adjacency(mol);
    // Terminal bond: an endpoint whose only neighbour is the other endpoint.
    if adj[i].len() < 2 || adj[j].len() < 2 {
        return None;
    }
    let j_side = side_atoms(&adj, j, i, j);
    // Ring: cutting i–j still leaves j connected to i → the bond can't rotate freely.
    if j_side.contains(&i) {
        return None;
    }
    let i_side = side_atoms(&adj, i, i, j);
    // Handles on every bond adjacent to the axis: the i-side neighbours (anchored at
    // i) and the j-side neighbours (anchored at j).
    let mut handles = Vec::new();
    for &n in &adj[i] {
        if n != j {
            handles.push(DihedralHandle { atom: n, anchor: i, side: DihedralSide::I });
        }
    }
    for &n in &adj[j] {
        if n != i {
            handles.push(DihedralHandle { atom: n, anchor: j, side: DihedralSide::J });
        }
    }
    Some(DihedralAxis { mol: mol.id, i, j, i_side, j_side, handles })
}

/// Colour of a side's handles (a distinct tint per side so the two rotatable groups
/// read apart).
fn side_color(side: DihedralSide) -> egui::Color32 {
    match side {
        DihedralSide::I => egui::Color32::from_rgb(120, 200, 255),
        DihedralSide::J => egui::Color32::from_rgb(255, 150, 120),
    }
}

impl App {
    /// Pointer handling for the DihedralRotate tool, dispatched from `draw_input` each
    /// frame. Selects a bond on click, grabs/rotates a handle on drag, and draws the
    /// axis/handle overlays. Camera navigation is left to `draw_viewport`, which
    /// suppresses the LMB orbit only while a handle drag is active; Alt+LMB always
    /// orbits (so no bond is grabbed while Alt is held).
    pub(super) fn dihedral_input(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        rect: egui::Rect,
        size_px: [u32; 2],
    ) {
        let alt = ui.input(|i| i.modifiers.alt);

        // Esc clears the current selection / drag.
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            if let Some(d) = self.draw.as_mut() {
                d.dihedral.axis = None;
                d.dihedral.drag = None;
            }
            ui.ctx().request_repaint();
        }

        // Resolve the axis molecule to a live index; drop the axis if it's gone.
        let axis_mol = self
            .draw
            .as_ref()
            .and_then(|d| d.dihedral.axis.as_ref().map(|a| a.mol));
        let axis_mi = axis_mol.and_then(|id| self.scene.mol_index(id));
        if axis_mol.is_some() && axis_mi.is_none() {
            if let Some(d) = self.draw.as_mut() {
                d.dihedral.axis = None;
                d.dihedral.drag = None;
            }
        }

        // --- Active handle drag: update the rotation, or finish on button release. ---
        if self.draw.as_ref().is_some_and(|d| d.dihedral.drag.is_some()) {
            if ui.input(|i| i.pointer.primary_down()) {
                self.dihedral_drag_update(response, rect, size_px, axis_mi);
            } else {
                // Drag finished: take the drag state and record the net rotation as one
                // undoable coordinate step (the fragment atoms' before → after coords).
                let edit = self
                    .draw
                    .as_mut()
                    .and_then(|d| d.dihedral.drag.take())
                    .map(|dr| (dr.edit_atoms, dr.before, dr.frame));
                if let Some(mi) = axis_mi {
                    self.scene.molecules[mi].refresh_bbox();
                    if let Some((atoms, before, frame)) = edit {
                        self.dihedral_record_rotation(mi, atoms, before, frame);
                    }
                }
            }
            ui.ctx().request_repaint();
            self.dihedral_overlays(ui, rect, size_px);
            return;
        }

        // --- Begin a drag if the primary button grabbed a handle (not while Alt orbits).
        // Otherwise the drag falls through to camera orbit (handled in draw_viewport). ---
        if !alt && response.drag_started_by(egui::PointerButton::Primary) {
            if let (Some(mi), Some(px)) = (axis_mi, response.interact_pointer_pos()) {
                if let Some(hidx) = self.dihedral_hovered_handle(mi, rect, size_px, px) {
                    self.dihedral_begin_drag(mi, hidx, px, rect, size_px);
                    ui.ctx().request_repaint();
                    self.dihedral_overlays(ui, rect, size_px);
                    return;
                }
            }
        }

        // --- A plain click selects the bond under the cursor as the rotation axis. ---
        if !alt && response.clicked() {
            if let Some(px) = response.interact_pointer_pos() {
                match self.dihedral_pick_bond(rect, size_px, px) {
                    Some((mi, k)) => {
                        let mol_id = self.scene.molecules[mi].id;
                        match build_axis(&self.scene.molecules[mi], k) {
                            Some(axis) => {
                                if let Some(d) = self.draw.as_mut() {
                                    d.dihedral.axis = Some(axis);
                                    d.dihedral.drag = None;
                                    d.target = Some(mol_id); // subsequent edits act here
                                }
                                self.status.clear();
                            }
                            None => {
                                if let Some(d) = self.draw.as_mut() {
                                    d.dihedral.axis = None;
                                }
                                self.status =
                                    "That bond can't be rotated (it's in a ring or terminal)."
                                        .into();
                            }
                        }
                    }
                    None => {
                        // Clicked empty space → clear the selection.
                        if let Some(d) = self.draw.as_mut() {
                            d.dihedral.axis = None;
                        }
                    }
                }
                ui.ctx().request_repaint();
            }
        }

        self.dihedral_overlays(ui, rect, size_px);
    }

    /// Nearest bond to the cursor across all visible **owned** molecules (a shared
    /// pymolar molecule can't be coordinate-edited, so it's skipped), as
    /// `(molecule index, bond index)`. Uses screen-space distance to the projected
    /// bond segment, like the Draw tool.
    pub(super) fn dihedral_pick_bond(
        &self,
        rect: egui::Rect,
        size_px: [u32; 2],
        cursor: egui::Pos2,
    ) -> Option<(usize, usize)> {
        let aspect = size_px[0] as f32 / size_px[1].max(1) as f32;
        let (view, proj) = (self.camera.view(), self.camera.proj(aspect));
        let ndc = px_to_ndc(cursor, rect);
        let ndc = glam::vec2(ndc.0, ndc.1);
        let mut best: Option<(usize, usize, f32)> = None;
        for (mi, mol) in self.scene.molecules.iter().enumerate() {
            if !mol.visible || mol.data.is_shared() || mol.bonds.is_empty() {
                continue;
            }
            if let Some((k, dist)) = pick::nearest_bond_dist(mol, view, proj, ndc, 0.02) {
                if best.is_none_or(|(_, _, bd)| dist < bd) {
                    best = Some((mi, k, dist));
                }
            }
        }
        best.map(|(mi, k, _)| (mi, k))
    }

    /// Index (into the axis's `handles`) of the handle whose drawn dot is nearest the
    /// cursor within [`HANDLE_GRAB_PX`], or `None`.
    pub(super) fn dihedral_hovered_handle(
        &self,
        mi: usize,
        rect: egui::Rect,
        size_px: [u32; 2],
        cursor: egui::Pos2,
    ) -> Option<usize> {
        let axis = self.draw.as_ref()?.dihedral.axis.as_ref()?;
        let mut best: Option<(usize, f32)> = None;
        for (idx, h) in axis.handles.iter().enumerate() {
            if let Some(p) = self
                .atom_world(mi, h.atom)
                .and_then(|w| self.world_to_pixel(w, rect, size_px))
            {
                let d = (cursor - p).length();
                if d <= HANDLE_GRAB_PX && best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((idx, d));
                }
            }
        }
        best.map(|(i, _)| i)
    }

    /// Start a handle drag: build the rotation frame (axis line + a ⟂ basis anchored on
    /// the grabbed handle) and record the cursor's initial angle in that plane.
    pub(super) fn dihedral_begin_drag(
        &mut self,
        mi: usize,
        hidx: usize,
        px: egui::Pos2,
        rect: egui::Rect,
        size_px: [u32; 2],
    ) {
        let (i, j, handle_atom, side, side_atoms) = {
            let Some(axis) = self.draw.as_ref().and_then(|d| d.dihedral.axis.as_ref()) else {
                return;
            };
            let Some(h) = axis.handles.get(hidx).copied() else {
                return;
            };
            let side_atoms = match h.side {
                DihedralSide::I => axis.i_side.clone(),
                DihedralSide::J => axis.j_side.clone(),
            };
            (axis.i, axis.j, h.atom, h.side, side_atoms)
        };
        // Capture the rotated side's coords + which store they live in, so the whole drag
        // records as one undoable delta at release. This works for any molecule: the frame
        // target keeps the undo correct even if the displayed frame changes afterward.
        let frame = self.scene.molecules[mi].coord_edit_target();
        let before = self.dihedral_coords_of(mi, &side_atoms);
        let edit_atoms = side_atoms;
        let (Some(pi), Some(pj), Some(ph)) = (
            self.atom_world(mi, i),
            self.atom_world(mi, j),
            self.atom_world(mi, handle_atom),
        ) else {
            return;
        };
        let u = (pj - pi).normalize_or_zero();
        if u == Vec3::ZERO {
            return;
        }
        let axis_point = pi;
        // e1 ⟂ u, pointing toward the grabbed handle (so the grip feels attached);
        // fall back to any perpendicular if the handle sits on the axis line.
        let c = axis_point + u * (ph - axis_point).dot(u);
        let mut e1 = ph - c;
        if e1.length() < 1e-5 {
            e1 = u.cross(Vec3::X);
            if e1.length() < 1e-5 {
                e1 = u.cross(Vec3::Y);
            }
        }
        let e1 = e1.normalize();
        let e2 = u.cross(e1).normalize();
        let ndc = px_to_ndc(px, rect);
        let angle = self.dihedral_plane_angle(rect, size_px, ndc, axis_point, u, e1, e2);
        if let Some(d) = self.draw.as_mut() {
            d.dihedral.drag = Some(DihedralDrag {
                side,
                handle: hidx,
                axis_point,
                axis_dir: u,
                e1,
                e2,
                last_angle: angle,
                edit_atoms,
                before,
                frame,
            });
        }
    }

    /// The **displayed** coordinates of `atoms` in molecule `mi` (nm), for undo capture —
    /// the current trajectory frame if one is loaded, else the owned System (matching where
    /// `rotate_fragment` writes and the store `coord_edit_target` recorded).
    fn dihedral_coords_of(&self, mi: usize, atoms: &[usize]) -> Vec<[f32; 3]> {
        let st = self.scene.molecules[mi].render_state();
        atoms
            .iter()
            .map(|&a| st.coords.get(a).map(|p| [p.x, p.y, p.z]).unwrap_or([0.0; 3]))
            .collect()
    }

    /// Record the completed twist of `atoms` (from their `before` coords to the current
    /// ones, in coordinate store `frame`) as one undoable
    /// [`crate::history::StructEdit::Coords`] step. No-op if nothing moved or the edit
    /// wasn't captured (empty `atoms`).
    fn dihedral_record_rotation(
        &mut self,
        mi: usize,
        atoms: Vec<usize>,
        before: Vec<[f32; 3]>,
        frame: Option<usize>,
    ) {
        if atoms.is_empty() {
            return;
        }
        let after = self.dihedral_coords_of(mi, &atoms);
        if before == after {
            return;
        }
        let id = self.scene.molecules[mi].id;
        self.history.record_struct(
            id,
            crate::history::StructEdit::Coords { atoms, before, after, frame },
            "rotate bond".into(),
        );
    }

    /// Angle (rad) of the cursor's ray, projected into the rotation plane (through
    /// `axis_point`, normal `u`), measured in the `(e1, e2)` basis. `None` when the ray
    /// is nearly parallel to the plane (edge-on axis) — the caller then holds the last
    /// angle so the fragment doesn't jump.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn dihedral_plane_angle(
        &self,
        _rect: egui::Rect,
        size_px: [u32; 2],
        ndc: (f32, f32),
        axis_point: Vec3,
        u: Vec3,
        e1: Vec3,
        e2: Vec3,
    ) -> Option<f32> {
        let aspect = size_px[0] as f32 / size_px[1].max(1) as f32;
        let (o, dir) = pick::cursor_ray(self.camera.view(), self.camera.proj(aspect), ndc.0, ndc.1);
        let denom = dir.dot(u);
        if denom.abs() < 1e-4 {
            return None;
        }
        let t = (axis_point - o).dot(u) / denom;
        if !t.is_finite() {
            return None;
        }
        let p = o + dir * t;
        let rel = p - axis_point;
        Some(rel.dot(e2).atan2(rel.dot(e1)))
    }

    /// Advance an active handle drag: rotate the grabbed side by the change in the
    /// cursor's plane angle since last frame.
    pub(super) fn dihedral_drag_update(
        &mut self,
        response: &egui::Response,
        rect: egui::Rect,
        size_px: [u32; 2],
        axis_mi: Option<usize>,
    ) {
        let Some(mi) = axis_mi else { return };
        let Some(cursor) = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos())
        else {
            return;
        };
        let ndc = px_to_ndc(cursor, rect);
        let (axis_point, u, e1, e2, side, last) = {
            let Some(d) = self.draw.as_ref().and_then(|d| d.dihedral.drag.as_ref()) else {
                return;
            };
            (d.axis_point, d.axis_dir, d.e1, d.e2, d.side, d.last_angle)
        };
        let Some(now) = self.dihedral_plane_angle(rect, size_px, ndc, axis_point, u, e1, e2) else {
            // Edge-on axis: can't measure the angle this frame. Freeze and drop the
            // reference so the next measurable frame re-baselines (delta 0) rather than
            // applying all the motion accumulated during the edge-on span as one jump.
            if let Some(d) = self.draw.as_mut().and_then(|d| d.dihedral.drag.as_mut()) {
                d.last_angle = None;
            }
            return;
        };
        // Advance the reference to `now`. A delta is only applied when we had a valid
        // reference last frame; otherwise this is the (re-)baseline frame → no rotation.
        if let Some(d) = self.draw.as_mut().and_then(|d| d.dihedral.drag.as_mut()) {
            d.last_angle = Some(now);
        }
        let Some(last) = last else { return };
        let mut delta = now - last;
        while delta > PI {
            delta -= TAU;
        }
        while delta < -PI {
            delta += TAU;
        }
        if delta == 0.0 {
            return;
        }
        // The atoms of the grabbed side.
        let atoms: Vec<usize> = {
            let Some(axis) = self.draw.as_ref().and_then(|d| d.dihedral.axis.as_ref()) else {
                return;
            };
            match side {
                DihedralSide::I => axis.i_side.clone(),
                DihedralSide::J => axis.j_side.clone(),
            }
        };
        self.scene.molecules[mi].rotate_fragment(&atoms, axis_point, u, delta);
        let mol = &mut self.scene.molecules[mi];
        for rep in &mut mol.reps {
            rep.coords_dirty = true;
        }
        if mol.pending.is_some() {
            mol.glow_dirty = true;
        }
        if mol.show_box {
            mol.box_dirty = true;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            mol.pick_dirty = true;
        }
        self.view_dirty = true;
    }

    /// Draw the dihedral overlays: a usage hint, and either the selected axis + its
    /// handles, or a faint preview highlight of the rotatable bond under the cursor.
    pub(super) fn dihedral_overlays(&self, ui: &egui::Ui, rect: egui::Rect, size_px: [u32; 2]) {
        let Some(draw) = self.draw.as_ref() else { return };
        let painter = ui.painter_at(rect);

        let Some(axis) = draw.dihedral.axis.as_ref() else {
            draw_modifier_hint_overlay(
                ui,
                rect,
                icon::ARROW_ARC_RIGHT,
                "Click a rotatable bond",
                AXIS_COLOR,
            );
            self.dihedral_preview_hover(&painter, rect, size_px);
            return;
        };
        let Some(mi) = self.scene.mol_index(axis.mol) else { return };

        draw_modifier_hint_overlay(
            ui,
            rect,
            icon::ARROW_ARC_RIGHT,
            "Drag a handle to rotate",
            AXIS_COLOR,
        );

        // Axis bond highlight (a thick line between the two endpoints).
        if let (Some(a), Some(b)) = (
            self.atom_world(mi, axis.i).and_then(|w| self.world_to_pixel(w, rect, size_px)),
            self.atom_world(mi, axis.j).and_then(|w| self.world_to_pixel(w, rect, size_px)),
        ) {
            painter.line_segment([a, b], egui::Stroke::new(4.0, AXIS_COLOR));
        }

        // Handles: a grip line from the axis endpoint to the neighbour, and a dot.
        let hover_h = ui
            .ctx()
            .pointer_latest_pos()
            .filter(|p| rect.contains(*p))
            .and_then(|px| self.dihedral_hovered_handle(mi, rect, size_px, px));
        let dragged = draw.dihedral.drag.as_ref().map(|dr| dr.handle);
        for (idx, h) in axis.handles.iter().enumerate() {
            let (Some(anchor), Some(hp)) = (
                self.atom_world(mi, h.anchor).and_then(|w| self.world_to_pixel(w, rect, size_px)),
                self.atom_world(mi, h.atom).and_then(|w| self.world_to_pixel(w, rect, size_px)),
            ) else {
                continue;
            };
            let col = side_color(h.side);
            painter.line_segment([anchor, hp], egui::Stroke::new(2.0, col.gamma_multiply(0.55)));
            let active = hover_h == Some(idx) || dragged == Some(idx);
            let r = if active { HANDLE_R + 2.0 } else { HANDLE_R };
            painter.circle_filled(hp, r, col);
            painter.circle_stroke(hp, r, egui::Stroke::new(1.5, egui::Color32::WHITE));
        }
    }

    /// Faintly highlight the rotatable bond under the cursor (before one is selected),
    /// so the user sees what a click would pick. A non-rotatable bond isn't drawn.
    fn dihedral_preview_hover(&self, painter: &egui::Painter, rect: egui::Rect, size_px: [u32; 2]) {
        let Some(px) = painter.ctx().pointer_latest_pos().filter(|p| rect.contains(*p)) else {
            return;
        };
        let Some((mi, k)) = self.dihedral_pick_bond(rect, size_px, px) else {
            return;
        };
        let mol = &self.scene.molecules[mi];
        if build_axis(mol, k).is_none() {
            return; // not rotatable → no preview
        }
        let bond = mol.bonds[k];
        if let (Some(a), Some(b)) = (
            self.atom_world(mi, bond.i1).and_then(|w| self.world_to_pixel(w, rect, size_px)),
            self.atom_world(mi, bond.i2).and_then(|w| self.world_to_pixel(w, rect, size_px)),
        ) {
            painter.line_segment([a, b], egui::Stroke::new(4.0, AXIS_COLOR.gamma_multiply(0.5)));
        }
    }

    /// Headless verification: enter edit mode with the DihedralRotate tool and select the
    /// first rotatable bond of molecule `mi` (so the axis + handles overlay is realistic
    /// for a windowed screenshot). If `rotate_deg` is given, also rotate that bond's
    /// J-side by it — so a `MOLAR_VIS_DEBUG_SAVE_IMAGE` render shows the twisted geometry
    /// without a mouse. Returns the selected `(i, j)` endpoints, or `None`.
    pub(super) fn debug_dihedral_select(
        &mut self,
        mi: usize,
        rotate_deg: Option<f32>,
    ) -> Option<(usize, usize)> {
        self.pick_mode = PickMode::Off;
        let (mol_id, axis) = {
            let mol = self.scene.molecules.get(mi)?;
            let k = (0..mol.bonds.len()).find(|&k| build_axis(mol, k).is_some())?;
            (mol.id, build_axis(mol, k)?)
        };
        let (i, j) = (axis.i, axis.j);
        if let Some(deg) = rotate_deg {
            if let (Some(pi), Some(pj)) = (self.atom_world(mi, i), self.atom_world(mi, j)) {
                let u = (pj - pi).normalize_or_zero();
                let j_side = axis.j_side.clone();
                self.scene.molecules[mi].rotate_fragment(&j_side, pi, u, deg.to_radians());
                let mol = &mut self.scene.molecules[mi];
                for rep in &mut mol.reps {
                    rep.geom_dirty = true;
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    mol.pick_dirty = true;
                }
            }
        }
        self.draw = Some(DrawSession {
            tool: DrawTool::DihedralRotate,
            target: Some(mol_id),
            dihedral: DihedralState { axis: Some(axis), drag: None },
            ..DrawSession::default()
        });
        self.view_dirty = true;
        Some((i, j))
    }
}
