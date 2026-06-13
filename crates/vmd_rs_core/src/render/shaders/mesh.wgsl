// Lambert-shaded triangle mesh for the Cartoon representation. Eye-space
// headlight (light = camera direction) with two-sided shading + ambient.

struct Camera {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal_eye: vec3<f32>,
    @location(1) color: vec3<f32>,
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
    out.clip = camera.proj * (camera.view * vec4<f32>(v.pos, 1.0));
    // view is rigid (rotation + translation); w=0 applies only the rotation.
    out.normal_eye = (camera.view * vec4<f32>(v.normal, 0.0)).xyz;
    out.color = unpack_color(v.color);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal_eye);
    // Headlight: camera looks down -z, so the view direction toward the eye is
    // +z. Two-sided (abs) so back faces of open ribbons are still lit.
    let diff = abs(n.z);
    let ambient = 0.35;
    let shade = ambient + (1.0 - ambient) * diff;
    return vec4<f32>(in.color * shade, 1.0);
}
