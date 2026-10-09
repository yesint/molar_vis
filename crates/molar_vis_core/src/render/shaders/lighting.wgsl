// Shared view-space lighting for raster and ray-traced surfaces. Mesh ribbons use
// a wider fill than analytic spheres/capsules, as in the live renderer.
fn shade_material(base: vec3<f32>, normal: vec3<f32>, view_dir: vec3<f32>, mat: vec4<f32>, mesh: bool) -> vec3<f32> {
    let light_dir = normalize(vec3<f32>(0.3, 0.4, 1.0));
    let ndotl = max(dot(normal, light_dir), 0.0);
    let half = normalize(light_dir + view_dir);
    let spec = mat.z * pow(max(dot(normal, half), 0.0), 2.0 + mat.w * 128.0);
    var fill = max(dot(normal, normalize(vec3<f32>(-0.2, -0.3, 0.6))), 0.0)
        * (1.0 - ndotl) * 0.35;
    if (mesh) {
        fill = max(dot(normal, normalize(vec3<f32>(-0.5, -0.3, 0.6))), 0.0)
            * (1.0 - ndotl) * (1.0 - ndotl) * 0.6;
    }
    return base * (mat.x + mat.y * (ndotl + fill)) + vec3<f32>(spec);
}

fn apply_outline(color: vec3<f32>, normal: vec3<f32>, view_dir: vec3<f32>, m: u32) -> vec3<f32> {
    let on = f32((m >> 31u) & 1u);
    let edge = pow(1.0 - abs(dot(normal, view_dir)), 2.0);
    return color * (1.0 - on * 0.9 * edge);
}

fn depth_cue(color: vec3<f32>, distance: f32, cue: vec4<f32>, fog: vec3<f32>) -> vec3<f32> {
    let t = clamp((distance - cue.x) / max(cue.y - cue.x, 1e-6), 0.0, 1.0);
    let k = 3.0;
    var b = t;
    if (cue.w > 1.5) {
        b = (1.0 - exp(-k * k * t * t)) / (1.0 - exp(-k * k));
    } else if (cue.w > 0.5) {
        b = (1.0 - exp(-k * t)) / (1.0 - exp(-k));
    }
    return mix(color, fog, b * cue.z);
}

const AO_KERNEL_SIZE: f32 = 128.0;
fn ao_disk_offset(index: f32) -> vec2<f32> {
    // Fixed kernel: no per-pixel transcendental operations or extra GPU binding.
    return AO_DISK[u32(index)];
}
fn ao_range(distance: f32, radius: f32) -> f32 {
    return 1.0 - smoothstep(radius * 0.7, radius, distance);
}
fn shadow_filter_width(softness: f32) -> f32 {
    // A disk footprint keeps soft cast-shadow edges smooth at close zoom.
    return clamp(softness, 0.0, 1.0) * 50.0;
}

fn shadow_angular_radius(softness: f32) -> f32 {
    return clamp(softness, 0.0, 1.0) * 0.45;
}

fn transparency_weight(distance: f32, alpha: f32, depth_range: vec2<f32>) -> f32 {
    let t = clamp((distance - depth_range.x) / max(depth_range.y - depth_range.x, 1e-6), 0.0, 1.0);
    let bias = pow(1.0 - t, 3.0);
    return clamp(alpha * (1.0e-2 + bias * 1.0e3), 1.0e-3, 1.0e3);
}
