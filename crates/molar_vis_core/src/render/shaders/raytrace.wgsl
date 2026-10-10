// GPU ray tracer — ray-traced ambient occlusion + shadows + Blinn-Phong, the VMD-Tachyon
// / PyMOL-`ray` look, over all primitive types: analytic spheres + cylinders (VDW /
// licorice / ball-and-stick) and triangle meshes (cartoon / surface). A compute pass
// accumulates `samples` paths per pixel (each: 1 primary + 1 shadow ray + 1 cosine-
// hemisphere AO ray) into a linear Rgba32Float target; `fs_resolve` tonemaps it into the
// sRGB scene color target. Layout mirrors `render/raytrace.rs`.

struct Sphere {
    c: vec4<f32>,   // xyz = center (nm), w = radius
    m: vec4<u32>,   // x = color (RGBA8), y = packed material
};
struct Cyl {
    profile: vec4<f32>,
    lane: vec4<f32>,
    c0: vec4<f32>,  // xyz = p0, w = radius
    c1: vec4<f32>,  // xyz = p1
    m: vec4<u32>,   // x = color, y = packed material
};
struct MeshVert {
    p: vec4<f32>,   // xyz = position, w = bitcast(color RGBA8)
    n: vec4<f32>,   // xyz = normal,   w = bitcast(packed material)
};
struct BvhNode {
    lo: vec4<f32>,  // xyz = aabb min, w = bitcast(link)   (interior: left child; leaf: first prim)
    hi: vec4<f32>,  // xyz = aabb max, w = bitcast(count)  (0 => interior, >0 => leaf)
};

struct RtUniform {
    inv_view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    shadow_matrix: mat4x4<f32>,
    shadow_u: vec4<f32>,
    shadow_v: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
    depth_range: vec4<f32>,
    eye: vec4<f32>,             // xyz eye world, w = perspective flag
    light_dir: vec4<f32>,       // xyz world dir toward the key light (shadow ray)
    ao: vec4<f32>,              // radius (nm), bias, strength, enabled
    shadow: vec4<f32>,          // strength, bias in nm, enabled, softness
    bg: vec4<f32>,              // background (linear); w = GI strength
    cue: vec4<f32>,             // depth cue: near, far (eye-space), strength, mode
    fog_color: vec4<f32>,       // color the fog fades toward
    dims: vec4<u32>,            // width, height, samples-this-step, frame_seed
    accum: vec4<u32>,           // prior_total_samples, reset(0/1), _, _
};

@group(0) @binding(0) var<uniform> U: RtUniform;
@group(0) @binding(1) var<storage, read> spheres: array<Sphere>;
@group(0) @binding(2) var<storage, read> cylinders: array<Cyl>;
@group(0) @binding(3) var<storage, read> mesh_verts: array<MeshVert>;
@group(0) @binding(4) var<storage, read> triangles: array<vec4<u32>>; // (i0,i1,i2,closed_surface)
@group(0) @binding(5) var<storage, read> nodes: array<BvhNode>;
@group(0) @binding(6) var<storage, read> prim_indices: array<u32>;    // (type<<30)|index
@group(0) @binding(7) var accum: texture_storage_2d<rgba32float, write>;
@group(0) @binding(9) var accum_prev: texture_2d<f32>; // running average to extend (ping-pong)

const PI: f32 = 3.14159265359;
const T_MAX: f32 = 1e30;
const IDX_MASK: u32 = 0x3fffffffu;

// Direct shading uses lighting.wgsl, exactly as the live renderer does.
// AO measures 3D hemisphere visibility with the same radius and strength controls.
// Shadow rays use the same light frame, world bias and filter footprint as the map.
const AO_RAYS: u32 = 4u;
const SHADOW_RAYS: u32 = 4u;

fn pcg(v_in: u32) -> u32 {
    let state = v_in * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}
fn rand(seed: ptr<function, u32>) -> f32 {
    *seed = pcg(*seed);
    return f32(*seed) / 4294967296.0;
}

fn unpack_color(c: u32) -> vec3<f32> {
    return vec3<f32>(f32(c & 0xffu), f32((c >> 8u) & 0xffu), f32((c >> 16u) & 0xffu)) / 255.0;
}

// Alpha byte = material opacity (geometry::build folds the material's opacity into the
// per-element colour alpha). 1 = opaque.
fn unpack_opacity(c: u32) -> f32 {
    return f32((c >> 24u) & 0xffu) / 255.0;
}
// Material lighting coefficients; top bit of shininess byte is the outline flag.
fn unpack_mat(m: u32) -> vec4<f32> {
    return vec4<f32>(
        f32(m & 0xffu) / 255.0,
        f32((m >> 8u) & 0xffu) / 255.0,
        f32((m >> 16u) & 0xffu) / 255.0,
        f32((m >> 24u) & 0x7fu) / 127.0,
    );
}

fn camera_ray(ndc: vec2<f32>, ro: ptr<function, vec3<f32>>, rd: ptr<function, vec3<f32>>) {
    let near4 = U.inv_view_proj * vec4<f32>(ndc, 0.0, 1.0);
    let far4 = U.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let near = near4.xyz / near4.w;
    let far = far4.xyz / far4.w;
    *ro = near;
    *rd = normalize(far - near);
}

