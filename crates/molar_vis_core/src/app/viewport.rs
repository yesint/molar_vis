//! Central 3D viewport: mouse nav, render, hover/lasso picking, pending selection.
use super::*;
use super::overlay::*;

/// Tile-submits to issue per frame while pumping a trace. Keeps each frame's GPU work bounded
/// (so the UI stays responsive) while the trace refines progressively over several frames.
const RT_STEP_SUBMITS: u32 = 4;

impl App {

    /// Central panel: rebuild dirty geometry, route VMD-style mouse navigation,
    /// re-render the 3D scene only when needed, and blit it as an egui image.
    pub(super) fn draw_viewport(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            let render_state = frame
                .wgpu_render_state()
                .expect("wgpu render state must exist");

            let geom_changed = self.rebuild_dirty(render_state);

            // Claim the whole central area as a draggable, scrollable surface.
            let available = ui.available_size();
            let (rect, response) =
                ui.allocate_exact_size(available, egui::Sense::click_and_drag());

            // VMD-style navigation:
            //   LMB = free 3D rotate · Shift+LMB = roll (screen-plane rotate)
            //   RMB = pan           · Shift+RMB = move along view Z (dolly)
            //   middle = pan        · wheel = scale (zoom)
            // In Lasso pick mode an LMB drag draws the selection polygon (the held
            // modifier picks the set op on release: plain = replace, Shift = add,
            // Ctrl = subtract) — except **Alt+LMB**, which orbits so the view can be
            // rotated without leaving Lasso mode.
            let lasso_mode = self.pick_mode == PickMode::Lasso;
            if !lasso_mode {
                self.lasso_path.clear();
            }
            // Draw mode: plain LMB is the active drawing tool (handled in `draw_input`);
            // Alt+LMB orbits, so the view can be rotated without leaving Draw. RMB/MMB/
            // wheel navigate as usual.
            let draw_mode = self.draw.is_some();
            let delta = response.drag_delta();
            let mods = ui.input(|i| i.modifiers);
            let (shift, alt) = (mods.shift, mods.alt);
            // Alt+drag in Lasso/Draw mode orbits; otherwise an LMB lasso draws the
            // polygon (lasso), or the drawing tool acts (draw — handled separately).
            let lasso_draw = lasso_mode && !alt;
            // A plain LMB in Draw mode does not orbit (the tool handles it).
            let draw_tool_drag = draw_mode && !alt;
            if response.dragged_by(egui::PointerButton::Primary) {
                if lasso_draw {
                    if let Some(pos) = response.interact_pointer_pos() {
                        // Drop near-duplicate points (drag jitter) for a clean polygon.
                        if self.lasso_path.last().is_none_or(|&p| (p - pos).length() > 1.5) {
                            self.lasso_path.push(pos);
                        }
                    }
                } else if draw_tool_drag {
                    // Drawing tool drag (e.g. Bond rubber-band) — see `draw_input`; no
                    // camera motion here.
                } else if shift && !lasso_mode && !draw_mode {
                    self.camera.roll(delta.x, self.settings.behavior.roll_sensitivity);
                } else {
                    // Non-lasso/non-draw LMB, or Alt+LMB in lasso/draw mode → free orbit.
                    self.camera
                        .orbit(delta.x, delta.y, self.settings.behavior.orbit_sensitivity);
                }
            } else if response.dragged_by(egui::PointerButton::Secondary) {
                if shift {
                    self.camera.zoom_drag(delta.y);
                } else {
                    self.camera.pan(delta.x, delta.y, rect.height());
                }
            } else if response.dragged_by(egui::PointerButton::Middle) {
                self.camera.pan(delta.x, delta.y, rect.height());
            }
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    // Zoom toward the cursor: pass its NDC position (y up) + aspect.
                    let ndc = response
                        .hover_pos()
                        .map(|p| {
                            glam::vec2(
                                ((p.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0,
                                1.0 - ((p.y - rect.top()) / rect.height().max(1.0)) * 2.0,
                            )
                        })
                        .unwrap_or(glam::Vec2::ZERO);
                    let aspect = rect.width() / rect.height().max(1.0);
                    self.camera.zoom_scroll(scroll, ndc, aspect);
                }
            }

            let ppp = ui.ctx().pixels_per_point();
            let size_px = [
                ((rect.width() * ppp).round() as u32).max(1),
                ((rect.height() * ppp).round() as u32).max(1),
            ];

            // A ray trace is requested (warming), running, or being held → suppress the
            // pulsing selection glow: it isn't part of the trace (the gather ignores it), and
            // the still that's shown has no glow, so animating/forcing-a-redraw for it would
            // only stop the still from ever settling. The realtime raster drawn *behind* the
            // trace is rendered with the glow hidden (`glow_pulse = 0`) so it doesn't flash.
            // Only a viewport **still** (R key) replaces the live view, so only it hides the
            // glow; a Save renders offscreen, leaving the live view (and its glow) interactive.
            let tracing = matches!(self.rt_warm, Some(RtKind::Still))
                || matches!(self.rt_job, Some(RtJob::Still))
                || self.rt_still;
            let has_glow = self
                .scene
                .molecules
                .iter()
                .any(|m| m.visible && (m.pending.is_some() || m.hover.is_some()));
            let pulsing = has_glow && !tracing;
            let glow_pulse = if tracing {
                0.0
            } else if pulsing {
                let t = ui.input(|i| i.time) as f32;
                0.70 + 0.30 * (t * 3.2).sin()
            } else {
                1.0
            };
            if pulsing {
                ui.ctx().request_repaint();
            }

            let cam_changed = self.last_render_camera != Some(self.camera);
            let size_changed = size_px != self.last_size;
            let raster_dirty = geom_changed || cam_changed || size_changed || self.view_dirty || pulsing;

            if geom_changed {
                self.rt_scene_dirty = true; // re-gather the tracer's scene before the next trace
            }

            // Ray-traced still (PyMOL-`ray` style): pressing **R** ray-traces the current view
            // and holds it until the camera/scene/size changes. No automatic tracing on idle.
            // Allowed even with a pending/hover selection (the glow just isn't traced/shown);
            // ignored while a text field has focus (so typing "r" in a selection doesn't fire).
            let rt_ok = self.renderer.raytrace_supported()
                && !self.scene.molecules.is_empty()
                && self.draw.is_none();
            if rt_ok
                && self.rt_warm.is_none()
                && self.rt_job.is_none()
                && !ui.ctx().egui_wants_keyboard_input()
                && ui.input(|i| i.key_pressed(egui::Key::R))
            {
                self.rt_warm = Some(RtKind::Still); // start deferred (overlay shows first)
                self.rt_warm_shown = false;
                self.rt_still = false;
            }
            // Any camera/scene/size change drops a showing still and aborts an in-progress /
            // pending **Still** back to the realtime view. (A Save renders offscreen at its own
            // captured view, so it keeps going.)
            if raster_dirty {
                self.rt_still = false;
                if matches!(self.rt_warm, Some(RtKind::Still)) {
                    self.rt_warm = None;
                }
                if matches!(self.rt_job, Some(RtJob::Still)) {
                    self.renderer.rt_trace_cancel();
                    self.rt_job = None;
                }
            }

            // Deferred trace start: show the "Ray tracing…/Saving…" overlay for one frame, THEN
            // do the (possibly blocking) scene gather + trace begin — so the overlay appears
            // immediately instead of only after the gather finishes. `force_raster` re-renders
            // the behind-view this frame so its glow is hidden (`glow_pulse = 0`, set above).
            let mut force_raster = false;
            if self.rt_warm.is_some() {
                if self.rt_warm_shown {
                    if self.rt_scene_dirty {
                        let dashed = self.settings.behavior.dashed_pbc_bonds;
                        self.renderer.prepare_raytrace(render_state, &self.scene, dashed);
                        self.rt_scene_dirty = false;
                    }
                    let samples = self.camera.rt_sample_target();
                    match self.rt_warm.take().unwrap() {
                        RtKind::Still => {
                            self.renderer.rt_still_begin(render_state, &self.camera, size_px, samples);
                            self.rt_job = Some(RtJob::Still);
                        }
                        RtKind::Save { scale, path } => {
                            let [vw, vh] = self.last_size;
                            let out = [vw.max(1) * scale.max(1), vh.max(1) * scale.max(1)];
                            if self.renderer.save_begin(render_state, &self.camera, out[0], out[1], samples) {
                                self.rt_job = Some(RtJob::Save { out, path, reading: None });
                            }
                        }
                    }
                } else {
                    self.rt_warm_shown = true;
                    force_raster = true;
                    ui.ctx().request_repaint();
                }
            }

            // Whether to paint the ray-traced still (vs the rasterized target) this frame.
            let mut paint_rt = false;
            if raster_dirty || force_raster {
                // Camera / scene / size changed (or animating, or warming a trace): realtime
                // raster. `glow_pulse` is 0 while tracing, so no glow shows behind the trace.
                let aspect = size_px[0] as f32 / size_px[1] as f32;
                let view = self.camera.view();
                let proj = self.camera.proj(aspect);
                self.renderer.render_scene(
                    render_state,
                    size_px,
                    view,
                    proj,
                    self.camera.is_perspective(),
                    self.camera.cue_uniform(),
                    self.camera.ao_uniform(),
                    self.camera.shadow_uniform(),
                    self.camera.background,
                    self.camera.eye_depth_range(),
                    glow_pulse,
                    &self.scene,
                );
                self.last_render_camera = Some(self.camera);
                self.last_size = size_px;
            }
            if matches!(self.rt_job, Some(RtJob::Still)) {
                // Pump the still trace a few tiles per frame (responsive + progressive). Paint
                // it once the first sample-chunk has landed; keep repainting until converged,
                // then hold the finished still.
                let done = self.renderer.rt_still_step(render_state, RT_STEP_SUBMITS);
                paint_rt = self.renderer.raytrace_samples() > 0;
                if done {
                    self.rt_job = None;
                    self.rt_still = true;
                } else {
                    ui.ctx().request_repaint();
                }
            } else if self.rt_still {
                // Holding the finished still (view unchanged): keep painting it, no re-trace.
                paint_rt = true;
            }
            self.view_dirty = false;

            // Paint the ray-traced output while it's showing, else the rasterized scene.
            let texture_id = match (paint_rt, self.renderer.rt_texture_id()) {
                (true, Some(id)) => id,
                _ => self.renderer.texture_id(),
            };
            egui::Image::new(egui::load::SizedTexture::new(texture_id, rect.size()))
                .paint_at(ui, rect);

            // Draw mode: handle the active tool (atom/bond/erase) for this frame's
            // pointer events, draw the bond rubber-band, and drive the debounced
            // minimizer. Plain LMB acts as the tool; Alt+LMB orbits (handled above).
            if self.draw.is_some() {
                self.draw_input(ui, &response, rect, size_px);
            }

            // Hover-info picking. On native the hit comes from the **async GPU
            // id-buffer** (collect a finished readback, request the next); on wasm it's
            // the CPU ray-cast. **Atoms** mode → ring + info box on the hit atom;
            // **Residues** mode → the whole residue staged as a steady glow
            // (`Molecule::hover`) + a residue box. Skipped while dragging the camera.
            //
            // Drain a finished async pick first — every frame, even when not hovering —
            // so the readback frees up (else `request_pick` would stay blocked). When
            // a result lands it updates the cached `hover_pick` ids.
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(ids) = self.renderer.poll_pick(render_state) {
                if std::env::var("MOLAR_VIS_DEBUG_PICK").is_ok() {
                    // DEBUG forces a center pick; compare to the CPU ray-cast there.
                    let aspect = size_px[0] as f32 / size_px[1] as f32;
                    let (view, proj) = (self.camera.view(), self.camera.proj(aspect));
                    let c = pick::pick(&self.scene, view, proj, 0.0, 0.0).map(|h| (h.mol, h.id));
                    let g = ids.map(|(m, _, a)| (m, a));
                    if g == c {
                        log::info!("pick ok: gpu == cpu == {g:?}");
                    } else {
                        log::warn!("pick mismatch: gpu {g:?} != cpu {c:?}");
                    }
                }
                self.hover_pick = ids;
            }

            let hovering = self.pick_mode == PickMode::Click && !response.dragged();
            let mut residue_hit = false;
            let mut lens_shown = false;
            if hovering {
                // Normally the cursor; the debug hook forces the viewport center so
                // the overlay can be screenshot without simulating a mouse.
                let ndc = if std::env::var("MOLAR_VIS_DEBUG_PICK").is_ok() {
                    Some((0.0, 0.0))
                } else {
                    response.hover_pos().map(|p| {
                        (
                            ((p.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0,
                            1.0 - ((p.y - rect.top()) / rect.height().max(1.0)) * 2.0,
                        )
                    })
                };
                if let Some((ndc_x, ndc_y)) = ndc {
                    let aspect = size_px[0] as f32 / size_px[1] as f32;
                    // GPU id-buffer pick on native (async O(1) readback, scales to huge
                    // systems); the CPU ray-cast stays the path on wasm (WebGL2 can't
                    // render/read back the integer id target).
                    #[cfg(not(target_arch = "wasm32"))]
                    let hit = {
                        // Cursor → pick-target pixel (the pick buffer is 1× = size_px).
                        let px = (((ndc_x * 0.5 + 0.5) * size_px[0] as f32) as i32)
                            .clamp(0, size_px[0] as i32 - 1) as u32;
                        let py = (((1.0 - (ndc_y * 0.5 + 0.5)) * size_px[1] as f32) as i32)
                            .clamp(0, size_px[1] as i32 - 1) as u32;
                        // Re-pick only when the cursor moved or the view changed (else a
                        // stationary hover would spin the GPU every frame).
                        let view_moved = geom_changed || cam_changed || size_changed;
                        if view_moved || self.last_pick_px != Some((px, py)) {
                            self.renderer.request_pick(render_state, &self.scene, px, py, size_px);
                            self.last_pick_px = Some((px, py));
                        }
                        if self.renderer.pick_in_flight() {
                            ui.ctx().request_repaint(); // keep polling until it lands
                        }
                        // Rebuild the PickHit from the cached ids each frame (keeps the
                        // displayed position current as coords change). Lags 1–2 frames.
                        self.hover_pick
                            .and_then(|(m, r, a)| pick::hit_for_atom(&self.scene, m, r, a))
                    };
                    #[cfg(target_arch = "wasm32")]
                    let hit = {
                        let view = self.camera.view();
                        let proj = self.camera.proj(aspect);
                        pick::pick(&self.scene, view, proj, ndc_x, ndc_y)
                    };
                    if let Some(hit) = hit {
                        // Click selects the hovered atom/residue: merge it into the
                        // active (pending) selection (Shift = add, Ctrl/⌘ = subtract,
                        // plain = replace), expanded per the scope mode. A plain LMB
                        // *drag* still orbits (handled above), so only a real click here.
                        if response.clicked() {
                            let m = ui.input(|i| i.modifiers);
                            let op = if m.shift {
                                LassoOp::Add
                            } else if m.command {
                                LassoOp::Subtract
                            } else {
                                LassoOp::Replace
                            };
                            if op == LassoOp::Replace {
                                for mol in &mut self.scene.molecules {
                                    if mol.pending.take().is_some() {
                                        mol.glow_dirty = true;
                                    }
                                }
                            }
                            let mode = self.effective_selection_mode();
                            let hits = {
                                let mol = &self.scene.molecules[hit.mol];
                                pick::expand_selection(&mol.data, &mol.bonds, &[hit.id], mode)
                            };
                            self.merge_into_pending(hit.mol, hits, op);
                            self.view_dirty = true;
                            ui.ctx().request_repaint();
                        }
                        if self.effective_selection_mode() == SelectionMode::Residues {
                            // Expand the hit to its whole residue → steady glow.
                            let atoms = {
                                let mol = &self.scene.molecules[hit.mol];
                                pick::expand_selection(
                                    &mol.data,
                                    &mol.bonds,
                                    &[hit.id],
                                    SelectionMode::Residues,
                                )
                            };
                            draw_residue_info_overlay(ui, rect, &hit, atoms.len());
                            if self.set_hover(hit.mol, atoms) {
                                ui.ctx().request_repaint();
                            }
                            residue_hit = true;
                        } else {
                            // Atoms mode: egui ring + atom info box.
                            draw_pick_overlay(ui, rect, &self.camera, aspect, &hit);
                        }
                    }
                    // Hover detail lens: **independent of any atom hit** — wherever the
                    // cursor view-line passes near a Cartoon/Surface molecule's atoms
                    // (including *between* atoms / in ribbon gaps, which is the whole
                    // point: hint where the atoms are), reveal a faded ball-and-stick of
                    // those atoms. Rebuilt as the cursor moves (grid query is cheap).
                    // Off by default (`hover_detail_lens` behavior setting); when off,
                    // `lens_shown` stays false and any stale lens is cleared below.
                    if self.settings.behavior.hover_detail_lens {
                        let moved = self.last_lens_ndc.is_none_or(|(lx, ly)| {
                            (lx - ndc_x).abs() > 0.004 || (ly - ndc_y).abs() > 0.004
                        });
                        if moved {
                            const R: f32 = 0.35; // lens radius (nm)
                            let view = self.camera.view();
                            let proj = self.camera.proj(aspect);
                            let (o, d) = pick::cursor_ray(view, proj, ndc_x, ndc_y);
                            let t_max = 2.0 * (self.camera.distance + self.camera.scene_radius);
                            // The Cartoon/Surface molecule with the most atoms in the
                            // view-line tube (one lens at a time).
                            let mut best: Option<(usize, Vec<usize>)> = None;
                            for mi in 0..self.scene.molecules.len() {
                                let mol = &mut self.scene.molecules[mi];
                                let wants = mol.visible
                                    && mol.reps.iter().any(|r| {
                                        r.visible
                                            && matches!(r.kind, RepKind::Cartoon | RepKind::Surface)
                                    });
                                if !wants {
                                    continue;
                                }
                                if mol.hover_grid.is_none() {
                                    // The grid holds the lens **seed** atoms — which
                                    // residues the view line passes near: for Cartoon the
                                    // **backbone** (what the ribbon traces); for Surface the
                                    // **solvent-exposed** atoms (per-atom SASA > 0), not
                                    // deep-buried ones. The query then keeps the near,
                                    // camera-facing seeds and expands them to whole residues.
                                    let has_cartoon = mol.reps.iter().any(|r| {
                                        r.visible && matches!(r.kind, RepKind::Cartoon)
                                    });
                                    let has_surface = mol.reps.iter().any(|r| {
                                        r.visible && matches!(r.kind, RepKind::Surface)
                                    });
                                    let grid = {
                                        let st = mol.render_state();
                                        let all = mol.data.select_all();
                                        let b = mol.data.bind_with_state(&all, st);
                                        let sasa = if has_surface {
                                            b.sasa().ok().map(|s| s.areas().to_vec())
                                        } else {
                                            None
                                        };
                                        let pts = b.iter_particle().filter_map(|p| {
                                            // Cartoon → the N–CA–C chain trace only (no
                                            // carbonyl / terminal backbone oxygens).
                                            let keep = (has_cartoon
                                                && matches!(p.atom.name.as_str(), "N" | "CA" | "C"))
                                                || (has_surface
                                                    && sasa.as_ref().is_some_and(|a| {
                                                        a.get(p.id).copied().unwrap_or(0.0) > 0.01
                                                    }));
                                            keep.then_some((
                                                p.id as u32,
                                                glam::Vec3::new(p.pos.x, p.pos.y, p.pos.z),
                                            ))
                                        });
                                        crate::spatial::AtomGrid::build(
                                            pts,
                                            mol.bbox_min,
                                            mol.bbox_max,
                                            R,
                                        )
                                    };
                                    mol.hover_grid = Some(grid);
                                }
                                // Show the **front-facing residues** under the view line
                                // (both Cartoon and Surface). The grid seeds mark which
                                // residues the line passes near — the ribbon backbone for
                                // Cartoon, the solvent-exposed atoms for Surface; keep only
                                // the seeds on the near (camera-facing) half along the ray
                                // (so the far side no longer bleeds through the
                                // cleared-depth overlay), then expand each to its whole
                                // residue so complete residues poke through.
                                let grid = mol.hover_grid.as_ref().unwrap();
                                let cand = grid.atoms_near_ray_t(o, d, R, 0.0, t_max);
                                let atoms: Vec<usize> = if cand.is_empty() {
                                    Vec::new()
                                } else {
                                    let (mut t_near, mut t_far) = (f32::INFINITY, f32::NEG_INFINITY);
                                    for &(_, t) in &cand {
                                        t_near = t_near.min(t);
                                        t_far = t_far.max(t);
                                    }
                                    let mid = 0.5 * (t_near + t_far);
                                    let seeds: Vec<usize> = cand
                                        .iter()
                                        .filter(|&&(_, t)| t <= mid)
                                        .map(|&(id, _)| id as usize)
                                        .collect();
                                    pick::expand_selection(
                                        &mol.data,
                                        &mol.bonds,
                                        &seeds,
                                        SelectionMode::Residues,
                                    )
                                };
                                if !atoms.is_empty()
                                    && best.as_ref().is_none_or(|(_, a)| atoms.len() > a.len())
                                {
                                    best = Some((mi, atoms));
                                }
                            }
                            if let Some((mi, atoms)) = best {
                                // The lens now shows whole residues (≈0.8 nm) for both
                                // Cartoon and Surface, so widen the perpendicular fade past
                                // the R-tube selection radius or residue side chains would
                                // fade out.
                                self.set_hover_detail(mi, atoms, o, d, R * 1.8);
                                lens_shown = true;
                            }
                            self.last_lens_ndc = Some((ndc_x, ndc_y));
                            ui.ctx().request_repaint();
                        } else {
                            // Cursor barely moved: keep the current lens (if any).
                            lens_shown =
                                self.scene.molecules.iter().any(|m| m.hover_detail.is_some());
                        }
                    }
                } else {
                    // Cursor left the viewport → drop the cached GPU hit.
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        self.hover_pick = None;
                        self.last_pick_px = None;
                    }
                }
            } else {
                // Not in hover mode (or dragging) → drop the cached GPU hit.
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.hover_pick = None;
                    self.last_pick_px = None;
                }
            }
            // Drop any stale hover highlight (left a residue, no hit, or not hovering).
            if !residue_hit && self.clear_hover() {
                ui.ctx().request_repaint();
            }
            // Drop the detail lens when the view-line isn't near a Cartoon/Surface
            // molecule's atoms (or not hovering).
            if !lens_shown && self.clear_hover_detail() {
                ui.ctx().request_repaint();
            }

            // The active (pending) selection is highlighted by a GPU glow pass
            // (`render_scene` pass 4), so there's nothing to draw here.

            // Lasso selection: draw the in-progress polygon, and on release stage
            // the enclosed (style-eligible) atoms as the active selection.
            if lasso_mode && self.lasso_path.len() >= 2 {
                let painter = ui.painter_at(rect);
                let col = egui::Color32::from_rgb(130, 215, 255);
                painter.add(egui::Shape::line(
                    self.lasso_path.clone(),
                    egui::Stroke::new(1.5, col),
                ));
                // Faint segment closing the loop back to the start.
                if let (Some(&first), Some(&last)) =
                    (self.lasso_path.first(), self.lasso_path.last())
                {
                    painter.line_segment(
                        [last, first],
                        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(130, 215, 255, 110)),
                    );
                }
            }
            if lasso_mode
                && !self.lasso_path.is_empty()
                && response.drag_stopped_by(egui::PointerButton::Primary)
            {
                // The modifier held at release picks the set operation: Shift adds to
                // the active selection, Ctrl (⌘ on mac) subtracts, plain replaces.
                let m = ui.input(|i| i.modifiers);
                let op = if m.shift {
                    LassoOp::Add
                } else if m.command {
                    LassoOp::Subtract
                } else {
                    LassoOp::Replace
                };
                self.finish_lasso(rect, size_px, op);
            }

