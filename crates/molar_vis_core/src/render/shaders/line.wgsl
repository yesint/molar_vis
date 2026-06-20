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
};

@group(0) @binding(0) var<uniform> camera: Camera;

// Depth cueing (VMD cuemode): fade toward the background as eye-space distance
// grows. cue = [near, far, strength, mode]; mode 0 = linear, 1 = exp, 2 = exp2.
// All curves are normalized to reach full fog at the far plane, scaled by strength.
fn apply_fog(color: vec3<f32>, eye_z: f32) -> vec3<f32> {
    let d = -eye_z; // eye-space distance (camera looks down -Z)
    let t = clamp((d - camera.cue.x) / max(camera.cue.y - camera.cue.x, 1e-6), 0.0, 1.0);
    let k = 3.0;
    var b = t; // linear
    if (camera.cue.w > 1.5) {
        b = (1.0 - exp(-k * k * t * t)) / (1.0 - exp(-k * k)); // exp2
    } else if (camera.cue.w > 0.5) {
        b = (1.0 - exp(-k * t)) / (1.0 - exp(-k)); // exp
    }
    return mix(color, camera.fog_color.rgb, b * camera.cue.z);
}

struct VsIn {
    @location(0) pos0: vec3<f32>,
    @location(1) color0: u32,
    @location(2) width: f32,
    @location(5) offset_px0: f32,
    @location(3) pos1: vec3<f32>,
    @location(4) color1: u32,
    @location(6) offset_px1: f32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>, // rgb + opacity
    @location(1) eye_z: f32,
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
    dir = select(vec2<f32>(1.0, 0.0), dir / len, len > 1e-6);
    let perp = vec2<f32>(-dir.y, dir.x); // unit, pixels

    // Quad corners: vidx 0,1 at endpoint 0; vidx 2,3 at endpoint 1. Even = -side.
    let at_end1 = vidx >= 2u;
    let side = select(-1.0, 1.0, (vidx & 1u) == 1u);

    let clip = select(clip0, clip1, at_end1);
    let color = unpack_color(select(seg.color0, seg.color1, at_end1));
    let eye_z = select(view0.z, view1.z, at_end1);

    // Multi-order strand offset (px, screen-space): shift the whole strand sideways
    // along the same screen perpendicular used for the width, so the parallel lines
    // of a double/triple/aromatic bond stay side-by-side and legible from any view
    // angle (the perpendicular is recomputed per frame from the projected segment).
    // 0.0 for a single bond.
    let strand_px = select(seg.offset_px0, seg.offset_px1, at_end1);

    // Offset by half-width pixels along the screen perpendicular (for the quad's
    // thickness) plus the strand offset. Convert the pixel offset to NDC (1 NDC
    // unit = viewport/2 px), then back to clip by *w so it survives the divide.
    let offset_px = perp * (side * (seg.width * 0.5) + strand_px);
    let offset_ndc = offset_px / (0.5 * viewport);

    var out: VsOut;
    out.clip = vec4<f32>(clip.xy + offset_ndc * clip.w, clip.z, clip.w);
    out.color = color;
    out.eye_z = eye_z;
    return out;
}

// Weighted-blended OIT weight, biased strongly toward the camera using linear
// eye-space depth across the molecule's extent (see sphere.wgsl for rationale).
fn oit_weight(eye_z: f32, a: f32) -> f32 {
    let d = -eye_z;
    let t = clamp((d - camera.depth_range.x) / max(camera.depth_range.y - camera.depth_range.x, 1e-6), 0.0, 1.0);
    let bias = pow(1.0 - t, 3.0);
    return clamp(a * (1.0e-2 + bias * 1.0e3), 1.0e-3, 1.0e3);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(apply_fog(in.color.rgb, in.eye_z), in.color.a);
}

// Additive cyan glow for the active (pending) selection (see sphere.wgsl). Lines
// are flat, so the glow is a constant additive cyan along the selected bonds.
const GLOW_COLOR: vec3<f32> = vec3<f32>(0.51, 0.84, 1.0);

@fragment
fn fs_glow(in: VsOut) -> @location(0) vec4<f32> {
    // `camera.params.w` is the animated pulse multiplier (see render.rs).
    return vec4<f32>(GLOW_COLOR, clamp(0.85 * camera.params.w, 0.0, 1.0));
}

struct OitOut {
    @location(0) accum: vec4<f32>,
    @location(1) reveal: f32,
};

@fragment
fn fs_oit(in: VsOut) -> OitOut {
    let rgb = apply_fog(in.color.rgb, in.eye_z);
    let a = in.color.a;
    let w = oit_weight(in.eye_z, a);
    var out: OitOut;
    out.accum = vec4<f32>(rgb * a, a) * w;
    out.reveal = a;
    return out;
}
