//! "Save image" — render the current view to a PNG at a window-independent resolution.
//!
//! The menu defers the request (`App::export_request`); `App::ui` services it here after
//! `draw_viewport`, where the eframe `Frame` (hence the wgpu render state) is available and
//! `last_size` is current. The renderer does the offscreen render + GPU→CPU readback
//! ([`SceneRenderer::capture_begin`]); native blocks on `device.poll(Wait)` and writes the
//! file via an `rfd` dialog, while wasm polls the readback each frame and triggers a browser
//! download (no filesystem).

use super::App;

/// Paths per pixel for the file (converged) ray trace. Each path contributes one AO +
/// one shadow sample, so this is also the AO/shadow sample count.
const RT_FILE_SAMPLES: u32 = 192;

impl App {
    /// Render the current view at `scale ×` the viewport and save it as a PNG.
    pub(super) fn export_image(&mut self, frame: &mut eframe::Frame, scale: u32) {
        let Some(rs) = frame.wgpu_render_state() else {
            self.status = "image export needs the wgpu backend".into();
            return;
        };
        let rs = rs.clone(); // Arc-backed; releases the borrow on `frame`.

        let [vw, vh] = self.last_size;
        let (out_w, out_h) = (vw.max(1) * scale.max(1), vh.max(1) * scale.max(1));

        // Prefer a full ray trace (the real "Render"); fall back to a high-res capture of
        // the rasterized view on WebGL2 (no compute) or when there's nothing to trace.
        let dashed = self.settings.behavior.dashed_pbc_bonds;
        let cap = if self.renderer.raytrace_supported() {
            self.renderer.prepare_raytrace(&rs, &self.scene, dashed);
            self.renderer
                .capture_begin_raytrace(&rs, out_w, out_h, &self.camera, RT_FILE_SAMPLES)
        } else {
            None
        };
        let cap = cap.unwrap_or_else(|| {
            let aspect = out_w as f32 / out_h as f32;
            let view = self.camera.view();
            let proj = self.camera.proj(aspect);
            self.renderer.capture_begin(
                &rs,
                out_w,
                out_h,
                view,
                proj,
                self.camera.is_perspective(),
                self.camera.cue_uniform(),
                self.camera.ao_uniform(),
                self.camera.shadow_uniform(),
                self.camera.background,
                self.camera.eye_depth_range(),
                &self.scene,
            )
        });

        #[cfg(not(target_arch = "wasm32"))]
        {
            // Drive the map to completion, then save synchronously.
            let _ = rs.device.poll(wgpu::PollType::wait_indefinitely());
            self.save_png_native(cap.read());
        }
        #[cfg(target_arch = "wasm32")]
        {
            // The browser drives the map; `poll_export` (each frame) finishes + downloads.
            self.pending_capture = Some((cap, "molar_vis.png".to_string()));
        }
    }

    /// Native: pop a save dialog and write the PNG.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_png_native(&mut self, img: image::RgbaImage) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG image", &["png"])
            .set_file_name("molar_vis.png")
            .save_file()
        else {
            return;
        };
        match img.save(&path) {
            Ok(()) => self.status = format!("saved image to {}", path.display()),
            Err(e) => {
                log::error!("save image: {e}");
                self.status = format!("save image failed: {e}");
            }
        }
    }

    /// Wasm: once the readback has completed, encode the PNG and trigger a download.
    /// Called each frame from `ui`; a no-op until the map resolves.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn poll_export(&mut self, ctx: &egui::Context) {
        let ready = self
            .pending_capture
            .as_ref()
            .is_some_and(|(c, _)| c.is_ready());
        if !ready {
            // Keep repainting so we re-check next frame (egui is otherwise idle).
            if self.pending_capture.is_some() {
                ctx.request_repaint();
            }
            return;
        }
        let (cap, name) = self.pending_capture.take().unwrap();
        let img = cap.read();
        let mut bytes = std::io::Cursor::new(Vec::new());
        match img.write_to(&mut bytes, image::ImageFormat::Png) {
            Ok(()) => {
                trigger_download(&name, &bytes.into_inner());
                self.status = format!("downloaded {name}");
            }
            Err(e) => {
                log::error!("encode png: {e}");
                self.status = format!("image export failed: {e}");
            }
        }
    }
}

/// Wasm: hand `bytes` to the browser as a download named `name` (Blob → object URL → a
/// synthetic `<a download>` click).
#[cfg(target_arch = "wasm32")]
fn trigger_download(name: &str, bytes: &[u8]) {
    use wasm_bindgen::JsCast as _;

    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&array);
    let Ok(blob) = web_sys::Blob::new_with_u8_array_sequence(&parts) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Ok(el) = doc.create_element("a") {
            if let Ok(a) = el.dyn_into::<web_sys::HtmlAnchorElement>() {
                a.set_href(&url);
                a.set_download(name);
                a.click();
            }
        }
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}
