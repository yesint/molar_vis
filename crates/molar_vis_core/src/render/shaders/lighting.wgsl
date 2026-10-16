fn is_glowing_path(material: u32) -> bool {
    return (material & 0xf0000000u) == MATERIAL_GLOWING_PATH;
}

fn glowing_path_axis(material: u32) -> vec3<f32> {
    let oct = vec2<f32>(f32(material & 1023u), f32((material >> 10u) & 1023u)) / 1023.0 * 2.0 - 1.0;
    var axis = vec3<f32>(oct, 1.0 - abs(oct.x) - abs(oct.y));
    let fold = max(-axis.z, 0.0);
    axis.x += select(fold, -fold, axis.x >= 0.0);
    axis.y += select(fold, -fold, axis.y >= 0.0);
    return normalize(axis);
}

// Recover the profile radius from one vertex on the enclosing capsule. Its
// quantized frame matches the CPU builder, including on the hemispherical ends.
fn glowing_path_radius_from_bound(vertex: vec3<f32>, center: vec3<f32>, axis: vec3<f32>, material: u32) -> f32 {
    let half_length = f32((material >> 20u) & 255u) / 255.0;
    let offset = vertex - center;
    let along = clamp(dot(offset, axis), -half_length, half_length);
    let bound_radius = length(offset - axis * along);
    let amplitude = min(0.009, half_length * 0.06);
    return max(0.0005, bound_radius - amplitude - 0.01);
}

// A world-space frame fixes the spiral phase as the camera rotates.
fn glowing_path_u(axis: vec3<f32>) -> vec3<f32> {
    let reference = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), abs(axis.z) > 0.9);
    return normalize(cross(axis, reference));
}

// Twelve samples per turn keep the spiral smooth without per-pixel trig.
const GLOWING_PATH_CIRCLE = array<vec2<f32>, 12>(
    vec2<f32>(1.0, 0.0), vec2<f32>(0.8660254, 0.5), vec2<f32>(0.5, 0.8660254),
    vec2<f32>(0.0, 1.0), vec2<f32>(-0.5, 0.8660254), vec2<f32>(-0.8660254, 0.5),
    vec2<f32>(-1.0, 0.0), vec2<f32>(-0.8660254, -0.5), vec2<f32>(-0.5, -0.8660254),
    vec2<f32>(0.0, -1.0), vec2<f32>(0.5, -0.8660254), vec2<f32>(0.8660254, -0.5)
);

fn glowing_path_helix_point(index: u32, half_length: f32, axis: vec3<f32>, u: vec3<f32>, v: vec3<f32>) -> vec3<f32> {
    let t = f32(index) / 72.0; // six turns
    let circle = GLOWING_PATH_CIRCLE[index % 12u];
    let q = clamp(min(t, 1.0 - t) / 0.08, 0.0, 1.0);
    let envelope = q * q * (3.0 - 2.0 * q);
    let amplitude = min(0.009, half_length * 0.06);
    return axis * ((2.0 * t - 1.0) * half_length)
        + (u * circle.x + v * circle.y) * (amplitude * envelope);
}

struct GlowingPathSample {
    depth_point: vec3<f32>,
    radius2: f32,
    intensity: f32,
};

// Measure the ray against a continuous polyline approximation of the helix.
// Testing every interval also handles overlapping turns in near-axial views,
// without local-solver jumps or extra transparent mesh layers.
fn glowing_path_sample(hit: vec3<f32>, center: vec3<f32>, view: vec3<f32>, axis: vec3<f32>, u: vec3<f32>, material: u32, tube_radius: f32) -> GlowingPathSample {
    let half_length = f32((material >> 20u) & 255u) / 255.0;
    let offset = center - hit;
    let v = cross(axis, u);
    var previous = glowing_path_helix_point(0u, half_length, axis, u, v);
    var best_radius2 = 1e10;
    var best_intensity = 0.0;
    var best_emission = 0.0;
    var best_point = previous;
    for (var interval = 0u; interval < 72u; interval += 1u) {
        let next = glowing_path_helix_point(interval + 1u, half_length, axis, u, v);
        let segment = next - previous;
        let transverse_segment = segment - view * dot(segment, view);
        let delta = offset + previous;
        let transverse_delta = delta - view * dot(delta, view);
        let fraction = clamp(-dot(transverse_delta, transverse_segment)
            / max(dot(transverse_segment, transverse_segment), 1e-10), 0.0, 1.0);
        let transverse = transverse_delta + transverse_segment * fraction;
        let t = (f32(interval) + fraction) / 72.0;
        let waist = 2.0 * t - 1.0;
        // The width setting scales the whole profile; axial brightness stays unchanged.
        let radius2 = dot(transverse, transverse) / (tube_radius * tube_radius);
        let axial = 0.2 + 0.8 * (1.0 - waist * waist);
        // A 1.5-power falloff strengthens the endpoint fade without dimming the middle.
        let intensity = axial * sqrt(axial);
        if (radius2 < 1.0) {
            let profile = glowing_path_profile(radius2);
            let emission = intensity * (profile.x + profile.y);
            // Overlapping turns show the strongest part of the continuous glow,
            // including in end-on views, without stacking transparent surfaces.
            if (emission > best_emission) {
                best_emission = emission;
                best_radius2 = radius2;
                best_intensity = intensity;
                best_point = previous + segment * fraction;
            }
        }
        previous = next;
    }
    let nearest = center + best_point;
    return GlowingPathSample(hit + view * dot(nearest - hit, view), best_radius2, best_intensity);
}

