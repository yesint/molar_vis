//! "Save image" — render the current view to a PNG at a window-independent resolution.
//!
//! The menu defers the request (`App::export_request`); `App::ui` services it here after
//! `draw_viewport`, where the eframe `Frame` (hence the wgpu render state) is available and
//! `last_size` is current. The renderer does the offscreen render + GPU→CPU readback
//! ([`SceneRenderer::capture_begin`]); native blocks on `device.poll(Wait)` and writes the
//! file via an `rfd` dialog, while wasm polls the readback each frame and triggers a browser
//! download (no filesystem).

use super::App;
#[cfg(not(target_arch = "wasm32"))]
use super::RtJob;

/// Paths per pixel for the file (converged) ray trace. Each path contributes one AO +
/// one shadow sample, so this is also the AO/shadow sample count. (Ray tracing needs compute
/// → native/WebGPU only; the WebGL2 wasm build uses the rasterized capture fallback.)
#[cfg(not(target_arch = "wasm32"))]
const RT_FILE_SAMPLES: u32 = 192;
/// Tile-submits per frame while pumping a Save trace (bounded per-frame GPU work → the UI
/// stays responsive with a "Saving…" overlay instead of freezing).
#[cfg(not(target_arch = "wasm32"))]
const SAVE_STEP_SUBMITS: u32 = 4;

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

        // Native + compute device: a full ray trace, **pumped across frames** so the app stays
        // responsive (with a "Saving…" overlay) instead of freezing. `service_rt_save` advances
        // it each frame and writes the PNG when done.
        #[cfg(not(target_arch = "wasm32"))]
        if self.renderer.raytrace_supported() {
            let dashed = self.settings.behavior.dashed_pbc_bonds;
            self.renderer.prepare_raytrace(&rs, &self.scene, dashed);
            // A Save trace shares the tracer with the R-key still — cancel any running still.
            if matches!(self.rt_job, Some(RtJob::Still)) {
                self.renderer.rt_trace_cancel();
            }
            self.rt_still = false;
            if self.renderer.save_begin(&rs, &self.camera, out_w, out_h, RT_FILE_SAMPLES) {
                self.rt_job = Some(RtJob::Save { out: [out_w, out_h], reading: None });
                self.status = "rendering image…".into();
                return;
            }
        }

        // Fallback: high-res capture of the rasterized view (WebGL2 / no compute), or the wasm
        // path (GI/ray tracing needs compute, unavailable on WebGL2).
        let aspect = out_w as f32 / out_h as f32;
        let view = self.camera.view();
        let proj = self.camera.proj(aspect);
        let cap = self.renderer.capture_begin(
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
        );

        #[cfg(not(target_arch = "wasm32"))]
        {
            // Raster capture is cheap → drive the map to completion and save synchronously.
            let _ = rs.device.poll(wgpu::PollType::wait_indefinitely());
            self.save_png_native(cap.read());
        }
        #[cfg(target_arch = "wasm32")]
        {
            // The browser drives the map; `poll_export` (each frame) finishes + downloads.
            self.pending_capture = Some((cap, "molar_vis.png".to_string()));
        }
    }

    /// Drive an in-progress frame-pumped "Save image" ray trace (native): advance the tiled
    /// trace a few tiles per frame, then read back + write the PNG once it converges. Keeps the
    /// UI responsive (a "Saving…" overlay shows meanwhile, drawn by the viewport).
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn service_rt_save(&mut self, frame: &mut eframe::Frame) {
        let Some(RtJob::Save { out, reading }) = self.rt_job.as_ref() else {
            return;
        };
        let out = *out;
        let reading_active = reading.is_some();
        let Some(rs) = frame.wgpu_render_state() else { return };
        let rs = rs.clone();

        if !reading_active {
            // Phase 1: pump the trace; when converged, kick off the GPU→CPU readback.
            if self.renderer.save_step(&rs, SAVE_STEP_SUBMITS) {
                match self.renderer.save_finish(&rs, out[0], out[1]) {
                    Some(rb) => {
                        if let Some(RtJob::Save { reading, .. }) = self.rt_job.as_mut() {
                            *reading = Some(rb);
                        }
                    }
                    None => self.rt_job = None,
                }
            }
            return;
        }

        // Phase 2: poll the readback (non-blocking); save when it lands.
        let _ = rs.device.poll(wgpu::PollType::Poll);
        let ready = matches!(
            self.rt_job.as_ref(),
            Some(RtJob::Save { reading: Some(rb), .. }) if rb.is_ready()
        );
        if ready {
            if let Some(RtJob::Save { reading: Some(rb), .. }) = self.rt_job.take() {
                self.save_png_native(rb.read());
            }
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
