//! Keep the native window responsive while the molecular renderer creates pipelines.

pub(super) struct Context {
    pub egui_ctx: eframe::egui::Context,
    #[cfg(not(target_arch = "wasm32"))]
    pub wgpu_render_state: Option<eframe::egui_wgpu::RenderState>,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::NativeStartup;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::Context;
    use crate::{app::App, launch::AppLaunch, render::SceneRenderer, settings::Settings};
    use eframe::egui;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    const MESSAGE_DELAY: Duration = Duration::from_millis(500);
    const MESSAGE: &str = "Compiling shaders...";

    struct Pending {
        context: Context,
        launch: AppLaunch,
        settings: Settings,
        result: mpsc::Receiver<SceneRenderer>,
        started: Instant,
        announced: bool,
    }

    pub(crate) struct NativeStartup {
        pending: Option<Pending>,
        app: Option<App>,
        error: Option<String>,
        capture: crate::app::RtState,
    }

    impl NativeStartup {
        pub(crate) fn new(
            cc: &eframe::CreationContext<'_>,
            launch: AppLaunch,
        ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
            let started = Instant::now();
            let settings = App::startup_settings(&cc.egui_ctx);
            let render_state = cc
                .wgpu_render_state
                .clone()
                .ok_or("wgpu render state unavailable (eframe must use the wgpu backend)")?;
            let rendering = settings.rendering.clone();
            let context = Context {
                egui_ctx: cc.egui_ctx.clone(),
                wgpu_render_state: Some(render_state.clone()),
            };
            let ctx = cc.egui_ctx.clone();
            let (tx, result) = mpsc::channel();
            std::thread::Builder::new()
                .name("molar-vis-shaders".into())
                .spawn(move || {
                    // If pipeline creation panics, disconnect the channel and repaint
                    // so the window reports failure instead of spinning indefinitely.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        SceneRenderer::new(&render_state, &rendering)
                    }));
                    if let Ok(renderer) = result {
                        let _ = tx.send(renderer);
                    }
                    drop(tx);
                    ctx.request_repaint();
                })?;
            Ok(Self {
                pending: Some(Pending {
                    context,
                    launch,
                    settings,
                    result,
                    started,
                    announced: false,
                }),
                app: None,
                error: None,
                capture: Default::default(),
            })
        }

        fn poll(&mut self) {
            let Some(pending) = &self.pending else { return };
            let renderer = match pending.result.try_recv() {
                Ok(renderer) => renderer,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.error = Some("Shader initialization failed.".into());
                    eprintln!("Shader initialization failed.");
                    return;
                }
            };
            let pending = self.pending.take().unwrap();
            match App::from_renderer(&pending.context, pending.launch, pending.settings, renderer) {
                Ok(app) => self.app = Some(app),
                Err(error) => {
                    eprintln!("molar_vis failed to start: {error}");
                    self.error = Some(format!("molar_vis failed to start: {error}"));
                }
            }
        }
    }

    fn draw_progress(ui: &mut egui::Ui, elapsed: Duration) -> bool {
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        if elapsed <= MESSAGE_DELAY {
            ui.ctx()
                .request_repaint_after(MESSAGE_DELAY.saturating_sub(elapsed));
            return false;
        }
        egui::Modal::new(egui::Id::new("shader-startup-progress")).show(ui.ctx(), |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(MESSAGE);
            });
        });
        ui.ctx().set_cursor_icon(egui::CursorIcon::Wait);
        ui.ctx().request_repaint_after(Duration::from_millis(16));
        true
    }

    impl eframe::App for NativeStartup {
        fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
            self.poll();
            if let Some(app) = &mut self.app {
                app.logic(ctx, frame);
            }
        }

        fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
            // Check completion before the timer, so a ready renderer never flashes
            // the message even if the event loop delivers this frame late.
            self.poll();
            if let Some(app) = &mut self.app {
                app.ui(ui, frame);
            } else if let Some(pending) = &mut self.pending {
                if draw_progress(ui, pending.started.elapsed()) {
                    if !pending.announced {
                        eprintln!("{MESSAGE}");
                        pending.announced = true;
                    }
                    if std::env::var_os("MOLAR_VIS_DEBUG_CAPTURE_SHADERS").is_some() {
                        self.capture.service_debug_ui_capture(ui.ctx());
                    }
                }
            } else if let Some(error) = &self.error {
                ui.label(error);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn shader_message_appears_only_after_half_a_second() {
            let ctx = egui::Context::default();
            for (millis, expected) in [(0, false), (499, false), (500, false), (501, true)] {
                let mut output = None;
                for _ in 0..3 {
                    output = Some(ctx.run_ui(egui::RawInput::default(), |ui| {
                        draw_progress(ui, Duration::from_millis(millis));
                    }));
                }
                let visible = output.unwrap().shapes.iter().any(|shape| {
                    matches!(
                        &shape.shape,
                        egui::Shape::Text(label) if label.galley.text() == MESSAGE
                    )
                });
                assert_eq!(visible, expected, "at {millis} ms");
            }
        }

        #[test]
        fn failed_worker_leaves_the_startup_screen() {
            let (tx, result) = mpsc::channel();
            let mut startup = NativeStartup {
                pending: Some(Pending {
                    context: Context {
                        egui_ctx: Default::default(),
                        wgpu_render_state: None,
                    },
                    launch: Default::default(),
                    settings: Default::default(),
                    result,
                    started: Instant::now(),
                    announced: false,
                }),
                app: None,
                error: None,
                capture: Default::default(),
            };
            startup.poll();
            assert!(startup.pending.is_some());
            assert!(startup.error.is_none());
            drop(tx);
            startup.poll();
            assert!(startup.pending.is_none());
            assert_eq!(
                startup.error.as_deref(),
                Some("Shader initialization failed.")
            );
        }
    }
}
