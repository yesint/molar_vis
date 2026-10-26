//! Frame-pumped offline export. The UI owns rendering; a worker owns encoding.
use super::animation::PreparedMovie;
use super::*;
use crate::animation::AnimationHandle;
use crate::render::CaptureReadback;
use crate::video::{FrameSink, VideoOptions};

pub(super) struct MovieJob {
    movie: PreparedMovie,
    options: VideoOptions,
    sink: FrameSink,
    handle: AnimationHandle,
    next: u64,
    total: u64,
    readback: Option<CaptureReadback>,
    tracing: bool,
    pending: Option<Vec<u8>>,
    preview: Option<egui::TextureHandle>,
    original: Camera,
    playback: Vec<(MolId, usize, bool)>,
}
impl Drop for MovieJob {
    fn drop(&mut self) {
        if !self.handle.is_done() {
            self.handle.cancel();
            self.handle.finish(Ok(()));
        }
    }
}
impl App {
    pub fn render_movie(
        &mut self,
        movie: &crate::animation::Movie,
        path: PathBuf,
        options: VideoOptions,
    ) -> Result<AnimationHandle, String> {
        if self
            .background_movie
            .as_ref()
            .is_some_and(|job| job.handle.is_done())
        {
            self.background_movie = None;
        }
        if self.movie_job.is_some() || self.background_movie.is_some() {
            return Err("movie export is already active".into());
        }
        if !options.background && (self.rt.job.is_some() || self.rt.warm.is_some()) {
            return Err("wait for the current image render to finish".into());
        }
        if !self.scene.loaders.is_empty() {
            return Err("wait for trajectory loading to finish".into());
        }
        options.validate()?;
        let total = options.frame_count(movie.duration())?;
        let rs = self
            .render_state
            .as_ref()
            .ok_or("wgpu render state unavailable")?;
        let limit = rs.device.limits().max_texture_dimension_2d;
        let scale = if options.raytrace {
            1
        } else {
            self.settings.rendering.ssaa.clamp(1, 4)
        };
        if options.width.saturating_mul(scale) > limit
            || options.height.saturating_mul(scale) > limit
        {
            return Err(format!(
                "output with supersampling exceeds device texture limit ({limit})"
            ));
        }
        if options.raytrace && !self.renderer.raytrace_supported() {
            return Err("ray tracing requires a compute-capable device".into());
        }
        // Screen-width receptor lines must be scored at the export resolution,
        // which can differ from the interactive viewport. Restore it on errors too.
        let viewport = self.last_size;
        self.last_size = [options.width, options.height];
        let prepared = self.prepare_movie(movie, options.width as f32 / options.height as f32);
        self.last_size = viewport;
        let prepared = prepared?;
        if options.background {
            return self.render_background_movie(movie, path, options, prepared);
        }
        let sink = FrameSink::start(path, options.clone())?;
        self.cancel_animation();
        self.unobstructed_job = None;
        self.rt.still = false;
        let playback = self
            .scene
            .molecules
            .iter_mut()
            .map(|m| {
                let saved = (m.id, m.trajectory.current, m.trajectory.playing);
                m.trajectory.set_playing(false);
                saved
            })
            .collect();
        let handle = AnimationHandle::new(total);
        self.movie_job = Some(MovieJob {
            movie: prepared,
            options,
            sink,
            handle: handle.clone(),
            next: 0,
            total,
            readback: None,
            tracing: false,
            pending: None,
            preview: None,
            original: self.camera,
            playback,
        });
        Ok(handle)
    }

    fn finish_movie(&mut self, job: MovieJob, result: Result<(), String>) {
        self.renderer.rt_trace_cancel();
        self.camera = job.original;
        job.movie.restore_groups(&mut self.scene);
        for &(id, current, playing) in &job.playback {
            if let Some(mi) = self.scene.mol_index(id) {
                let mol = &mut self.scene.molecules[mi];
                mol.trajectory.set_current(current);
                mol.apply_current_frame();
                mol.trajectory.set_playing(playing);
            }
        }
        self.rt.scene_dirty = true;
        self.view_dirty = true;
        self.status = if job.handle.cancelled() {
            "Movie export cancelled".into()
        } else {
            match &result {
                Ok(()) => "Movie export complete".into(),
                Err(e) => format!("Movie export failed: {e}"),
            }
        };
        job.handle.finish(result);
    }

