// Lambert-shaded triangle mesh for the Cartoon representation. Eye-space
// headlight (light = camera direction) with two-sided shading + ambient.

struct Camera {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    params: vec4<f32>,
    cue: vec4<f32>,    // depth cue: near, far, strength, _
    fog_color: vec4<f32>,
    depth_range: vec4<f32>, // OIT: eye-space [front, back, _, _]
};

@group(0) @binding(0) var<uniform> camera: Camera;

// Linear depth cueing: fade toward the background as eye-space distance grows.
fn apply_fog(color: vec3<f32>, eye_z: f32) -> vec3<f32> {
    let d = -eye_z; // eye-space distance (camera looks down -Z)
    let f = clamp((d - camera.cue.x) / max(camera.cue.y - camera.cue.x, 1e-6), 0.0, 1.0) * camera.cue.z;
    return mix(color, camera.fog_color.rgb, f);
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
    let shn = f32((m >> 24u) & 0xffu) / 255.0;
    return vec4<f32>(amb, dif, spc, shn);
}

// Blinn-Phong shade in view space (white specular highlight; `view_dir` to eye).
fn shade_material(base: vec3<f32>, normal: vec3<f32>, view_dir: vec3<f32>, mat: vec4<f32>) -> vec3<f32> {
    let light_dir = normalize(vec3<f32>(0.3, 0.4, 1.0));
    let ndotl = max(dot(normal, light_dir), 0.0);
    // A dim fill from the opposite-front side keeps the thin lateral rims of the
    // flat ribbon from going near-black (their normals point ⊥ to the key light).
    // Gated by (1-ndotl)² so it only lifts shadow/terminator: surfaces the key
    // already lights — and the specular highlight — are left exactly as before,
    // preserving the slick look.
    let fill_dir = normalize(vec3<f32>(-0.5, -0.3, 0.6));
    let fill = max(dot(normal, fill_dir), 0.0);
    let shadow = (1.0 - ndotl) * (1.0 - ndotl);
    let diffuse = mat.y * (ndotl + 0.6 * fill * shadow);
    let half = normalize(light_dir + view_dir);
    let ndoth = max(dot(normal, half), 0.0);
    let exponent = 2.0 + mat.w * 128.0;
    let spec = mat.z * pow(ndoth, exponent);
    return base * (mat.x + diffuse) + vec3<f32>(spec);
}

// Weighted-blended OIT weight, biased strongly toward the camera using linear
// eye-space depth across the molecule's extent (see sphere.wgsl for rationale).
fn oit_weight(eye_z: f32, a: f32) -> f32 {
    let d = -eye_z;
    let t = clamp((d - camera.depth_range.x) / max(camera.depth_range.y - camera.depth_range.x, 1e-6), 0.0, 1.0);
    let bias = pow(1.0 - t, 3.0);
    return clamp(a * (1.0e-2 + bias * 1.0e3), 1.0e-3, 1.0e3);
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
    let lit = shade_material(in.color.rgb, n, view_dir, unpack_mat(in.mat));
    return vec4<f32>(apply_fog(lit, in.view_pos.z), in.color.a);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return shade(in);
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