fn hit_aabb(lo: vec3<f32>, hi: vec3<f32>, ro: vec3<f32>, inv: vec3<f32>, tmax: f32) -> bool {
    let t0 = (lo - ro) * inv;
    let t1 = (hi - ro) * inv;
    let tsmall = min(t0, t1);
    let tbig = max(t0, t1);
    let enter = max(max(tsmall.x, tsmall.y), max(tsmall.z, 1e-4));
    let exit = min(min(tbig.x, tbig.y), min(tbig.z, tmax));
    return enter <= exit;
}

fn ray_sphere(s: Sphere, ro: vec3<f32>, rd: vec3<f32>, exit_inside: bool) -> f32 {
    let oc = ro - s.c.xyz;
    let b = dot(oc, rd);
    let c = dot(oc, oc) - s.c.w * s.c.w;
    let disc = b * b - c;
    if (disc < 0.0) { return -1.0; }
    var t = -b - sqrt(disc);
    if (exit_inside && t <= 1e-4) { t = -b + sqrt(disc); }
    if (t > 1e-4) { return t; }
    return -1.0;
}

// Ray-cast a **capsule**: the finite cylinder wall over h ∈ [0, seg] plus a hemispherical cap
// at each end. This mirrors the rasterizer's `cylinder.wgsl::compute_hit` — since bonds became
// two-tone capsules, Licorice/Ball-and-Stick emit an atom sphere only for *bondless* atoms, so a
// capless cylinder here left every bond an open tube (you could see down its dark interior) with
// no atom ends at all. Nearest valid hit of {wall, cap0, cap1}; `rd` is unit length, so the
// ray-sphere tests are monic.
//
// `m.w & FLAG_FLAT_ENDS` suppresses the caps, for primitives standing in for the rasterizer's
// flat-ended **line** quads: a dashed line's gaps are only a few pixels, so rounded ends bridged
// them and every dashed contact line traced as a solid one.
fn ray_cylinder(c: Cyl, ro: vec3<f32>, rd: vec3<f32>, exit_inside: bool) -> f32 {
    let flat = (c.m.w & 1u) != 0u;
    let p0 = c.c0.xyz;
    let r = c.c0.w;
    let axis = c.c1.xyz - p0;
    let seg = length(axis);
    if (seg < 1e-9) { return -1.0; }
    let ua = axis / seg;
    if (bond_profile_enabled(c.profile)) {
        return bond_profile_ray(p0, ua, seg, r, c.profile, c.lane.w, c.lane.xyz, ro, rd, exit_inside).x;
    }
    let oc = ro - p0;
    var best = -1.0;

    // Wall: infinite cylinder, then clip h to the segment.
    let rd_p = rd - ua * dot(rd, ua);
    let oc_p = oc - ua * dot(oc, ua);
    let a = dot(rd_p, rd_p);
    let b = 2.0 * dot(rd_p, oc_p);
    let cc = dot(oc_p, oc_p) - r * r;
    let disc = b * b - 4.0 * a * cc;
    if (disc >= 0.0 && a >= 1e-12) {
        let roots = vec2<f32>((-b - sqrt(disc)) / (2.0 * a), (-b + sqrt(disc)) / (2.0 * a));
        for (var i = 0u; i < select(1u, 2u, exit_inside); i = i + 1u) {
            let t = roots[i];
            let h = dot(ro + t * rd - p0, ua);
            if (t > 1e-4 && h >= 0.0 && h <= seg && (best < 0.0 || t < best)) { best = t; }
        }
    }
    if (flat) { return best; }
    // Cap at p0 — only its outward (h ≤ 0) hemisphere; the rest is inside the wall.
    let b0 = dot(rd, oc);
    let c0 = dot(oc, oc) - r * r;
    let d0 = b0 * b0 - c0;
    if (d0 >= 0.0) {
        let roots = vec2<f32>(-b0 - sqrt(d0), -b0 + sqrt(d0));
        for (var i = 0u; i < select(1u, 2u, exit_inside); i = i + 1u) {
            let t = roots[i];
            if (t > 1e-4 && (best < 0.0 || t < best) && dot(ro + t * rd - p0, ua) <= 0.0) { best = t; }
        }
    }
    // Cap at p1 — only its outward (h ≥ seg) hemisphere.
    let far = p0 + ua * seg;
    let ocf = ro - far;
    let b1 = dot(rd, ocf);
    let c1 = dot(ocf, ocf) - r * r;
    let d1 = b1 * b1 - c1;
    if (d1 >= 0.0) {
        let roots = vec2<f32>(-b1 - sqrt(d1), -b1 + sqrt(d1));
        for (var i = 0u; i < select(1u, 2u, exit_inside); i = i + 1u) {
            let t = roots[i];
            if (t > 1e-4 && (best < 0.0 || t < best) && dot(ro + t * rd - p0, ua) >= seg) { best = t; }
        }
    }
    return best;
}

// Möller–Trumbore; returns (t, u, v), t < 0 = miss.
fn ray_triangle(v0: vec3<f32>, v1: vec3<f32>, v2: vec3<f32>, ro: vec3<f32>, rd: vec3<f32>) -> vec3<f32> {
    let e1 = v1 - v0;
    let e2 = v2 - v0;
    let pv = cross(rd, e2);
    let det = dot(e1, pv);
    if (abs(det) < 1e-9) { return vec3<f32>(-1.0); }
    let inv = 1.0 / det;
    let tv = ro - v0;
    let u = dot(tv, pv) * inv;
    if (u < 0.0 || u > 1.0) { return vec3<f32>(-1.0); }
    let qv = cross(tv, e1);
    let v = dot(rd, qv) * inv;
    if (v < 0.0 || u + v > 1.0) { return vec3<f32>(-1.0); }
    let t = dot(e2, qv) * inv;
    if (t <= 1e-4) { return vec3<f32>(-1.0); }
    return vec3<f32>(t, u, v);
}

