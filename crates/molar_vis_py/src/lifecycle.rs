//! Stop native viewers before Python's process exit handlers unload GPU drivers.
use crate::eframe::egui;
use pyo3::{exceptions::PyRuntimeError, prelude::*};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread::JoinHandle,
    time::Duration,
};

struct Viewer {
    context: Arc<OnceLock<egui::Context>>,
    /// Set at shutdown. A windowed viewer stops on `ViewportCommand::Close`; a headless
    /// host has no viewport to close, so it polls this instead.
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

struct Registry {
    shutting_down: bool,
    viewers: Vec<Viewer>,
}

static VIEWERS: Mutex<Registry> = Mutex::new(Registry {
    shutting_down: false,
    viewers: Vec::new(),
});

pub(super) fn spawn(
    run: impl FnOnce(Arc<OnceLock<egui::Context>>) + Send + 'static,
) -> PyResult<()> {
    spawn_with_stop(move |context, _stop| run(context))
}

/// As [`spawn`], but the thread also receives the shutdown flag. Used by the headless
/// host, which has no egui viewport for `stop_and_join` to send a close command to.
pub(super) fn spawn_with_stop(
    run: impl FnOnce(Arc<OnceLock<egui::Context>>, Arc<AtomicBool>) + Send + 'static,
) -> PyResult<()> {
    let mut registry = VIEWERS.lock().unwrap();
    if registry.shutting_down {
        return Err(PyRuntimeError::new_err(
            "viewer shutdown has already started",
        ));
    }
    let context = Arc::new(OnceLock::new());
    let ui_context = context.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    // Registration and thread creation share the lock, so shutdown cannot miss
    // a thread that has started but has not yet published its egui context.
    let thread = std::thread::Builder::new()
        .name("molar_vis-ui".into())
        .spawn(move || run(ui_context, thread_stop))
        .map_err(|error| {
            PyRuntimeError::new_err(format!("failed to start viewer thread: {}", molar_vis_core::error_chain(&error)))
        })?;
    registry.viewers.push(Viewer { context, stop, thread });
    Ok(())
}

#[pyfunction(name = "_shutdown_viewers")]
pub(super) fn shutdown(py: Python<'_>) {
    let viewers = {
        let mut registry = VIEWERS.lock().unwrap();
        registry.shutting_down = true;
        std::mem::take(&mut registry.viewers)
    };
    // Shared molecular sources may acquire the GIL while the UI finishes its
    // last frame. Holding it while joining would deadlock those viewers.
    py.detach(move || stop_and_join(viewers));
}

fn stop_and_join(viewers: Vec<Viewer>) {
    // Close all viewers together, including contexts published during shutdown.
    // Repeating the request also handles a close sent before the first UI frame.
    // There is deliberately no timeout that abandons a live rendering thread:
    // returning to process teardown would reintroduce the driver-unload race.
    while viewers.iter().any(|viewer| !viewer.thread.is_finished()) {
        for viewer in &viewers {
            if !viewer.thread.is_finished() {
                viewer.stop.store(true, Ordering::Relaxed);
                if let Some(ctx) = viewer.context.get() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    ctx.request_repaint();
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    for viewer in viewers {
        if viewer.thread.join().is_err() {
            log::warn!("molar_vis viewer thread panicked before shutdown");
        }
    }
}
