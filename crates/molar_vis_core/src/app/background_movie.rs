//! Native movie rendering on an owned scene, independent of the viewer thread.
use super::animation::PreparedMovie;
use super::*;
use crate::animation::{AnimationHandle, Movie};
use crate::history::RepState;
use crate::video::{FrameSink, VideoOptions};
use molar::prelude::{System, Topology};
use std::{
    ops::RangeInclusive,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};

struct MoleculeSnapshot {
    id: MolId,
    name: String,
    source: MoleculeSource,
    topology: Topology,
    state: State,
    frames: Vec<State>,
    current: usize,
    bonds: Vec<Bond>,
    reps: Vec<(RepState, Option<SsMap>)>,
    meshes: Vec<crate::mesh::MeshState>,
    visible: bool,
    group: Option<GroupId>,
    n_shared: usize,
    show_box: bool,
    aromatic_rings: Vec<Vec<usize>>,
}
struct SceneSnapshot {
    molecules: Vec<MoleculeSnapshot>,
    groups: Vec<MolGroup>,
}

/// Keep original frame indices without copying unused coordinate arrays. Include
/// smoothing neighbors and the currently displayed frame for untracked molecules.
fn frame_ranges(mol: &scene::Molecule, movie: &Movie) -> Vec<RangeInclusive<usize>> {
    let n = mol.trajectory.n_frames();
    if n == 0 {
        return Vec::new();
    }
    let half = mol
        .reps
        .iter()
        .map(|r| r.smooth_window.saturating_sub(1) as usize / 2)
        .max()
        .unwrap_or(0);
    let current = mol.trajectory.current.min(n - 1);
    let mut ranges = vec![current.saturating_sub(half)..=current.saturating_add(half).min(n - 1)];
    if let Some(track) = movie.trajectories.iter().find(|t| t.mol == mol.id.0) {
        ranges.push(
            track.start.min(track.end).saturating_sub(half)
                ..=track.start.max(track.end).saturating_add(half).min(n - 1),
        );
    }
    ranges.sort_by_key(|r| *r.start());
    if ranges.len() == 2 && *ranges[1].start() <= ranges[0].end().saturating_add(1) {
        return vec![*ranges[0].start()..=(*ranges[0].end()).max(*ranges[1].end())];
    }
    ranges
}
fn copy_frame(state: &State) -> State {
    State {
        coords: state.coords.clone(),
        time: state.time,
        pbox: state.pbox.clone(),
        ..Default::default()
    }
}
impl SceneSnapshot {
    fn capture(scene: &Scene, movie: &Movie) -> Result<Self, String> {
        let molecules = scene
            .molecules
            .iter()
            .map(|mol| {
                // Shared sources must make this copy under their host's lock (e.g. the GIL).
                let (topology, state) = mol.data.snapshot()?;
                let mut frames = vec![State::default(); mol.trajectory.n_frames()];
                for range in frame_ranges(mol, movie) {
                    for i in range {
                        frames[i] = copy_frame(&mol.trajectory.frames[i]);
                    }
                }
                Ok(MoleculeSnapshot {
                    id: mol.id,
                    name: mol.name.clone(),
                    source: mol.source.clone(),
                    topology,
                    state,
                    frames,
                    current: mol.trajectory.current,
                    bonds: mol.bonds.clone(),
                    reps: mol
                        .reps
                        .iter()
                        .map(|r| (RepState::capture(r), r.ss_cache.clone()))
                        .collect(),
                    // Mesh sources are immutable Arcs; edits replace them, so sharing is safe.
                    meshes: mol.meshes.iter().map(|m| m.state.clone()).collect(),
                    visible: mol.visible,
                    group: mol.group,
                    n_shared: mol.n_shared,
                    show_box: mol.show_box,
                    aromatic_rings: mol.aromatic_rings.clone(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            molecules,
            groups: scene.groups.clone(),
        })
    }
    fn into_scene(self, settings: &Settings) -> Result<Scene, String> {
        let mut scene = Scene::default();
        scene.groups = self.groups;
        for snapshot in self.molecules {
            let system =
                System::new(snapshot.topology, snapshot.state).map_err(|e| e.to_string())?;
            let n_atoms = system.topology().atoms.len();
            let mut mol = scene::Molecule::new(
                snapshot.id,
                data::RawMolecule {
                    name: snapshot.name,
                    source: snapshot.source,
                    system,
                    n_atoms,
                    bonds: snapshot.bonds,
                    bbox_min: glam::Vec3::ZERO,
                    bbox_max: glam::Vec3::ZERO,
                },
                &settings.reps,
            );
            mol.trajectory.frames = snapshot.frames;
            mol.trajectory.set_current(snapshot.current);
            mol.reps = snapshot
                .reps
                .into_iter()
                .map(|(state, ss)| {
                    let mut rep = state.to_representation();
                    rep.ss_cache = ss;
                    rep
                })
                .collect();
            mol.meshes = snapshot
                .meshes
                .into_iter()
                .map(crate::mesh::Mesh::from_state)
                .collect();
            mol.visible = snapshot.visible;
            mol.group = snapshot.group;
            mol.n_shared = snapshot.n_shared;
            mol.show_box = snapshot.show_box;
            mol.aromatic_rings = snapshot.aromatic_rings;
            mol.aromatic_dirty = true;
            let (min, max) = mol.current_bbox();
            mol.bbox_min = min;
            mol.bbox_max = max;
            scene.molecules.push(mol);
        }
        Ok(scene)
    }
}

pub(super) fn memory_estimate(
    scene: &Scene,
    movie: &Movie,
    options: &VideoOptions,
    settings: &Settings,
) -> u64 {
    let mut bytes = 64u64 * 1024 * 1024; // worker, pipeline metadata and allocator allowance
    for mol in &scene.molecules {
        let atoms = mol.n_atoms as u64;
        // Topology/strings, base coordinates, bonds, per-frame State headers.
        bytes = bytes
            .saturating_add(atoms.saturating_mul(268))
            .saturating_add((mol.bonds.len() as u64).saturating_mul(32))
            .saturating_add(
                (mol.trajectory.n_frames() as u64)
                    .saturating_mul(std::mem::size_of::<State>() as u64),
            );
        for range in frame_ranges(mol, movie) {
            let count = range.end().saturating_sub(*range.start()).saturating_add(1) as u64;
            bytes = bytes.saturating_add(atoms.saturating_mul(12).saturating_mul(count));
        }
        for rep in &mol.reps {
            let geometry = rep
                .cached_geometry(settings.behavior.dashed_pbc_bonds)
                .map(|g| {
                    std::mem::size_of_val(g.spheres)
                        + std::mem::size_of_val(g.cylinders)
                        + std::mem::size_of_val(g.lines)
                        + std::mem::size_of_val(g.points)
                        + std::mem::size_of_val(g.mesh.vertices.as_slice())
                        + std::mem::size_of_val(g.mesh.indices.as_slice())
                })
                .map(|n| n as u64)
                .unwrap_or_else(|| atoms.saturating_mul(128));
            // Geometry construction scratch/caches; tracing also builds a CPU BVH.
            bytes =
                bytes.saturating_add(geometry.saturating_mul(if options.raytrace { 4 } else { 2 }));
        }
        for mesh in &mol.meshes {
            bytes = bytes
                .saturating_add((mesh.state.source.vertices.len() as u64).saturating_mul(64))
                .saturating_add((mesh.state.source.indices.len() as u64).saturating_mul(24));
        }
    }
    let pixels = u64::from(options.width) * u64::from(options.height);
    let ssaa = if options.raytrace {
        1
    } else {
        u64::from(settings.rendering.ssaa.clamp(1, 4)).pow(2)
    };
    // Raw frame queue, readback/downsample, and an approximate encoder lookahead budget.
    bytes
        .saturating_add(pixels.saturating_mul(4 * (5 + ssaa)))
        .saturating_add(pixels.saturating_mul(if options.png_sequence {
            8
        } else if options.width & 1 != 0 || options.height & 1 != 0 {
            144
        } else {
            96
        }))
}

pub(super) struct BackgroundMovie {
    pub(super) handle: AnimationHandle,
    active_input: Arc<AtomicBool>,
    result: mpsc::Receiver<Result<(), String>>,
    preview: mpsc::Receiver<image::RgbaImage>,
    texture: Option<egui::TextureHandle>,
}
impl Drop for BackgroundMovie {
    fn drop(&mut self) {
        if !self.handle.is_done() {
            self.handle.cancel();
        }
    }
}

impl App {
    pub(super) fn render_background_movie(
        &mut self,
        movie: &Movie,
        path: PathBuf,
        options: VideoOptions,
        prepared: PreparedMovie,
    ) -> Result<AnimationHandle, String> {
        let snapshot = SceneSnapshot::capture(&self.scene, movie)?;
        let rs = self
            .render_state
            .clone()
            .ok_or("wgpu render state unavailable")?;
        let settings = self.settings.clone();
        let camera = self.camera;
        let handle = AnimationHandle::new(options.frame_count(movie.duration())?);
        let worker_handle = handle.clone();
        let active_input = Arc::new(AtomicBool::new(false));
        let active = active_input.clone();
        let (tx, result) = mpsc::channel();
        let (images, preview) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("background-movie".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_worker(
                        snapshot,
                        rs,
                        settings,
                        camera,
                        prepared,
                        path,
                        options,
                        &worker_handle,
                        &active,
                        images,
                    )
                }))
                .unwrap_or_else(|_| Err("background render worker failed".into()));
                // Publish completion on the worker so wait() does not depend on UI polling.
                worker_handle.finish(result.clone());
                let _ = tx.send(result);
            })
            .map_err(|e| format!("cannot start background render: {e}"))?;
        self.background_movie = Some(BackgroundMovie {
            handle: handle.clone(),
            active_input,
            result,
            preview,
            texture: None,
        });
        Ok(handle)
    }

    pub(super) fn service_background_movie(&mut self, ctx: &egui::Context) {
        let Some(mut job) = self.background_movie.take() else {
            return;
        };
        job.active_input.store(
            ctx.egui_is_using_pointer() || ctx.egui_wants_keyboard_input(),
            Ordering::Relaxed,
        );
        match job.result.try_recv() {
            Ok(result) => {
                self.status = if job.handle.cancelled() {
                    "Background render cancelled".into()
                } else {
                    match result {
                        Ok(()) => "Background render complete".into(),
                        Err(e) => format!("Background render failed: {e}"),
                    }
                };
                ctx.request_repaint();
                return;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                job.handle
                    .finish(Err("background render worker stopped".into()));
                return;
            }
            Err(mpsc::TryRecvError::Empty) => (),
        }
        if let Ok(image) = job.preview.try_recv() {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [image.width() as usize, image.height() as usize],
                image.as_raw(),
            );
            if let Some(texture) = &mut job.texture {
                texture.set(image, egui::TextureOptions::LINEAR);
            } else {
                job.texture = Some(ctx.load_texture(
                    "background-movie-preview",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        let progress = job.handle.progress();
        egui::Area::new(egui::Id::new("background-movie-progress"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, -12.0])
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(260.0);
                    ui.horizontal(|ui| {
                        ui.label(format!("{}  Background render", icon::FILM_STRIP));
                        if ui.button("Cancel").clicked() {
                            job.handle.cancel();
                        }
                    });
                    ui.add(
                        egui::ProgressBar::new(
                            progress.completed as f32 / progress.total.max(1) as f32,
                        )
                        .text(format!("{} / {}", progress.completed, progress.total)),
                    );
                    if let Some(texture) = &job.texture {
                        ui.collapsing("Preview", |ui| {
                            ui.add(egui::Image::new(texture).max_width(250.0));
                        });
                    }
                });
            });
        self.background_movie = Some(job);
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

