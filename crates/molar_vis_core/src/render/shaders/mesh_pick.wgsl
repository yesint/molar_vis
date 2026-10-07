// Pick id-buffer pass for mesh reps (Cartoon / Surface): each vertex carries the pick id
// of its source atom (x = mol+1, y = rep<<21 | atom), written flat into the Rg32Uint id
// target. Rasterized depth against the sphere impostors' analytic depth, as in the lit
// passes, so the front-most drawn geometry wins the pixel.

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
    @location(1) pick: vec2<u32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) pick: vec2<u32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.proj * (camera.view * vec4<f32>(v.pos, 1.0));
    out.pick = v.pick;
    return out;
}

@fragment
fn fs_pick(in: VsOut) -> @location(0) vec2<u32> {
    return in.pick;
}
