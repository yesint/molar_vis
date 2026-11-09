// Screen-space fat lines for the Lines representation. WebGPU can only rasterize
// 1-px line primitives, so each segment is drawn as an instanced quad expanded
// perpendicular to the segment in pixel space by `width` pixels — constant width
// at any zoom, like VMD's line thickness.

struct Camera {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    params: vec4<f32>, // x: 1.0 = perspective; yz: viewport size in pixels
    cue: vec4<f32>,    // depth cue: near, far, strength, mode
    fog_color: vec4<f32>,
    depth_range: vec4<f32>, // OIT: eye-space [front, back, _, _]
    // Selection-glow color (rgb), chosen on the CPU from the viewport background so the
    // glow and the cues egui draws over the scene are one decision. See `theme::glow_color`.
    glow_color: vec4<f32>,
    path_animation: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

// Depth cueing (VMD cuemode): fade toward the background as eye-space distance
// grows. cue = [near, far, strength, mode]; mode 0 = linear, 1 = exp, 2 = exp2.
// All curves are normalized to reach full fog at the far plane, scaled by strength.
fn apply_fog(color: vec3<f32>, eye_z: f32) -> vec3<f32> {
    return depth_cue(color, -eye_z, camera.cue, camera.fog_color.rgb);
}

struct VsIn {
    @location(0) pos0: vec3<f32>,
    @location(1) color0: u32,
    @location(2) width: f32,
    @location(5) offset_px0: f32,
    @location(3) pos1: vec3<f32>,
    @location(4) color1: u32,
    @location(6) offset_px1: f32,
    @location(7) cap0: f32,
    @location(8) cap1: f32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) color0: vec4<f32>,
    @location(1) @interpolate(flat) color1: vec4<f32>,
    @location(2) local_w: vec3<f32>,
    @location(3) @interpolate(flat) shape: vec3<f32>, // projected length, radius, world length
    @location(4) @interpolate(flat) clip_w: vec2<f32>,
    @location(5) @interpolate(flat) depth: vec2<f32>,
    @location(6) @interpolate(flat) eye_z: vec2<f32>,
    @location(7) @interpolate(flat) dashed: u32,
};