#[allow(clippy::too_many_arguments)]
fn run_worker(
    snapshot: SceneSnapshot,
    mut rs: eframe::egui_wgpu::RenderState,
    settings: Settings,
    mut camera: Camera,
    movie: PreparedMovie,
    path: PathBuf,
    options: VideoOptions,
    handle: &AnimationHandle,
    active: &AtomicBool,
    preview: mpsc::SyncSender<image::RgbaImage>,
) -> Result<(), String> {
    if handle.cancelled() {
        return Err("background render cancelled".into());
    }
    // Share the native device/queue, but never the live renderer's registry or buffers.
    rs.renderer = Arc::new(egui::mutex::RwLock::new(egui_wgpu::Renderer::new(
        &rs.device,
        rs.target_format,
        egui_wgpu::RendererOptions::default(),
    )));
    let mut scene = snapshot.into_scene(&settings)?;
    let mut renderer = SceneRenderer::new(&rs, &settings.rendering);
    if handle.cancelled() {
        return Err("background render cancelled".into());
    }
    let mut sink = FrameSink::start(path, options.clone())?;
    let total = options.frame_count(movie.duration())?;
    let cancelled = || {
        if handle.cancelled() {
            Err("background render cancelled".to_string())
        } else {
            Ok(())
        }
    };
    for index in 0..total {
        cancelled()?;
        movie.apply(&mut scene, &mut camera, index as f64 / options.fps as f64)?;
        if index == 0 {
            build::rebuild_movie_snapshot(&mut scene, &renderer, &settings, &rs);
        } else {
            build::rebuild_dirty(&mut scene, &renderer, &settings, true, &rs, None, false);
        }
        camera.depth_bounds = scene.visible_depth_bounds();
        let (w, h) = (options.width, options.height);
        let capture = if options.raytrace {
            renderer.prepare_raytrace(
                &rs,
                &scene,
                &camera,
                [w, h],
                settings.behavior.dashed_pbc_bonds,
            );
            if !renderer.save_begin(&rs, &camera, w, h, options.samples) {
                return Err("ray tracing unavailable or scene is empty".into());
            }
            loop {
                cancelled()?;
                if renderer.save_step(&rs, 1) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(if active.load(Ordering::Relaxed) {
                    8
                } else {
                    1
                }));
            }
            renderer
                .save_finish(&rs, w, h)
                .ok_or("ray trace readback failed")?
        } else {
            renderer.capture_begin(
                &rs,
                w,
                h,
                camera.view(),
                camera.proj(w as f32 / h as f32),
                camera.is_perspective(),
                camera.cue_uniform(),
                camera.ao_uniform(),
                camera.shadow_uniform(),
                camera.background,
                camera.eye_depth_range(),
                &scene,
            )
        };
        while !capture.is_ready() {
            cancelled()?;
            rs.device
                .poll(wgpu::PollType::Poll)
                .map_err(|e| format!("GPU polling failed: {e}"))?;
            std::thread::sleep(Duration::from_millis(1));
        }
        if let Some(error) = capture.error() {
            return Err(error);
        }
        let image = capture.read();
        let factor = (320.0 / w as f32).min(180.0 / h as f32).min(1.0);
        let thumbnail = image::imageops::thumbnail(
            &image,
            (w as f32 * factor).round().max(1.0) as u32,
            (h as f32 * factor).round().max(1.0) as u32,
        );
        let _ = preview.try_send(thumbnail);
        let mut pixels = image.into_raw();
        loop {
            cancelled()?;
            match sink
                .tx
                .as_ref()
                .ok_or("encoder input closed")?
                .try_send(pixels)
            {
                Ok(()) => break,
                Err(mpsc::TrySendError::Full(p)) => {
                    pixels = p;
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return match sink.poll() {
                        Some(Err(e)) => Err(e),
                        _ => Err("encoder worker stopped".into()),
                    };
                }
            }
        }
        handle.advance(index + 1);
        if active.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(16));
        }
    }
    sink.tx.take();
    loop {
        cancelled()?;
        if let Some(result) = handle.complete_when_ready(|| sink.poll()) {
            return result;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::{Easing, TrajectoryTrack, ViewAction, ViewStep};
    fn scene_with_frames() -> Scene {
        let raw = data::load(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/2lao.pdb"
        )))
        .unwrap();
        let mut scene = Scene::default();
        scene.add(raw, &RepDefaults::default());
        let state = scene.molecules[0].render_state().clone();
        scene.molecules[0].trajectory.frames = (0..20)
            .map(|i| {
                let mut frame = copy_frame(&state);
                frame.coords[0].x += i as f32;
                frame
            })
            .collect();
        scene.molecules[0].trajectory.set_current(2);
        scene
    }
    fn recipe() -> Movie {
        Movie {
            steps: vec![ViewStep {
                action: ViewAction::Hold,
                duration: 0.25,
                easing: Easing::Linear,
            }],
            trajectories: Vec::new(),
        }
    }
    #[test]
    fn snapshot_copies_only_needed_frames_and_is_independent_of_live_edits() {
        fn assert_send<T: Send>() {}
        assert_send::<SceneSnapshot>();
        let mut scene = scene_with_frames();
        scene.molecules[0].reps[0].smooth_window = 5;
        let mut movie = recipe();
        movie.trajectories.push(TrajectoryTrack {
            mol: scene.molecules[0].id.0,
            start: 10,
            end: 12,
            fps: 12.0,
        });
        assert_eq!(
            frame_ranges(&scene.molecules[0], &movie),
            vec![0..=4, 8..=14]
        );
        let old = scene.molecules[0].trajectory.frames[10].coords[0];
        let snapshot = SceneSnapshot::capture(&scene, &movie).unwrap();
        scene.molecules[0].trajectory.frames[10].coords[0].x += 100.0;
        scene.molecules[0].reps[0].sel_text = "index 0".into();
        scene.molecules[0].visible = false;
        let copy = snapshot.into_scene(&Settings::default()).unwrap();
        assert_eq!(copy.molecules[0].id, scene.molecules[0].id);
        assert_eq!(copy.molecules[0].trajectory.frames[10].coords[0], old);
        assert!(copy.molecules[0].trajectory.frames[7].coords.is_empty());
        assert_eq!(copy.molecules[0].trajectory.current, 2);
        assert_eq!(copy.molecules[0].reps[0].sel_text, "all");
        assert!(copy.molecules[0].visible);
    }
    #[test]
    fn estimate_accounts_for_frame_ranges_and_output_buffers() {
        let scene = scene_with_frames();
        let mut movie = recipe();
        let options = VideoOptions {
            width: 64,
            height: 48,
            ..Default::default()
        };
        let settings = Settings::default();
        let still = memory_estimate(&scene, &movie, &options, &settings);
        movie.trajectories.push(TrajectoryTrack {
            mol: scene.molecules[0].id.0,
            start: 10,
            end: 12,
            fps: 12.0,
        });
        let trajectory = memory_estimate(&scene, &movie, &options, &settings);
        assert_eq!(
            trajectory - still,
            3 * 12 * scene.molecules[0].n_atoms as u64
        );
        let larger = VideoOptions {
            width: 1920,
            height: 1080,
            ..options.clone()
        };
        assert!(memory_estimate(&scene, &movie, &larger, &settings) > trajectory);
        assert!(still > 64 * 1024 * 1024);
    }
    #[test]
    fn shared_sources_are_snapshotted_through_the_host_hook() {
        use molar::prelude::{Sel, SelectionExpr};
        use std::{cell::RefCell, rc::Rc};
        struct Shared(Rc<RefCell<(Topology, State)>>);
        impl crate::moldata::SharedSource for Shared {
            fn topology(&self) -> &Topology {
                panic!("must use the synchronized snapshot hook")
            }
            fn state(&self) -> &State {
                panic!("must use the synchronized snapshot hook")
            }
            fn evaluate(&self, _: &str) -> Result<(SelectionExpr, Sel), scene::EvalError> {
                unreachable!()
            }
            fn coords_version(&self) -> u64 {
                0
            }
            fn snapshot(&self) -> Result<(Topology, State), String> {
                let data = self.0.borrow();
                Ok((data.0.clone(), copy_frame(&data.1)))
            }
        }
        let mut scene = scene_with_frames();
        let data = scene.molecules[0].data.snapshot().unwrap();
        let old = data.1.coords[0];
        let shared = Rc::new(RefCell::new(data));
        scene.molecules[0].data = crate::moldata::MolData::Shared(Box::new(Shared(shared.clone())));
        let snapshot = SceneSnapshot::capture(&scene, &recipe()).unwrap();
        shared.borrow_mut().1.coords[0].x += 100.0;
        let copy = snapshot.into_scene(&Settings::default()).unwrap();
        assert_eq!(copy.molecules[0].data.state().coords[0], old);
        assert!(!copy.molecules[0].data.is_shared());
    }
}