struct Hit { t: f32, prim: u32, uv: vec2<f32>, leaf: u32 };

fn primitive_opacity(prim: u32, p: vec3<f32>, uv: vec2<f32>) -> f32 {
    let typ = prim >> 30u;
    let idx = prim & IDX_MASK;
    if (typ == 0u) { return unpack_opacity(spheres[idx].m.x); }
    if (typ == 1u) {
        let cy = cylinders[idx];
        let axis = cy.c1.xyz - cy.c0.xyz;
        return unpack_opacity(select(cy.m.z, cy.m.x, dot(p - cy.c0.xyz, axis) < dot(axis, axis) * 0.5));
    }
    let tri = triangles[idx];
    return (1.0 - uv.x - uv.y) * unpack_opacity(bitcast<u32>(mesh_verts[tri.x].p.w))
        + uv.x * unpack_opacity(bitcast<u32>(mesh_verts[tri.y].p.w))
        + uv.y * unpack_opacity(bitcast<u32>(mesh_verts[tri.z].p.w));
}

// Match the rasterizer's union boundary; groups isolate representations and images.
fn envelope_group(tagged: u32) -> u32 {
    let typ = tagged >> 30u;
    let idx = tagged & IDX_MASK;
    if (typ == 0u) { return spheres[idx].m.z; }
    if (typ == 1u) { return cylinders[idx].m.w >> 1u; }
    return 0u;
}
fn inside_envelope(p: vec3<f32>, own: u32) -> bool {
    let group = envelope_group(own);
    if (group == 0u) { return false; }
    var stack: array<u32, 32>;
    var sp = 1u;
    stack[0] = 0u;
    loop {
        if (sp == 0u) { break; }
        sp = sp - 1u;
        let n = nodes[stack[sp]];
        if (any(p < n.lo.xyz - vec3<f32>(1e-5)) || any(p > n.hi.xyz + vec3<f32>(1e-5))) { continue; }
        let count = bitcast<u32>(n.hi.w);
        let link = bitcast<u32>(n.lo.w);
        if (count == 0u) {
            stack[sp] = link; stack[sp + 1u] = link + 1u; sp = sp + 2u;
        } else {
            for (var k = 0u; k < count; k = k + 1u) {
                let tagged = prim_indices[link + k];
                if (tagged == own || envelope_group(tagged) != group) { continue; }
                let typ = tagged >> 30u;
                let idx = tagged & IDX_MASK;
                var nearest: vec3<f32>;
                var radius: f32;
                if (typ == 0u) {
                    nearest = spheres[idx].c.xyz;
                    radius = spheres[idx].c.w;
                } else {
                    let c = cylinders[idx];
                    let ab = c.c1.xyz - c.c0.xyz;
                    if (bond_profile_enabled(c.profile)) {
                        let d = bond_profile_containment(p, c.c0.xyz, normalize(ab), length(ab), c.c0.w, c.profile, c.lane.w, c.lane.xyz);
                        if (d < -2e-4 || (abs(d) <= 2e-4 && tagged < own)) { return true; }
                        continue;
                    }
                    let along = clamp(dot(p - c.c0.xyz, ab) / max(dot(ab, ab), 1e-16), 0.0, 1.0);
                    nearest = c.c0.xyz + along * ab;
                    radius = c.c0.w;
                }
                let delta = p - nearest;
                let r2 = radius * radius;
                let d2 = dot(delta, delta);
                let tolerance = max(1e-10, r2 * 2e-4);
                if (d2 < r2 - tolerance || (abs(d2 - r2) <= tolerance && tagged < own)) { return true; }
            }
        }
    }
    return false;
}

// Entry distance for ordering children. The interval and epsilon match hit_aabb.
fn aabb_entry(lo: vec3<f32>, hi: vec3<f32>, ro: vec3<f32>, inv: vec3<f32>, tmax: f32) -> f32 {
    let t0 = (lo - ro) * inv;
    let t1 = (hi - ro) * inv;
    let near = min(t0, t1);
    let far = max(t0, t1);
    let enter = max(max(near.x, near.y), max(near.z, 1e-4));
    let exit = min(min(far.x, far.y), min(far.z, tmax));
    return select(T_MAX, enter, enter <= exit);
}

fn closest_hit(ro: vec3<f32>, rd: vec3<f32>) -> Hit {
    return closest_hit_filtered(ro, rd, false);
}