fn unpack_color(c: u32) -> vec4<f32> {
    let r = f32((c >> 0u) & 0xffu) / 255.0;
    let g = f32((c >> 8u) & 0xffu) / 255.0;
    let b = f32((c >> 16u) & 0xffu) / 255.0;
    let a = f32((c >> 24u) & 0xffu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

@vertex
fn vs_main(@builtin(vertex_index) vidx: u32, seg: VsIn) -> VsOut {
    let viewport = max(camera.params.yz, vec2<f32>(1.0, 1.0));

    // Both endpoints in view + clip space.
    let view0 = camera.view * vec4<f32>(seg.pos0, 1.0);
    let view1 = camera.view * vec4<f32>(seg.pos1, 1.0);
    let clip0 = camera.proj * view0;
    let clip1 = camera.proj * view1;

    // Screen-space direction of the segment (pixels) from the two NDC positions.
    let ndc0 = clip0.xy / clip0.w;
    let ndc1 = clip1.xy / clip1.w;
    let screen0 = ndc0 * 0.5 * viewport;
    let screen1 = ndc1 * 0.5 * viewport;
    var dir = screen1 - screen0;
    let len = length(dir);
    dir = select(vec2<f32>(1.0, 0.0), dir / max(len, 1e-6), len > 1e-6);
    let perp = vec2<f32>(-dir.y, dir.x); // unit, pixels

    // Quad corners: vidx 0,1 at endpoint 0; vidx 2,3 at endpoint 1. Even = -side.
    let at_end1 = vidx >= 2u;
    let side = select(-1.0, 1.0, (vidx & 1u) == 1u);

    let clip = select(clip0, clip1, at_end1);

    // Multi-order strand offset (px, screen-space): shift the whole strand sideways
    // along the same screen perpendicular used for the width, so the parallel lines
    // of a double/triple/aromatic bond stay side-by-side and legible from any view
    // angle (the perpendicular is recomputed per frame from the projected segment).
    // 0.0 for a single bond.
    let strand_px = select(seg.offset_px0, seg.offset_px1, at_end1);

    // Offset by half-width pixels along the screen perpendicular (for the quad's
    // thickness) plus the strand offset. Convert the pixel offset to NDC (1 NDC
    // unit = viewport/2 px), then back to clip by *w so it survives the divide.
    let radius = abs(seg.width) * 0.5;
    let cap = select(seg.cap0, seg.cap1, at_end1);
    let extension = select(-1.0, 1.0, at_end1) * radius * cap;
    let offset_px = perp * (side * radius + strand_px) + dir * extension;
    let offset_ndc = offset_px / (0.5 * viewport);

    var out: VsOut;
    out.clip = vec4<f32>(clip.xy + offset_ndc * clip.w, clip.z, clip.w);
    out.color0 = unpack_color(seg.color0);
    out.color1 = unpack_color(seg.color1);
    // Multiplying by w and dividing after interpolation gives screen-linear
    // coordinates without noperspective, which is unavailable in WebGL2.
    out.local_w = vec3<f32>(vec2<f32>(select(0.0, len, at_end1) + extension, side * radius) * clip.w, clip.w);
    out.shape = vec3<f32>(len, radius, distance(seg.pos0, seg.pos1));
    out.clip_w = vec2<f32>(clip0.w, clip1.w);
    out.depth = vec2<f32>(clip0.z / clip0.w, clip1.z / clip1.w);
    out.eye_z = vec2<f32>(view0.z, view1.z);
    out.dashed = select(0u, 1u, seg.width < 0.0);
    return out;
}

// The expanded quad bounds a screen-space capsule. Only marked endpoints extend;
// midpoint color splits and dash ends remain flat. Clamp interpolation to the
// original segment so caps do not shift its color transition, fog, or depth.
fn screen_t(in: VsOut) -> f32 {
    return clamp((in.local_w.x / in.local_w.z) / max(in.shape.x, 1e-6), 0.0, 1.0);
}
fn world_t(in: VsOut) -> f32 {
    let t = screen_t(in);
    return t * in.clip_w.x / mix(in.clip_w.y, in.clip_w.x, t);
}
fn outside_line(in: VsOut) -> bool {
    let local = in.local_w.xy / in.local_w.z;
    let along = local.x - clamp(local.x, 0.0, in.shape.x);
    if dot(vec2<f32>(along, local.y), vec2<f32>(along, local.y)) > in.shape.y * in.shape.y {
        return true;
    }
    let along_nm = world_t(in) * in.shape.z;
    let phase = along_nm - floor(along_nm / LINE_DASH_PERIOD) * LINE_DASH_PERIOD;
    return in.dashed != 0u && phase > LINE_DASH_LEN;
}
fn line_color(in: VsOut) -> vec4<f32> {
    return mix(in.color0, in.color1, world_t(in));
}
fn line_eye_z(in: VsOut) -> f32 {
    return mix(in.eye_z.x, in.eye_z.y, world_t(in));
}
fn line_depth(in: VsOut) -> f32 {
    return mix(in.depth.x, in.depth.y, screen_t(in));
}

// Weighted-blended OIT weight, biased strongly toward the camera using linear
// eye-space depth across the molecule's extent (see sphere.wgsl for rationale).
fn oit_weight(eye_z: f32, a: f32) -> f32 {
    return transparency_weight(-eye_z, a, camera.depth_range.xy);
}

struct OpaqueOut {
    @location(0) color: vec4<f32>,
    @location(1) normal: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_main(in: VsOut) -> OpaqueOut {
    if outside_line(in) { discard; }
    var out: OpaqueOut;
    let color = line_color(in);
    out.color = vec4<f32>(apply_fog(color.rgb, line_eye_z(in)), color.a);
    out.depth = line_depth(in);
    out.normal = vec4<f32>(0.0, 0.0, 1.0, 1.0);
    return out;
}

// Additive cyan glow for the active (pending) selection (see sphere.wgsl). Lines
// are flat, so the glow is a constant additive cyan along the selected bonds.

struct GlowOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};
@fragment
fn fs_glow(in: VsOut) -> GlowOut {
    if outside_line(in) { discard; }
    // `camera.params.w` is the animated pulse multiplier (see render.rs).
    var out: GlowOut;
    out.color = vec4<f32>(camera.glow_color.rgb, clamp(0.85 * camera.params.w, 0.0, 1.0));
    out.depth = line_depth(in);
    return out;
}

struct OitOut {
    @location(0) accum: vec4<f32>,
    @location(1) reveal: f32,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_oit(in: VsOut) -> OitOut {
    if outside_line(in) { discard; }
    let color = line_color(in);
    let rgb = apply_fog(color.rgb, line_eye_z(in));
    let a = color.a;
    let w = oit_weight(line_eye_z(in), a);
    var out: OitOut;
    out.accum = vec4<f32>(rgb * a, a) * w;
    out.reveal = a;
    out.depth = line_depth(in);
    return out;
}
