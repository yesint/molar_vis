//! Interactive orbit/arcball camera. A quaternion `orientation` rotates the view
//! around a `target` at a given `distance` (zoom). VMD-style mouse mapping:
//! left-drag = rotate, middle-drag = pan, right-drag / wheel = zoom.
//!
//! All distances are in nanometers (molar's native unit); near/far are derived
//! from the scene radius each frame, never hardcoded.

use glam::{Mat4, Quat, Vec2, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Projection {
    Perspective,
    Orthographic,
}

/// Depth-cue (fog) falloff curve, matching VMD's `cuemode` (the OpenGL fog
/// equations). All are normalized so the fog reaches `strength` at the back of
/// the scene; the mode only changes the *shape* of the ramp from `start` to back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum CueMode {
    /// Fog grows linearly with eye-space distance (the previous behavior).
    #[default]
    Linear,
    /// `1 − e^(−k·t)` — fades in quickly up front, then eases off.
    Exp,
    /// `1 − e^(−(k·t)²)` — stays clear up front, then ramps hard toward the back.
    Exp2,
}

impl CueMode {
    pub const ALL: [CueMode; 3] = [CueMode::Linear, CueMode::Exp, CueMode::Exp2];
    pub fn label(self) -> &'static str {
        match self {
            CueMode::Linear => "Linear",
            CueMode::Exp => "Exp",
            CueMode::Exp2 => "Exp²",
        }
    }
}

/// Depth cueing (fog): geometry fades toward the background color as it recedes
/// from the camera, adding depth perception (VMD's "Depth Cueing"). `start`/
/// `strength` are unitless and scene-relative so they stay meaningful at any zoom;
/// the actual eye-space range is derived from the camera each frame. `mode`
/// selects the falloff curve (see [`CueMode`]).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DepthCue {
    pub enabled: bool,
    /// Where the fog begins, as a fraction of the molecule's front→back depth
    /// span (0 = front of the scene, 1 = back).
    pub start: f32,
    /// Fog opacity at the back of the scene (0 = none, 1 = fully background).
    pub strength: f32,
    /// Falloff curve. `#[serde(default)]` so sessions written before this field
    /// existed still load (defaulting to `Linear`).
    #[serde(default)]
    pub mode: CueMode,
}

impl Default for DepthCue {
    fn default() -> Self {
        Self { enabled: true, start: 0.3, strength: 0.55, mode: CueMode::Linear }
    }
}

/// Screen-space ambient occlusion: darken creases/contact points where geometry
/// occludes nearby ambient light, adding depth/shape cues (off by default, like
/// VMD). `radius` is the sampling radius in nm; `strength` scales the darkening.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Ao {
    pub enabled: bool,
    pub strength: f32,
    /// World-space sampling radius (nm).
    pub radius: f32,
}

impl Default for Ao {
    fn default() -> Self {
        Self { enabled: false, strength: 0.9, radius: 0.4 }
    }
}

/// Real-time cast shadows (shadow mapping): the scene is rendered from a key light
/// into a depth map, then the shadow is applied deferred in the AO pass. Off by
/// default; `strength` scales how dark the shadowed areas get.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Shadow {
    pub enabled: bool,
    pub strength: f32,
    /// Edge softness in [0,1] (0 = hard). Only the ray tracer uses it (soft penumbra via
    /// a jittered shadow ray); the rasterized shadow map ignores it. `#[serde(default)]`
    /// so older sessions still load.
    #[serde(default = "default_shadow_softness")]
    pub softness: f32,
}

fn default_shadow_softness() -> f32 {
    0.4
}

impl Default for Shadow {
    fn default() -> Self {
        Self { enabled: false, strength: 0.6, softness: default_shadow_softness() }
    }
}

/// Viewport background: a flat color or a vertical (top→bottom) gradient. Drives
/// the clear color, the gradient pass, and the depth-cue fog target (geometry
/// fades toward the background as it recedes).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BgKind {
    Solid,
    Gradient,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Background {
    pub kind: BgKind,
    /// Solid color (the gradient is ignored when `kind == Solid`).
    pub color: [f32; 4],
    /// Gradient endpoints (screen top / bottom).
    pub top: [f32; 4],
    pub bottom: [f32; 4],
}

impl Default for Background {
    fn default() -> Self {
        // The previous hardcoded near-black `BG`, plus a subtle blue gradient preset.
        Self {
            kind: BgKind::Solid,
            color: [0.02, 0.02, 0.05, 1.0],
            top: [0.11, 0.13, 0.20, 1.0],
            bottom: [0.01, 0.01, 0.02, 1.0],
        }
    }
}

