// Screen-space ambient occlusion. A fullscreen pass reads the scene depth and,
// for each pixel, estimates how occluded it is by nearby geometry. A view-space
// tangent plane from the smooth normal buffer rejects the pixel's own tilted surface;
// only neighbours above that plane occlude it. The AO factor is written back to
// the color target with a **multiply** blend, darkening crevices. A fixed spiral
// kernel avoids noise. Enough samples are essential on concave ribbons: a sparse
// kernel imprints overlapping copies of the silhouette as polygon-shaped patches.

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
@group(0) @binding(4) var normal_tex: texture_2d<f32>;

// Cast-shadow test: project the view-space point into the light's clip space and
// compare against the shadow map (disk PCF). Returns 1 (lit) … `1-strength`
// (fully shadowed). `textureSampleCompareLevel` (no derivatives) is used so it's
// valid in the per-pixel control flow below.
fn shadow_factor(p_view: vec3<f32>, normal_view: vec3<f32>) -> f32 {
    if (u.shadow_params.z < 0.5) {
        return 1.0; // disabled
    }
    let lc = u.shadow_matrix * vec4<f32>(p_view, 1.0);
    let ndc = lc.xyz / lc.w;
    if (ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0; // outside the light frustum → treat as lit
    }
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    // Increase bias only at grazing incidence. The light-depth row points along
    // the shadow camera's view axis; unlike screen derivatives this remains stable
    // under viewport zoom and directly tracks the receiver's slope to the light.
    let light_depth_dir = normalize(vec3<f32>(
        u.shadow_matrix[0].z,
        u.shadow_matrix[1].z,
        u.shadow_matrix[2].z,
    ));
    let incidence = abs(dot(normal_view, light_depth_dir));
    let receiver_bias = u.shadow_params.y * (1.25 - 0.25 * incidence);
    let z_ref = ndc.z - receiver_bias;
    // Each PCF tap lies at a different point on the receiving tangent plane.
    // Comparing every tap with the center depth falsely shadows sloped surfaces,
    // especially when a soft light uses a broad filter footprint.
    let row_x = vec3<f32>(u.shadow_matrix[0].x, u.shadow_matrix[1].x, u.shadow_matrix[2].x);
    let row_y = vec3<f32>(u.shadow_matrix[0].y, u.shadow_matrix[1].y, u.shadow_matrix[2].y);
    let row_z = vec3<f32>(u.shadow_matrix[0].z, u.shadow_matrix[1].z, u.shadow_matrix[2].z);
    let clip_normal = vec3<f32>(dot(normal_view, row_x) / dot(row_x, row_x),
        dot(normal_view, row_y) / dot(row_y, row_y),
        dot(normal_view, row_z) / dot(row_z, row_z));
    let nz = select(-1.0, 1.0, clip_normal.z >= 0.0) * max(abs(clip_normal.z), length(clip_normal) * 0.05);
    // The projected plane is singular at the terminator. Fade its correction to
    // zero there so changing the sign of nz cannot introduce a sharp shadow seam.
    let depth_slope = -2.0 * vec2<f32>(clip_normal.x, -clip_normal.y) / nz
        * smoothstep(0.0, 0.25, incidence);
    let texel = u.misc.z; // 1/shadow_res (PCF step), from the settings
    var lit = 0.0;
    let samples = select(128, 1, u.shadow_params.w <= 0.0);
    let filter_width = texel * shadow_filter_width(u.shadow_params.w);
    for (var i = 0; i < samples; i = i + 1) {
        // Hard shadows have zero filter width; the disk sample is immaterial.
        let o = ao_disk_offset(f32(i)) * filter_width;
        lit += textureSampleCompareLevel(shadow_map, shadow_samp, uv + o, z_ref + dot(depth_slope, o));
    }
    lit /= f32(samples);
    // Smooth-normal terminator for closed molecular surfaces, matching the RT
    // hemisphere test. The depth map accounts for other occluders, not this side.
    let facing = dot(normal_view, -light_depth_dir);
    let angular_radius = max(shadow_angular_radius(u.shadow_params.w), 1e-4);
    lit *= smoothstep(-angular_radius, angular_radius, facing);
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

const N: i32 = i32(AO_KERNEL_SIZE);

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
    // Screen-space depth alone cannot compare `q.z > p.z`: half of every tilted
    // plane is closer to the camera and would falsely occlude itself. Use the same
    // smooth normals as lighting; depth derivatives are only a fallback.
    let stored_normal = textureLoad(normal_tex, coord, 0).xyz;
    var normal = stored_normal;
    let stored_len = length(stored_normal);
    if (stored_len <= 0.5) {
        normal = cross(dpdx(p), dpdy(p));
    }
    let normal_len = length(normal);
    normal = select(vec3<f32>(0.0, 0.0, 1.0), normal / max(normal_len, 1e-12), normal_len > 1e-8);
    let view_dir = select(vec3<f32>(0.0, 0.0, 1.0), normalize(-p), u.params.w > 0.5);
    if (dot(normal, view_dir) < 0.0) { normal = -normal; }
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
    for (var i = 0; i < N && strength > 0.0; i = i + 1) {
        let off = ao_disk_offset(f32(i));
        let suv = uv + off * r_uv;
        let scoord = clamp(vec2<i32>(suv * dim), vec2<i32>(0, 0), imax);
        let sd = textureLoad(depth_tex, scoord, 0);
        if (sd >= 1.0) {
            continue;
        }
        let q = view_pos((vec2<f32>(scoord) + 0.5) / dim, sd);
        let height = dot(q - p, normal); // > 0: neighbour is above the tangent plane
        let dist = length(q - p);
        // Ignore neighbours beyond the radius (no haloing from distant geometry).
        let range = ao_range(dist, radius);
        // A soft world-space threshold avoids 1/N contour steps when a blocker
        // crosses the numerical bias, especially on high-contrast dark materials.
        let above = smoothstep(bias, bias + max(0.1 * radius, 1e-4), height);
        occ += range * above;
    }
    let ao = clamp(1.0 - (occ / f32(N)) * strength, 0.0, 1.0);
    // Combine with the cast-shadow factor; both darken the color via multiply blend.
    let f = ao * shadow_factor(p, normal);
    return vec4<f32>(f, f, f, 1.0);
}
