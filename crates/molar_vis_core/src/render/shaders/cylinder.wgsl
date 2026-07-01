// Cylinder impostor. Each instance is one half-bond (p0..p1). A camera-facing
// billboard is generated in view space; the fragment shader ray-casts a finite,
// capless cylinder and writes analytic depth. Joints are covered by sphere caps.

struct Camera {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    params: vec4<f32>, // params.x: 1.0 = perspective, 0.0 = orthographic
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

struct Instance {
    @location(0) p0: vec3<f32>,
    @location(1) radius: f32,
    @location(2) p1: vec3<f32>,
    @location(3) color: u32,   // p0 half
    @location(6) color1: u32,  // p1 half
    @location(4) mat: u32,
    // Multi-order strand offset: x = signed slot (−1,0,+1…), y = gap (nm). The
    // strand is shifted by slot*gap along the bond's screen-plane perpendicular so
    // the parallel tubes of a double/triple/aromatic bond stay side-by-side and
    // legible from any angle. [0,0] for a single bond → a no-op.
    @location(5) offset: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) view_pos: vec3<f32>,
    @location(1) @interpolate(flat) base: vec3<f32>,   // p0 in view space
    @location(2) @interpolate(flat) axis: vec3<f32>,   // unit axis, view space
    @location(3) @interpolate(flat) radius: f32,
    @location(4) @interpolate(flat) seg_len: f32,
    @location(5) @interpolate(flat) color: vec4<f32>,  // p0 half: rgb + opacity
    @location(6) @interpolate(flat) mat: u32,          // packed material lighting
    @location(7) @interpolate(flat) color1: vec4<f32>, // p1 half: rgb + opacity
};

