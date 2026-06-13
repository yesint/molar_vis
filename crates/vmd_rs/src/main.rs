//! Native binary entry point: parse argv, set up logging, hand off to the core.
//!
//! All native-only concerns (argv, logging, and later file dialogs) live here so
//! that `vmd_rs_core` stays compilable to `wasm32-unknown-unknown`.

use std::path::PathBuf;

use vmd_rs_core::{run, AppLaunch};

fn main() {
    env_logger::init();

    let files: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();

    if let Err(err) = run(AppLaunch { files }) {
        eprintln!("vmd_rs failed: {err}");
        std::process::exit(1);
    }
}
