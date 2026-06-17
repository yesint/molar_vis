// Fullscreen vertical background gradient (top → bottom). Drawn first in the
// opaque pass, color only.

struct Bg {
    top: vec4<f32>,
    bottom: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Bg;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) t: f32, // 0 at screen bottom, 1 at screen top
};

@vertex
fn vs_main(@builtin(vertex_index) vidx: u32) -> VsOut {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let xy = p[vidx];
    var out: VsOut;
    out.pos = vec4<f32>(xy, 1.0, 1.0);
    out.t = xy.y * 0.5 + 0.5;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = clamp(in.t, 0.0, 1.0);
    return vec4<f32>(mix(u.bottom.rgb, u.top.rgb, t), 1.0);
}
