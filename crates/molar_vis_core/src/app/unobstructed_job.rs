//! Interactive native search. Workers own an atom snapshot and never mutate the app.
use super::*;
use glam::Vec3;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

pub(super) struct ViewJob {
    started: Instant,
    cancelled: Arc<AtomicBool>,
    result: mpsc::Receiver<Result<Vec3, String>>,
    visual_bounds: Vec<(Vec3, f32)>,
}

enum JobPoll {
    Pending,
    Cancelled,
    Finished(Result<Vec3, String>),
}

impl ViewJob {
    fn poll(&self, ctx: &egui::Context) -> JobPoll {
        // Cancellation wins even when a completed result is already queued.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            return JobPoll::Cancelled;
        }
        match self.result.try_recv() {
            Ok(result) => JobPoll::Finished(result),
            Err(mpsc::TryRecvError::Disconnected) => {
                JobPoll::Finished(Err("Unobstructed view worker stopped".into()))
            }
            Err(mpsc::TryRecvError::Empty) => JobPoll::Pending,
        }
    }

    fn feedback_due(&self) -> bool {
        self.started.elapsed() >= Duration::from_millis(500)
    }
}

impl Drop for ViewJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl App {
    pub(super) fn start_unobstructed_view(
        &mut self,
        mol: usize,
        rep: usize,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let started = Instant::now();
        let mut atoms = build::gather_unobstructed_atoms(&mut self.scene, &[(mol, rep)])?;
        self.view_dirty = true;
        let visual_bounds = std::mem::take(&mut atoms.visual_bounds);
        let rs = self.render_state.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let repaint = ctx.clone();
        let (tx, result) = mpsc::channel();
        std::thread::Builder::new()
            .name("unobstructed-view".into())
            .spawn(move || {
                let is_cancelled = || cancel.load(Ordering::Relaxed);
                let gpu = rs
                    .as_ref()
                    .filter(|rs| {
                        atoms.target.len() >= 1024
                            && rs.adapter.get_info().device_type != wgpu::DeviceType::Cpu
                            && std::env::var_os("MOLAR_VIS_DEBUG_UNOBSTRUCTED_CPU").is_none()
                            && rs
                                .adapter
                                .get_downlevel_capabilities()
                                .flags
                                .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS)
                    })
                    .and_then(|rs| {
                        crate::render::unobstructed::UnobstructedGpu::new(&rs.device).map(
                            |scorer| {
                                scorer.best_direction_cancellable(
                                    &rs.device,
                                    &rs.queue,
                                    &atoms.target,
                                    &atoms.occluders,
                                    256,
                                    &is_cancelled,
                                )
                            },
                        )
                    });
                let gpu_used = matches!(&gpu, Some(Ok(_)));
                let direction = match gpu {
                    Some(Ok(dir)) => Ok(dir),
                    other => {
                        if let Some(Err(e)) = other {
                            if !is_cancelled() {
                                log::warn!("unobstructed GPU search failed; using CPU: {e}");
                            }
                        }
                        crate::unobstructed::best_unobstructed_direction_cancellable(
                            &atoms.target,
                            &atoms.occluders,
                            256,
                            &is_cancelled,
                        )
                    }
                };
                if direction.is_ok() && !is_cancelled() {
                    log::info!(
                        "unobstructed view: {}, {} target atoms, {} occluders, {:.1} ms",
                        if gpu_used { "GPU" } else { "CPU" },
                        atoms.target.len(),
                        atoms.occluders.len(),
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                }
                let _ = tx.send(direction);
                repaint.request_repaint();
            })
            .map_err(|e| format!("Cannot start unobstructed view: {e}"))?;
        self.unobstructed_job = Some(ViewJob {
            started,
            cancelled,
            result,
            visual_bounds,
        });
        ctx.request_repaint();
        Ok(())
    }

    /// Esc wins even if the worker completed just before this frame.
    pub(super) fn service_unobstructed_job(&mut self, ctx: &egui::Context) -> bool {
        let Some(job) = self.unobstructed_job.as_ref() else {
            return false;
        };
        // Startup searches can finish before the first viewport has been laid out.
        // Render that first frame before using its aspect ratio to apply the fit.
        if self.last_size.iter().any(|&size| size <= 1) {
            ctx.request_repaint();
            return true;
        }
        match job.poll(ctx) {
            JobPoll::Cancelled => {
                self.unobstructed_job = None;
                self.status = "Unobstructed view cancelled".into();
                false
            }
            JobPoll::Finished(result) => {
                let job = self.unobstructed_job.take().unwrap();
                match result {
                    Ok(dir) => {
                        self.camera.orientation = crate::unobstructed::look_along_quat(dir);
                        let [width, height] = self.last_size;
                        self.camera.focus_visual_bounds(&job.visual_bounds,
                            width.max(1) as f32 / height.max(1) as f32, 1.0);
                        self.view_dirty = true;
                    }
                    Err(e) => {
                        self.status = e;
                    }
                }
                false
            }
            JobPoll::Pending => {
                ctx.request_repaint_after(Duration::from_millis(16));
                true
            }
        }
    }

    pub(super) fn draw_unobstructed_progress(&self, ctx: &egui::Context) {
        if self
            .unobstructed_job
            .as_ref()
            .is_some_and(|job| job.feedback_due())
        {
            egui::Modal::new(egui::Id::new("unobstructed-progress")).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Computing... Press Esc to cancel");
                });
            });
            ctx.set_cursor_icon(egui::CursorIcon::Wait);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_wins_over_a_completed_result() {
        let ctx = egui::Context::default();
        let (tx, result) = mpsc::channel();
        tx.send(Ok(Vec3::Z)).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let job = ViewJob {
            started: Instant::now(),
            cancelled: cancelled.clone(),
            result,
            visual_bounds: vec![(Vec3::ZERO, 1.0)],
        };
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        });
        assert!(matches!(job.poll(&ctx), JobPoll::Cancelled));
        // Esc is consumed so other app modes do not act on it.
        assert!(!ctx.input(|i| i.key_pressed(egui::Key::Escape)));
        drop(job);
        assert!(cancelled.load(Ordering::Relaxed));
        let _ = ctx.end_pass();
    }

    #[test]
    fn feedback_is_delayed_and_dropping_job_cancels_worker() {
        let (_tx, result) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut job = ViewJob {
            started: Instant::now(),
            cancelled: cancelled.clone(),
            result,
            visual_bounds: vec![(Vec3::ZERO, 1.0)],
        };
        assert!(!job.feedback_due());
        job.started = Instant::now() - Duration::from_millis(501);
        assert!(job.feedback_due());
        assert!(!cancelled.load(Ordering::Relaxed));
        drop(job);
        assert!(cancelled.load(Ordering::Relaxed));
    }
}