            // VMD-style orientation axes gizmo in the chosen corner (a gizmo painted
            // onto the 3D image; its on/off + corner live in the top view toolbar).
            if self.axes_on {
                draw_axes_overlay(ui, rect, &self.camera, self.axes_corner);
            }

            // Selection-modifier hint, floating over the 3D image (top-center) while a
            // modifier is held in Click/Lasso mode — an overlay, not a toolbar row, so
            // it never resizes the viewport. Matches `finish_lasso`'s `LassoOp` + the
            // click-to-select path.
            if matches!(self.pick_mode, PickMode::Click | PickMode::Lasso) {
                let m = ui.input(|i| i.modifiers);
                let hint = if m.alt && self.pick_mode == PickMode::Lasso {
                    Some((icon::ARROWS_CLOCKWISE, "rotate view", egui::Color32::from_rgb(150, 190, 230)))
                } else if m.shift {
                    Some((icon::PLUS_CIRCLE, "add to selection", egui::Color32::from_rgb(120, 220, 120)))
                } else if m.command {
                    Some((icon::MINUS_CIRCLE, "subtract from selection", egui::Color32::from_rgb(230, 140, 140)))
                } else {
                    None
                };
                if let Some((glyph, text, color)) = hint {
                    draw_modifier_hint_overlay(ui, rect, glyph, text, color);
                }
            }

