// Flat colored lines for the Lines representation.

struct Camera {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    params: vec4<f32>,
    cue: vec4<f32>,    // depth cue: near, far, strength, _
    fog_color: vec4<f32>,
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
    @location(1) color: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) eye_z: f32,
};

fn unpack_color(c: u32) -> vec3<f32> {
    let r = f32((c >> 0u) & 0xffu) / 255.0;
    let g = f32((c >> 8u) & 0xffu) / 255.0;
    let b = f32((c >> 16u) & 0xffu) / 255.0;
    return vec3<f32>(r, g, b);
}

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    let view_pos = camera.view * vec4<f32>(v.pos, 1.0);
    out.clip = camera.proj * view_pos;
    out.color = unpack_color(v.color);
    out.eye_z = view_pos.z;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(apply_fog(in.color, in.eye_z), 1.0);
}
