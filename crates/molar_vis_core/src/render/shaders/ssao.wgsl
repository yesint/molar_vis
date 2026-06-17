// Screen-space ambient occlusion. A fullscreen pass reads the scene depth and,
// for each pixel, estimates how occluded it is by nearby geometry — normal-free:
// it counts neighbours that sit *in front* of it in view space (so flat surfaces
// don't self-darken, but creases/contacts do). The AO factor is written back to
// the color target with a **multiply** blend, darkening crevices. A fixed spiral
// kernel (no per-pixel rotation) avoids noise; the 2× SSAA downsample smooths the
// mild banding that leaves.

struct Ssao {
    proj: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
    shadow_matrix: mat4x4<f32>, // view space -> light clip space
    params: vec4<f32>,          // radius, bias, strength, perspective(1/0)
    misc: vec4<f32>,            // render_w, render_h, _, _
    shadow_params: vec4<f32>,   // strength, bias, enabled, _
};

@group(0) @binding(0) var depth_tex: texture_depth_2d;
@group(0) @binding(1) var<uniform> u: Ssao;
@group(0) @binding(2) var shadow_map: texture_depth_2d;
@group(0) @binding(3) var shadow_samp: sampler_comparison;

// Cast-shadow test: project the view-space point into the light's clip space and
// compare against the shadow map (3×3 PCF). Returns 1 (lit) … `1-strength`
// (fully shadowed). `textureSampleCompareLevel` (no derivatives) is used so it's
// valid in the per-pixel control flow below.
fn shadow_factor(p_view: vec3<f32>) -> f32 {
    if (u.shadow_params.z < 0.5) {
        return 1.0; // disabled
    }
    let lc = u.shadow_matrix * vec4<f32>(p_view, 1.0);
    let ndc = lc.xyz / lc.w;
    if (ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0; // outside the light frustum → treat as lit
    }
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    let z_ref = ndc.z - u.shadow_params.y;
    let texel = 1.0 / 2048.0; // shadow map is 2048²
    var lit = 0.0;
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let o = vec2<f32>(f32(i), f32(j)) * texel;
            lit += textureSampleCompareLevel(shadow_map, shadow_samp, uv + o, z_ref);
        }
    }
    lit /= 9.0;
    return mix(1.0, lit, u.shadow_params.x);
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vidx: u32) -> VsOut {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: VsOut;
    out.pos = vec4<f32>(p[vidx], 0.0, 1.0);
    return out;
}

// Reconstruct view-space position from a UV (0..1, y-down) and the stored [0,1]
// depth, using the inverse projection.
fn view_pos(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec3<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth);
    let c = u.inv_proj * vec4<f32>(ndc, 1.0);
    return c.xyz / c.w;
}

const N: i32 = 16;
const GOLDEN: f32 = 2.3999632; // golden angle, for an even spiral kernel

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let dim = u.misc.xy;
    let coord = vec2<i32>(i32(in.pos.x), i32(in.pos.y));
    let depth = textureLoad(depth_tex, coord, 0);
    if (depth >= 1.0) {
        return vec4<f32>(1.0); // background: no occlusion
    }

    let uv = vec2<f32>(in.pos.xy) / dim;
    let p = view_pos(uv, depth);
    let radius = u.params.x;
    let bias = u.params.y;
    let strength = u.params.z;
    let persp = u.params.w > 0.5;
    // World radius → uv radius (per axis) via the projection scale. Orthographic
    // doesn't shrink with distance; perspective divides by eye-space depth.
    let w_denom = select(1.0, -p.z, persp);
    let r_uv = radius * vec2<f32>(u.proj[0][0], u.proj[1][1]) * 0.5 / max(w_denom, 1e-4);

    let imax = vec2<i32>(dim) - vec2<i32>(1, 1);
    var occ = 0.0;
    for (var i = 0; i < N; i = i + 1) {
        let fi = f32(i) + 0.5;
        let rr = sqrt(fi / f32(N));      // varied radii → less banding
        let a = fi * GOLDEN;
        let off = vec2<f32>(cos(a), sin(a)) * rr;
        let suv = uv + off * r_uv;
        let scoord = clamp(vec2<i32>(suv * dim), vec2<i32>(0, 0), imax);
        let sd = textureLoad(depth_tex, scoord, 0);
        if (sd >= 1.0) {
            continue;
        }
        let q = view_pos(suv, sd);
        let dz = q.z - p.z;              // > 0: neighbour is closer to the camera
        let dist = length(q - p);
        // Ignore neighbours beyond the radius (no haloing from distant geometry).
        let range = 1.0 - smoothstep(radius * 0.7, radius, dist);
        occ += select(0.0, range, dz > bias);
    }
    let ao = clamp(1.0 - (occ / f32(N)) * strength, 0.0, 1.0);
    // Combine with the cast-shadow factor; both darken the color via multiply blend.
    let f = ao * shadow_factor(p);
    return vec4<f32>(f, f, f, 1.0);
}