impl Background {
    /// Color depth-cue fog fades geometry toward (the solid color, or the
    /// gradient's midpoint).
    pub fn fog_color(&self) -> [f32; 4] {
        match self.kind {
            BgKind::Solid => self.color,
            BgKind::Gradient => [
                (self.top[0] + self.bottom[0]) * 0.5,
                (self.top[1] + self.bottom[1]) * 0.5,
                (self.top[2] + self.bottom[2]) * 0.5,
                1.0,
            ],
        }
    }

    /// Clear color for the color target (the gradient pass overwrites it when on).
    pub fn clear_color(&self) -> [f32; 4] {
        match self.kind {
            BgKind::Solid => self.color,
            BgKind::Gradient => self.bottom,
        }
    }

    pub fn is_gradient(&self) -> bool {
        matches!(self.kind, BgKind::Gradient)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
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
    pub depth_cue: DepthCue,
    /// Screen-space ambient occlusion. `#[serde(default)]` so sessions written
    /// before this field existed still load.
    #[serde(default)]
    pub ao: Ao,
    /// Real-time cast shadows (shadow mapping).
    #[serde(default)]
    pub shadow: Shadow,
    /// Viewport background (solid color or gradient).
    #[serde(default)]
    pub background: Background,
    /// Fraction of the viewport the framed object fills along its longest
    /// dimension (was the `FILL` constant). Seeded from the program settings;
    /// `#[serde(default)]` so older sessions still load.
    #[serde(default = "default_fill")]
    pub fill: f32,
    /// Progressively ray-trace the viewport when the camera is idle (PyMOL-`ray`-style),
    /// dropping to the realtime raster while moving. Opt-out (default on); only takes
    /// effect on a compute-capable device.
    #[serde(default = "default_true")]
    pub raytrace_inplace: bool,
    /// Use full path-traced global illumination in the ray tracer (tier 2). Off by default.
    #[serde(default)]
    pub gi: bool,
}

fn default_fill() -> f32 {
    0.9
}

fn default_true() -> bool {
    true
}

impl Default for Camera {
    fn default() -> Self {
        Self::frame_bbox(Vec3::splat(-1.0), Vec3::splat(1.0), default_fill())
    }
}

/// Eye distance that makes a box of half-extents `half` fill ~`FILL` of the vertical
/// field of view along its **longest** dimension (so the object is large in frame,
/// not lost inside its bounding-sphere diagonal). Works for both perspective and
/// orthographic (same small-angle relation). `scene_radius` (the bounding-sphere
/// radius) handles near/far separately, so this can be tight without clipping.
fn fit_distance(half: Vec3, fov_y: f32, fill: f32) -> f32 {
    let fit_r = half.max_element().max(1e-3);
    fit_r / (fill.clamp(0.1, 1.0) * (fov_y * 0.5).tan())
}

impl Camera {
    /// Position the camera to frame a bounding box, filling `fill` of the viewport
    /// along the box's longest dimension.
    pub fn frame_bbox(min: Vec3, max: Vec3, fill: f32) -> Self {
        let fov_y = 45_f32.to_radians();
        let half = (max - min) * 0.5;
        Self {
            target: (min + max) * 0.5,
            orientation: Quat::IDENTITY,
            distance: fit_distance(half, fov_y, fill),
            scene_radius: half.length().max(1e-3),
            fov_y,
            projection: Projection::Orthographic,
            depth_cue: DepthCue::default(),
            ao: Ao::default(),
            shadow: Shadow::default(),
            background: Background::default(),
            fill,
            raytrace_inplace: true,
            gi: false,
        }
    }

    /// Reframe to bring the bounding box `[min, max]` optimally into view —
    /// recenter on it and set the zoom to fit its bounding sphere — while
    /// **keeping** the current orientation, projection, and depth cue (a "zoom to
    /// selection / focus" action, unlike [`frame_bbox`](Self::frame_bbox) which
    /// resets the orientation).
    pub fn focus_bbox(&mut self, min: Vec3, max: Vec3) {
        let half = (max - min) * 0.5;
        self.target = (min + max) * 0.5;
        self.scene_radius = half.length().max(1e-3);
        self.distance = fit_distance(half, self.fov_y, self.fill);
    }

    pub fn is_perspective(&self) -> bool {
        matches!(self.projection, Projection::Perspective)
    }