fn glowing_path_profile(r2: f32) -> vec2<f32> {
    let core = 0.68 * exp(-r2 * 32.0);
    let halo = 0.28 * exp(-r2 * 3.8) * (1.0 - smoothstep(0.65, 1.0, r2));
    return vec2<f32>(core, halo);
}

// Emission depends on distance to the path, with no surface lighting.
// Smooth Gaussian emission has no opaque plateau or reflective highlights.
fn glowing_path_color(base: vec3<f32>, r2: f32, opacity: f32, background: vec3<f32>) -> vec4<f32> {
    let profile = glowing_path_profile(r2);
    let luminance = dot(background, vec3<f32>(0.2126, 0.7152, 0.0722));
    let light_background = smoothstep(0.25, 0.75, luminance);
    // A luminous tinted center on dark backgrounds; saturated dark emission on
    // light backgrounds. This is emission across the profile, not a light response.
    let core_color = mix(mix(base, vec3<f32>(1.0), 0.32), base * 0.62, light_background);
    let halo_color = base * mix(1.0, 0.40, light_background);
    let core_alpha = profile.x;
    let halo_alpha = profile.y;
    let color = (core_color * core_alpha + halo_color * halo_alpha) / max(core_alpha + halo_alpha, 1e-6);
    return vec4<f32>(color, opacity * (core_alpha + halo_alpha));
}

// Edited presets use a four-bit shader tag and 28 coefficient bits.
// Velvet uses 5/5/6/6/6 bits for ambient/diffuse/sheen/roughness/fuzz.
// Factory words retain their original encoding and exact appearance.
fn material_kind(m: u32) -> u32 {
    if (m == MATERIAL_FLAT_OUTLINE || m == MATERIAL_MOLECULAR_NODES || m == MATERIAL_VELVET) { return m; }
    if ((m >> 28u) == 0xau) { return MATERIAL_FLAT_OUTLINE; }
    if ((m >> 28u) == 0xeu) { return MATERIAL_VELVET; }
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
    if (m == MATERIAL_VELVET) { return vec4<f32>(0.16, 0.55, 0.85, 0.65); }
    if (m == MATERIAL_MOLECULAR_NODES) { return vec4<f32>(0.34, 0.62, 0.3, 0.4); }
    if ((m >> 28u) == 0xeu) {
        return vec4<f32>(f32(m & 31u) / 31.0, f32((m >> 5u) & 31u) / 31.0,
            f32((m >> 10u) & 63u) / 63.0, f32((m >> 16u) & 63u) / 63.0);
    }
    if ((m >> 28u) >= 0xau) { return custom_coefficients(m); }
    return vec4<f32>(f32(m & 0xffu) / 255.0, f32((m >> 8u) & 0xffu) / 255.0,
        f32((m >> 16u) & 0xffu) / 255.0, f32((m >> 24u) & 0x7fu) / 127.0);
}

// Normal-buffer alpha carries the deferred-effects policy per pixel.
// 0: flat outline or velvet (no dark hard-surface AO/shadows);
// 1: classic; 2: Molecular Nodes contact AO.
fn material_effects(m: u32) -> f32 {
    if (material_kind(m) == MATERIAL_FLAT_OUTLINE || material_kind(m) == MATERIAL_VELVET) { return 0.0; }
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

// Saved hairiness now controls shading softness, without additional geometry.
fn material_fuzz(m: u32) -> f32 {
    if (m == MATERIAL_VELVET) { return 0.65; }
    return f32((m >> 22u) & 63u) / 63.0;
}
fn shade_velvet(base: vec3<f32>, normal: vec3<f32>, v: vec3<f32>, mat: vec4<f32>, fuzz: f32) -> vec3<f32> {
    let nv = clamp(dot(normal, v), 0.0, 1.0);
    let roughness = clamp(mat.w, 0.0, 1.0);
    let rim = pow(1.0 - nv, mix(3.5, 1.2, roughness));
    // Broad wrapped lighting keeps the body matte. At grazing angles, reduce
    // normal contrast and add a muted, color-tinted sheen rather than a bright rim.
    let key = clamp((dot(normal, normalize(vec3<f32>(0.3, 0.4, 1.0))) + 0.5) / 1.5, 0.0, 1.0);
    let fill = clamp((dot(normal, normalize(vec3<f32>(-0.6, 0.2, 0.7))) + 0.5) / 1.5, 0.0, 1.0);
    let body = base * (mat.x + mat.y * (0.35 + 0.65 * key + 0.2 * fill));
    let tint = mix(base, sqrt(max(base, vec3<f32>(0.0))), 0.3);
    let soft_edge = tint * (mat.x + mat.y * 0.85);
    let sheen = tint * (0.3 * mat.z * rim * (0.6 + 0.4 * key));
    return mix(body, soft_edge, fuzz * rim * 0.65) + sheen;

}

// Shared view-space lighting for raster and ray-traced surfaces. Mesh ribbons use
// a wider fill than analytic spheres/capsules, as in the live renderer.
fn shade_material(base: vec3<f32>, normal: vec3<f32>, view_dir: vec3<f32>, mat: vec4<f32>, mesh: bool, packed: u32) -> vec3<f32> {
    if (is_glowing_path(packed)) { return base; }
    if (material_kind(packed) == MATERIAL_FLAT_OUTLINE) { return base; }
    if (material_kind(packed) == MATERIAL_MOLECULAR_NODES) { return shade_molecular_nodes(base, normal, view_dir, mat); }
    if (material_kind(packed) == MATERIAL_VELVET) { return shade_velvet(base, normal, view_dir, mat, material_fuzz(packed)); }
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
    if (material_kind(m) == MATERIAL_MOLECULAR_NODES || material_kind(m) == MATERIAL_VELVET) { return color; }
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
