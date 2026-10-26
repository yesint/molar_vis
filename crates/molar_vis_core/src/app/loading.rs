//! Loading feedback and native read/parse workers. Scene changes stay on the UI thread.
use super::*;

#[cfg(not(target_arch = "wasm32"))]
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

#[cfg(not(target_arch = "wasm32"))]
type Commit = Box<dyn FnOnce(&mut App) + Send>;

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct Feedback {
    started: Instant,
    announced: bool,
    terminal: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl Feedback {
    pub(super) fn new() -> Self {
        Self {
            started: Instant::now(),
            announced: false,
            terminal: false,
        }
    }
    fn due(&self) -> bool {
        self.started.elapsed() >= Duration::from_secs(1)
    }
    fn announce(&mut self) {
        self.announce_elapsed(self.started.elapsed());
    }
    fn announce_elapsed(&mut self, elapsed: Duration) {
        if self.terminal && elapsed >= Duration::from_secs(1) && !self.announced {
            eprintln!("Loading...");
            self.announced = true;
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct Job {
    feedback: Feedback,
    result: mpsc::Receiver<(Duration, Result<Commit, String>)>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Job {
    fn spawn<T: Send + 'static>(
        ctx: egui::Context,
        read: impl FnOnce() -> Result<T, String> + Send + 'static,
        apply: impl FnOnce(&mut App, T) + Send + 'static,
    ) -> Self {
        let (tx, result) = mpsc::channel();
        let feedback = Feedback::new();
        let started = feedback.started;
        std::thread::spawn(move || {
            let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(read))
                .map_err(|_| "file loading worker failed".to_string())
                .and_then(|result| result)
                .map(|loaded| Box::new(move |app: &mut App| apply(app, loaded)) as Commit);
            let _ = tx.send((started.elapsed(), loaded));
            ctx.request_repaint();
        });
        Self { feedback, result }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn loading_modal(ctx: &egui::Context) {
    egui::Modal::new(egui::Id::new("file-loading-progress")).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Loading...");
        });
    });
    ctx.set_cursor_icon(egui::CursorIcon::Wait);
}

impl App {
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn begin_loading<T: Send + 'static>(
        &mut self,
        read: impl FnOnce() -> Result<T, String> + Send + 'static,
        apply: impl FnOnce(&mut App, T) + Send + 'static,
    ) {
        self.loading_jobs
            .push_back(Job::spawn(self.loading_context.clone(), read, apply));
        self.loading_context.request_repaint();
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn install_startup(&mut self, loaded: Startup) {
        let errors = loaded.install(&mut self.scene, &self.rep_defaults, &self.settings);
        for error in &errors {
            log::error!("{error}");
        }
        self.scene.selected_mol = (!self.scene.molecules.is_empty()).then_some(0);
        // Frame the loaded geometry while preserving the already initialized
        // theme, background, lighting, and projection.
        self.reset_view_immediate();
        self.status = if errors.is_empty() {
            format!("{} molecule(s) loaded", self.scene.molecules.len())
        } else {
            errors.join("\n")
        };
        self.history = History::new(EditState::capture(&self.scene));
        self.view_dirty = true;
    }

    /// Poll without blocking. Applying completed data is the only scene mutation.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn service_loading(&mut self, ctx: &egui::Context) -> bool {
        while let Some(mut job) = self.loading_jobs.pop_front() {
            match job.result.try_recv() {
                Ok((elapsed, result)) => {
                    job.feedback.announce_elapsed(elapsed);
                    match result {
                        Ok(apply) => apply(self),
                        Err(error) => {
                            log::error!("{error}");
                            self.status = error;
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.status = "file loading worker disconnected".into();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    job.feedback.announce();
                    self.loading_jobs.push_front(job);
                    ctx.request_repaint_after(Duration::from_millis(16));
                    return true;
                }
            }
        }
        false
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn loading_feedback_visible(&self) -> bool {
        self.loading_jobs
            .front()
            .is_some_and(|job| job.feedback.due())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn draw_loading_progress(&self, ctx: &egui::Context) {
        if self.loading_feedback_visible() {
            loading_modal(ctx);
        }
    }

    pub(super) fn async_loading_active(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            !self.scene.loaders.is_empty()
        }
        #[cfg(target_arch = "wasm32")]
        {
            !self.scene.wasm_loaders.is_empty()
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn mark_command_line_loading(&mut self) {
        if let Some(job) = self.loading_jobs.back_mut() {
            job.feedback.terminal = true;
        }
    }

    pub(super) fn draw_async_loading_progress(&self, ctx: &egui::Context) {
        if !self.async_loading_active() {
            return;
        }
        let loaded: usize = self
            .async_load_progress
            .values()
            .map(|progress| progress.loaded)
            .sum();
        let total: Option<usize> = self
            .async_load_progress
            .values()
            .map(|progress| progress.total)
            .try_fold(0usize, |sum, total| total.map(|n| sum.saturating_add(n)));
        let total = if self.async_load_progress.is_empty() {
            None
        } else {
            total
        };
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut bottom = 12.0;
        #[cfg(not(target_arch = "wasm32"))]
        if self.background_movie.is_some() {
            if let Some(rect) = ctx.data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new("background-movie-progress-rect"))
            }) {
                bottom += rect.height() + 12.0;
            }
        }
        async_loading_panel(ctx, loaded, total, bottom);
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

pub(super) struct AsyncProgress {
    pub(super) loaded: usize,
    total: Option<usize>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) request: Option<super::loaders::LoadDialog>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) record_index: Option<usize>,
}

impl AsyncProgress {
    pub(super) fn new(options: &LoadOptions) -> Self {
        Self {
            loaded: 0,
            #[cfg(not(target_arch = "wasm32"))]
            request: None,
            #[cfg(not(target_arch = "wasm32"))]
            record_index: None,
            total: options.to.map(|to| {
                (to.saturating_sub(options.from) / options.stride.max(1)).saturating_add(1)
            }),
        }
    }
}

fn async_loading_panel(
    ctx: &egui::Context,
    loaded: usize,
    total: Option<usize>,
    bottom: f32,
) -> egui::Rect {
    super::widgets::corner_progress_panel(ctx, "async-loading-progress", bottom, |ui| {
        ui.horizontal(|ui| {
            ui.label(format!("{}  Loading...", icon::FOLDER_OPEN));
            if total.is_none() {
                ui.spinner();
            }
        });
        let (fraction, text) = match total {
            Some(total) => (
                loaded as f32 / total.max(1) as f32,
                format!("{loaded} / {total} frames"),
            ),
            None => (0.0, format!("{loaded} frames")),
        };
        let mut bar = egui::ProgressBar::new(fraction)
            .text(egui::RichText::new(text).color(ui.visuals().text_color()));
        if total.is_none() {
            // No percentage is known: keep the track clear beneath the counter.
            bar = bar.fill(ui.visuals().extreme_bg_color);
        }
        ui.add(bar);
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct StartupGroup {
    records: Vec<data::RawMolecule>,
    path: PathBuf,
    frames: Vec<(TrajLoad, Vec<State>)>,
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct Startup {
    groups: Vec<StartupGroup>,
    errors: Vec<String>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Startup {
    pub(super) fn read(files: &[Vec<PathBuf>], bonds: data::bonds::BondParams) -> Self {
        let mut groups = Vec::new();
        let mut errors = Vec::new();
        for files in files {
            let Some((path, extra)) = files.split_first() else {
                continue;
            };
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let records = if extra.is_empty() && matches!(ext.as_str(), "sdf" | "sd" | "mol") {
                data::load_records(path, &bonds)
            } else {
                data::load_with(path, &bonds).map(|raw| vec![raw])
            };
            let records = match records {
                Ok(records) if !records.is_empty() => records,
                Ok(_) => {
                    errors.push(format!("no molecules in {}", path.display()));
                    continue;
                }
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };
            let mut frames = Vec::new();
            if records.len() == 1 {
                let n = records[0].n_atoms;
                for (path, from) in std::iter::once((path, 1)).chain(extra.iter().map(|p| (p, 0))) {
                    let opts = LoadOptions {
                        from,
                        to: None,
                        stride: 1,
                    };
                    match data::traj_loader::read_frames_sync(path, &opts, n) {
                        Ok(read) if !read.is_empty() => frames.push((
                            TrajLoad {
                                path: path.clone(),
                                from,
                                to: None,
                                stride: 1,
                            },
                            read,
                        )),
                        Ok(_) => (),
                        Err(e) => errors.push(format!("{}: {e}", path.display())),
                    }
                }
            }
            groups.push(StartupGroup {
                records,
                path: path.clone(),
                frames,
            });
        }
        Self { groups, errors }
    }

    pub(super) fn install(
        self,
        scene: &mut Scene,
        defaults: &RepDefaults,
        settings: &Settings,
    ) -> Vec<String> {
        for group in self.groups {
            let StartupGroup {
                mut records,
                path,
                frames,
            } = group;
            if records.len() > 1 {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                scene.add_group(records, MoleculeSource::File(path), name, defaults);
            } else if let Some(mut raw) = records.pop() {
                raw.source = MoleculeSource::File(path);
                let id = scene.add(raw, defaults);
                let mi = scene.mol_index(id).unwrap();
                let mol = &mut scene.molecules[mi];
                mol.trajectory.speed_fps = settings.behavior.traj_fps;
                mol.trajectory.loop_mode = settings.behavior.loop_mode;
                if !frames.is_empty() {
                    mol.seed_frame0();
                }
                for (record, frames) in frames {
                    mol.append_frames(frames);
                    mol.traj_loads.push(record);
                }
                mol.apply_current_frame();
            }
        }
        self.errors
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn feedback_waits_one_second_and_announces_once() {
        let mut gui = Feedback::new();
        gui.announce_elapsed(Duration::from_secs(2));
        assert!(
            !gui.announced,
            "GUI loads must remain silent in the terminal"
        );
        let mut feedback = Feedback::new();
        feedback.terminal = true;
        assert!(!feedback.due());
        feedback.announce_elapsed(Duration::from_millis(999));
        assert!(!feedback.announced);
        feedback.started -= Duration::from_secs(1);
        assert!(feedback.due());
        feedback.announce();
        assert!(feedback.announced);
        feedback.announce();
        assert!(feedback.announced);
        // A fast read stays silent even if the UI receives its result late.
        let mut fast = Feedback {
            started: Instant::now() - Duration::from_secs(2),
            announced: false,
            terminal: true,
        };
        fast.announce_elapsed(Duration::from_millis(10));
        assert!(!fast.announced);
    }

    #[test]
    fn async_panel_shows_frame_counts_in_the_movie_panel_corner() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0));
        let options = LoadOptions {
            from: 10,
            to: Some(30),
            stride: 3,
        };
        assert_eq!(AsyncProgress::new(&options).total, Some(7));
        for (total, text) in [(Some(100), "7 / 100 frames"), (None, "7 frames")] {
            let mut output = None;
            let mut panel = egui::Rect::NOTHING;
            for _ in 0..3 {
                output = Some(ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| {
                        ui.label("Existing interface");
                        panel = async_loading_panel(&ctx, 7, total, 12.0);
                        assert!(ui.is_enabled());
                    },
                ));
            }
            let output = output.unwrap();
            assert!((panel.right() - screen.right() + 12.0).abs() < 1.0);
            assert!((panel.bottom() - screen.bottom() + 12.0).abs() < 1.0);
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(label) if label.galley.text() == text)));
            let counter_top = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(label) if label.galley.text() == text => Some(label.pos.y),
                    _ => None,
                })
                .unwrap();
            let spinners: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Path(path) if path.stroke.width == 3.0 => Some(path),
                    _ => None,
                })
                .collect();
            assert_eq!(spinners.len(), usize::from(total.is_none()));
            assert!(
                spinners
                    .iter()
                    .all(|path| path.points.iter().all(|point| point.y < counter_top)),
                "the spinner must stay above the frame counter"
            );
        }
    }

    #[test]
    fn worker_is_nonblocking_and_delivers_errors_without_a_scene_commit() {
        let (release, wait) = mpsc::channel();
        let job = Job::spawn(
            egui::Context::default(),
            move || {
                wait.recv().unwrap();
                Err::<(), _>("unreadable trajectory".into())
            },
            |_, ()| panic!("failed reads must never modify the scene"),
        );
        assert!(matches!(
            job.result.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release.send(()).unwrap();
        let (_, result) = job.result.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(result, Err(error) if error == "unreadable trajectory"));
    }

    #[test]
    fn loading_modal_is_centered_and_dims_the_interface() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0));
        let mut output = None;
        for _ in 0..3 {
            output = Some(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| {
                    ui.label("Existing interface");
                    loading_modal(&ctx);
                },
            ));
        }
        let output = output.unwrap();
        let center = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Loading..." => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap();
        assert!((center.y - screen.center().y).abs() < 20.0);
        assert!((center.x - screen.center().x).abs() < 40.0);
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Rect(rect) if rect.rect == screen && rect.fill.a() > 0 && rect.fill.a() < 255)));
        assert_eq!(output.platform_output.cursor_icon, egui::CursorIcon::Wait);
    }

    #[test]
    fn startup_preparation_preserves_groups_and_structure_frame_zero() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests");
        let prepared = Startup::read(
            &[
                vec![root.join("2lao.pdb"), root.join("2lao.pdb")],
                vec![root.join("ligands20.sdf")],
                vec![root.join("missing-structure.pdb")],
            ],
            data::bonds::BondParams::default(),
        );
        let mut scene = Scene::default();
        let settings = Settings::default();
        let errors = prepared.install(&mut scene, &settings.reps, &settings);
        assert_eq!(errors.len(), 1);
        assert_eq!(scene.groups.len(), 1);
        assert_eq!(scene.groups[0].members.len(), 20);
        let mol = &scene.molecules[0];
        assert_eq!(mol.trajectory.frames.len(), 2);
        assert_eq!(mol.trajectory.current, 0);
        assert_eq!(mol.trajectory.frames[0].coords, mol.data.state().coords);
        assert_eq!(mol.traj_loads.len(), 1);
        assert_eq!(mol.traj_loads[0].from, 0);
    }

    #[test]
    #[ignore = "requires native GPU; checks blocking-load completion, error cleanup and async feedback"]
    fn jobs_apply_on_the_ui_thread_and_async_loading_stays_interactive() {
        let ctx = egui::Context::default();
        let mut cc = eframe::CreationContext::_new_kittest(ctx.clone());
        cc.wgpu_render_state = Some(crate::render::test_gpu());
        let mut app = App::new(&cc, AppLaunch::default()).unwrap();
        let (release, wait) = mpsc::channel();
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/2lao.pdb");
        app.begin_loading(
            move || {
                wait.recv().unwrap();
                data::load(&file)
            },
            |app, raw| app.add_loaded(raw),
        );
        assert!(app.service_loading(&ctx));
        assert!(app.scene.molecules.is_empty());
        assert!(!app.loading_feedback_visible());
        app.loading_jobs.front_mut().unwrap().feedback.started -= Duration::from_secs(1);
        assert!(app.loading_feedback_visible());
        assert!(app.service_loading(&ctx));
        assert!(
            !app.loading_jobs.front().unwrap().feedback.announced,
            "GUI loads must not print"
        );
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.service_loading(&ctx) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(app.scene.molecules.len(), 1);
        assert!(!app.loading_feedback_visible());
        let id = app.scene.molecules[0].id;
        let (tx, rx) = mpsc::channel();
        app.scene.loaders.insert(id, rx);
        app.async_load_progress
            .insert(id, AsyncProgress::new(&LoadOptions::default()));
        tx.send(LoadMsg::Frame(app.scene.molecules[0].data.state().clone()))
            .unwrap();
        app.poll_loaders();
        assert_eq!(app.async_load_progress[&id].loaded, 1);
        assert!(app.async_loading_active());
        assert!(
            !app.service_loading(&ctx),
            "streaming loads must not block the UI"
        );
        tx.send(LoadMsg::Done).unwrap();
        app.poll_loaders();
        assert!(!app.async_loading_active());
        assert!(app.async_load_progress.is_empty());
        app.reset_document();
        let background = app.camera.background;
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/2lao.pdb");
        app.begin_loading(
            move || {
                Ok(Startup::read(
                    &[vec![file]],
                    data::bonds::BondParams::default(),
                ))
            },
            App::install_startup,
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.service_loading(&ctx) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(app.scene.molecules.len(), 1);
        assert_eq!(
            app.camera.background, background,
            "deferred startup must preserve rendering settings"
        );
    }
}