    /// Depth-cue parameters for the GPU camera uniform: `[near, far, strength,
    /// mode]` in eye-space distance (positive, away from the camera). The fog ramps
    /// from `near` (no fog) to `far` (full `strength`) along the curve `mode`
    /// (0 = linear, 1 = exp, 2 = exp²; see [`CueMode`]). When disabled, `strength`
    /// is 0 so the shaders apply no fog.
    pub fn cue_uniform(&self) -> [f32; 4] {
        let near = (self.distance - self.scene_radius) + self.depth_cue.start * 2.0 * self.scene_radius;
        let far = (self.distance + self.scene_radius).max(near + 1e-3);
        let strength = if self.depth_cue.enabled { self.depth_cue.strength } else { 0.0 };
        let mode = match self.depth_cue.mode {
            CueMode::Linear => 0.0,
            CueMode::Exp => 1.0,
            CueMode::Exp2 => 2.0,
        };
        [near, far, strength, mode]
    }

    /// SSAO parameters for the renderer: `[radius, bias, strength, enabled]`
    /// (radius/bias in nm/eye-space; `enabled == 0` skips the AO pass).
    pub fn ao_uniform(&self) -> [f32; 4] {
        let enabled = if self.ao.enabled { 1.0 } else { 0.0 };
        [self.ao.radius.max(1e-3), 0.015, self.ao.strength, enabled]
    }

    /// Shadow parameters for the renderer: `[strength, bias, enabled, softness]`
    /// (`enabled == 0` skips the shadow map + test; `softness` is used only by the ray
    /// tracer's soft shadow).
    pub fn shadow_uniform(&self) -> [f32; 4] {
        let enabled = if self.shadow.enabled { 1.0 } else { 0.0 };
        [self.shadow.strength, 0.0025, enabled, self.shadow.softness.clamp(0.0, 1.0)]
    }

    /// Eye-space distance range `[front, back]` (positive, away from the camera)
    /// bracketing the molecule's bounding sphere. Used by the weighted-blended OIT
    /// shaders to normalize per-fragment depth across the molecule's own extent —
    /// the molecule occupies a razor-thin, non-linear slice of NDC depth, so the
    /// raw window depth can't discriminate transparent layers; linear eye-space
    /// depth across `[front, back]` can, letting near layers dominate the blend.
    pub fn eye_depth_range(&self) -> [f32; 2] {
        let front = (self.distance - self.scene_radius).max(1e-3);
        let back = (self.distance + self.scene_radius).max(front + 1e-3);
        [front, back]
    }

    pub fn right(&self) -> Vec3 {
        self.orientation * Vec3::X
    }
    pub fn up(&self) -> Vec3 {
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
    pub fn orbit(&mut self, dx: f32, dy: f32, sensitivity: f32) {
        const K: f32 = 0.006;
        let k = K * sensitivity;
        let q = Quat::from_axis_angle(self.up(), -dx * k)
            * Quat::from_axis_angle(self.right(), -dy * k);
        self.orientation = (q * self.orientation).normalize();
    }

    /// Roll (shift+left-drag): rotate within the screen plane, about the view
    /// axis. Horizontal drag drives the angle. `sensitivity` scales the rate.
    pub fn roll(&mut self, dx: f32, sensitivity: f32) {
        const K: f32 = 0.01;
        // View axis (screen normal, toward the eye) = orientation·Z.
        let axis = self.orientation * Vec3::Z;
        let q = Quat::from_axis_angle(axis, dx * K * sensitivity);
        self.orientation = (q * self.orientation).normalize();
    }

    // --- Programmatic nav in intuitive units (for the Python/scripting API) ---

    /// Orbit by absolute angles (degrees): `yaw` about the view-up axis, `pitch`
    /// about the view-right axis. Same trackball convention as [`orbit`](Self::orbit).
    pub fn rotate_deg(&mut self, yaw: f32, pitch: f32) {
        let q = Quat::from_axis_angle(self.up(), -yaw.to_radians())
            * Quat::from_axis_angle(self.right(), -pitch.to_radians());
        self.orientation = (q * self.orientation).normalize();
    }

    /// Roll about the view axis by an absolute angle (degrees).
    pub fn roll_deg(&mut self, deg: f32) {
        let axis = self.orientation * Vec3::Z;
        let q = Quat::from_axis_angle(axis, deg.to_radians());
        self.orientation = (q * self.orientation).normalize();
    }

    /// Pan by a fraction of the viewport height (`1.0` = one screen-height), so it
    /// needs no pixel/viewport context. `+dx` moves the view right, `+dy` up.
    pub fn pan_fraction(&mut self, dx: f32, dy: f32) {
        let h = self.distance * 2.0 * (self.fov_y * 0.5).tan();
        self.target += -self.right() * dx * h + self.up() * dy * h;
    }

    /// Zoom by a multiplicative factor: `>1` moves closer (zoom in), `<1` farther.
    /// Clamped to the same range as the interactive zoom.
    pub fn zoom_by(&mut self, factor: f32) {
        self.distance = (self.distance / factor.max(1e-3))
            .clamp(self.scene_radius * 0.05, self.scene_radius * 20.0);
    }

    /// Pan (middle-drag): slide `target` in the camera plane so the molecule
    /// tracks the cursor. `viewport_h` is the viewport height in points.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_h: f32) {
        let scale = self.distance * 2.0 * (self.fov_y * 0.5).tan() / viewport_h.max(1.0);
        self.target += -self.right() * dx * scale + self.up() * dy * scale;
    }

