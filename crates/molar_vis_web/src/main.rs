//! Browser entry point for molar_vis. `trunk` compiles this to wasm and calls
//! `main`, which hands off to [`molar_vis_core::run_web`] (the eframe `WebRunner`).
//! All viewer logic lives in `molar_vis_core`; this crate is just the wasm shell,
//! mirroring the native `molar_vis` binary.

#[cfg(target_arch = "wasm32")]
fn main() {
    molar_vis_core::run_web();
}

// The crate is a workspace member, so a plain `cargo build` compiles it for the
// host too — there's no browser there, so just point the user at the right tools.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!(
        "molar_vis_web is the browser build. Build/serve it with `trunk serve` from \
         crates/molar_vis_web. For the desktop app, run the `molar_vis` binary."
    );
}
