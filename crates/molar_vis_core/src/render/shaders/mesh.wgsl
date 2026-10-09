// Lambert-shaded triangle mesh for the Cartoon representation. Eye-space
// headlight (light = camera direction) with two-sided shading + ambient.

struct Camera {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    params: vec4<f32>,
    cue: vec4<f32>,    // depth cue: near, far, strength, mode
    fog_color: vec4<f32>,
    depth_range: vec4<f32>, // OIT: eye-space [front, back, _, _]
    // Selection-glow color (rgb), chosen on the CPU from the viewport background so the
    // glow and the cues egui draws over the scene are one decision. See `theme::glow_color`.
    glow_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

// Depth cueing (VMD cuemode): fade toward the background as eye-space distance
// grows. cue = [near, far, strength, mode]; mode 0 = linear, 1 = exp, 2 = exp2.
// All curves are normalized to reach full fog at the far plane, scaled by strength.
fn apply_fog(color: vec3<f32>, eye_z: f32) -> vec3<f32> {
    return depth_cue(color, -eye_z, camera.cue, camera.fog_color.rgb);
}

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: u32,
    @location(3) mat: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal_eye: vec3<f32>,
    @location(1) color: vec4<f32>, // rgb + opacity
    @location(2) view_pos: vec3<f32>,
    @location(3) @interpolate(flat) mat: u32, // packed material lighting
};

fn unpack_color(c: u32) -> vec4<f32> {
    let r = f32((c >> 0u) & 0xffu) / 255.0;
    let g = f32((c >> 8u) & 0xffu) / 255.0;
    let b = f32((c >> 16u) & 0xffu) / 255.0;
    let a = f32((c >> 24u) & 0xffu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

// Unpack the per-element material lighting coefficients (ambient, diffuse,
// specular, shininess).
fn unpack_mat(m: u32) -> vec4<f32> {
    let amb = f32((m >> 0u) & 0xffu) / 255.0;
    let dif = f32((m >> 8u) & 0xffu) / 255.0;
    let spc = f32((m >> 16u) & 0xffu) / 255.0;
    let shn = f32((m >> 24u) & 0x7fu) / 127.0; // top bit is the outline flag
    return vec4<f32>(amb, dif, spc, shn);
}

// Weighted-blended OIT weight, biased strongly toward the camera using linear
// eye-space depth across the molecule's extent (see sphere.wgsl for rationale).
fn oit_weight(eye_z: f32, a: f32) -> f32 {
    return transparency_weight(-eye_z, a, camera.depth_range.xy);
}

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    let view_pos = camera.view * vec4<f32>(v.pos, 1.0);
    out.clip = camera.proj * view_pos;
    // view is rigid (rotation + translation); w=0 applies only the rotation.
    out.normal_eye = (camera.view * vec4<f32>(v.normal, 0.0)).xyz;
    out.color = unpack_color(v.color);
    out.view_pos = view_pos.xyz;
    out.mat = v.mat;
    return out;
}

// Shaded (fogged) color for this fragment; opacity rides in the returned alpha.
fn shade(in: VsOut) -> vec4<f32> {
    // Guard against degenerate (zero-length) interpolated normals — they occur at
    // failed orientation frames and arrow tips. `normalize` of a zero vector is
    // NaN, which NVIDIA writes to the UNORM target as white (AMD as 0), producing
    // "white steps" along ribbon edges. Fall back to a toward-eye normal instead.
    let nlen = length(in.normal_eye);
    var n = select(vec3<f32>(0.0, 0.0, 1.0), in.normal_eye / nlen, nlen > 1e-6);
    // View direction toward the eye (origin for perspective, +z for ortho).
    let view_dir = select(vec3<f32>(0.0, 0.0, 1.0), normalize(-in.view_pos), camera.params.x > 0.5);
    // Two-sided: flip the normal to face the eye so back faces of open ribbons
    // are lit rather than dark.
    if (dot(n, view_dir) < 0.0) {
        n = -n;
    }
    var lit = shade_material(in.color.rgb, n, view_dir, unpack_mat(in.mat), true);
    lit = apply_outline(lit, n, view_dir, in.mat);
    return vec4<f32>(apply_fog(lit, in.view_pos.z), in.color.a);
}

struct OpaqueOut {
    @location(0) color: vec4<f32>,
    @location(1) normal: vec4<f32>,
};

// Closed meshes cast from their light-space exit surface. Classify with the
// interpolated normal, as the ray tracer does: geometric face culling alone
// leaves triangle-shaped self-shadow stripes at a smooth grazing terminator.
@fragment
fn fs_shadow(in: VsOut) {
    if (in.normal_eye.z >= 0.0) { discard; }
}

@fragment
fn fs_main(in: VsOut) -> OpaqueOut {
    let nlen = length(in.normal_eye);
    var n = select(vec3<f32>(0.0, 0.0, 1.0), in.normal_eye / max(nlen, 1e-12), nlen > 1e-6);
    let view_dir = select(vec3<f32>(0.0, 0.0, 1.0), normalize(-in.view_pos), camera.params.x > 0.5);
    if (dot(n, view_dir) < 0.0) { n = -n; }
    var out: OpaqueOut;
    out.color = shade(in);
    out.normal = vec4<f32>(n, 1.0);
    return out;
}

// Additive cyan "rim glow" for the active (pending) selection (see sphere.wgsl):
// the ribbon, in its own style, glows brightest at grazing angles. Two-sided like
// `shade`. Drawn depth-tested (≤), no depth-write, additive.

@fragment
fn fs_glow(in: VsOut) -> @location(0) vec4<f32> {
    let nlen = length(in.normal_eye);
    var n = select(vec3<f32>(0.0, 0.0, 1.0), in.normal_eye / nlen, nlen > 1e-6);
    let view_dir = select(vec3<f32>(0.0, 0.0, 1.0), normalize(-in.view_pos), camera.params.x > 0.5);
    if (dot(n, view_dir) < 0.0) {
        n = -n;
    }
    let ndotv = max(dot(n, view_dir), 0.0);
    let rim = pow(1.0 - ndotv, 1.5);
    // `camera.params.w` is the animated pulse multiplier (see render.rs).
    let alpha = clamp((0.45 + 1.15 * rim) * camera.params.w, 0.0, 1.0);
    return vec4<f32>(camera.glow_color.rgb, alpha);
}

struct OitOut {
    @location(0) accum: vec4<f32>,
    @location(1) reveal: f32,
};

@fragment
fn fs_oit(in: VsOut) -> OitOut {
    let c = shade(in);
    let w = oit_weight(in.view_pos.z, c.a);
    var out: OitOut;
    out.accum = vec4<f32>(c.rgb * c.a, c.a) * w;
    out.reveal = c.a;
    return out;
}
