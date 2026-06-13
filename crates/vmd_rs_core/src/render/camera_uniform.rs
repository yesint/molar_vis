//! GPU camera uniform shared by all rendering pipelines (bind group 0).

use bytemuck::{Pod, Zeroable};
use glam::Mat4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct CameraUniform {
    /// World → view (eye) space.
    pub view: [[f32; 4]; 4],
    /// View → clip space (right-handed, [0,1] depth).
    pub proj: [[f32; 4]; 4],
    /// x = 1.0 for perspective (eye-ray impostors), 0.0 for orthographic
    /// (parallel-ray impostors). y,z,w reserved.
    pub params: [f32; 4],
}

impl CameraUniform {
    pub fn new(view: Mat4, proj: Mat4, perspective: bool) -> Self {
        Self {
            view: view.to_cols_array_2d(),
            proj: proj.to_cols_array_2d(),
            params: [if perspective { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
        }
    }
}
