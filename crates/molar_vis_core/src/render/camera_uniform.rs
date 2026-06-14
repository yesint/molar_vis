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
    /// (parallel-ray impostors). y,z = viewport size in pixels (used by the
    /// fat-line shader to expand segments to a constant pixel width). w reserved.
    pub params: [f32; 4],
    /// Depth cueing: `[near, far, strength, _]` in eye-space distance. Geometry
    /// fades to `fog_color` from `near` (none) to `far` (full `strength`).
    /// `strength == 0` disables fog.
    pub cue: [f32; 4],
    /// Background color geometry fades toward under depth cueing (matches the
    /// scene clear color so distant geometry dissolves into the background).
    pub fog_color: [f32; 4],
    /// Eye-space depth range `[front, back, _, _]` (positive distances) bracketing
    /// the molecule. The weighted-blended OIT shaders normalize each fragment's
    /// eye-space depth across this range so nearer transparent layers dominate.
    pub depth_range: [f32; 4],
}

impl CameraUniform {
    pub fn new(
        view: Mat4,
        proj: Mat4,
        perspective: bool,
        viewport: [f32; 2],
        cue: [f32; 4],
        fog_color: [f32; 4],
        depth_range: [f32; 2],
    ) -> Self {
        Self {
            view: view.to_cols_array_2d(),
            proj: proj.to_cols_array_2d(),
            params: [if perspective { 1.0 } else { 0.0 }, viewport[0], viewport[1], 0.0],
            cue,
            fog_color,
            depth_range: [depth_range[0], depth_range[1], 0.0, 0.0],
        }
    }
}