fn closest_hit_filtered(ro: vec3<f32>, rd: vec3<f32>, opaque_only: bool) -> Hit {
    var hit: Hit;
    hit.t = T_MAX;
    hit.prim = 0xffffffffu;
    hit.leaf = 0u;
    if (arrayLength(&nodes) == 0u) { return hit; }
    let inv = 1.0 / rd;
    var stack: array<u32, 32>;
    var sp = 0;
    stack[sp] = 0u; sp = sp + 1;
    loop {
        if (sp == 0) { break; }
        sp = sp - 1;
        let n = nodes[stack[sp]];
        if (!hit_aabb(n.lo.xyz, n.hi.xyz, ro, inv, hit.t)) { continue; }
        let count = bitcast<u32>(n.hi.w);
        let link = bitcast<u32>(n.lo.w);
        if (count == 0u) {
            let left = nodes[link];
            let right = nodes[link + 1u];
            let dl = aabb_entry(left.lo.xyz, left.hi.xyz, ro, inv, hit.t);
            let dr = aabb_entry(right.lo.xyz, right.hi.xyz, ro, inv, hit.t);
            // Far child first on the stack, near child first during traversal.
            if (dl < dr) {
                if (dr < T_MAX) { stack[sp] = link + 1u; sp = sp + 1; }
                if (dl < T_MAX) { stack[sp] = link; sp = sp + 1; }
            } else {
                if (dl < T_MAX) { stack[sp] = link; sp = sp + 1; }
                if (dr < T_MAX) { stack[sp] = link + 1u; sp = sp + 1; }
            }
        } else {
            for (var k = 0u; k < count; k = k + 1u) {
                let tagged = prim_indices[link + k];
                let typ = tagged >> 30u;
                let idx = tagged & IDX_MASK;
                if (typ == 0u) {
                    let t = ray_sphere(spheres[idx], ro, rd, false);
                    if (t > 0.0 && (t < hit.t || (t == hit.t && hit.prim != 0xffffffffu && link > hit.leaf)) && (!opaque_only || primitive_opacity(tagged, ro + rd * t, vec2<f32>(0.0)) >= 0.999) && !inside_envelope(ro + rd * t, tagged)) { hit.t = t; hit.prim = tagged; hit.leaf = link; }
                } else if (typ == 1u) {
                    let t = ray_cylinder(cylinders[idx], ro, rd, false);
                    if (t > 0.0 && (t < hit.t || (t == hit.t && hit.prim != 0xffffffffu && link > hit.leaf)) && (!opaque_only || primitive_opacity(tagged, ro + rd * t, vec2<f32>(0.0)) >= 0.999) && !inside_envelope(ro + rd * t, tagged)) { hit.t = t; hit.prim = tagged; hit.leaf = link; }
                } else {
                    let tri = triangles[idx];
                    let r = ray_triangle(mesh_verts[tri.x].p.xyz, mesh_verts[tri.y].p.xyz, mesh_verts[tri.z].p.xyz, ro, rd);
                    if (r.x > 0.0 && (r.x < hit.t || (r.x == hit.t && hit.prim != 0xffffffffu && link > hit.leaf)) && (!opaque_only || primitive_opacity(tagged, ro + rd * r.x, r.yz) >= 0.999)) { hit.t = r.x; hit.prim = tagged; hit.leaf = link; hit.uv = r.yz; }
                }
            }
        }
    }
    return hit;
}

fn any_hit(ro: vec3<f32>, rd: vec3<f32>, tmax: f32, skip_surface_exits: bool, skip_prim: u32) -> bool {
    if (arrayLength(&nodes) == 0u) { return false; }
    let inv = 1.0 / rd;
    var stack: array<u32, 32>;
    var sp = 0;
    stack[sp] = 0u; sp = sp + 1;
    loop {
        if (sp == 0) { break; }
        sp = sp - 1;
        let n = nodes[stack[sp]];
        if (!hit_aabb(n.lo.xyz, n.hi.xyz, ro, inv, tmax)) { continue; }
        let count = bitcast<u32>(n.hi.w);
        let link = bitcast<u32>(n.lo.w);
        if (count == 0u) {
            stack[sp] = link; sp = sp + 1;
            stack[sp] = link + 1u; sp = sp + 1;
        } else {
            for (var k = 0u; k < count; k = k + 1u) {
                let tagged = prim_indices[link + k];
                // Secondary mesh rays start numerically on their source triangle.
                // Exclude that exact primitive instead of relying on a world-space
                // epsilon whose rounding differs between GPU vendors.
                if (tagged == skip_prim) { continue; }
                let typ = tagged >> 30u;
                let idx = tagged & IDX_MASK;
                var t = -1.0;
                var uv = vec2<f32>(0.0);
                if (typ == 0u) {
                    t = ray_sphere(spheres[idx], ro, rd, true);
                } else if (typ == 1u) {
                    t = ray_cylinder(cylinders[idx], ro, rd, true);
                } else {
                    let tri = triangles[idx];
                    let a = mesh_verts[tri.x].p.xyz;
                    let b = mesh_verts[tri.y].p.xyz;
                    let c = mesh_verts[tri.z].p.xyz;
                    let result = ray_triangle(a, b, c, ro, rd);
                    t = result.x; uv = result.yz;
                    // Classify exits in the same smooth normal field as shading.
                    // A geometric face normal can call a grazing exit an entry and
                    // stamp the triangle onto curved ribbons' shadow terminators.
                    // Incoming hits on other closed objects still block the ray.
                    if (t > 0.0 && skip_surface_exits && tri.w != 0u) {
                        let hit_normal = (1.0 - uv.x - uv.y) * mesh_verts[tri.x].n.xyz
                            + uv.x * mesh_verts[tri.y].n.xyz + uv.y * mesh_verts[tri.z].n.xyz;
                        if (dot(hit_normal, rd) > 0.0) { continue; }
                    }
                }
                if (t > 1e-4 && t < tmax && primitive_opacity(tagged, ro + rd * t, uv) >= 0.999) { return true; }
            }
        }
    }
    return false;
}