    /// Zoom by mouse wheel **toward the cursor** (`scroll` in points; positive =
    /// wheel up = zoom in). `ndc` is the cursor position in `[-1, 1]` (y up) and
    /// `aspect = width/height`; the target is panned so the world point under the
    /// cursor stays fixed on screen as the zoom changes (map-style zoom-to-cursor).
    pub fn zoom_scroll(&mut self, scroll: f32, ndc: Vec2, aspect: f32) {
        let d0 = self.distance;
        self.apply_zoom(-scroll * 0.0015);
        // At the target plane the view half-height is `distance·tan(fov/2)` for both
        // projections, so the cursor's world offset from the target scales with the
        // distance. Shift the target by the change in that offset to keep the point
        // under the cursor put (so zooming in homes in on the cursor).
        let k = (d0 - self.distance) * (self.fov_y * 0.5).tan();
        self.target += self.right() * (ndc.x * aspect * k) + self.up() * (ndc.y * k);
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
        let cam = Camera::frame_bbox(Vec3::new(-2.0, 0.0, 1.0), Vec3::new(4.0, 6.0, 3.0), 0.9);
        assert!((cam.target - Vec3::new(1.0, 3.0, 2.0)).length() < 1e-5);
        // Eye sits `distance` away from the target.
        assert!(((cam.eye() - cam.target).length() - cam.distance).abs() < 1e-4);
        assert!(cam.distance > cam.scene_radius); // far enough to see the whole box
    }

    #[test]
    fn orbit_preserves_distance_and_moves_eye() {
        let mut cam = Camera::frame_bbox(Vec3::splat(-1.0), Vec3::splat(1.0), 0.9);
        let eye0 = cam.eye();
        let d0 = cam.distance;
        cam.orbit(60.0, 25.0, 1.0);
        assert!((cam.distance - d0).abs() < 1e-6, "orbit must not change zoom");
        assert!((cam.eye() - cam.target).length() > 0.0);
        assert!((cam.eye() - eye0).length() > 1e-3, "eye should move when orbiting");
    }

    #[test]
    fn scroll_zooms_in_and_clamps() {
        let mut cam = Camera::frame_bbox(Vec3::splat(-1.0), Vec3::splat(1.0), 0.9);
        let d0 = cam.distance;
        cam.zoom_scroll(100.0, Vec2::ZERO, 1.0); // wheel up = zoom in = smaller distance
        assert!(cam.distance < d0);
        for _ in 0..1000 {
            cam.zoom_scroll(1000.0, Vec2::ZERO, 1.0);
        }
        assert!(cam.distance >= cam.scene_radius * 0.05 - 1e-6, "zoom-in is clamped");
    }

    #[test]
    fn pan_moves_target() {
        let mut cam = Camera::frame_bbox(Vec3::splat(-1.0), Vec3::splat(1.0), 0.9);
        let t0 = cam.target;
        cam.pan(50.0, -30.0, 800.0);
        assert!((cam.target - t0).length() > 1e-4);
    }

    /// Wheel-zoom is cursor-centered: the world point under the cursor before the
    /// zoom must still project to the same screen position after it, for both
    /// projections.
    #[test]
    fn zoom_is_centered_on_cursor() {
        let aspect = 1.6_f32;
        let ndc = Vec2::new(0.7, -0.4);
        for proj in [Projection::Perspective, Projection::Orthographic] {
            let mut cam = Camera::frame_bbox(Vec3::splat(-2.0), Vec3::splat(2.0), 0.9);
            cam.orientation = Quat::from_rotation_y(0.6) * Quat::from_rotation_x(0.3);
            cam.projection = proj;
            // World point on the focal (target) plane under the cursor, pre-zoom.
            let half_h = cam.distance * (cam.fov_y * 0.5).tan();
            let p = cam.target + cam.right() * (ndc.x * half_h * aspect) + cam.up() * (ndc.y * half_h);
            cam.zoom_scroll(120.0, ndc, aspect);
            // After zooming in, that point must still land under the cursor.
            let clip = cam.proj(aspect) * cam.view() * p.extend(1.0);
            let got = Vec2::new(clip.x / clip.w, clip.y / clip.w);
            assert!((got - ndc).length() < 1e-3, "{proj:?}: {got:?} vs {ndc:?}");
        }
    }
}
