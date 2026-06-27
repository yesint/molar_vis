//! Loading molecular structures via molar and turning them into GPU geometry.

pub(crate) mod bonds;
mod loader;
// Trajectory frame reading uses `std::thread` + filesystem paths; native-only.
// The wasm build feeds frames through the same `trajectory::LoadMsg` channel
// from a Web Worker instead.
#[cfg(not(target_arch = "wasm32"))]
pub mod traj_loader;
// Browser trajectory streaming: incremental frame parsing from an in-memory buffer
// (no threads), feeding the same `Trajectory` as the native loader.
#[cfg(target_arch = "wasm32")]
pub mod traj_wasm;

pub use bonds::BondParams;
pub use loader::{load_with, RawMolecule};
// `load` (default bond params) is only used by unit tests now that production code
// threads the settings-derived `BondParams` through `load_with`.
#[cfg(test)]
pub use loader::load;
#[cfg(target_arch = "wasm32")]
pub use loader::load_from_bytes;
