//! Loading molecular structures via molar and turning them into GPU geometry.

mod bonds;
mod loader;
// Trajectory frame reading uses `std::thread` + filesystem paths; native-only.
// The wasm build feeds frames through the same `trajectory::LoadMsg` channel
// from a Web Worker instead.
#[cfg(not(target_arch = "wasm32"))]
pub mod traj_loader;

pub use loader::{load, RawMolecule};
