//! molar_vis_core — all viewer logic, kept WASM-safe (no native-only deps live here).
//!
//! The crate exposes [`run`] (the eframe entry point) and [`AppLaunch`] (launch
//! parameters). Native-only concerns (argv parsing, file dialogs, logging setup)
//! live in the `molar_vis` binary so this crate can compile to `wasm32-unknown-unknown`.

mod app;
mod camera;
mod color;
mod data;
mod geometry;
mod history;
mod launch;
mod material;
mod pick;
mod render;
mod scene;
mod secstruct;
mod session;
mod settings;
mod spatial;
mod suggest;
mod theme;
mod trajectory;

pub use app::App;
pub use launch::{parse_file_args, AppLaunch};
#[cfg(not(target_arch = "wasm32"))]
pub use launch::run;
#[cfg(target_arch = "wasm32")]
pub use launch::run_web;

// Re-export eframe so downstream crates (the bin, a future web crate) can name
// its types without pinning the version themselves.
pub use eframe;
