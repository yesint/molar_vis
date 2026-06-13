//! Loading molecular structures via molar and turning them into GPU geometry.

mod bonds;
mod loader;

pub use loader::{load, RawMolecule};
