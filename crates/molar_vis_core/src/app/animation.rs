//! Resolve camera destinations once, then evaluate the same timeline for all hosts.
use super::*;
use crate::animation::{AnimationHandle, Movie, Segment, Timeline, ViewAction, ViewPose, ViewStep};

#[derive(Clone)]
pub struct PreparedMovie {
    pub(crate) timeline: Timeline,
    pub(crate) tracks: Vec<crate::animation::TrajectoryTrack>,
}
impl PreparedMovie {
    pub fn duration(&self) -> f64 {
        self.timeline.duration
    }
    pub(crate) fn apply(
        &self,
        scene: &mut Scene,
        camera: &mut Camera,
        time: f64,
    ) -> Result<(), String> {
        // Resolve every track first so a removed molecule cannot cause a partial seek.
        let tracks = self
            .tracks
            .iter()
            .map(|t| {
                let mi = scene
                    .mol_index(MolId(t.mol))
                    .ok_or("movie molecule was removed")?;
                let index = t.frame(time.min(self.duration()));
                if index >= scene.molecules[mi].trajectory.n_frames() {
                    return Err("trajectory changed since movie preparation".into());
                }
                Ok((mi, index))
            })
            .collect::<Result<Vec<_>, String>>()?;
        ViewPose::capture(&self.timeline.sample(time)).apply(camera);
        for (mi, index) in tracks {
            let mol = &mut scene.molecules[mi];
            if mol.trajectory.current != index {
                mol.trajectory.set_current(index);
                mol.apply_current_frame();
            }
        }
        Ok(())
    }
}

pub(super) struct Preview {
    movie: PreparedMovie,
    started: web_time::Instant,
    pub(super) handle: AnimationHandle,
    playback: Vec<(MolId, bool)>,
}
impl Drop for Preview {
    fn drop(&mut self) {
        if !self.handle.is_done() {
            self.handle.cancel();
            self.handle.finish(Ok(()));
        }
    }
}
impl App {
    pub fn view_pose(&self) -> ViewPose {
        ViewPose::capture(&self.camera)
    }

