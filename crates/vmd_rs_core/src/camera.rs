//! Interactive orbit/arcball camera. A quaternion `orientation` rotates the view
//! around a `target` at a given `distance` (zoom). VMD-style mouse mapping:
//! left-drag = rotate, middle-drag = pan, right-drag / wheel = zoom.
//!
//! All distances are in nanometers (molar's native unit); near/far are derived
//! from the scene radius each frame, never hardcoded.

use glam::{Mat4, Quat, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    Perspective,
    Orthographic,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Point the camera looks at (pannable).
    pub target: Vec3,
    /// Camera orientation: rotates the eye offset and up vector around `target`.
    pub orientation: Quat,
    /// Eye distance from `target` (zoom).
    pub distance: f32,
    /// Scene radius, used to derive near/far and clamp zoom.
    pub scene_radius: f32,
    pub fov_y: f32,
    pub projection: Projection,
}

impl Default for Camera {
    fn default() -> Self {
        Self::frame_bbox(Vec3::splat(-1.0), Vec3::splat(1.0))
    }
}

impl Camera {
    /// Position the camera to frame a bounding box.
    pub fn frame_bbox(min: Vec3, max: Vec3) -> Self {
        let center = (min + max) * 0.5;
        let radius = ((max - min).length() * 0.5).max(1e-3);
        let fov_y = 45_f32.to_radians();
        Self {
            target: center,
            orientation: Quat::IDENTITY,
            distance: radius / (fov_y * 0.5).sin() * 1.3,
            scene_radius: radius,
            fov_y,
            projection: Projection::Perspective,
        }
    }

    pub fn is_perspective(&self) -> bool {
        matches!(self.projection, Projection::Perspective)
    }

    fn right(&self) -> Vec3 {
        self.orientation * Vec3::X
    }
    fn up(&self) -> Vec3 {
        self.orientation * Vec3::Y
    }
    /// Eye position: behind the target along the camera's local +Z.
    pub fn eye(&self) -> Vec3 {
        self.target + self.orientation * (Vec3::Z * self.distance)
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, self.up())
    }

    /// Projection matrix, [0,1] NDC depth (wgpu). Near/far track the zoom distance
    /// and scene radius so we neither clip the molecule nor z-fight. Orthographic
    /// extents are chosen so the focal plane matches the perspective framing,
    /// making the perspective↔ortho switch visually continuous.
    pub fn proj(&self, aspect: f32) -> Mat4 {
        let aspect = aspect.max(1e-3);
        let znear = (self.distance - self.scene_radius)
            .max(self.scene_radius * 0.02)
            .max(1e-4);
        let zfar = self.distance + self.scene_radius * 3.0 + 1e-3;
        match self.projection {
            Projection::Perspective => Mat4::perspective_rh(self.fov_y, aspect, znear, zfar),
            Projection::Orthographic => {
                let half_h = self.distance * (self.fov_y * 0.5).tan();
                let half_w = half_h * aspect;
                Mat4::orthographic_rh(-half_w, half_w, -half_h, half_h, znear, zfar)
            }
        }
    }

    /// Orbit (left-drag). `dx`/`dy` are pointer deltas in points. Builds the
    /// rotation from the *current* camera axes, giving free trackball rotation
    /// with no gimbal lock.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        const K: f32 = 0.006;
        let q = Quat::from_axis_angle(self.up(), -dx * K)
            * Quat::from_axis_angle(self.right(), -dy * K);
        self.orientation = (q * self.orientation).normalize();
    }

    /// Pan (middle-drag): slide `target` in the camera plane so the molecule
    /// tracks the cursor. `viewport_h` is the viewport height in points.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_h: f32) {
        let scale = self.distance * 2.0 * (self.fov_y * 0.5).tan() / viewport_h.max(1.0);
        self.target += -self.right() * dx * scale + self.up() * dy * scale;
    }

    /// Zoom by mouse wheel (`scroll` in points; positive = wheel up = zoom in).
    pub fn zoom_scroll(&mut self, scroll: f32) {
        self.apply_zoom(-scroll * 0.0015);
    }

    /// Zoom by right-drag (`dy` in points; drag down = zoom out).
    pub fn zoom_drag(&mut self, dy: f32) {
        self.apply_zoom(dy * 0.005);
    }

    fn apply_zoom(&mut self, log_factor: f32) {
        self.distance = (self.distance * log_factor.exp())
            .clamp(self.scene_radius * 0.05, self.scene_radius * 20.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_bbox_centers_and_fits() {
        let cam = Camera::frame_bbox(Vec3::new(-2.0, 0.0, 1.0), Vec3::new(4.0, 6.0, 3.0));
        assert!((cam.target - Vec3::new(1.0, 3.0, 2.0)).length() < 1e-5);
        // Eye sits `distance` away from the target.
        assert!(((cam.eye() - cam.target).length() - cam.distance).abs() < 1e-4);
        assert!(cam.distance > cam.scene_radius); // far enough to see the whole box
    }

    #[test]
    fn orbit_preserves_distance_and_moves_eye() {
        let mut cam = Camera::frame_bbox(Vec3::splat(-1.0), Vec3::splat(1.0));
        let eye0 = cam.eye();
        let d0 = cam.distance;
        cam.orbit(60.0, 25.0);
        assert!((cam.distance - d0).abs() < 1e-6, "orbit must not change zoom");
        assert!((cam.eye() - cam.target).length() > 0.0);
        assert!((cam.eye() - eye0).length() > 1e-3, "eye should move when orbiting");
    }

    #[test]
    fn scroll_zooms_in_and_clamps() {
        let mut cam = Camera::frame_bbox(Vec3::splat(-1.0), Vec3::splat(1.0));
        let d0 = cam.distance;
        cam.zoom_scroll(100.0); // wheel up = zoom in = smaller distance
        assert!(cam.distance < d0);
        for _ in 0..1000 {
            cam.zoom_scroll(1000.0);
        }
        assert!(cam.distance >= cam.scene_radius * 0.05 - 1e-6, "zoom-in is clamped");
    }

    #[test]
    fn pan_moves_target() {
        let mut cam = Camera::frame_bbox(Vec3::splat(-1.0), Vec3::splat(1.0));
        let t0 = cam.target;
        cam.pan(50.0, -30.0, 800.0);
        assert!((cam.target - t0).length() > 1e-4);
    }
}
