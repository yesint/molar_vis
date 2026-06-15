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

/// Launch the viewer in a browser, rendering into the `<canvas id="molar_vis_canvas">`
/// of the host page. wasm-only entry point (the `molar_vis_web` binary calls this);
/// the native build uses [`run`]. Starts eframe's `WebRunner` (wgpu backend, with a
/// WebGL2 fallback) on the async executor and returns immediately.
#[cfg(target_arch = "wasm32")]
pub fn run_web() {
    use wasm_bindgen::JsCast as _;

    // Route Rust panics and `log` output to the browser console. Info level:
    // shows eframe/wgpu adapter + backend selection and any warnings/errors,
    // without naga's very chatty per-expression shader-compilation debug logs.
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    log::info!("molar_vis: starting web runner");

    let web_options = eframe::WebOptions::default();
    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window()
            .and_then(|w| w.document())
            .expect("no document");
        let canvas = document
            .get_element_by_id("molar_vis_canvas")
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("page must contain a <canvas id=\"molar_vis_canvas\">");

        let result = eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| {
                    let mut app = App::new(cc, AppLaunch::default())?;
                    // Open to a bundled molecule so the demo isn't an empty viewport.
                    app.load_demo();
                    Ok(Box::new(app))
                }),
            )
            .await;

        // Surface a startup failure both to the console and into the page (the
        // `#loading` element), so the cause is visible without opening devtools.
        if let Err(e) = result {
            let msg = format!("molar_vis failed to start: {e:?}");
            log::error!("{msg}");
            if let Some(el) = document.get_element_by_id("loading") {
                el.set_text_content(Some(&msg));
            }
        } else {
            log::info!("molar_vis: web runner started");
        }
    });
}