fn onb(n: vec3<f32>, t: ptr<function, vec3<f32>>, b: ptr<function, vec3<f32>>) {
    let s = select(-1.0, 1.0, n.z >= 0.0);
    let a = -1.0 / (s + n.z);
    let bb = n.x * n.y * a;
    *t = vec3<f32>(1.0 + s * n.x * n.x * a, s * bb, -s * n.x);
    *b = vec3<f32>(bb, s + n.y * n.y * a, -n.y);
}
fn cosine_hemisphere(n: vec3<f32>, u1: f32, u2: f32) -> vec3<f32> {
    let r = sqrt(u1);
    let phi = 2.0 * PI * u2;
    var t: vec3<f32>;
    var b: vec3<f32>;
    onb(n, &t, &b);
    return normalize(r * cos(phi) * t + r * sin(phi) * b + sqrt(max(0.0, 1.0 - u1)) * n);
}

// Depth cueing (fog), the same model and the same three curves as the rasterizer's shared
// `apply_fog` — the trace is meant to reproduce the view, and a fogged view traced unfogged
// loses all of its depth. `d` is the **axial** eye-space distance (distance from the camera
// plane along the view axis), which is what `cue.x/.y` are expressed in; the view axis comes
// from unprojecting the frustum centre, so no extra uniform is needed and it is correct for
// both projections.
fn view_axis() -> vec3<f32> {
    let a = U.inv_view_proj * vec4<f32>(0.0, 0.0, 0.0, 1.0);
    let b = U.inv_view_proj * vec4<f32>(0.0, 0.0, 1.0, 1.0);
    return normalize(b.xyz / b.w - a.xyz / a.w);
}

fn apply_fog(color: vec3<f32>, p: vec3<f32>, axis: vec3<f32>) -> vec3<f32> {
    return depth_cue(color, dot(p - U.eye.xyz, axis), U.cue, U.fog_color.rgb);
}

// Uniform sky-dome radiance gathered by GI bounce rays that escape the scene (the ambient/
// indirect fill). Decoupled from the visible background (`U.bg`) so a dark backdrop still
// lights the molecule; cavities self-shadow because their bounces hit geometry instead.
const GI_SKY: vec3<f32> = vec3<f32>(0.38, 0.38, 0.38);
// Diffuse bounces when GI is on (must match `GI_BOUNCES` in raytrace.rs).
const GI_BOUNCES: u32 = 3u;

// A decoded ray hit: world position, eye-facing normal, base colour, unpacked material
// (ambient, diffuse, specular, shininess) + the raw material word (for the outline bit).
struct Surf {
    p: vec3<f32>,
    ray_p: vec3<f32>, // smooth-surface origin for secondary rays; primary depth stays planar
    nrm: vec3<f32>,
    base: vec3<f32>,
    mat: vec4<f32>,
    mat_raw: u32,
    opacity: f32,
    mesh: bool,
    closed: bool,
    prim: u32,
};

