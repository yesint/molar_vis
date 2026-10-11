// Camera-facing point discs with a fixed pixel diameter.
struct Camera {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    params: vec4<f32>,
    cue: vec4<f32>,
    fog_color: vec4<f32>,
    depth_range: vec4<f32>,
    glow_color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;
struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) size: f32,
    @location(2) color: u32,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) eye_z: f32,
    @location(2) uv: vec2<f32>,
};
@vertex
fn vs_main(@builtin(vertex_index) vidx: u32, point: VsIn) -> VsOut {
    let uv = vec2<f32>(select(-1.0, 1.0, (vidx & 1u) == 1u), select(-1.0, 1.0, vidx >= 2u));
    let view = camera.view * vec4<f32>(point.pos, 1.0);
    let clip = camera.proj * view;
    let viewport = max(camera.params.yz, vec2<f32>(1.0));
    var out: VsOut;
    out.clip = vec4<f32>(clip.xy + uv * point.size / viewport * clip.w, clip.z, clip.w);
    out.color = vec4<f32>(f32(point.color & 255u), f32((point.color >> 8u) & 255u),
        f32((point.color >> 16u) & 255u), f32(point.color >> 24u)) / 255.0;
    out.eye_z = view.z;
    out.uv = uv;
    return out;
}
fn color(in: VsOut) -> vec3<f32> {
    return depth_cue(in.color.rgb, -in.eye_z, camera.cue, camera.fog_color.rgb);
}
struct OpaqueOut {
    @location(0) color: vec4<f32>,
    @location(1) normal: vec4<f32>,
};
@fragment
fn fs_main(in: VsOut) -> OpaqueOut {
    if dot(in.uv, in.uv) > 1.0 { discard; }
    var out: OpaqueOut;
    out.color = vec4<f32>(color(in), in.color.a);
    out.normal = vec4<f32>(0.0, 0.0, 1.0, 1.0);
    return out;
}
struct OitOut {
    @location(0) accum: vec4<f32>,
    @location(1) reveal: f32,
};
@fragment
fn fs_oit(in: VsOut) -> OitOut {
    if dot(in.uv, in.uv) > 1.0 { discard; }
    let a = in.color.a;
    let w = transparency_weight(-in.eye_z, a, camera.depth_range.xy);
    var out: OitOut;
    out.accum = vec4<f32>(color(in) * a, a) * w;
    out.reveal = a;
    return out;
}
@fragment
fn fs_glow(in: VsOut) -> @location(0) vec4<f32> {
    if dot(in.uv, in.uv) > 1.0 { discard; }
    return vec4<f32>(camera.glow_color.rgb, clamp(0.85 * camera.params.w, 0.0, 1.0));
}
