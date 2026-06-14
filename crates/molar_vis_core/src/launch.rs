//! Application launch parameters and the eframe entry point.

use std::path::PathBuf;

use crate::app::App;

/// Parameters handed to the viewer at startup. Built by the binary from argv
/// (and later by a web shell from URL params), so this struct is the single
/// platform-agnostic launch surface.
#[derive(Debug, Default, Clone)]
pub struct AppLaunch {
    /// Structure/trajectory files to load on startup (PDB/GRO/… — anything molar reads).
    pub files: Vec<PathBuf>,
}

/// Launch the native viewer window. Returns once the window is closed.
/// Native-only: the web build uses `eframe::WebRunner` from a wasm entry point.
#[cfg(not(target_arch = "wasm32"))]
pub fn run(launch: AppLaunch) -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("molar_vis")
            .with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        "molar_vis",
        native_options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, launch)?))),
    )
}
