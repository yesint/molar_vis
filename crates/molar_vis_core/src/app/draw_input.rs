//! Draw-mode engine: input gestures, geometry, edits, minimization driver.
use super::*;
use super::draw::*;
use super::overlay::*;


/// Above this atom count, draw-mode edits skip the automatic whole-molecule
/// perception + relax (relax is O(N²) per step). Editor-scale fragments relax/
/// aromatize live; a large loaded structure is edited without those.
pub(super) const DRAW_AUTO_MAX_ATOMS: usize = 300;

/// Length (nm) of a freshly drawn bond before relaxation (a generic single bond).
pub(super) const DRAW_BOND_LEN: f32 = 0.15;
impl App {

    /// Project a viewport pixel onto the active drawing plane → a world point (nm).
    /// The plane passes through `plane_depth` (the last-touched atom, else the camera
    /// target) with the camera view direction as its normal, so freshly placed atoms
    /// land on the focal plane the user is looking at. `None` if the ray is parallel
    /// to the plane (degenerate).
    pub(super) fn drawing_plane_point(&self, px: egui::Pos2, rect: egui::Rect, size_px: [u32; 2]) -> Option<glam::Vec3> {
        let ndc_x = ((px.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0;
        let ndc_y = 1.0 - ((px.y - rect.top()) / rect.height().max(1.0)) * 2.0;
        let aspect = size_px[0] as f32 / size_px[1] as f32;
        let view = self.camera.view();
        let proj = self.camera.proj(aspect);
        let (ro, rd) = pick::cursor_ray(view, proj, ndc_x, ndc_y);
        // Plane normal = the view direction toward the eye (camera-facing).
        let n = (self.camera.eye() - self.camera.target).normalize_or_zero();
        let p0 = self
            .draw
            .as_ref()
            .and_then(|d| d.plane_depth)
            .unwrap_or(self.camera.target);
        let denom = rd.dot(n);
        if denom.abs() < 1.0e-5 {
            return None;
        }
        let t = (p0 - ro).dot(n) / denom;
        if !t.is_finite() {
            return None;
        }
        Some(ro + rd * t)
    }

    /// Pixel → world ray on the current camera (for snap-to-atom hit tests).
    pub(super) fn cursor_world_ray(&self, px: egui::Pos2, rect: egui::Rect, size_px: [u32; 2]) -> (glam::Vec3, glam::Vec3) {
        let ndc_x = ((px.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0;
        let ndc_y = 1.0 - ((px.y - rect.top()) / rect.height().max(1.0)) * 2.0;
        let aspect = size_px[0] as f32 / size_px[1] as f32;
        pick::cursor_ray(self.camera.view(), self.camera.proj(aspect), ndc_x, ndc_y)
    }

    /// Project a world point (nm) to a viewport pixel (for the rubber-band's start).
    pub(super) fn world_to_pixel(&self, world: glam::Vec3, rect: egui::Rect, size_px: [u32; 2]) -> Option<egui::Pos2> {
        let aspect = size_px[0] as f32 / size_px[1] as f32;
        let mvp = self.camera.proj(aspect) * self.camera.view();
        let clip = mvp * world.extend(1.0);
        if clip.w.abs() < 1.0e-6 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some(egui::pos2(
            rect.left() + (ndc.x * 0.5 + 0.5) * rect.width(),
            rect.top() + (1.0 - (ndc.y * 0.5 + 0.5)) * rect.height(),
        ))
    }

    /// World position (nm) of atom `i` of molecule `mi` at the displayed frame.
    pub(super) fn atom_world(&self, mi: usize, i: usize) -> Option<glam::Vec3> {
        let mol = self.scene.molecules.get(mi)?;
        let p = mol.render_state().coords.get(i)?;
        Some(glam::vec3(p.x, p.y, p.z))
    }

    /// Pointer handling for the active drawing tool (atom/bond/erase) + the bond
    /// rubber-band + the debounced minimizer. Called from `draw_viewport` each frame
    /// while Draw mode is on. Alt+LMB orbits (handled by the navigation block), so the
    /// tool only acts when Alt is *not* held.
    pub(super) fn draw_input(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        rect: egui::Rect,
        size_px: [u32; 2],
    ) {
        let alt = ui.input(|i| i.modifiers.alt);

        // Keyboard shortcuts (unless a text field is focused): element + bond order.
        if !ui.ctx().egui_wants_keyboard_input() {
            use egui::Key;
            let el = ui.input(|i| {
                if i.key_pressed(Key::C) { Some(Element::C) }
                else if i.key_pressed(Key::N) { Some(Element::N) }
                else if i.key_pressed(Key::O) { Some(Element::O) }
                else if i.key_pressed(Key::H) { Some(Element::H) }
                else if i.key_pressed(Key::P) { Some(Element::P) }
                else if i.key_pressed(Key::S) { Some(Element::S) }
                else { None }
            });
            let ord = ui.input(|i| {
                if i.key_pressed(Key::Num1) { Some(crate::minimize::BondOrder::Single) }
                else if i.key_pressed(Key::Num2) { Some(crate::minimize::BondOrder::Double) }
                else if i.key_pressed(Key::Num3) { Some(crate::minimize::BondOrder::Triple) }
                else { None }
            });
            if let Some(d) = self.draw.as_mut() {
                if let Some(el) = el {
                    d.element = el;
                    d.tool = DrawTool::Draw;
                }
                if let Some(o) = ord {
                    d.bond_order = o;
                }
            }
        }

        // Snapshot the session's tool params (avoids holding a borrow on `self.draw`
        // while we mutate `self.scene`).
        let (tool, element, bond_order, target) = match self.draw.as_ref() {
            Some(d) => (d.tool, d.element, d.bond_order, d.target),
            None => return,
        };

        // Resolve the target molecule's index (it may have been deleted/reordered).
        let target_mi = target.and_then(|id| self.scene.molecules.iter().position(|m| m.id == id));

        // --- Draw-gesture rubber-band (drawn regardless of which event fired) --
        // Update the live cursor + paint the line from the gesture's start (an atom
        // or an empty-space point) to the cursor.
        if matches!(tool, DrawTool::Draw) && !alt {
            if let Some(pos) = response.interact_pointer_pos().or_else(|| response.hover_pos()) {
                if let Some(d) = self.draw.as_mut() {
                    match &mut d.drag {
                        DrawDrag::FromAtom { current, .. }
                        | DrawDrag::FromEmpty { current, .. } => *current = pos,
                        DrawDrag::Idle => {}
                    }
                }
            }
            // Start point of the rubber band: the source atom's screen position, or
            // the empty-space start pixel.
            let band: Option<(egui::Pos2, egui::Pos2)> = match self.draw.as_ref().map(|d| &d.drag) {
                Some(DrawDrag::FromAtom { from, current }) => target_mi
                    .and_then(|mi| self.atom_world(mi, *from))
                    .and_then(|w| self.world_to_pixel(w, rect, size_px))
                    .map(|s| (s, *current)),
                Some(DrawDrag::FromEmpty { start, current }) => Some((*start, *current)),
                _ => None,
            };
            if let Some((start, current)) = band {
                let painter = ui.painter_at(rect);
                painter.line_segment(
                    [start, current],
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(130, 215, 255)),
                );
                ui.ctx().request_repaint();
            }
        }

        // --- Tool events (suppressed while Alt orbits) -------------------------
        if !alt {
            match tool {
                DrawTool::Draw => self.draw_input_draw(response, rect, size_px, element, bond_order, target_mi),
                DrawTool::Erase => self.draw_input_erase(response, rect, size_px, target_mi),
            }
            if response.clicked() || response.drag_stopped_by(egui::PointerButton::Primary) {
                ui.ctx().request_repaint();
            }
        }

        // --- Debounced minimizer + active relax job ----------------------------
        self.drive_minimize(ui);

        // --- Overlays: hover highlight, aromatic ring circles, "?" on unspecified ---
        self.draw_draw_overlays(ui, rect, size_px);
    }

    /// Unit world direction from `src` toward the cursor, in the screen-parallel
    /// drawing plane (so a new bonded atom grows in the dragged direction). Falls back
    /// to camera-right when degenerate.
    pub(super) fn drag_dir(&self, src: glam::Vec3, cursor: egui::Pos2, rect: egui::Rect, size_px: [u32; 2]) -> glam::Vec3 {
        let aim = self.drawing_plane_point(cursor, rect, size_px).unwrap_or(src);
        let d = (aim - src).normalize_or_zero();
        if d == glam::Vec3::ZERO {
            self.camera.right()
        } else {
            d
        }
    }

    /// The projected screen radius (px) of atom `i` as drawn (its rep's sphere).
    pub(super) fn atom_screen_radius(&self, mi: usize, i: usize, rect: egui::Rect, size_px: [u32; 2]) -> f32 {
        let mol = &self.scene.molecules[mi];
        let (Some(w), Some(atom)) = (self.atom_world(mi, i), mol.system.topology().get_atom(i)) else {
            return 4.0;
        };
        let r_world = mol
            .reps
            .iter()
            .find(|r| r.visible)
            .map_or(0.05, |r| pick::effective_radius(&r.params, atom));
        let right = self.camera.orientation * glam::Vec3::X;
        match (self.world_to_pixel(w, rect, size_px), self.world_to_pixel(w + right * r_world, rect, size_px)) {
            (Some(c), Some(e)) => (e - c).length().max(3.0),
            _ => 4.0,
        }
    }

    /// What the cursor is over: an atom **only when the cursor is inside that atom's
    /// drawn sphere** (front-most), otherwise the nearest bond *outside* the spheres.
    /// Shared by hover-highlight and click so they agree.
    pub(super) fn draw_hit_test(&self, mi: usize, rect: egui::Rect, size_px: [u32; 2], cursor: egui::Pos2) -> Option<HitTarget> {
        let mol = &self.scene.molecules[mi];
        // Front-most atom whose drawn sphere the cursor is inside.
        let (ro, rd) = self.cursor_world_ray(cursor, rect, size_px);
        if let Some((i, _)) = pick::nearest_atom(mol, ro, rd, 0.04) {
            if let Some(c) = self.atom_world(mi, i).and_then(|w| self.world_to_pixel(w, rect, size_px)) {
                if (cursor - c).length() <= self.atom_screen_radius(mi, i, rect, size_px) {
                    return Some(HitTarget::Atom(i));
                }
            }
        }
        // Outside every sphere → the nearest bond, if the cursor is close to its line.
        let aspect = size_px[0] as f32 / size_px[1].max(1) as f32;
        let (view, proj) = (self.camera.view(), self.camera.proj(aspect));
        let ndc = glam::vec2(
            ((cursor.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0,
            1.0 - ((cursor.y - rect.top()) / rect.height().max(1.0)) * 2.0,
        );
        pick::nearest_bond(mol, view, proj, ndc, 0.02).map(HitTarget::Bond)
    }

    /// Painter overlays for the drawing editor: a hover highlight on the atom/bond
    /// under the cursor (shown during a drag too), a gray circle in the plane of each
    /// aromatic ring, and a "?" over each unspecified-order bond that's front-facing
    /// and on-screen (capped, so a big loaded molecule isn't flooded).
    pub(super) fn draw_draw_overlays(&self, ui: &egui::Ui, rect: egui::Rect, size_px: [u32; 2]) {
        let Some(d) = self.draw.as_ref() else { return };
        let Some(mi) = d
            .target
            .and_then(|id| self.scene.molecules.iter().position(|m| m.id == id))
        else {
            return;
        };
        let mol = &self.scene.molecules[mi];
        let painter = ui.painter_at(rect);
        let view = self.camera.view();
        let px_of = |w: glam::Vec3| self.world_to_pixel(w, rect, size_px);
        let eye_z = |w: glam::Vec3| (view * w.extend(1.0)).z; // larger (less negative) = nearer

        // "?" over unspecified bonds — only the front-facing, on-screen ones, capped.
        let center = (mol.bbox_min + mol.bbox_max) * 0.5;
        let center_z = eye_z(center);
        let font = egui::FontId::proportional(13.0);
        let mut shown = 0usize;
        const Q_MAX: usize = 150;
        for b in &mol.bonds {
            if shown >= Q_MAX {
                break;
            }
            if b.order != crate::minimize::BondOrder::Unspecified {
                continue;
            }
            let (Some(p), Some(q)) = (self.atom_world(mi, b.i1), self.atom_world(mi, b.i2)) else {
                continue;
            };
            let mid = (p + q) * 0.5;
            if eye_z(mid) < center_z {
                continue; // back half of the molecule
            }
            if let Some(m) = px_of(mid).filter(|m| rect.contains(*m)) {
                painter.text(m, egui::Align2::CENTER_CENTER, "?", font.clone(), egui::Color32::from_rgb(255, 200, 90));
                shown += 1;
            }
        }

        // (The aromatic-ring circle is drawn as depth-tested 3-D line geometry in the
        // scene — `Molecule::aromatic_gpu` — not as a flat overlay here, so it occludes.)

        // Hover highlight (also during a drag, so you see the atom you'd bond to —
        // `pointer_latest_pos` stays valid while the button is held, unlike hover_pos).
        if let Some(px) = ui.ctx().pointer_latest_pos().filter(|p| rect.contains(*p)) {
            match self.draw_hit_test(mi, rect, size_px, px) {
                Some(HitTarget::Atom(i)) => {
                    if let Some(c) = self.atom_world(mi, i).and_then(px_of) {
                        // Ring sized to the atom's drawn sphere (tracks zoom).
                        let rpx = self.atom_screen_radius(mi, i, rect, size_px);
                        draw_glow_ring(&painter, c, rpx);
                    }
                }
                Some(HitTarget::Bond(k)) => {
                    let b = mol.bonds[k];
                    if let (Some(p), Some(q)) =
                        (self.atom_world(mi, b.i1).and_then(px_of), self.atom_world(mi, b.i2).and_then(px_of))
                    {
                        painter.line_segment(
                            [p, q],
                            egui::Stroke::new(5.0, egui::Color32::from_rgba_unmultiplied(120, 220, 255, 160)),
                        );
                    }
                }
                None => {}
            }
        }
    }

    /// Place an atom of `element` at world `pos` (nm), creating the drawn molecule
    /// from this first atom if none exists yet (molar can't append to a 0-atom
    /// system). Returns `(molecule index, new atom's global index)`.
    pub(super) fn place_atom(&mut self, element: Element, pos: glam::Vec3) -> Option<(usize, usize)> {
        let target = self.draw.as_ref().and_then(|d| d.target);
        let mi = target.and_then(|id| self.scene.molecules.iter().position(|m| m.id == id));
        match mi {
            Some(mi) => {
                let idx = self.scene.molecules[mi].add_atom(&element.make_atom(), pos)?;
                Some((mi, idx))
            }
            None => {
                let raw = match data::RawMolecule::single_atom("drawn", element.make_atom(), pos) {
                    Ok(raw) => raw,
                    Err(e) => {
                        log::error!("draw: {e}");
                        self.status = e;
                        return None;
                    }
                };
                let mut session = self.draw.take().unwrap_or_default();
                session.element = element;
                self.start_drawn_molecule(raw, &mut session);
                let mi = session
                    .target
                    .and_then(|id| self.scene.molecules.iter().position(|m| m.id == id));
                self.draw = Some(session);
                mi.map(|mi| (mi, 0)) // the seed atom is index 0
            }
        }
    }

    /// The unified Draw tool. The gesture decides the action (no separate atom/bond
    /// mode): click empty → place an atom (the first creates the molecule); drag from
    /// an atom → grow a bond (to another atom, or to a new atom on empty space); drag
    /// from empty → two bonded atoms; click a bond → cycle its order; click an atom →
    /// no-op (reserved for element-change).
    pub(super) fn draw_input_draw(
        &mut self,
        response: &egui::Response,
        rect: egui::Rect,
        size_px: [u32; 2],
        element: Element,
        bond_order: crate::minimize::BondOrder,
        target_mi: Option<usize>,
    ) {
        // --- Begin a drag: from an existing atom, or from empty space. ---------
        if response.drag_started_by(egui::PointerButton::Primary) {
            let Some(px) = response.interact_pointer_pos() else { return };
            let from_atom = target_mi.and_then(|mi| {
                let (ro, rd) = self.cursor_world_ray(px, rect, size_px);
                pick::nearest_atom(&self.scene.molecules[mi], ro, rd, 0.08).map(|(i, _)| i)
            });
            // Drag from an existing atom → set the drawing plane to that atom's depth, so
            // a new bonded atom on release lands next to it (not on the far camera-target
            // plane, which produced a stray long bond on big molecules).
            let from_depth = from_atom
                .zip(target_mi)
                .and_then(|(from, mi)| self.atom_world(mi, from));
            if let Some(d) = self.draw.as_mut() {
                if let Some(w) = from_depth {
                    d.plane_depth = Some(w);
                }
                d.drag = match from_atom {
                    Some(from) => DrawDrag::FromAtom { from, current: px },
                    None => DrawDrag::FromEmpty { start: px, current: px },
                };
            }
            return;
        }

        // --- Finish a drag → add a bond / two bonded atoms. --------------------
        if response.drag_stopped_by(egui::PointerButton::Primary) {
            let drag = match self.draw.as_mut() {
                Some(d) => std::mem::replace(&mut d.drag, DrawDrag::Idle),
                None => return,
            };
            let Some(px) = response.interact_pointer_pos() else { return };
            match drag {
                DrawDrag::FromAtom { from, .. } => {
                    let mi = match target_mi {
                        Some(mi) => mi,
                        None => return,
                    };
                    let (ro, rd) = self.cursor_world_ray(px, rect, size_px);
                    let dest =
                        pick::nearest_atom(&self.scene.molecules[mi], ro, rd, 0.08).map(|(i, _)| i);
                    match dest {
                        // Onto another atom → add a bond, or override an existing
                        // bond's order if one already joins them.
                        Some(to) if to != from => {
                            if self.scene.molecules[mi].set_or_add_bond(from, to, bond_order) {
                                self.after_draw_edit(mi, self.atom_world(mi, to));
                            }
                        }
                        Some(_) => {} // same atom → no-op
                        // Onto empty space → a new atom one bond-length from the source
                        // in the dragged direction (fixed length, not wherever the cursor
                        // landed — robust + Marvin-style; the relax refines small molecules).
                        None => {
                            let src = self.atom_world(mi, from).unwrap_or(self.camera.target);
                            let new_pos = src + self.drag_dir(src, px, rect, size_px) * DRAW_BOND_LEN;
                            if let Some((mi, new_idx)) = self.place_atom(element, new_pos) {
                                self.scene.molecules[mi].add_bond(from, new_idx, bond_order);
                                self.after_draw_edit(mi, Some(new_pos));
                            }
                        }
                    }
                }
                DrawDrag::FromEmpty { start, .. } => {
                    // Drag from empty space → two bonded atoms: the first at the start
                    // point, the second one bond-length away in the drag direction. A
                    // negligible drag falls back to a single atom.
                    let Some(p_start) = self.drawing_plane_point(start, rect, size_px) else { return };
                    let Some((mi, a)) = self.place_atom(element, p_start) else { return };
                    if (px - start).length() < 6.0 {
                        self.after_draw_edit(mi, Some(p_start)); // too short → one atom
                        return;
                    }
                    let p_end = p_start + self.drag_dir(p_start, px, rect, size_px) * DRAW_BOND_LEN;
                    if let Some((_, b)) = self.place_atom(element, p_end) {
                        self.scene.molecules[mi].add_bond(a, b, bond_order);
                        self.after_draw_edit(mi, Some(p_end));
                    }
                }
                DrawDrag::Idle => {}
            }
            return;
        }

        // --- A plain click (no drag). ------------------------------------------
        if response.clicked() {
            let Some(px) = response.interact_pointer_pos() else { return };
            // On an atom → replace its element (keeping bonds). On a bond → cycle its
            // order. Else place a new atom. Uses the same closer-wins hit-test as the
            // hover highlight so the click matches what was highlighted.
            if let Some(mi) = target_mi {
                match self.draw_hit_test(mi, rect, size_px, px) {
                    Some(HitTarget::Atom(i)) => {
                        if self.scene.molecules[mi].system.topology().get_atom(i).map(|a| a.atomic_number)
                            != Some(element.atomic_number())
                        {
                            let src = element.make_atom();
                            self.scene.molecules[mi].set_atom_element(i, &src);
                            self.after_draw_edit(mi, self.atom_world(mi, i));
                        }
                        return;
                    }
                    Some(HitTarget::Bond(k)) => {
                        self.scene.molecules[mi].cycle_bond_order(k);
                        self.after_draw_edit(mi, None);
                        return;
                    }
                    None => {}
                }
            }
            // Empty space → place an atom (creating the molecule if needed).
            let pos = self
                .drawing_plane_point(px, rect, size_px)
                .unwrap_or(self.camera.target);
            if let Some((mi, _)) = self.place_atom(element, pos) {
                self.after_draw_edit(mi, Some(pos));
            }
        }
    }

    /// Common post-edit bookkeeping for a Draw-tool change: refresh the molecule's
    /// bbox, flag the rebuild, advance the drawing-plane depth to the last-touched
    /// point, and arm the debounced relax (only worth it once there's something to
    /// relax — more than a lone atom).
    pub(super) fn after_draw_edit(&mut self, mi: usize, plane: Option<glam::Vec3>) {
        let worth_relaxing = {
            let mol = &mut self.scene.molecules[mi];
            mol.refresh_bbox();
            // Auto-perception + auto-relax only for editor-scale molecules: ring
            // perception is cheap but relaxing the whole molecule is O(N²) per step, so
            // editing a large loaded structure (e.g. a protein) must NOT relax/aromatize
            // the whole thing on every click. Above the cap, edits apply instantly; the
            // user can still hand-build a small fragment.
            if mol.n_atoms <= DRAW_AUTO_MAX_ATOMS {
                // Perceive rings + aromaticity: a freshly closed ring with alternating
                // orders becomes aromatic (bonds → Aromatic, rings cached for the overlay).
                mol.perceive_aromaticity();
                mol.n_atoms > 1 || !mol.bonds.is_empty()
            } else {
                false // skip the whole-molecule relax on large structures
            }
        };
        self.flag_edit(mi);
        if let Some(d) = self.draw.as_mut() {
            if let Some(p) = plane {
                d.plane_depth = Some(p);
            }
            if worth_relaxing {
                d.minimize_pending = true;
            }
        }
    }

    /// Toggle explicit hydrogens on the drawn molecule: if it already has any H, remove
    /// them all; otherwise add the implicit-hydrogen count to every heavy atom (placed at
    /// rough offsets and then relaxed). Undoable + perceived like any draw edit.
    pub(super) fn toggle_hydrogens(&mut self, mi: usize) {
        let has_h = {
            let mol = &self.scene.molecules[mi];
            let topo = mol.system.topology();
            (0..mol.n_atoms).any(|i| topo.get_atom(i).is_some_and(|a| a.atomic_number == 1))
        };
        if has_h {
            let mol = &mut self.scene.molecules[mi];
            let topo = mol.system.topology();
            let h_idx: Vec<usize> = (0..mol.n_atoms)
                .filter(|&i| topo.get_atom(i).is_some_and(|a| a.atomic_number == 1))
                .collect();
            // Don't strip an all-hydrogen molecule down to nothing.
            if !h_idx.is_empty() && h_idx.len() < mol.n_atoms {
                mol.remove_atoms(&h_idx);
            }
        } else {
            // Offset directions for placed H (FIRE relaxes them into real geometry); the
            // per-H nudge breaks symmetry so e.g. methane doesn't start in a planar saddle.
            const DIRS: [glam::Vec3; 6] = [
                glam::Vec3::X, glam::Vec3::NEG_X, glam::Vec3::Y,
                glam::Vec3::NEG_Y, glam::Vec3::Z, glam::Vec3::NEG_Z,
            ];
            let mol = &mut self.scene.molecules[mi];
            let counts = mol.implicit_hydrogens();
            let parents: Vec<(usize, glam::Vec3, u8)> = (0..mol.n_atoms)
                .filter_map(|i| {
                    let c = *counts.get(i)?;
                    let p = mol.system.state().coords.get(i)?;
                    (c > 0).then_some((i, glam::vec3(p.x, p.y, p.z), c))
                })
                .collect();
            let h_atom = Element::H.make_atom();
            for (i, p, c) in parents {
                for k in 0..c as usize {
                    let k = k as f32;
                    let dir = (DIRS[(c as usize - 1 + k as usize) % 6]
                        + glam::vec3(0.01 * k, 0.013 * (k + 1.0), 0.017 * k))
                    .normalize_or_zero();
                    if let Some(hi) = mol.add_atom(&h_atom, p + dir * 0.11) {
                        mol.add_bond(i, hi, crate::minimize::BondOrder::Single);
                    }
                }
            }
        }
        self.after_draw_edit(mi, None);
    }

    /// Erase tool: a click deletes the nearest atom (and its bonds; deleting the
    /// molecule if it becomes empty), else the nearest bond.
    pub(super) fn draw_input_erase(
        &mut self,
        response: &egui::Response,
        rect: egui::Rect,
        size_px: [u32; 2],
        target_mi: Option<usize>,
    ) {
        if !response.clicked() {
            return;
        }
        let Some(mi) = target_mi else { return };
        let Some(px) = response.interact_pointer_pos() else { return };
        let (ro, rd) = self.cursor_world_ray(px, rect, size_px);
        if let Some((i, _)) = pick::nearest_atom(&self.scene.molecules[mi], ro, rd, 0.08) {
            let empty = self.scene.molecules[mi].remove_atom(i);
            if empty {
                // The molecule is now empty → delete it (park in trash, undoable).
                let m = self.scene.molecules.remove(mi);
                self.loaders.remove(&m.id);
                #[cfg(target_arch = "wasm32")]
                self.wasm_loaders.remove(&m.id);
                self.scene.trash.insert(m.id, m);
                self.scene.clamp_selection();
                if let Some(d) = self.draw.as_mut() {
                    d.target = None;
                    d.drag = DrawDrag::Idle;
                    d.minimize_pending = false;
                }
                self.view_dirty = true;
            } else {
                self.flag_edit(mi);
                if let Some(d) = self.draw.as_mut() {
                    d.minimize_pending = true;
                }
            }
            return;
        }
        // No atom hit → try a bond.
        let aspect = size_px[0] as f32 / size_px[1] as f32;
        let (view, proj) = (self.camera.view(), self.camera.proj(aspect));
        let ndc = glam::vec2(
            ((px.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0,
            1.0 - ((px.y - rect.top()) / rect.height().max(1.0)) * 2.0,
        );
        if let Some(k) = pick::nearest_bond(&self.scene.molecules[mi], view, proj, ndc, 0.02) {
            self.scene.molecules[mi].remove_bond_at(k);
            self.flag_edit(mi);
            if let Some(d) = self.draw.as_mut() {
                d.minimize_pending = true;
            }
        }
    }

    /// Mark molecule `mi`'s reps dirty after a structural edit (rebuild geometry +
    /// the GPU pick buffer) and flag a re-render.
    pub(super) fn flag_edit(&mut self, mi: usize) {
        if let Some(mol) = self.scene.molecules.get_mut(mi) {
            for rep in &mut mol.reps {
                rep.sel_dirty = true; // the selection set ("all") grows/shrinks
                rep.geom_dirty = true;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                mol.pick_dirty = true;
            }
        }
        self.view_dirty = true;
    }

    /// Debounced minimizer: when an edit is pending and the pointer has settled, run a
    /// Quick relax; an active relax job (Quick or Cleanup) runs one `relax_in_system`
    /// call then clears. Repaints only while a relax job is active (idle = 0 GPU).
    pub(super) fn drive_minimize(&mut self, ui: &mut egui::Ui) {
        // Promote a pending edit to a relax job once the pointer has settled — the same
        // predicate the history checkpoint uses, so we never relax mid-drag.
        let settled = !ui.ctx().egui_is_using_pointer();
        let (start_quick, target) = match self.draw.as_ref() {
            Some(d) => (
                d.minimize_pending && settled && d.relax.is_none(),
                d.target,
            ),
            None => (false, None),
        };
        if start_quick {
            if let Some(d) = self.draw.as_mut() {
                d.relax = Some(RelaxJob { remaining: 1, to_convergence: false });
                d.minimize_pending = false;
            }
        }

        // Run an active relax job (one-shot: a single relax call, then done).
        let kind = match self.draw.as_ref().and_then(|d| d.relax.as_ref()) {
            Some(job) if job.to_convergence => Some(crate::minimize::RelaxKind::Cleanup),
            Some(_) => Some(crate::minimize::RelaxKind::Quick),
            None => None,
        };
        if let Some(kind) = kind {
            if let Some(mi) = target.and_then(|id| self.scene.molecules.iter().position(|m| m.id == id)) {
                let mol = &mut self.scene.molecules[mi];
                // Relaxing is O(N²) per step (all-pairs vdW) — never auto-relax a large
                // loaded structure; clear the job and leave coordinates as drawn.
                if mol.n_atoms > DRAW_AUTO_MAX_ATOMS {
                    if let Some(d) = self.draw.as_mut() {
                        d.relax = None;
                    }
                    return;
                }
                let _ = crate::minimize::relax_in_system(
                    &mut mol.system,
                    &mol.bonds,
                    kind,
                );
                mol.refresh_bbox();
                // Coordinates moved → in-place GPU coord update (no rebuild) + glow +
                // aromatic circles follow.
                for rep in &mut mol.reps {
                    rep.coords_dirty = true;
                }
                if !mol.aromatic_rings.is_empty() {
                    mol.aromatic_dirty = true;
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    mol.pick_dirty = true;
                }
                if mol.pending.is_some() {
                    mol.glow_dirty = true;
                }
                self.view_dirty = true;
            }
            // One-shot: clear the job.
            if let Some(d) = self.draw.as_mut() {
                d.relax = None;
            }
            // Keep repainting while relaxing (a Cleanup ran in one frame, but request
            // one more paint so the moved coords show immediately).
            ui.ctx().request_repaint();
        }
    }
}