fn unpack_color(c: u32) -> vec4<f32> {
    let r = f32((c >> 0u) & 0xffu) / 255.0;
    let g = f32((c >> 8u) & 0xffu) / 255.0;
    let b = f32((c >> 16u) & 0xffu) / 255.0;
    let a = f32((c >> 24u) & 0xffu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

// Unpack the per-element material lighting coefficients (ambient, diffuse,
// specular, shininess).
fn unpack_mat(m: u32) -> vec4<f32> {
    let amb = f32((m >> 0u) & 0xffu) / 255.0;
    let dif = f32((m >> 8u) & 0xffu) / 255.0;
    let spc = f32((m >> 16u) & 0xffu) / 255.0;
    let shn = f32((m >> 24u) & 0x7fu) / 127.0; // top bit is the outline flag
    return vec4<f32>(amb, dif, spc, shn);
}

// VMD "Outline": darken fragments at grazing angles (silhouette edge). The flag is
// the top bit of the packed shininess byte (see sphere.wgsl).
fn apply_outline(color: vec3<f32>, normal: vec3<f32>, view_dir: vec3<f32>, m: u32) -> vec3<f32> {
    let on = f32((m >> 31u) & 1u);
    let edge = pow(1.0 - abs(dot(normal, view_dir)), 2.0);
    return color * (1.0 - on * 0.9 * edge);
}

// Blinn-Phong shade in view space (white specular highlight; `view_dir` to eye).
fn shade_material(base: vec3<f32>, normal: vec3<f32>, view_dir: vec3<f32>, mat: vec4<f32>) -> vec3<f32> {
    let light_dir = normalize(vec3<f32>(0.3, 0.4, 1.0));
    let ndotl = max(dot(normal, light_dir), 0.0);
    let half = normalize(light_dir + view_dir);
    let ndoth = max(dot(normal, half), 0.0);
    let exponent = 2.0 + mat.w * 128.0;
    let spec = mat.z * pow(ndoth, exponent);
    // Subtle fill from the opposite-lower side. The single headlight leaves undersides
    // and the curvature-discontinuity creases at sphere↔cylinder joints at pure ambient
    // — near-black rings that read as "gaps" breaking the sticks. Gated by (1 − ndotl)
    // so it only lifts the shadowed side; the lit side and the highlight are untouched
    // (the slick look is preserved). See sphere.wgsl for the matching term.
    let fill_dir = normalize(vec3<f32>(-0.2, -0.3, 0.6));
    let fill = max(dot(normal, fill_dir), 0.0) * (1.0 - ndotl) * FILL_STRENGTH;
    return base * (mat.x + mat.y * (ndotl + fill)) + vec3<f32>(spec);
}

// Fill-light strength (see shade_material). Shared with sphere.wgsl.
const FILL_STRENGTH: f32 = 0.35;

// Weighted-blended OIT weight, biased strongly toward the camera using linear
// eye-space depth across the molecule's extent (see sphere.wgsl for rationale).
fn oit_weight(eye_z: f32, a: f32) -> f32 {
    let d = -eye_z;
    let t = clamp((d - camera.depth_range.x) / max(camera.depth_range.y - camera.depth_range.x, 1e-6), 0.0, 1.0);
    let bias = pow(1.0 - t, 3.0);
    return clamp(a * (1.0e-2 + bias * 1.0e3), 1.0e-3, 1.0e3);
}

@vertex
fn vs_main(@builtin(vertex_index) vidx: u32, inst: Instance) -> VsOut {
    var a0 = (camera.view * vec4<f32>(inst.p0, 1.0)).xyz;
    var a1 = (camera.view * vec4<f32>(inst.p1, 1.0)).xyz;

    // Multi-order strand offset (screen-plane, per-frame). The screen plane in view
    // space is XY (camera looks down -Z), so the bond's screen-perpendicular is
    // cross(axis, +Z): perpendicular to the bond AND lying in the screen plane, so
    // the strands stay side-by-side from any angle. If the bond points at the camera
    // (axis ∥ Z) the cross degenerates → fall back to a fixed X. The shift is applied
    // in view space (so the impostor math below is unchanged); [0,0] → no-op.
    if (inst.offset.y != 0.0) {
        let ax = a1 - a0;
        let axl = length(ax);
        let dir = select(vec3<f32>(1.0, 0.0, 0.0), ax / axl, axl > 1e-8);
        var sp = cross(dir, vec3<f32>(0.0, 0.0, 1.0));
        let spl = length(sp);
        sp = select(vec3<f32>(1.0, 0.0, 0.0), sp / spl, spl > 1e-4);
        let shift = sp * (inst.offset.x * inst.offset.y);
        a0 = a0 + shift;
        a1 = a1 + shift;
    }

    let axis_v = a1 - a0;
    let seg_len = length(axis_v);
    let d = select(vec3<f32>(0.0, 0.0, 1.0), axis_v / seg_len, seg_len > 1e-8);

    // Screen-perpendicular extent: perpendicular to both the axis and the view
    // direction, so the billboard covers the silhouette width. The view
    // direction is the eye ray (perspective) or constant -Z (orthographic).
    let mid = (a0 + a1) * 0.5;
    let view_dir = select(vec3<f32>(0.0, 0.0, 1.0), normalize(mid), camera.params.x > 0.5);

    // Build the billboard in the **screen plane** (⊥ view_dir) so it robustly covers
    // the whole capsule — including the round caps — at ANY angle, crucially when the
    // bond points at the camera (end-on). The old billboard used
    // `perp = cross(axis, view_dir)`, which degenerates to a thin strip end-on (the
    // separate atom spheres used to cover the cap there — now that a bond is one capped
    // capsule, the billboard itself must cover it). `u` = the tube axis projected into
    // the screen plane; `w` = the screen perpendicular. Both fall back cleanly when the
    // tube is exactly end-on (projected axis ≈ 0).
    var u = d - view_dir * dot(d, view_dir);
    let ul = length(u);
    u = select(vec3<f32>(1.0, 0.0, 0.0), u / ul, ul > 1e-4);
    var w = cross(u, view_dir);
    let wl = length(w);
    w = select(vec3<f32>(0.0, 1.0, 0.0), w / wl, wl > 1e-4);

    // Oversize by 1.4× so the curved silhouette is fully covered (extra fragments miss
    // the ray test and `discard`); the sphere impostor does the same (1.25×). Extent
    // along the tube = projected half-length + cap; across = the radius.
    let bb = inst.radius * 1.4;
    let half_axis = 0.5 * abs(dot(a1 - a0, u)); // projected half-length of the tube
    let su = half_axis + bb;
    let sw = bb;

    // vidx -> quad corner in (u, w).
    var us = array<f32, 4>(-1.0, -1.0, 1.0, 1.0);
    var ws = array<f32, 4>(-1.0, 1.0, -1.0, 1.0);
    let pos = mid + u * (us[vidx] * su) + w * (ws[vidx] * sw);

    var out: VsOut;
    out.clip = camera.proj * vec4<f32>(pos, 1.0);
    // Conservative near-depth for `@early_depth_test(greater_equal)`. Keep clip.xy /
    // clip.w — hence the screen coverage, near-plane clipping, AND the interpolated
    // `view_pos` the fragment uses for the ray — identical to the plain billboard, so
    // the rendered image is unchanged. Override only clip.z to a lower bound on the
    // hit depth. This MUST be a **per-instance constant** (identical at all 4
    // vertices), NOT a per-vertex value: the rasterizer screen-linearly interpolates
    // NDC depth, and a straight line between two pulled endpoint depths rises *above*
    // the hyperbolic true-surface depth in the middle of a foreshortened billboard
    // (close-up grazing) → the "lower bound" breaks and early-Z wrongly culls
    // fragments (the black-wedge bug). A constant plane can't overshoot. Use the NDC
    // depth of the cylinder's **nearest point to the camera**. NDC depth is a function
    // of **eye-z only** (for BOTH perspective and orthographic — the projection's
    // z-row ignores x,y), so the minimum NDC depth is at the maximum eye-z. Over the
    // cylinder that is the larger endpoint z plus at most `radius` of perpendicular
    // bulge toward the camera: `max(a0.z, a1.z) + radius`. (An earlier version pulled
    // the nearest-by-*distance* axis point along the eye ray, which is wrong for
    // perspective — distance ≠ eye-z — and left early-Z unsound at grazing close-ups.)
    // Since only z matters, x,y are arbitrary.
    let near_z = max(a0.z, a1.z) + inst.radius;
    let near_c = camera.proj * vec4<f32>(0.0, 0.0, near_z, 1.0);
    // Guard the near-plane / behind-eye case (w → 0): fall back to depth 0 (the
    // nearest representable depth — a valid lower bound, just no early-Z benefit).
    var z_ndc_near = 0.0;
    if (near_c.w > 1e-6) {
        z_ndc_near = near_c.z / near_c.w;
    }
    // CRITICAL: clip.z is also used for primitive **clipping** (0 ≤ z ≤ w), not just
    // the depth test. If the conservative z goes below 0 (near pole in front of the
    // near plane — the atom sits at the near plane on a close-up) the billboard quad
    // gets near-clipped and the impostor develops holes/gaps. Clamp z into [0,1] so
    // clip.z ∈ [0, w] never causes extra clipping; a clamped-to-0 lower bound is still
    // ≤ the true depth, so early-Z stays sound.
    out.clip.z = clamp(z_ndc_near, 0.0, 1.0) * out.clip.w;
    out.view_pos = pos;
    out.base = a0;
    out.axis = d;
    out.radius = inst.radius;
    out.seg_len = seg_len;
    out.color = unpack_color(inst.color);
    out.color1 = unpack_color(inst.color1);
    out.mat = inst.mat;
    return out;
}

struct FsOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

// Result of ray-casting the impostor cylinder: shaded (fogged) color, opacity
// and the analytic [0,1] window depth. Misses `discard`. `normal`/`view_dir`
// (view space) are also returned for the selection glow's Fresnel rim.
struct Hit {
    color: vec3<f32>,
    alpha: f32,
    depth: f32,
    eye_z: f32,
    normal: vec3<f32>,
    view_dir: vec3<f32>,
};

fn compute_hit(in: VsOut) -> Hit {
    var ro: vec3<f32>;
    var rd: vec3<f32>;
    if (camera.params.x > 0.5) {
        ro = vec3<f32>(0.0, 0.0, 0.0);
        rd = normalize(in.view_pos);
    } else {
        // Parallel ray; origin on the camera plane (z=0) so the whole scene
        // (which lies at z<0) is in front and the near intersection has t>0.
        ro = vec3<f32>(in.view_pos.x, in.view_pos.y, 0.0);
        rd = vec3<f32>(0.0, 0.0, -1.0);
    }
    let ua = in.axis;

    // Ray-cast a **capsule**: the finite cylinder wall over h ∈ [0, seg_len] PLUS a
    // hemispherical cap at the **base** (`in.base`, the atom end). The mid end
    // (h = seg_len) stays flat so a bond's two half-strands meet seamlessly. Rounding
    // the atom end makes a bond read as one smooth capped tube — even end-on — instead
    // of a capless cylinder wall abutting a separate atom sphere, whose hard occlusion
    // seam showed as a dark "crescent". Take the nearest valid hit of {wall, cap}.
    var best_t = 1e30;
    var normal = vec3<f32>(0.0, 0.0, 1.0);

    // Cylinder wall (infinite cylinder, then clip h to the segment).
    let oc = ro - in.base;
    let rd_p = rd - ua * dot(rd, ua);
    let oc_p = oc - ua * dot(oc, ua);
    let a = dot(rd_p, rd_p);
    let b = 2.0 * dot(rd_p, oc_p);
    let c = dot(oc_p, oc_p) - in.radius * in.radius;
    let disc = b * b - 4.0 * a * c;
    if (disc >= 0.0 && a >= 1e-12) {
        let tw = (-b - sqrt(disc)) / (2.0 * a);
        let hw = dot((ro + tw * rd) - in.base, ua);
        if (tw > 0.0 && hw >= 0.0 && hw <= in.seg_len) {
            best_t = tw;
            normal = normalize((ro + tw * rd) - (in.base + ua * hw));
        }
    }

    // Hemispherical cap at the base (p0 atom) — only the outward (h ≤ 0) hemisphere;
    // the h > 0 half sits inside the cylinder and is covered by the wall. `rd` is unit
    // length (perspective: normalize; ortho: (0,0,-1)), so this ray-sphere is monic.
    let bs = dot(rd, oc);
    let cs = dot(oc, oc) - in.radius * in.radius;
    let discs = bs * bs - cs;
    if (discs >= 0.0) {
        let ts = -bs - sqrt(discs);
        if (ts > 0.0 && ts < best_t && dot((ro + ts * rd) - in.base, ua) <= 0.0) {
            best_t = ts;
            normal = normalize((ro + ts * rd) - in.base);
        }
    }

    // Hemispherical cap at the far end (p1 atom) — only the outward (h ≥ seg_len)
    // hemisphere. Capping BOTH ends makes each bond a full capsule, so an on-axis ray
    // always hits a cap (no end-on hole) and the surface is continuous (no seam).
    let far = in.base + ua * in.seg_len;
    let ocf = ro - far;
    let bf = dot(rd, ocf);
    let cf = dot(ocf, ocf) - in.radius * in.radius;
    let discf = bf * bf - cf;
    if (discf >= 0.0) {
        let tf = -bf - sqrt(discf);
        if (tf > 0.0 && tf < best_t && dot((ro + tf * rd) - far, ua) >= 0.0) {
            best_t = tf;
            normal = normalize((ro + tf * rd) - far);
        }
    }

    if (best_t > 1e29) {
        discard; // ray missed the capsule
    }
    let hit = ro + best_t * rd;
    // Two-tone (VMD half-bond) color: the p0 half up to the midpoint, then p1.
    let h_final = dot(hit - in.base, ua);
    let base_color = select(in.color, in.color1, h_final >= in.seg_len * 0.5);
    let clip = camera.proj * vec4<f32>(hit, 1.0);

    // Per-material Blinn-Phong (view dir to eye: origin for perspective, +z ortho).
    let view_dir = select(vec3<f32>(0.0, 0.0, 1.0), normalize(-hit), camera.params.x > 0.5);
    var lit = shade_material(base_color.rgb, normal, view_dir, unpack_mat(in.mat));
    lit = apply_outline(lit, normal, view_dir, in.mat);

    return Hit(apply_fog(lit, hit.z), base_color.a, clip.z / clip.w, hit.z, normal, view_dir);
}

// Additive cyan "rim glow" for the active (pending) selection (see sphere.wgsl).
const GLOW_COLOR: vec3<f32> = vec3<f32>(0.51, 0.84, 1.0);

// `camera.params.w` is the animated pulse multiplier (see render.rs).
fn glow_alpha(normal: vec3<f32>, view_dir: vec3<f32>) -> f32 {
    let ndotv = max(dot(normal, view_dir), 0.0);
    let rim = pow(1.0 - ndotv, 1.5);
    return clamp((0.45 + 1.15 * rim) * camera.params.w, 0.0, 1.0);
}

@fragment
fn fs_glow(in: VsOut) -> FsOut {
    let h = compute_hit(in);
    var out: FsOut;
    out.depth = h.depth;
    out.color = vec4<f32>(GLOW_COLOR, glow_alpha(h.normal, h.view_dir));
    return out;
}

@fragment
fn fs_main(in: VsOut) -> FsOut {
    let hit = compute_hit(in);
    var out: FsOut;
    out.depth = hit.depth;
    out.color = vec4<f32>(hit.color, hit.alpha);
    return out;
}

struct OitOut {
    @location(0) accum: vec4<f32>,
    @location(1) reveal: f32,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_oit(in: VsOut) -> OitOut {
    let hit = compute_hit(in);
    let w = oit_weight(hit.eye_z, hit.alpha);
    var out: OitOut;
    out.depth = hit.depth;
    out.accum = vec4<f32>(hit.color * hit.alpha, hit.alpha) * w;
    out.reveal = hit.alpha;
    return out;
}
