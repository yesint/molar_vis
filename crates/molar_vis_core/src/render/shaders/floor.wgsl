// Reflective ground plane: a large view-space quad at y = floor_y, sampling the
// pre-rendered reflection (the scene mirrored across the plane) by screen position.

struct Floor {
    proj: mat4x4<f32>,
    params: vec4<f32>,  // floor_y, half_extent, z_near, z_far (view space)
    params2: vec4<f32>, // reflectivity, fade_start, fade_end, _
    dims: vec4<f32>,    // render_w, render_h, _, _
    base: vec4<f32>,    // floor base color
};

@group(0) @binding(0) var<uniform> u: Floor;
@group(0) @binding(1) var reflect_tex: texture_2d<f32>;
@group(0) @binding(2) var reflect_samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) view_z: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vidx: u32) -> VsOut {
    let fy = u.params.x;
    let w = u.params.y;
    let zn = u.params.z;
    let zf = u.params.w;
    var corners = array<vec3<f32>, 4>(
        vec3<f32>(-w, fy, zn),
        vec3<f32>(w, fy, zn),
        vec3<f32>(-w, fy, zf),
        vec3<f32>(w, fy, zf),
    );
    let vp = corners[vidx];
    var out: VsOut;
    out.pos = u.proj * vec4<f32>(vp, 1.0);
    out.view_z = vp.z;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.pos.xy / u.dims.xy;
    let refl = textureSampleLevel(reflect_tex, reflect_samp, uv, 0.0).rgb;
    let reflectivity = u.params2.x;
    let col = mix(u.base.rgb, refl, reflectivity);
    // Fade with view-space distance so the floor melts into the background toward
    // the horizon instead of ending on a hard line.
    let dist = -in.view_z;
    let fade = clamp(
        1.0 - (dist - u.params2.y) / max(u.params2.z - u.params2.y, 1e-3),
        0.0,
        1.0,
    );
    return vec4<f32>(col, fade);
}
