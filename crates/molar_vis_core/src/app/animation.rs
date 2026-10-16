//! Resolve camera destinations once, then evaluate the same timeline for all hosts.
use super::*;
use crate::animation::{AnimationHandle, Movie, Segment, Timeline, ViewAction, ViewPose, ViewStep};

#[derive(Clone)]
pub struct PreparedMovie {
    pub(crate) timeline: Timeline,
    pub(crate) tracks: Vec<crate::animation::TrajectoryTrack>,
    pub(crate) groups: Vec<(u64, usize)>,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod group_timeline_tests {
    use super::*;
    use crate::animation::Easing;

    fn fixture() -> (Scene, PreparedMovie) {
        let mut scene = Scene::default();
        let records = (0..3)
            .map(|i| {
                data::RawMolecule::single_atom(
                    &format!("member{i}"),
                    molar::prelude::Atom::new().with_name("CA"),
                    glam::Vec3::X * i as f32,
                )
                .unwrap()
            })
            .collect();
        scene.add_group(
            records,
            MoleculeSource::default(),
            "group".into(),
            &RepDefaults::default(),
        );
        let group = scene.groups[0].id.0;
        let step = |action, duration| Segment {
            start: Camera::default(),
            end: Camera::default(),
            step: ViewStep {
                action,
                duration,
                easing: Easing::Linear,
            },
        };
        let movie = PreparedMovie {
            timeline: Timeline {
                duration: 6.0,
                segments: vec![
                    step(ViewAction::Hold, 3.0),
                    step(
                        ViewAction::GroupUnobstructed {
                            group,
                            member: 1,
                            zoom_out: 1.0,
                            resolution: 32,
                        },
                        1.0,
                    ),
                    step(
                        ViewAction::GroupUnobstructed {
                            group,
                            member: 2,
                            zoom_out: 1.0,
                            resolution: 32,
                        },
                        1.0,
                    ),
                    step(ViewAction::Hold, 1.0),
                ],
            },
            tracks: vec![],
            groups: vec![(group, 0)],
        };
        (scene, movie)
    }

    #[test]
    fn group_switches_at_transition_start_and_seeks_backwards() {
        let (mut scene, movie) = fixture();
        let rep = scene.group_shared_reps(0)[0].id;
        let mut camera = Camera::default();
        for (time, member) in [
            (0.0, 0),
            (2.99, 0),
            (3.0, 1),
            (3.99, 1),
            (4.0, 2),
            (6.0, 2),
            (3.5, 1),
            (0.0, 0),
        ] {
            movie.apply(&mut scene, &mut camera, time).unwrap();
            assert_eq!(scene.groups[0].current, member);
            assert_eq!(scene.molecules.iter().filter(|m| m.visible).count(), 1);
            assert_eq!(scene.molecules.iter().map(|m| m.n_shared).sum::<usize>(), 1);
            assert_eq!(scene.group_shared_reps(0)[0].id, rep);
        }
        movie.apply(&mut scene, &mut camera, 4.5).unwrap();
        movie.restore_groups(&mut scene);
        assert_eq!(scene.groups[0].current, 0);
    }

    #[test]
    fn changed_group_fails_before_mutating_camera_or_members() {
        let (mut scene, movie) = fixture();
        scene.groups[0].members.pop();
        let mut camera = Camera::default();
        let original = camera;
        assert!(movie.apply(&mut scene, &mut camera, 4.5).is_err());
        assert_eq!(scene.groups[0].current, 0);
        assert_eq!(camera, original);
    }
}
impl PreparedMovie {
    pub fn duration(&self) -> f64 {
        self.timeline.duration
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn restore_groups(&self, scene: &mut Scene) {
        restore_groups(scene, &self.groups);
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
        let mut groups = self.groups.clone();
        let mut elapsed = 0.0;
        for segment in &self.timeline.segments {
            if elapsed > time {
                break;
            }
            if let ViewAction::GroupUnobstructed { group, member, .. } = segment.step.action {
                if let Some((_, shown)) = groups.iter_mut().find(|(id, _)| *id == group) {
                    *shown = member;
                }
            }
            elapsed += segment.step.duration;
        }
        let groups = groups
            .into_iter()
            .map(|(id, member)| {
                let gi = scene
                    .group_index(scene::GroupId(id))
                    .ok_or("movie group was removed")?;
                if member >= scene.groups[gi].members.len() {
                    return Err("group membership changed since movie preparation".to_string());
                }
                if scene.mol_index(scene.groups[gi].members[member]).is_none() {
                    return Err("movie group member was removed".into());
                }
                Ok((gi, member))
            })
            .collect::<Result<Vec<_>, String>>()?;
        for (gi, member) in groups {
            scene.switch_group_member(gi, member);
        }
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

pub(super) fn restore_groups(scene: &mut Scene, groups: &[(u64, usize)]) {
    for &(id, member) in groups {
        if let Some(gi) = scene.group_index(scene::GroupId(id)) {
            if member < scene.groups[gi].members.len() {
                scene.switch_group_member(gi, member);
            }
        }
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
        let mut groups = Vec::new();
        for step in &movie.steps {
            if let ViewAction::GroupUnobstructed { group, member, .. } = step.action {
                let gi = self.resolve_group(group)?;
                if member >= self.scene.groups[gi].members.len() {
                    return Err("group member is out of range".into());
                }
                if !groups.iter().any(|(id, _)| *id == group) {
                    groups.push((group, self.scene.groups[gi].current));
                }
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
                    ViewAction::GroupUnobstructed {
                        group,
                        member,
                        zoom_out,
                        resolution,
                    } => {
                        self.show_group_member(*group, *member)?;
                        let gi = self.resolve_group(*group)?;
                        let g = &self.scene.groups[gi];
                        let mi = self
                            .scene
                            .mol_index(g.members[g.current])
                            .ok_or("group member was removed")?;
                        let mol = &self.scene.molecules[mi];
                        let targets = mol.reps[..mol.n_shared]
                            .iter()
                            .enumerate()
                            .filter(|(_, r)| r.visible && r.kind != RepKind::Interactions)
                            .map(|(ri, _)| (mi, ri))
                            .collect::<Vec<_>>();
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
                groups: groups.clone(),
            })
        })();
        self.camera = original;
        restore_groups(&mut self.scene, &groups);
        if !groups.is_empty() {
            self.view_dirty = true;
        }
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