fn surface_at(hit: Hit, ro: vec3<f32>, rd: vec3<f32>, persp: bool) -> Surf {
    let p = ro + rd * hit.t;
    let typ = hit.prim >> 30u;
    let idx = hit.prim & IDX_MASK;
    var nrm: vec3<f32>;
    var ray_p = p;
    var closed = false;
    var base: vec3<f32>;
    var mat_raw: u32;
    var opacity: f32;
    if (typ == 0u) {
        let sp = spheres[idx];
        nrm = normalize(p - sp.c.xyz);
        base = unpack_color(sp.m.x);
        mat_raw = sp.m.y;
        opacity = unpack_opacity(sp.m.x);
    } else if (typ == 1u) {
        let cy = cylinders[idx];
        let axis = cy.c1.xyz - cy.c0.xyz;
        let seg = length(axis);
        let ua = axis / max(seg, 1e-9);
        let h = dot(p - cy.c0.xyz, ua);
        // Clamped nearest axis point ⇒ the wall's radial normal *and* both caps' spherical ones.
        let ap = cy.c0.xyz + ua * clamp(h, 0.0, seg);
        nrm = normalize(p - ap);
        if (bond_profile_enabled(cy.profile)) {
            nrm = bond_profile_normal(p, cy.c0.xyz, ua, seg, cy.c0.w, cy.profile, cy.lane.w, cy.lane.xyz);
        }
        // Two-tone half-bond coloring, split at the midpoint exactly as the rasterizer does:
        // `m.x` is the p0 half, `m.z` the p1 half. Tracing only `m.x` painted every bond in its
        // first atom's color, so a C–F bond came out all grey and the fluorine vanished.
        let packed = select(cy.m.z, cy.m.x, h < seg * 0.5);
        base = unpack_color(packed);
        mat_raw = cy.m.y;
        opacity = unpack_opacity(packed);
        if (cy.c1.w > 0.0) {
            let t = clamp((h / max(seg, 1e-9) - 0.5) / cy.c1.w + 0.5, 0.0, 1.0);
            base = mix(unpack_color(cy.m.x), unpack_color(cy.m.z), t);
            opacity = mix(unpack_opacity(cy.m.x), unpack_opacity(cy.m.z), t);
        }
        // Screen-space line quads interpolate endpoint colors continuously.
        if ((cy.m.w & 1u) != 0u) {
            let t = clamp(h / max(seg, 1e-9), 0.0, 1.0);
            base = mix(unpack_color(cy.m.x), unpack_color(cy.m.z), t);
            opacity = mix(unpack_opacity(cy.m.x), unpack_opacity(cy.m.z), t);
        }
    } else {
        let tri = triangles[idx];
        closed = tri.w != 0u;
        let a = mesh_verts[tri.x];
        let b = mesh_verts[tri.y];
        let c = mesh_verts[tri.z];
        let wt = 1.0 - hit.uv.x - hit.uv.y;
        let face = normalize(cross(b.p.xyz - a.p.xyz, c.p.xyz - a.p.xyz));
        let interpolated = wt * a.n.xyz + hit.uv.x * b.n.xyz + hit.uv.y * c.n.xyz;
        nrm = face;
        if (dot(interpolated, interpolated) > 1e-12) { nrm = normalize(interpolated); }
        // The planar hit lies below the smooth surface's vertex tangent planes.
        // Project toward those planes before tracing, to avoid triangle-shaped
        // shadow terminators. Use the eye-facing side for open/two-sided ribbons.
        let side = select(-1.0, 1.0, dot(nrm, -rd) >= 0.0);
        let na = a.n.xyz * side;
        let nb = b.n.xyz * side;
        let nc = c.n.xyz * side;
        let smooth_n = nrm * side;
        let ha = max(0.0, -dot(p - a.p.xyz, na)) / max(dot(smooth_n, na), 0.25);
        let hb = max(0.0, -dot(p - b.p.xyz, nb)) / max(dot(smooth_n, nb), 0.25);
        let hc = max(0.0, -dot(p - c.p.xyz, nc)) / max(dot(smooth_n, nc), 0.25);
        // Clear all three tangent planes, not their weighted average: the average
        // can leave grazing rays below a neighboring face. Bound the correction
        // by triangle size so sharp ribbon corners cannot cause a large jump.
        let limit = 0.25 * min(length(b.p.xyz-a.p.xyz), min(length(c.p.xyz-b.p.xyz), length(a.p.xyz-c.p.xyz)));
        ray_p = p + smooth_n * min(max(ha, max(hb, hc)), limit);
        let ca = bitcast<u32>(a.p.w);
        let cb = bitcast<u32>(b.p.w);
        let cc = bitcast<u32>(c.p.w);
        base = wt * unpack_color(ca) + hit.uv.x * unpack_color(cb) + hit.uv.y * unpack_color(cc);
        opacity = wt * unpack_opacity(ca) + hit.uv.x * unpack_opacity(cb) + hit.uv.y * unpack_opacity(cc);
        mat_raw = bitcast<u32>(a.n.w);
    }
    let view_dir = select(-rd, normalize(U.eye.xyz - p), persp);
    if (dot(nrm, view_dir) < 0.0) { nrm = -nrm; } // two-sided
    return Surf(p, ray_p, nrm, base, unpack_mat(mat_raw), mat_raw, opacity, typ == 2u, closed, hit.prim);
}

// Sample a finite directional light. Moving the origin across shadow-map texels
// puts rays inside curved surfaces; varying direction keeps the origin on the surface
// and gives penumbrae that widen with blocker distance.
// Progressive, evenly distributed quadrature, shared across pixels. Independent
// random disks per pixel turn a smooth penumbra into visible grain at still-image
// sample budgets. The global sample index keeps tiled/progressive renders equal.
fn effect_sample(index: u32) -> vec2<f32> {
    return fract((f32(index) + 0.5) * vec2<f32>(0.754877666, 0.569840296));
}

fn shadow_at(s: Surf, light: vec3<f32>, sample: vec2<f32>) -> f32 {
    if (U.shadow.z <= 0.5) { return 1.0; }
    let lc = U.shadow_matrix * vec4<f32>(s.p, 1.0);
    let ndc = lc.xyz / lc.w;
    if (any(ndc < vec3<f32>(-1.0, -1.0, 0.0)) || any(ndc > vec3<f32>(1.0))) { return 1.0; }
    let angle = 6.2831853 * sample.x;
    let radius = sqrt(sample.y) * shadow_angular_radius(U.shadow.w);
    let dir = normalize(light + radius * (cos(angle) * normalize(U.shadow_u.xyz)
        + sin(angle) * normalize(U.shadow_v.xyz)));
    // On a closed smooth surface an inward ray is immediately blocked by the
    // surface itself. Decide this before offsetting: a numerical offset can make
    // grazing inward rays miss, producing a bias-dependent, jagged terminator.
    if (s.closed && dot(s.nrm, dir) <= 0.0) { return 1.0 - U.shadow.x; }
    // Mesh positions are planar while their normals describe a smooth surface.
    // Use the same world-space surface clearance as AO: a floating-point epsilon
    // alone starts grazing shadow rays inside neighboring facets and reveals their
    // triangles. Analytic primitives only need the numerical offset. The closed
    // hemisphere test above keeps the terminator independent of this clearance.
    let bias = max(U.shadow.y, 2e-6 * max(max(abs(s.p.x), abs(s.p.y)), abs(s.p.z)));
    let ro = s.p + s.nrm * select(bias, max(bias, U.ao.y), s.mesh);
    let origin_depth = (U.shadow_matrix * vec4<f32>(ro, 1.0)).z;
    let depth_step = (U.shadow_matrix * vec4<f32>(dir, 0.0)).z;
    let tmax = max(0.0, -origin_depth / depth_step);
    let skip_prim = select(0xffffffffu, s.prim, s.mesh);
    if (any_hit(ro, dir, tmax, dot(s.nrm, dir) > 0.0, skip_prim)) { return 1.0 - U.shadow.x; }
    return 1.0;
}

