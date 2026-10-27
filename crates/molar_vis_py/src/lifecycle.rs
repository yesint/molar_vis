//! Stop native viewers before Python's process exit handlers unload GPU drivers.
use crate::eframe::egui;
use pyo3::{exceptions::PyRuntimeError, prelude::*};
use std::{
    sync::{Arc, Mutex, OnceLock},
    thread::JoinHandle,
    time::Duration,
};

struct Viewer {
    context: Arc<OnceLock<egui::Context>>,
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
    let mut registry = VIEWERS.lock().unwrap();
    if registry.shutting_down {
        return Err(PyRuntimeError::new_err(
            "viewer shutdown has already started",
        ));
    }
    let context = Arc::new(OnceLock::new());
    let ui_context = context.clone();
    // Registration and thread creation share the lock, so shutdown cannot miss
    // a thread that has started but has not yet published its egui context.
    let thread = std::thread::Builder::new()
        .name("molar_vis-ui".into())
        .spawn(move || run(ui_context))
        .map_err(|error| {
            PyRuntimeError::new_err(format!("failed to start viewer thread: {error}"))
        })?;
    registry.viewers.push(Viewer { context, thread });
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
