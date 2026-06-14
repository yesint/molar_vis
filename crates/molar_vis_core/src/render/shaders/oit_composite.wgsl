// Weighted-blended OIT resolve. A fullscreen triangle reads the accumulation
// (sum of weighted premultiplied color) and revealage (product of 1-alpha)
// targets and blends the order-independent transparent color over the opaque
// scene color. Composited with blend (SrcAlpha, OneMinusSrcAlpha):
//   out = avg * (1 - reveal) + opaque * reveal
// (McGuire & Bavoil, "Weighted Blended Order-Independent Transparency").

@group(0) @binding(0) var accum_tex: texture_2d<f32>;
@group(0) @binding(1) var reveal_tex: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vidx: u32) -> VsOut {
    // Oversized fullscreen triangle.
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: VsOut;
    out.pos = vec4<f32>(p[vidx], 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let coord = vec2<i32>(i32(in.pos.x), i32(in.pos.y));
    let accum = textureLoad(accum_tex, coord, 0);
    let reveal = textureLoad(reveal_tex, coord, 0).r;
    // Average color over the accumulated weight; guard against overflow/zero.
    let avg = accum.rgb / max(accum.a, 1e-5);
    return vec4<f32>(avg, 1.0 - reveal);
}