fn camera_hit(ro: vec3<f32>, rd: vec3<f32>, opaque_only: bool) -> Hit {
    var hit = closest_hit_filtered(ro, rd, opaque_only);
    if (hit.prim != 0xffffffffu) {
        let clip = U.proj * U.view * vec4<f32>(ro + rd * hit.t, 1.0);
        if (clip.z < 0.0 || clip.z > clip.w) { hit.prim = 0xffffffffu; }
    }
    return hit;
}

// True 3D ambient visibility: cosine-weighted rays leave the surface hemisphere
// and can hit cavities or blockers absent from the camera's depth buffer. Keep
// the user's radius in nm and strength linear; no scene scaling or contrast boost.
fn ambient_visibility(s: Surf, sample_index: u32) -> f32 {
    if (U.ao.w <= 0.5 || U.ao.z <= 0.0) { return 1.0; }
    let ro = s.ray_p + s.nrm * U.ao.y;
    let skip_prim = select(0xffffffffu, s.prim, s.mesh);
    var occ = 0.0;
    for (var i = 0u; i < AO_RAYS; i = i + 1u) {
        let sample = effect_sample(sample_index * AO_RAYS + i + 47u);
        let dir = cosine_hemisphere(s.nrm, sample.x, sample.y);
        if (any_hit(ro, dir, U.ao.x, true, skip_prim)) { occ = occ + 1.0; }
    }
    return clamp(1.0 - U.ao.z * occ / f32(AO_RAYS), 0.0, 1.0);
}

// Return unoccluded lighting plus the deferred darkening factor. Fog and color
// clamping precede AO/shadow multiplication in the live renderer.
fn shade_tier1(s: Surf, rd: vec3<f32>, persp: bool, light: vec3<f32>, sample_index: u32) -> vec4<f32> {
    let view_dir = select(-rd, normalize(U.eye.xyz - s.p), persp);
    let normal_view = normalize((U.view * vec4<f32>(s.nrm, 0.0)).xyz);
    let dir_view = normalize((U.view * vec4<f32>(view_dir, 0.0)).xyz);
    var shaded = shade_material(s.base, normal_view, dir_view, s.mat, s.mesh);
    shaded = apply_outline(shaded, s.nrm, view_dir, s.mat_raw);
    // Raster AO/shadows act on opaque geometry, before transparent compositing.
    var visibility = 1.0;
    if (s.opacity >= 0.999) {
        var shadow = 0.0;
        let count = select(SHADOW_RAYS, 1u, U.shadow.w <= 0.0 || U.shadow.z <= 0.5);
        for (var i = 0u; i < count; i = i + 1u) {
            shadow += shadow_at(s, light, effect_sample(sample_index * count + i));
        }
        visibility = (shadow / f32(count)) * ambient_visibility(s, sample_index);
    }
    return vec4<f32>(shaded, visibility);
}

// Tier-2 global illumination: a diffuse path tracer. At each hit: direct key light
// (soft-shadowed) + a cosine-weighted diffuse bounce that gathers the sky dome on a miss —
// so cavities self-shadow (true AO) and colour bleeds between surfaces. Russian-roulette
// terminated. Converges over the same progressive accumulation as tier-1 (just more samples).
// Returns the FULL GI shading; the caller blends it with tier-1 by the strength slider
// (`mix(tier1, gi, gi_str)`) so GI ramps in continuously from the tier-1 look (no abrupt
// model switch at strength → 0).
fn shade_gi(first: Surf, persp: bool, light: vec3<f32>, max_bounces: u32, seed: ptr<function, u32>) -> vec3<f32> {
    var radiance = vec3<f32>(0.0);
    var throughput = vec3<f32>(1.0);
    var s = first;
    var bounce = 0u;
    loop {
        let ndotl = max(dot(s.nrm, light), 0.0);
        let shadow_vis = shadow_at(s, light, vec2<f32>(rand(seed), rand(seed)));
        radiance = radiance + throughput * s.base * (s.mat.y * ndotl * shadow_vis);

        if (bounce >= max_bounces) { break; }
        if (bounce >= 2u) {
            let q = clamp(max(throughput.r, max(throughput.g, throughput.b)), 0.05, 1.0);
            if (rand(seed) > q) { break; }
            throughput = throughput / q;
        }
        // Cosine-weighted diffuse bounce (cos/pdf cancel → multiply by albedo).
        throughput = throughput * s.base;
        let ro = s.ray_p + s.nrm * max(U.ao.y, 1e-4);
        let rd = cosine_hemisphere(s.nrm, rand(seed), rand(seed));
        let hit = closest_hit(ro, rd);
        if (hit.prim == 0xffffffffu) {
            radiance = radiance + throughput * GI_SKY;
            break;
        }
        s = surface_at(hit, ro, rd, persp);
        bounce = bounce + 1u;
    }
    return radiance;
}