    pub fn prepare_movie(&mut self, movie: &Movie, aspect: f32) -> Result<PreparedMovie, String> {
        movie.validate()?;
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("aspect must be finite and positive".into());
        }
        let mut ids = std::collections::HashSet::new();
        for track in &movie.trajectories {
            if !ids.insert(track.mol) {
                return Err("duplicate trajectory track for a molecule".into());
            }
            let mi = self.resolve_molecule(track.mol)?;
            let n = self.scene.molecules[mi].trajectory.n_frames();
            if track.start >= n || track.end >= n {
                return Err("trajectory track frame is out of range".into());
            }
        }
        let original = self.camera;
        let previous_preview = self.preview.take();
        let result = (|| {
            let mut segments = Vec::new();
            for step in &movie.steps {
                let start = self.camera;
                match &step.action {
                    ViewAction::Move {
                        yaw,
                        pitch,
                        roll,
                        zoom,
                        pan,
                    } => {
                        self.camera.rotate_deg(*yaw, *pitch);
                        self.camera.roll_deg(*roll);
                        self.camera.zoom_by(*zoom);
                        self.camera.pan_fraction(pan[0], pan[1]);
                    }
                    ViewAction::To { view } => view.apply(&mut self.camera),
                    ViewAction::Focus { mol, selection } => {
                        let mi = self.resolve_molecule(*mol)?;
                        let sel = self
                            .scene
                            .select(mi, selection)
                            .map_err(|e| format!("focus selection: {e:?}"))?;
                        let (min, max) = self.scene.molecules[mi].sel_bbox(&sel);
                        let bounds: Vec<_> = [min.x, max.x]
                            .into_iter()
                            .flat_map(|x| {
                                [min.y, max.y].into_iter().flat_map(move |y| {
                                    [min.z, max.z]
                                        .into_iter()
                                        .map(move |z| (glam::Vec3::new(x, y, z), 0.0))
                                })
                            })
                            .collect();
                        self.camera.focus_visual_bounds(&bounds, aspect, 1.0);
                    }
                    ViewAction::Unobstructed {
                        targets,
                        zoom_out,
                        resolution,
                    } => {
                        let targets = targets
                            .iter()
                            .map(|&(m, r)| self.resolve_rep(m, r))
                            .collect::<Result<Vec<_>, _>>()?;
                        // Search is resolved once, before the timeline clock starts.
                        self.unobstructed_view_multi_immediate(&targets, *zoom_out, *resolution)?;
                        let atoms = build::gather_unobstructed_atoms(&mut self.scene, &targets)?;
                        self.camera
                            .focus_visual_bounds(&atoms.visual_bounds, aspect, *zoom_out);
                    }
                    ViewAction::Reset => self.reset_view_immediate(),
                    ViewAction::Hold => (),
                }
                segments.push(Segment {
                    start,
                    end: self.camera,
                    step: step.clone(),
                });
            }
            Ok(PreparedMovie {
                timeline: Timeline {
                    segments,
                    duration: movie.duration(),
                },
                tracks: movie.trajectories.clone(),
            })
        })();
        self.camera = original;
        self.preview = previous_preview;
        result
    }

    pub fn prepare_movie_viewport(&mut self, movie: &Movie) -> Result<PreparedMovie, String> {
        let [w, h] = self.last_size;
        self.prepare_movie(movie, w.max(1) as f32 / h.max(1) as f32)
    }

    pub fn animate_view(&mut self, step: ViewStep) -> Result<AnimationHandle, String> {
        let movie = Movie {
            steps: vec![step],
            trajectories: Vec::new(),
        };
        let [w, h] = self.last_size;
        let prepared = self.prepare_movie(&movie, w.max(1) as f32 / h.max(1) as f32)?;
        self.preview_movie(prepared)
    }

    pub fn preview_movie(&mut self, movie: PreparedMovie) -> Result<AnimationHandle, String> {
        #[cfg(not(target_arch = "wasm32"))]
        if self.movie_job.is_some() {
            return Err("movie export is active".into());
        }
        self.cancel_animation();
        let handle = AnimationHandle::new(0);
        if movie.duration() == 0.0 {
            self.seek_movie(&movie, 0.0)?;
            handle.finish(Ok(()));
            return Ok(handle);
        }
        let playback = self
            .scene
            .molecules
            .iter_mut()
            .map(|m| {
                let playing = m.trajectory.playing;
                m.trajectory.set_playing(false);
                (m.id, playing)
            })
            .collect();
        self.preview = Some(Preview {
            movie,
            started: web_time::Instant::now(),
            handle: handle.clone(),
            playback,
        });
        Ok(handle)
    }

    pub fn seek_movie(&mut self, movie: &PreparedMovie, time: f64) -> Result<(), String> {
        if !time.is_finite() || time < 0.0 {
            return Err("time must be finite and non-negative".into());
        }
        movie.apply(&mut self.scene, &mut self.camera, time)?;
        self.view_dirty = true;
        Ok(())
    }

    pub fn cancel_animation(&mut self) {
        if let Some(preview) = self.preview.take() {
            for &(id, playing) in &preview.playback {
                if let Some(mi) = self.scene.mol_index(id) {
                    self.scene.molecules[mi].trajectory.set_playing(playing);
                }
            }
            preview.handle.cancel();
            preview.handle.finish(Ok(()));
        }
    }

    pub(super) fn service_animation(&mut self, ctx: &egui::Context) {
        let Some(preview) = self.preview.take() else {
            return;
        };
        if preview.handle.cancelled() {
            self.preview = Some(preview);
            self.cancel_animation();
            return;
        }
        let elapsed = preview.started.elapsed().as_secs_f64();
        let result = self.seek_movie(&preview.movie, elapsed);
        if result.is_err() || elapsed >= preview.movie.duration() {
            for &(id, playing) in &preview.playback {
                if let Some(mi) = self.scene.mol_index(id) {
                    self.scene.molecules[mi].trajectory.set_playing(playing);
                }
            }
            preview.handle.finish(result);
        } else {
            self.preview = Some(preview);
            ctx.request_repaint();
        }
    }

    /// Schedule ordinary navigation using the saved duration unless explicitly overridden.
    /// Movie recipes and animate_view always use their own explicit timing.
    pub fn program_view(
        &mut self,
        action: ViewAction,
        duration: Option<f64>,
        easing: crate::animation::Easing,
    ) -> Result<AnimationHandle, String> {
        self.animate_view(ViewStep {
            action,
            duration: duration
                .unwrap_or(f64::from(self.settings.rendering.camera_transition_ms) / 1000.0),
            easing,
        })
    }

    pub(super) fn navigate_view(&mut self, action: ViewAction) {
        if let Err(error) = self.program_view(action, None, crate::animation::Easing::Smooth) {
            self.status = error;
        }
    }

    pub(super) fn transition_camera(&mut self, target: Camera) {
        self.navigate_view(ViewAction::To {
            view: ViewPose::capture(&target),
        });
    }

    pub(super) fn focus_camera_bounds(&mut self, min: glam::Vec3, max: glam::Vec3) {
        let mut target = self.camera;
        target.focus_bbox(min, max);
        self.transition_camera(target);
    }

    pub(super) fn center_camera_on(&mut self, point: glam::Vec3) {
        let mut target = self.camera;
        target.recenter(point);
        self.transition_camera(target);
    }
}
