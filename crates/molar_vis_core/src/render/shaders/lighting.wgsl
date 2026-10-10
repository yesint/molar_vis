// Edited presets use four 7-bit coefficients plus a four-bit shader tag.
// Factory words retain their original encoding and exact appearance.
fn material_kind(m: u32) -> u32 {
    if (m == MATERIAL_FLAT_OUTLINE || m == MATERIAL_MOLECULAR_NODES) { return m; }
    if ((m >> 28u) == 0xau) { return MATERIAL_FLAT_OUTLINE; }
    if ((m >> 28u) == 0xbu) { return MATERIAL_MOLECULAR_NODES; }
    return m;
}
fn custom_coefficients(m: u32) -> vec4<f32> {
    return vec4<f32>(f32(m & 127u), f32((m >> 7u) & 127u),
        f32((m >> 14u) & 127u), f32((m >> 21u) & 127u)) / 127.0;
}
// Reserved words select specialized shading without changing classic materials.
fn unpack_material(m: u32) -> vec4<f32> {
    if (m == MATERIAL_FLAT_OUTLINE) { return vec4<f32>(1.0, 0.0, 0.0, 0.0); }
    if (m == MATERIAL_MOLECULAR_NODES) { return vec4<f32>(0.34, 0.62, 0.3, 0.4); }
    if ((m >> 28u) >= 0xau) { return custom_coefficients(m); }
    return vec4<f32>(f32(m & 0xffu) / 255.0, f32((m >> 8u) & 0xffu) / 255.0,
        f32((m >> 16u) & 0xffu) / 255.0, f32((m >> 24u) & 0x7fu) / 127.0);
}

// Normal-buffer alpha carries the deferred-effects policy per pixel.
// 0: flat outline (no AO/shadows); 1: classic; 2: Molecular Nodes contact AO.
fn material_effects(m: u32) -> f32 {
    if (material_kind(m) == MATERIAL_FLAT_OUTLINE) { return 0.0; }
    if (material_kind(m) == MATERIAL_MOLECULAR_NODES) { return 2.0; }
    return 1.0;
}

// Dielectric GGX highlight with broad studio fill. Roughness 0.4 and F0 0.04
// approximate a neutral plastic Principled BSDF; no metallic tint or hard rim.
fn molecular_studio_light(base: vec3<f32>, n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, mat: vec4<f32>) -> vec3<f32> {
    let nl = max(dot(n, l), 0.0);
    let nv = max(dot(n, v), 0.001);
    let h = normalize(v + l);
    let nh = max(dot(n, h), 0.0);
    let vh = max(dot(v, h), 0.0);
    let roughness = clamp(mat.w, 0.05, 1.0);
    let alpha2 = pow(roughness, 4.0);
    let d = alpha2 / (3.14159265 * pow(nh * nh * (alpha2 - 1.0) + 1.0, 2.0));
    let k = pow(roughness + 1.0, 2.0) / 8.0;
    let g = nv / (nv * (1.0 - k) + k) * nl / (nl * (1.0 - k) + k);
    let f = 0.04 + 0.96 * pow(1.0 - vh, 5.0);
    let spec = d * g * f / max(4.0 * nv, 0.001);
    return base * (1.0 - f) * nl + vec3<f32>(spec * mat.z / 0.3);
}

fn shade_molecular_nodes(base: vec3<f32>, n: vec3<f32>, v: vec3<f32>, mat: vec4<f32>) -> vec3<f32> {
    return base * mat.x
        + molecular_studio_light(base, n, v, normalize(vec3<f32>(-0.45, 0.65, 1.0)), mat) * mat.y
        + molecular_studio_light(base, n, v, normalize(vec3<f32>(0.7, -0.2, 0.9)), mat) * (mat.y * (0.28 / 0.62));
}

// Shared view-space lighting for raster and ray-traced surfaces. Mesh ribbons use
// a wider fill than analytic spheres/capsules, as in the live renderer.
fn shade_material(base: vec3<f32>, normal: vec3<f32>, view_dir: vec3<f32>, mat: vec4<f32>, mesh: bool, packed: u32) -> vec3<f32> {
    if (material_kind(packed) == MATERIAL_FLAT_OUTLINE) { return base; }
    if (material_kind(packed) == MATERIAL_MOLECULAR_NODES) { return shade_molecular_nodes(base, normal, view_dir, mat); }
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
    return base * (mat.x + mat.y * (ndotl + fill)) + vec3<f32>(spec * mat.z / 0.3);
}

fn apply_outline(color: vec3<f32>, normal: vec3<f32>, view_dir: vec3<f32>, m: u32) -> vec3<f32> {
    if (material_kind(m) == MATERIAL_FLAT_OUTLINE) {
        var width = 0.5;
        var strength = 1.0;
        if (m != MATERIAL_FLAT_OUTLINE) {
            let coeff = custom_coefficients(m); width = coeff.x; strength = coeff.y;
        }
        let facing = abs(dot(normal, view_dir));
        let ink = 1.0 - smoothstep(width * 0.7, max(width, 0.001), facing);
        return mix(color, vec3<f32>(0.045), ink * strength);
    }
    if (material_kind(m) == MATERIAL_MOLECULAR_NODES) { return color; }
    var on = f32((m >> 31u) & 1u);
    if ((m >> 28u) >= 0xau) { on = select(0.0, 1.0, (m >> 28u) == 0xdu); }
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