fn shade_surface(s: Surf, rd: vec3<f32>, persp: bool, light: vec3<f32>, axis: vec3<f32>, sample_index: u32, seed: ptr<function, u32>) -> vec3<f32> {
    let direct = shade_tier1(s, rd, persp, light, sample_index);
    var tier1 = apply_fog(direct.xyz, s.p, axis);
    if (s.opacity >= 0.999) {
        // Opaque color is stored in a normalized target before deferred effects.
        tier1 = clamp(tier1, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    tier1 = tier1 * direct.w;
    if (U.bg.w > 0.001) {
        let gi = shade_gi(s, persp, light, GI_BOUNCES, seed);
        return mix(tier1, apply_fog(aces(gi), s.p, axis), U.bg.w);
    }
    return tier1;
}

@compute @workgroup_size(8, 8, 1)
fn cs_trace(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = U.dims.x;
    let h = U.dims.y;
    // Pixel coord in the FULL image = tile origin (accum.zw) + local invocation id. The file
    // render sweeps the image in blocks over many short submits to dodge the GPU watchdog/TDR
    // on big scenes; the in-place path uses one full-image tile (origin 0,0), so it's unchanged.
    let gx = U.accum.z + gid.x;
    let gy = U.accum.w + gid.y;
    if (gx >= w || gy >= h) { return; }
    let pix = vec2<i32>(i32(gx), i32(gy));
    let samples = U.dims.z;
    let light = normalize(U.light_dir.xyz);
    let persp = U.eye.w > 0.5;
    let axis = view_axis(); // for the depth-cue distance; constant per pixel
    var color = vec3<f32>(0.0);

    for (var s = 0u; s < samples; s = s + 1u) {
        var seed = pcg(gx + gy * w + (s + U.dims.w) * 9781u + 0x9e3779b9u);
        let px = (f32(gx) + rand(&seed)) / f32(w);
        let py = (f32(gy) + rand(&seed)) / f32(h);
        let ndc = vec2<f32>(px * 2.0 - 1.0, 1.0 - py * 2.0);
        var ro: vec3<f32>;
        var rd: vec3<f32>;
        camera_ray(ndc, &ro, &rd);

        let backdrop = mix(U.bg_bottom.xyz, U.bg_top.xyz, 1.0 - py);
        let first = camera_hit(ro, rd, false);
        if (first.prim == 0xffffffffu) {
            color = color + backdrop;
            continue;
        }
        let first_surface = surface_at(first, ro, rd, persp);
        if (first_surface.opacity >= 0.999) {
            color = color + shade_surface(first_surface, rd, persp, light, axis, s + U.dims.w, &seed);
            continue;
        }
        // Match the live renderer's weighted OIT, rather than changing transparent
        // materials to a different stochastic transmission model on pressing R.
        let opaque = camera_hit(ro, rd, true);
        var opaque_color = backdrop;
        var opaque_distance = T_MAX;
        if (opaque.prim != 0xffffffffu) {
            let surface = surface_at(opaque, ro, rd, persp);
            opaque_color = shade_surface(surface, rd, persp, light, axis, s + U.dims.w, &seed);
            opaque_distance = opaque.t;
        }
        var accum_color = vec3<f32>(0.0);
        var accum_weight = 0.0;
        var reveal = 1.0;
        var cursor = ro;
        var hit = first;
        // Bound pathological transparent stacks, as the previous transmission walk
        // did. Normal molecular views terminate far before this limit.
        for (var layer = 0u; layer < 256u; layer = layer + 1u) {
            if (hit.prim == 0xffffffffu) { break; }
            let surface = surface_at(hit, cursor, rd, persp);
            if (dot(surface.p - ro, rd) >= opaque_distance || surface.opacity >= 0.999) { break; }
            let eye_z = (U.view * vec4<f32>(surface.p, 1.0)).z;
            let weight = transparency_weight(-eye_z, surface.opacity, U.depth_range.xy);
            let shaded = shade_surface(surface, rd, persp, light, axis, s + U.dims.w, &seed);
            accum_color = accum_color + shaded * surface.opacity * weight;
            accum_weight = accum_weight + surface.opacity * weight;
            reveal = reveal * (1.0 - surface.opacity);
            cursor = surface.p + rd * 1e-4;
            hit = camera_hit(cursor, rd, false);
        }
        let transparent = accum_color / max(accum_weight, 1e-5);
        color = color + transparent * (1.0 - reveal) + opaque_color * reveal;
    }

    // `color` holds this step's raw radiance sum over `samples` paths. Blend it into the
    // running average: avg' = (avg·prior + sum) / (prior + samples). `reset` starts fresh.
    let prior = U.accum.x;
    let new_total = prior + samples;
    var prev = vec3<f32>(0.0);
    if (U.accum.y == 0u) {
        prev = textureLoad(accum_prev, pix, 0).xyz;
    }
    let avg = (prev * f32(prior) + color) / f32(max(new_total, 1u));
    textureStore(accum, pix, vec4<f32>(avg, 1.0));
}

// ---- Resolve: fullscreen triangle, tonemap the accumulator into the sRGB target ----
// `U` (binding 0) is also bound here so the resolve knows whether GI is on (U.bg.w).
@group(0) @binding(8) var src: texture_2d<f32>;

// ACES filmic tonemap (Narkowicz fit) — compresses GI's HDR radiance into [0,1] with a
// filmic shoulder. Only used for the GI path; tier-1 stays a near-identity clamp so it keeps
// matching the rasterized view.
fn aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

@vertex
fn vs_resolve(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_resolve(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(src, vec2<i32>(frag.xy), 0).xyz;
    // Both renderers use the same color target; it performs any required sRGB encoding.
    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