            // Progress hint while a trace is requested (warming) or running (the R-key still or
            // a Save image), so the user sees it's working — shown from the very first frame.
            let rt_hint = match (&self.rt_warm, &self.rt_job) {
                (Some(RtKind::Still), _) | (_, Some(RtJob::Still)) => Some("Ray tracing…"),
                (Some(RtKind::Save { .. }), _) | (_, Some(RtJob::Save { .. })) => Some("Saving…"),
                _ => None,
            };
            if let Some(text) = rt_hint {
                draw_modifier_hint_overlay(
                    ui,
                    rect,
                    icon::CUBE_TRANSPARENT,
                    text,
                    egui::Color32::from_rgb(150, 190, 230),
                );
            }
        });
    }

    /// The selection mode in effect for the current pick mode. `Bound H` is
    /// meaningless for single-atom hover picking, so it falls back to `Atoms` there
    /// (and the toolbar hides it); it stays available for the lasso.
    pub(super) fn effective_selection_mode(&self) -> SelectionMode {
        if self.pick_mode == PickMode::Click && self.selection_mode == SelectionMode::BoundH {
            SelectionMode::Atoms
        } else {
            self.selection_mode
        }
    }

    /// Set molecule `mi`'s steady hover highlight to `atoms`, clearing every other
    /// molecule's. Returns whether anything changed (so the caller can request a
    /// repaint to rebuild the glow).
    pub(super) fn set_hover(&mut self, mi: usize, atoms: Vec<usize>) -> bool {
        let mut changed = false;
        for (i, mol) in self.scene.molecules.iter_mut().enumerate() {
            if i != mi && mol.hover.take().is_some() {
                mol.hover_dirty = true;
                changed = true;
            }
        }
        if let Some(mol) = self.scene.molecules.get_mut(mi) {
            if mol.hover.as_deref() != Some(atoms.as_slice()) {
                mol.hover = Some(atoms);
                mol.hover_dirty = true;
                changed = true;
            }
        }
        changed
    }

    /// Clear every molecule's hover highlight. Returns whether anything changed.
    pub(super) fn clear_hover(&mut self) -> bool {
        let mut changed = false;
        for mol in &mut self.scene.molecules {
            if mol.hover.take().is_some() {
                mol.hover_dirty = true;
                changed = true;
            }
        }
        changed
    }

    /// Stage molecule `mi`'s hover detail lens (always rebuilt — the fade tracks the
    /// ray, so any cursor move changes it). Clears the lens on other molecules.
    pub(super) fn set_hover_detail(
        &mut self,
        mi: usize,
        atoms: Vec<usize>,
        ray_o: glam::Vec3,
        ray_d: glam::Vec3,
        radius: f32,
    ) {
        for (i, mol) in self.scene.molecules.iter_mut().enumerate() {
            if i != mi && mol.hover_detail.take().is_some() {
                mol.hover_detail_dirty = true;
            }
        }
        if let Some(mol) = self.scene.molecules.get_mut(mi) {
            mol.hover_detail = Some(crate::scene::HoverDetail { atoms, ray_o, ray_d, radius });
            mol.hover_detail_dirty = true;
        }
    }

    /// Clear every molecule's hover detail lens. Returns whether anything changed.
    pub(super) fn clear_hover_detail(&mut self) -> bool {
        let mut changed = false;
        for mol in &mut self.scene.molecules {
            if mol.hover_detail.take().is_some() {
                mol.hover_detail_dirty = true;
                changed = true;
            }
        }
        self.last_lens_ndc = None;
        changed
    }

    /// Finish a lasso gesture: convert the screen-space path to a clip-space
    /// polygon, collect the enclosed atoms (per `pick::lasso_select`, honoring the
    /// per-rep style logic), and combine them with each molecule's **active (pending)
    /// selection** per `op` — a glowing highlight with a minimal accept/discard UI,
    /// not yet a real representation. Staging is not undoable; accepting it (→ a
    /// Ball-and-Stick rep) is.
    pub(super) fn finish_lasso(&mut self, rect: egui::Rect, size_px: [u32; 2], op: LassoOp) {
        let path = std::mem::take(&mut self.lasso_path);
        if path.len() < 3 {
            return;
        }
        // Screen px → clip-space NDC (y up), matching `pick`'s convention.
        let polygon: Vec<glam::Vec2> = path
            .iter()
            .map(|p| {
                glam::Vec2::new(
                    ((p.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0,
                    1.0 - ((p.y - rect.top()) / rect.height().max(1.0)) * 2.0,
                )
            })
            .collect();
        let aspect = size_px[0] as f32 / size_px[1] as f32;
        let view = self.camera.view();
        let proj = self.camera.proj(aspect);
        let results = pick::lasso_select(&self.scene, view, proj, &polygon);

        // Replace clears the previous active selection everywhere first; Add/Subtract
        // merge into the existing per-molecule pending set.
        if op == LassoOp::Replace {
            for mol in &mut self.scene.molecules {
                if mol.pending.take().is_some() {
                    mol.glow_dirty = true;
                }
            }
        }
        let mode = self.selection_mode;
        for res in results {
            // Expand this gesture's raw hits per the selection mode (exact atoms /
            // whole residues / heavy + bonded H), then merge into the pending set.
            let hits = {
                let mol = &self.scene.molecules[res.mol];
                pick::expand_selection(&mol.data, &mol.bonds, &res.atoms, mode)
            };
            self.merge_into_pending(res.mol, hits, op);
        }
        self.view_dirty = true;
    }

    /// Combine `hits` into molecule `mi`'s **active (pending) selection** per `op`
    /// (Replace/Add union the atoms in, Subtract removes them) and flag its glow.
    /// Shared by the lasso and click-to-select paths; clearing *other* molecules'
    /// pending sets for a `Replace` is the caller's job.
    pub(super) fn merge_into_pending(&mut self, mi: usize, hits: Vec<usize>, op: LassoOp) {
        let mol = &mut self.scene.molecules[mi];
        let mut set: std::collections::BTreeSet<usize> = mol
            .pending
            .as_ref()
            .map(|p| p.atoms.iter().copied().collect())
            .unwrap_or_default();
        match op {
            LassoOp::Replace | LassoOp::Add => set.extend(hits),
            LassoOp::Subtract => {
                for a in &hits {
                    set.remove(a);
                }
            }
        }
        let atoms: Vec<usize> = set.into_iter().collect();
        match pick::index_selection_string(&atoms) {
            Some(sel_text) => {
                mol.pending = Some(scene::PendingSelection { sel_text, atoms });
                mol.reps_open = true;
            }
            // Empty result (e.g. subtracted everything) → no active selection.
            None => mol.pending = None,
        }
        mol.glow_dirty = true;
    }
}