    /// Returns true while export owns the UI. Input is blocked except for Cancel/Esc.
    pub(super) fn service_movie(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(mut job) = self.movie_job.take() else {
            return false;
        };
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            job.handle.cancel();
        }
        if job.handle.cancelled() {
            ctx.request_repaint();
            self.finish_movie(job, Ok(()));
            return true;
        }
        if let Some(result) = job.sink.poll() {
            ctx.request_repaint();
            self.finish_movie(job, result);
            return true;
        }
        let result = self.advance_movie(&mut job, &ctx);
        self.draw_movie_progress(ui, &job);
        if let Err(error) = result {
            self.finish_movie(job, Err(error));
        } else {
            self.movie_job = Some(job);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(5));
        true
    }

    fn advance_movie(&mut self, job: &mut MovieJob, ctx: &egui::Context) -> Result<(), String> {
        use std::sync::mpsc::TrySendError;
        if let Some(pixels) = job.pending.take() {
            let tx = job.sink.tx.as_ref().ok_or("encoder input is closed")?;
            match tx.try_send(pixels) {
                Ok(()) => {
                    job.next += 1;
                    job.handle.advance(job.next);
                }
                Err(TrySendError::Full(pixels)) => {
                    job.pending = Some(pixels);
                    return Ok(());
                }
                // poll() will retrieve the worker's detailed error on the next UI frame.
                Err(TrySendError::Disconnected(pixels)) => {
                    job.pending = Some(pixels);
                    return Ok(());
                }
            }
        }
        if job.next == job.total {
            job.sink.tx.take();
            return Ok(());
        }
        let rs = self
            .render_state
            .clone()
            .ok_or("wgpu render state unavailable")?;
        rs.device
            .poll(wgpu::PollType::Poll)
            .map_err(|e| format!("GPU polling failed: {e}"))?;
        if job.readback.as_ref().is_some_and(|rb| rb.is_ready()) {
            let readback = job.readback.take().unwrap();
            if let Some(error) = readback.error() {
                return Err(error);
            }
            let image = readback.read();
            let preview = egui::ColorImage::from_rgba_unmultiplied(
                [image.width() as usize, image.height() as usize],
                image.as_raw(),
            );
            if let Some(texture) = &mut job.preview {
                texture.set(preview, egui::TextureOptions::LINEAR);
            } else {
                job.preview =
                    Some(ctx.load_texture("movie-frame", preview, egui::TextureOptions::LINEAR));
            }
            job.pending = Some(image.into_raw());
            return Ok(());
        }
        if job.readback.is_some() {
            return Ok(());
        }
        let (w, h) = (job.options.width, job.options.height);
        if job.tracing {
            if self.renderer.save_step(&rs, 4) {
                job.readback = Some(
                    self.renderer
                        .save_finish(&rs, w, h)
                        .ok_or("ray trace readback failed")?,
                );
                job.tracing = false;
            }
            return Ok(());
        }
        self.seek_movie(&job.movie, job.next as f64 / job.options.fps as f64)?;
        // Synchronous geometry construction also supersedes old asynchronous work.
        let gray = self.draw_gray_active();
        build::rebuild_dirty(
            &mut self.scene,
            &self.renderer,
            &self.settings,
            self.view_dirty,
            &rs,
            gray,
            false,
        );
        self.camera.depth_bounds = self.scene.visible_depth_bounds();
        self.renderer.set_edit_active(gray);
        if job.options.raytrace {
            self.renderer.prepare_raytrace(
                &rs,
                &self.scene,
                &self.camera,
                [w, h],
                self.settings.behavior.dashed_pbc_bonds,
            );
            if !self
                .renderer
                .save_begin(&rs, &self.camera, w, h, job.options.samples)
            {
                return Err("ray tracing unavailable or scene is empty".into());
            }
            job.tracing = true;
        } else {
            job.readback = Some(self.renderer.capture_begin(
                &rs,
                w,
                h,
                self.camera.view(),
                self.camera.proj(w as f32 / h as f32),
                self.camera.is_perspective(),
                self.camera.cue_uniform(),
                self.camera.ao_uniform(),
                self.camera.shadow_uniform(),
                self.camera.background,
                self.camera.eye_depth_range(),
                &self.scene,
            ));
        }
        self.view_dirty = false;
        Ok(())
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum MovieKind {
    Trajectory,
    #[default]
    FlyAround,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum MovieSize {
    #[default]
    ActualScreen,
    Custom,
}

pub(super) struct MovieDialog {
    kind: MovieKind,
    size: MovieSize,
    options: VideoOptions,
    duration: f64,
    yaw: f32,
    zoom: f32,
    molecule: Option<MolId>,
    first_frame: usize,
    last_frame: usize,
    playback_fps: f64,
}
impl Default for MovieDialog {
    fn default() -> Self {
        Self {
            kind: MovieKind::FlyAround,
            size: MovieSize::ActualScreen,
            options: VideoOptions::default(),
            duration: 8.0,
            yaw: 360.0,
            zoom: 1.0,
            molecule: None,
            first_frame: 0,
            last_frame: 0,
            playback_fps: 15.0,
        }
    }
}
impl MovieDialog {
    pub(super) fn new(scene: &Scene) -> Self {
        let mut dialog = Self::default();
        let selected = scene
            .selected_mol
            .and_then(|i| scene.molecules.get(i))
            .filter(|m| m.visible && m.trajectory.n_frames() > 1)
            .or_else(|| {
                scene
                    .molecules
                    .iter()
                    .find(|m| m.visible && m.trajectory.n_frames() > 1)
            });
        if let Some(molecule) = selected {
            dialog.kind = MovieKind::Trajectory;
            dialog.select_molecule(molecule);
        }
        dialog
    }

    fn update_screen_size(&mut self, viewport: [u32; 2]) {
        if self.size == MovieSize::ActualScreen {
            self.options.width = viewport[0].max(1);
            self.options.height = viewport[1].max(1);
        }
    }

    fn select_molecule(&mut self, molecule: &scene::Molecule) {
        self.molecule = Some(molecule.id);
        self.first_frame = 0;
        self.last_frame = molecule.trajectory.n_frames().saturating_sub(1);
        self.playback_fps = f64::from(molecule.trajectory.speed_fps).max(0.01);
    }

    fn movie(&self, scene: &Scene) -> Result<crate::animation::Movie, String> {
        use crate::animation::{Easing, Movie, TrajectoryTrack, ViewAction, ViewStep};
        self.options.validate()?;
        let movie = match self.kind {
            MovieKind::FlyAround => Movie {
                steps: vec![ViewStep {
                    action: ViewAction::Move {
                        yaw: self.yaw,
                        pitch: 0.0,
                        roll: 0.0,
                        zoom: self.zoom,
                        pan: [0.0; 2],
                    },
                    duration: self.duration,
                    easing: Easing::Linear,
                }],
                trajectories: Vec::new(),
            },
            MovieKind::Trajectory => {
                let molecule = self
                    .molecule
                    .and_then(|id| scene.mol_index(id))
                    .and_then(|i| scene.molecules.get(i))
                    .filter(|m| m.visible && m.trajectory.n_frames() > 1)
                    .ok_or("Load a trajectory for a visible molecule first.")?;
                if self.first_frame >= molecule.trajectory.n_frames()
                    || self.last_frame >= molecule.trajectory.n_frames()
                {
                    return Err("The selected frame range is no longer available.".into());
                }
                if !self.playback_fps.is_finite() || self.playback_fps <= 0.0 {
                    return Err("Playback rate must be finite and positive.".into());
                }
                let duration =
                    (self.first_frame.abs_diff(self.last_frame) + 1) as f64 / self.playback_fps;
                Movie {
                    steps: vec![ViewStep {
                        action: ViewAction::Hold,
                        duration,
                        easing: Easing::Linear,
                    }],
                    trajectories: vec![TrajectoryTrack {
                        mol: molecule.id.0,
                        start: self.first_frame,
                        end: self.last_frame,
                        fps: self.playback_fps,
                    }],
                }
            }
        };
        movie.validate()?;
        self.options.frame_count(movie.duration())?;
        Ok(movie)
    }
}
impl App {
    fn draw_movie_progress(&mut self, ui: &mut egui::Ui, job: &MovieJob) {
        let ctx = ui.ctx().clone();
        // Retain the normal scene controls for context while export owns the scene.
        let opacity = ui.painter().opacity();
        ui.disable();
        ui.set_opacity(opacity);
        self.draw_left_panel(ui);
        self.draw_view_toolbar(ui);
        let viewport = egui::CentralPanel::default()
            .show_inside(ui, |ui| {
                let rect = ui.available_rect_before_wrap();
                let (texture, size) = match &job.preview {
                    Some(texture) => (
                        texture.id(),
                        egui::vec2(job.options.width as f32, job.options.height as f32),
                    ),
                    None => (
                        self.renderer.texture_id(),
                        egui::vec2(
                            self.last_size[0].max(1) as f32,
                            self.last_size[1].max(1) as f32,
                        ),
                    ),
                };
                let scale = (rect.width() / size.x).min(rect.height() / size.y);
                let image_rect = egui::Rect::from_center_size(rect.center(), size * scale);
                egui::Image::new(egui::load::SizedTexture::new(texture, image_rect.size()))
                    .paint_at(ui, image_rect);
                rect
            })
            .inner;
        // A separate area keeps Cancel active while the scene controls are disabled.
        egui::Area::new(egui::Id::new("movie-progress"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .fixed_pos(viewport.center_bottom() - egui::vec2(0.0, 12.0))
            .show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(300.0);
                    ui.horizontal(|ui| {
                        ui.label(icon::FILM_STRIP);
                        if job.next == job.total {
                            ui.label("Finishing movie…");
                        } else {
                            ui.label(format!(
                                "Rendering frame {} / {}",
                                (job.next + 1).min(job.total),
                                job.total
                            ));
                        }
                        if ui.button("Cancel").clicked() {
                            job.handle.cancel();
                        }
                    });
                    ui.add(egui::ProgressBar::new(job.next as f32 / job.total as f32));
                });
            });
    }

    pub(super) fn draw_movie_dialog(&mut self, ctx: &egui::Context) {
        use super::widgets::{modal_shell, ModalBody, ModalSpec};
        modal_shell(
            self,
            ctx,
            |app| &mut app.movie_dialog,
            ModalSpec {
                id: "render_movie_dialog",
                width: 380.0,
                heading: "Render movie",
                commit: "Save…",
            },
            |ui, dlg, _err, app| {
                ui.horizontal(|ui| {
                    ui.label("Movie type");
                    egui::ComboBox::from_id_salt("movie-type")
                        .selected_text(match dlg.kind {
                            MovieKind::Trajectory => "Trajectory",
                            MovieKind::FlyAround => "Fly around",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut dlg.kind, MovieKind::Trajectory, "Trajectory");
                            ui.selectable_value(&mut dlg.kind, MovieKind::FlyAround, "Fly around");
                        });
                });
                match dlg.kind {
                    MovieKind::Trajectory => {
                        let sources: Vec<_> = app
                            .scene
                            .molecules
                            .iter()
                            .filter(|m| m.visible && m.trajectory.n_frames() > 1)
                            .collect();
                        if !sources.is_empty() {
                            if !sources.iter().any(|m| Some(m.id) == dlg.molecule) {
                                dlg.select_molecule(sources[0]);
                            }
                            let previous = dlg.molecule;
                            let selected =
                                sources.iter().find(|m| Some(m.id) == dlg.molecule).unwrap();
                            egui::ComboBox::from_id_salt("movie-trajectory-source")
                                .selected_text(&selected.name)
                                .show_ui(ui, |ui| {
                                    for molecule in &sources {
                                        ui.selectable_value(
                                            &mut dlg.molecule,
                                            Some(molecule.id),
                                            format!(
                                                "{} ({} frames)",
                                                molecule.name,
                                                molecule.trajectory.n_frames()
                                            ),
                                        );
                                    }
                                });
                            if dlg.molecule != previous {
                                if let Some(molecule) =
                                    sources.iter().find(|m| Some(m.id) == dlg.molecule)
                                {
                                    dlg.select_molecule(molecule);
                                }
                            }
                            let last = sources
                                .iter()
                                .find(|m| Some(m.id) == dlg.molecule)
                                .unwrap()
                                .trajectory
                                .n_frames()
                                - 1;
                            ui.horizontal(|ui| {
                                ui.label("Frames (inclusive, from 0)");
                                ui.add(egui::DragValue::new(&mut dlg.first_frame).range(0..=last));
                                ui.label("to");
                                ui.add(egui::DragValue::new(&mut dlg.last_frame).range(0..=last));
                            });
                            ui.horizontal(|ui| {
                                ui.label("Playback (source frames/s)");
                                ui.add(
                                    egui::DragValue::new(&mut dlg.playback_fps)
                                        .range(0.01..=240.0)
                                        .speed(0.1),
                                );
                            });
                            let duration = (dlg.first_frame.abs_diff(dlg.last_frame) + 1) as f64
                                / dlg.playback_fps;
                            ui.label(format!("Duration: {duration:.2} seconds"));
                        }
                    }
                    MovieKind::FlyAround => {
                        ui.horizontal(|ui| {
                            ui.label("Duration (seconds)");
                            ui.add(
                                egui::DragValue::new(&mut dlg.duration)
                                    .range(0.1..=3600.0)
                                    .speed(0.1),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label("Rotation (degrees)");
                            ui.add(egui::DragValue::new(&mut dlg.yaw));
                        });
                        ui.horizontal(|ui| {
                            let tooltip = "Final zoom relative to the starting view. 1 keeps the scale; 2 zooms in 2×; 0.5 zooms out 2×. Changes smoothly over the movie.";
                            ui.label("Zoom factor").on_hover_text(tooltip);
                            ui.add(
                                egui::DragValue::new(&mut dlg.zoom)
                                    .range(0.01..=100.0)
                                    .speed(0.01),
                            ).on_hover_text(tooltip);
                        });
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Size");
                    egui::ComboBox::from_id_salt("movie-size")
                        .selected_text(match dlg.size {
                            MovieSize::ActualScreen => "Actual screen",
                            MovieSize::Custom => "Custom",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut dlg.size,
                                MovieSize::ActualScreen,
                                "Actual screen",
                            );
                            ui.selectable_value(&mut dlg.size, MovieSize::Custom, "Custom");
                        });
                    dlg.update_screen_size(app.last_size);
                    if dlg.size == MovieSize::ActualScreen {
                        ui.label(format!("{} × {} px", dlg.options.width, dlg.options.height));
                    } else {
                        ui.add(egui::DragValue::new(&mut dlg.options.width).range(1..=16384));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut dlg.options.height).range(1..=16384));
                    }
                });
                if dlg.size == MovieSize::Custom {
                    ui.horizontal(|ui| {
                        ui.label("Aspect ratio");
                        for (label, width, height) in
                            [("16:9", 16_u32, 9_u32), ("16:10", 16, 10), ("4:3", 4, 3)]
                        {
                            if ui.button(label).clicked() {
                                let units = ((dlg.options.width + width / 2) / width)
                                    .clamp(1, 16384 / width);
                                dlg.options.width = units * width;
                                dlg.options.height = units * height;
                            }
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.label("Video frame rate (fps)");
                    ui.add(egui::DragValue::new(&mut dlg.options.fps).range(1..=240));
                });
                ui.checkbox(&mut dlg.options.raytrace, "Ray tracing");
                if dlg.options.raytrace {
                    ui.horizontal(|ui| {
                        ui.label("Samples per frame");
                        ui.add(egui::DragValue::new(&mut dlg.options.samples).range(1..=65536));
                    });
                }
                ui.checkbox(&mut dlg.options.background, "Background render");
                if let Ok(movie) = dlg.movie(&app.scene) {
                    let bytes = super::background_movie::memory_estimate(
                        &app.scene,
                        &movie,
                        &dlg.options,
                        &app.settings,
                    );
                    ui.small(format!(
                        "requires ~{} Mb additional RAM",
                        bytes.saturating_add(999_999) / 1_000_000
                    ));
                }
                let valid = dlg.movie(&app.scene);
                if let Err(error) = &valid {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                ModalBody::enabled(valid.is_ok())
            },
            |app, dlg| {
                dlg.update_screen_size(app.last_size);
                let movie = dlg.movie(&app.scene)?;
                let Some(path) = rfd::FileDialog::new()
                    .add_filter("MP4 video", &["mp4"])
                    .set_file_name("molar_vis.mp4")
                    .save_file()
                else {
                    return Ok(());
                };
                app.render_movie(&movie, path, dlg.options.clone())
                    .map(|_| ())
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{Easing, Movie, TaskState, TrajectoryTrack, ViewAction, ViewStep};
    fn pump(app: &mut App, ctx: &egui::Context, handle: &AnimationHandle) -> Vec<egui::ColorImage> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut previews = Vec::new();
        while !handle.is_done() {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(960.0, 640.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    app.service_movie(ui);
                },
            );
            if let Some(texture) = app.movie_job.as_ref().and_then(|job| job.preview.as_ref()) {
                for (id, delta) in &output.textures_delta.set {
                    if *id == texture.id() {
                        let egui::ImageData::Color(image) = &delta.image;
                        previews.push((**image).clone());
                    }
                }
                let draws = ctx.tessellate(output.shapes, output.pixels_per_point);
                assert!(draws.iter().any(|draw| matches!(&draw.primitive, egui::epaint::Primitive::Mesh(mesh) if mesh.texture_id == texture.id())), "export preview must be painted in the viewport");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "movie export timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        previews
    }

    fn trajectory_scene() -> Scene {
        let raw = crate::data::load(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/2lao.pdb"
        )))
        .unwrap();
        let mut scene = Scene::default();
        scene.add(raw, &crate::settings::RepDefaults::default());
        let state = scene.molecules[0].render_state().clone();
        scene.molecules[0].trajectory.frames = vec![state; 5];
        scene.selected_mol = Some(0);
        scene
    }

    #[test]
    fn actual_screen_preserves_exact_viewport_pixels_and_custom_size() {
        let mut dialog = MovieDialog::default();
        dialog.update_screen_size([987, 653]);
        assert_eq!([dialog.options.width, dialog.options.height], [987, 653]);
        assert!(dialog.options.validate().is_ok());
        dialog.update_screen_size([1111, 777]);
        assert_eq!([dialog.options.width, dialog.options.height], [1111, 777]);
        dialog.size = MovieSize::Custom;
        dialog.options.width = 1920;
        dialog.options.height = 1080;
        dialog.update_screen_size([987, 653]);
        assert_eq!([dialog.options.width, dialog.options.height], [1920, 1080]);
    }

    #[test]
    fn movie_dialog_distinguishes_trajectory_and_fly_around() {
        let scene = trajectory_scene();
        let mut dialog = MovieDialog::new(&scene);
        assert!(dialog.kind == MovieKind::Trajectory);
        dialog.first_frame = 1;
        dialog.last_frame = 4;
        dialog.playback_fps = 2.0;
        let movie = dialog.movie(&scene).unwrap();
        assert_eq!(movie.duration(), 2.0);
        assert!(matches!(movie.steps[0].action, ViewAction::Hold));
        assert_eq!(movie.trajectories[0].mol, scene.molecules[0].id.0);
        assert_eq!(movie.trajectories[0].frame(0.0), 1);
        assert_eq!(movie.trajectories[0].frame(1.5), 4);
        dialog.first_frame = 4;
        dialog.last_frame = 1;
        assert_eq!(dialog.movie(&scene).unwrap().trajectories[0].frame(1.5), 1);
        dialog.kind = MovieKind::FlyAround;
        let movie = dialog.movie(&scene).unwrap();
        assert!(movie.trajectories.is_empty());
        assert_eq!(movie.duration(), 8.0);
        assert!(matches!(
            movie.steps[0].action,
            ViewAction::Move { yaw: 360.0, .. }
        ));
    }

    #[test]
    fn trajectory_dialog_rejects_missing_or_hidden_sources_and_stale_ranges() {
        let mut dialog = MovieDialog {
            kind: MovieKind::Trajectory,
            ..Default::default()
        };
        assert!(dialog.movie(&Scene::default()).is_err());
        let mut scene = trajectory_scene();
        dialog = MovieDialog::new(&scene);
        dialog.last_frame = 5;
        assert!(dialog.movie(&scene).is_err());
        dialog.last_frame = 4;
        scene.molecules[0].visible = false;
        assert!(dialog.movie(&scene).is_err());
    }

    #[test]
    #[ignore = "requires GPU, ffmpeg and MOLAR_VIS_DEBUG_DEFAULTS=1"]
    fn movie_export_raster_trace_and_cancellation_restore_state() {
        assert!(
            std::env::var_os("MOLAR_VIS_DEBUG_DEFAULTS").is_some(),
            "run with MOLAR_VIS_DEBUG_DEFAULTS=1 to avoid user settings"
        );
        let ctx = egui::Context::default();
        let mut cc = eframe::CreationContext::_new_kittest(ctx.clone());
        cc.wgpu_render_state = Some(crate::render::test_gpu());
        let launch = AppLaunch {
            files: vec![vec![PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tests/2lao.pdb"
            ))]],
            ..Default::default()
        };
        let mut app = App::new(&cc, launch).unwrap();
        // Ordinary navigation inherits settings; explicit timing and movie preparation do not.
        let initial = app.camera;
        let immediate = app
            .program_view(ViewAction::zoom(1.1), None, Easing::Smooth)
            .unwrap();
        assert!(immediate.is_done());
        assert!(app.preview.is_none());
        app.camera = initial;
        app.settings.rendering.camera_transition_ms = 450;
        app.zoom_view(1.1);
        assert_eq!(app.camera, initial);
        assert!(app.preview.is_some());
        app.cancel_animation();
        let immediate = app
            .program_view(ViewAction::zoom(1.1), Some(0.0), Easing::Smooth)
            .unwrap();
        assert!(immediate.is_done());
        assert_ne!(app.camera, initial);
        app.camera = initial;
        app.reset_view();
        assert!(app.preview.is_some());
        app.cancel_animation();
        app.focus_selection(0, &[0, 1, 2]).unwrap();
        assert!(app.preview.is_some());
        app.cancel_animation();
        app.unobstructed_view(0, 0, 1.0, 1).unwrap();
        assert!(app.preview.is_some());
        app.cancel_animation();
        let mut destination = initial;
        destination.zoom_by(1.1);
        app.transition_camera(destination);
        assert!(app.preview.is_some());
        app.cancel_animation();
        app.settings.rendering.camera_transition_ms = 0;
        app.transition_camera(destination);
        assert!(app.preview.is_none());
        assert_eq!(app.camera, destination);
        app.camera = initial;
        // A nonzero navigation setting must not create nested previews during export.
        app.settings.rendering.camera_transition_ms = 450;
        let state = app.scene.molecules[0].render_state().clone();
        app.scene.molecules[0].trajectory.frames = vec![state.clone(), state];
        app.scene.molecules[0].trajectory.set_playing(true);
        let id = app.scene.molecules[0].id.0;
        let movie = Movie {
            steps: vec![ViewStep {
                action: ViewAction::rotation(360.0, 0.0),
                duration: 0.25,
                easing: Easing::Linear,
            }],
            trajectories: vec![TrajectoryTrack {
                mol: id,
                start: 0,
                end: 1,
                fps: 12.0,
            }],
        };
        let original = app.camera;
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/movie-gpu-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        // Deterministic seeking does not accumulate rotation, and failed preparation preserves a preview.
        let prepared = app.prepare_movie(&movie, 4.0 / 3.0).unwrap();
        app.seek_movie(&prepared, 0.125).unwrap();
        let midpoint = app.camera;
        app.seek_movie(&prepared, 0.0).unwrap();
        app.seek_movie(&prepared, 0.125).unwrap();
        assert_eq!(midpoint, app.camera);
        app.camera = original;
        app.scene.molecules[0].trajectory.set_current(0);
        for (raytrace, png_sequence, name) in [(false, false, "raster.mp4"), (true, true, "trace")]
        {
            let path = directory.join(name);
            if path.is_dir() {
                std::fs::remove_dir_all(&path).unwrap();
            } else {
                let _ = std::fs::remove_file(&path);
            }
            let options = VideoOptions {
                width: 64,
                height: 48,
                fps: 12,
                raytrace,
                samples: 2,
                png_sequence,
                ..Default::default()
            };
            let handle = app.render_movie(&movie, path.clone(), options).unwrap();
            let previews = pump(&mut app, &ctx, &handle);
            assert_eq!(
                previews.len(),
                3,
                "every rendered frame must update the visible preview"
            );
            assert_eq!(previews[0].size, [64, 48]);
            assert_ne!(
                previews[0].pixels, previews[1].pixels,
                "preview must show the changing scene"
            );
            handle.wait().unwrap();
            assert_eq!(app.camera, original);
            assert_eq!(app.scene.molecules[0].trajectory.current, 0);
            assert!(app.scene.molecules[0].trajectory.playing);
            assert_eq!(handle.progress().completed, 3);
            if png_sequence {
                assert_eq!(std::fs::read_dir(&path).unwrap().count(), 3);
            } else {
                let output = std::process::Command::new("ffprobe")
                    .args([
                        "-v",
                        "error",
                        "-count_frames",
                        "-show_entries",
                        "stream=nb_read_frames",
                        "-of",
                        "csv=p=0",
                    ])
                    .arg(&path)
                    .output()
                    .unwrap();
                assert!(output.status.success());
                assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "3");
            }
        }
        std::fs::write(directory.join("cancelled.mp4"), b"previous movie").unwrap();
        let cancelled = app
            .render_movie(
                &movie,
                directory.join("cancelled.mp4"),
                VideoOptions {
                    width: 64,
                    height: 48,
                    ..Default::default()
                },
            )
            .unwrap();
        cancelled.cancel();
        pump(&mut app, &ctx, &cancelled);
        assert!(matches!(cancelled.progress().state, TaskState::Cancelled));
        assert_eq!(app.camera, original);
        assert!(app.scene.molecules[0].trajectory.playing);
        assert_eq!(
            std::fs::read(directory.join("cancelled.mp4")).unwrap(),
            b"previous movie"
        );
        // Missing encoders fail before any playback state is changed.
        assert!(app
            .render_movie(
                &movie,
                directory.join("missing.mp4"),
                VideoOptions {
                    ffmpeg: "/missing/ffmpeg".into(),
                    ..Default::default()
                }
            )
            .is_err());
        assert_eq!(app.camera, original);
        assert!(app.scene.molecules[0].trajectory.playing);
    }
}