#[cfg(test)]
mod gpu_tests {
    use super::*;
    use crate::animation::{Easing, TaskState, ViewAction, ViewStep};
    fn wait_background(app: &mut App, ctx: &egui::Context, handle: &AnimationHandle) {
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        while !handle.is_done() {
            // This is only progress polling. Rendering must advance on the worker.
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                app.service_background_movie(ui.ctx())
            });
            assert!(
                std::time::Instant::now() < deadline,
                "background render timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        app.service_background_movie(ctx);
    }
    fn wait_foreground(app: &mut App, ctx: &egui::Context, handle: &AnimationHandle) {
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        while !handle.is_done() {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                app.service_movie(ui);
            });
            assert!(
                std::time::Instant::now() < deadline,
                "foreground reference timed out"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        handle.wait().unwrap();
    }

    #[test]
    #[ignore = "requires GPU and MOLAR_VIS_DEBUG_DEFAULTS=1"]
    fn background_render_survives_live_scene_replacement_and_matches_foreground() {
        assert!(std::env::var_os("MOLAR_VIS_DEBUG_DEFAULTS").is_some());
        let ctx = egui::Context::default();
        let mut cc = eframe::CreationContext::_new_kittest(ctx.clone());
        cc.wgpu_render_state = Some(crate::render::test_gpu());
        let file = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/2lao.pdb"));
        let mut app = App::new(
            &cc,
            AppLaunch {
                files: vec![vec![file.clone()]],
                ..Default::default()
            },
        )
        .unwrap();
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/background-gpu-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let movie = Movie {
            steps: vec![ViewStep {
                action: ViewAction::Hold,
                duration: 0.25,
                easing: Easing::Linear,
            }],
            trajectories: Vec::new(),
        };
        for (raytrace, name) in [(false, "raster"), (true, "trace")] {
            if app.scene.molecules.is_empty() {
                app.add_loaded(data::load(&file).unwrap());
            }
            app.camera.depth_bounds = app.scene.visible_depth_bounds();
            let camera = app.camera;
            let reference = directory.join(format!("{name}-foreground"));
            let _ = std::fs::remove_dir_all(&reference);
            let foreground = app
                .render_movie(
                    &movie,
                    reference.clone(),
                    VideoOptions {
                        width: 64,
                        height: 48,
                        fps: 12,
                        samples: 2,
                        raytrace,
                        png_sequence: true,
                        ..Default::default()
                    },
                )
                .unwrap();
            wait_foreground(&mut app, &ctx, &foreground);
            let path = directory.join(name);
            let _ = std::fs::remove_dir_all(&path);
            let options = VideoOptions {
                width: 64,
                height: 48,
                fps: 12,
                samples: 2,
                raytrace,
                png_sequence: true,
                background: true,
                ..Default::default()
            };
            let handle = app.render_movie(&movie, path.clone(), options).unwrap();
            assert!(app.movie_job.is_none());
            assert!(app.background_movie.is_some());
            assert_eq!(app.camera, camera);
            // Host jobs must run immediately during rendering. New session must neither
            // cancel the worker nor be undone when its frozen movie finishes.
            let (tx, rx) = mpsc::channel();
            app.set_jobs(rx);
            let ran = Arc::new(AtomicBool::new(false));
            let done = ran.clone();
            tx.send(Box::new(move |app: &mut App| {
                app.new_session();
                app.rotate_view(45.0, 0.0);
                done.store(true, Ordering::Relaxed);
            }) as AppJob)
                .unwrap();
            app.run_external_jobs();
            assert!(ran.load(Ordering::Relaxed));
            assert!(app.scene.molecules.is_empty());
            let edited_camera = app.camera;
            wait_background(&mut app, &ctx, &handle);
            handle.wait().unwrap();
            assert_eq!(handle.progress().completed, 3);
            assert!(app.scene.molecules.is_empty());
            assert_eq!(app.camera, edited_camera);
            for i in 0..3 {
                let reference = image::open(reference.join(format!("frame_{i:06}.png")))
                    .unwrap()
                    .to_rgba8();
                let rendered = image::open(path.join(format!("frame_{i:06}.png")))
                    .unwrap()
                    .to_rgba8();
                let differing = rendered
                    .pixels()
                    .zip(reference.pixels())
                    .filter(|(a, b)| a != b)
                    .count();
                assert_eq!(
                    differing, 0,
                    "background {name} frame {i} must match the frozen foreground sequence"
                );
            }
        }
        app.add_loaded(data::load(&file).unwrap());
        let existing = directory.join("cancelled.mp4");
        std::fs::write(&existing, b"previous movie").unwrap();
        let handle = app
            .render_movie(
                &movie,
                existing.clone(),
                VideoOptions {
                    width: 64,
                    height: 48,
                    background: true,
                    ..Default::default()
                },
            )
            .unwrap();
        handle.cancel();
        app.new_session();
        let camera = app.camera;
        wait_background(&mut app, &ctx, &handle);
        assert!(matches!(handle.progress().state, TaskState::Cancelled));
        assert_eq!(std::fs::read(existing).unwrap(), b"previous movie");
        assert!(app.scene.molecules.is_empty());
        assert_eq!(app.camera, camera);
    }
}
