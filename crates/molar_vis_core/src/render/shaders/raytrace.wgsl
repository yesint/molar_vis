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
    eye: vec4<f32>,             // xyz eye world, w = perspective flag
    light_dir: vec4<f32>,       // xyz world dir toward the key light
    ao: vec4<f32>,              // radius (nm), bias, strength, enabled
    shadow: vec4<f32>,          // strength, bias, enabled, _
    bg: vec4<f32>,              // background (linear)
    dims: vec4<u32>,            // width, height, samples-this-step, frame_seed
    accum: vec4<u32>,           // prior_total_samples, reset(0/1), _, _
};

@group(0) @binding(0) var<uniform> U: RtUniform;
@group(0) @binding(1) var<storage, read> spheres: array<Sphere>;
@group(0) @binding(2) var<storage, read> cylinders: array<Cyl>;
@group(0) @binding(3) var<storage, read> mesh_verts: array<MeshVert>;
@group(0) @binding(4) var<storage, read> triangles: array<vec4<u32>>; // (i0,i1,i2,_)
@group(0) @binding(5) var<storage, read> nodes: array<BvhNode>;
@group(0) @binding(6) var<storage, read> prim_indices: array<u32>;    // (type<<30)|index
@group(0) @binding(7) var accum: texture_storage_2d<rgba32float, write>;
@group(0) @binding(9) var accum_prev: texture_2d<f32>; // running average to extend (ping-pong)

const PI: f32 = 3.14159265359;
const T_MAX: f32 = 1e30;
const IDX_MASK: u32 = 0x3fffffffu;

// Lighting model (tier 1, Tachyon/PyMOL-`ray`-like): a directional key light (occluded by
// a soft shadow) plus a hemispheric ambient fill that ambient occlusion occludes. The
// ambient FLOOR keeps deep crevices / shadowed faces from going black (softens shadows);
// the FILL is what AO darkens (so AO is clearly visible, not lost in a tiny ambient term).
const KEY_INTENSITY: f32 = 0.8;
const AMBIENT_FILL: f32 = 0.45;
const AMBIENT_FLOOR: f32 = 0.1;
// Shadow-ray cone half-angle (radians) at softness = 1; the per-sample jitter over this
// cone gives a soft penumbra. The actual cone = `shadow.softness` (0..1) × this. Wide
// enough (~26°) that the Softness slider has clearly visible range — softness 0 is a
// razor-hard edge, softness 1 a broad diffuse penumbra (the user found 0.15 rad too
// subtle to tell apart, and the hard shadows "too harsh").
const MAX_SHADOW_CONE: f32 = 0.45;
// AO rays per sample (a per-sample occlusion fraction, so the contrast curve below can be
// applied locally) and the contrast exponent. cos-weighted hemisphere AO reads physically
// "correct" but light — most surface points see only ~10–20 % occlusion, so they barely
// darken, whereas screen-space AO over-darkens the whole bumpy surface. `pow(frac, <1)`
// boosts that modest occlusion into the strong edge/contact darkening SSAO shows (e.g. 0.2
// → 0.38 at 0.6), making the ray-traced AO visually comparable to the realtime SSAO.
const AO_RAYS: u32 = 4u;
const AO_CONTRAST: f32 = 0.55;

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
// (ambient, diffuse, specular, shininess); top bit of shininess byte = outline (ignored).
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

fn ray_sphere(s: Sphere, ro: vec3<f32>, rd: vec3<f32>) -> f32 {
    let oc = ro - s.c.xyz;
    let b = dot(oc, rd);
    let c = dot(oc, oc) - s.c.w * s.c.w;
    let disc = b * b - c;
    if (disc < 0.0) { return -1.0; }
    let t = -b - sqrt(disc);
    if (t > 1e-4) { return t; }
    return -1.0;
}

// Capless finite cylinder (joints are covered by separate sphere prims).
fn ray_cylinder(c: Cyl, ro: vec3<f32>, rd: vec3<f32>) -> f32 {
    let p0 = c.c0.xyz;
    let r = c.c0.w;
    let axis = c.c1.xyz - p0;
    let seg = length(axis);
    if (seg < 1e-9) { return -1.0; }
    let ua = axis / seg;
    let oc = ro - p0;
    let rd_p = rd - ua * dot(rd, ua);
    let oc_p = oc - ua * dot(oc, ua);
    let a = dot(rd_p, rd_p);
    let b = 2.0 * dot(rd_p, oc_p);
    let cc = dot(oc_p, oc_p) - r * r;
    let disc = b * b - 4.0 * a * cc;
    if (disc < 0.0 || a < 1e-12) { return -1.0; }
    let t = (-b - sqrt(disc)) / (2.0 * a);
    if (t <= 1e-4) { return -1.0; }
    let h = dot(ro + t * rd - p0, ua);
    if (h < 0.0 || h > seg) { return -1.0; }
    return t;
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

struct Hit { t: f32, prim: u32, uv: vec2<f32> };

fn closest_hit(ro: vec3<f32>, rd: vec3<f32>) -> Hit {
    var hit: Hit;
    hit.t = T_MAX;
    hit.prim = 0xffffffffu;
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
            stack[sp] = link; sp = sp + 1;
            stack[sp] = link + 1u; sp = sp + 1;
        } else {
            for (var k = 0u; k < count; k = k + 1u) {
                let tagged = prim_indices[link + k];
                let typ = tagged >> 30u;
                let idx = tagged & IDX_MASK;
                if (typ == 0u) {
                    let t = ray_sphere(spheres[idx], ro, rd);
                    if (t > 0.0 && t < hit.t) { hit.t = t; hit.prim = tagged; }
                } else if (typ == 1u) {
                    let t = ray_cylinder(cylinders[idx], ro, rd);
                    if (t > 0.0 && t < hit.t) { hit.t = t; hit.prim = tagged; }
                } else {
                    let tri = triangles[idx];
                    let r = ray_triangle(mesh_verts[tri.x].p.xyz, mesh_verts[tri.y].p.xyz, mesh_verts[tri.z].p.xyz, ro, rd);
                    if (r.x > 0.0 && r.x < hit.t) { hit.t = r.x; hit.prim = tagged; hit.uv = r.yz; }
                }
            }
        }
    }
    return hit;
}

fn any_hit(ro: vec3<f32>, rd: vec3<f32>, tmax: f32) -> bool {
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
                let typ = tagged >> 30u;
                let idx = tagged & IDX_MASK;
                var t = -1.0;
                if (typ == 0u) {
                    t = ray_sphere(spheres[idx], ro, rd);
                } else if (typ == 1u) {
                    t = ray_cylinder(cylinders[idx], ro, rd);
                } else {
                    let tri = triangles[idx];
                    t = ray_triangle(mesh_verts[tri.x].p.xyz, mesh_verts[tri.y].p.xyz, mesh_verts[tri.z].p.xyz, ro, rd).x;
                }
                if (t > 1e-4 && t < tmax) { return true; }
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

// Perturb `dir` within a small cone of half-angle ~`softness` (a soft area light).
fn jitter_cone(dir: vec3<f32>, softness: f32, u1: f32, u2: f32) -> vec3<f32> {
    var t: vec3<f32>;
    var b: vec3<f32>;
    onb(dir, &t, &b);
    let r = softness * sqrt(u1);
    let phi = 2.0 * PI * u2;
    return normalize(dir + r * (cos(phi) * t + sin(phi) * b));
}

@compute @workgroup_size(8, 8, 1)
fn cs_trace(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = U.dims.x;
    let h = U.dims.y;
    if (gid.x >= w || gid.y >= h) { return; }
    let samples = U.dims.z;
    let light = normalize(U.light_dir.xyz);
    let persp = U.eye.w > 0.5;
    var color = vec3<f32>(0.0);

    for (var s = 0u; s < samples; s = s + 1u) {
        var seed = pcg(gid.x + gid.y * w + (s + U.dims.w) * 9781u + 0x9e3779b9u);
        let px = (f32(gid.x) + rand(&seed)) / f32(w);
        let py = (f32(gid.y) + rand(&seed)) / f32(h);
        let ndc = vec2<f32>(px * 2.0 - 1.0, 1.0 - py * 2.0);
        var ro: vec3<f32>;
        var rd: vec3<f32>;
        camera_ray(ndc, &ro, &rd);

        let hit = closest_hit(ro, rd);
        if (hit.prim == 0xffffffffu) {
            color = color + U.bg.xyz;
            continue;
        }
        let p = ro + rd * hit.t;
        let typ = hit.prim >> 30u;
        let idx = hit.prim & IDX_MASK;

        var nrm: vec3<f32>;
        var base: vec3<f32>;
        var mat: vec4<f32>;
        if (typ == 0u) {
            let sp = spheres[idx];
            nrm = normalize(p - sp.c.xyz);
            base = unpack_color(sp.m.x);
            mat = unpack_mat(sp.m.y);
        } else if (typ == 1u) {
            let cy = cylinders[idx];
            let axis = cy.c1.xyz - cy.c0.xyz;
            let seg = length(axis);
            let ua = axis / max(seg, 1e-9);
            let ap = cy.c0.xyz + ua * clamp(dot(p - cy.c0.xyz, ua), 0.0, seg);
            nrm = normalize(p - ap);
            base = unpack_color(cy.m.x);
            mat = unpack_mat(cy.m.y);
        } else {
            let tri = triangles[idx];
            let a = mesh_verts[tri.x];
            let b = mesh_verts[tri.y];
            let c = mesh_verts[tri.z];
            let wt = 1.0 - hit.uv.x - hit.uv.y;
            nrm = normalize(wt * a.n.xyz + hit.uv.x * b.n.xyz + hit.uv.y * c.n.xyz);
            base = wt * unpack_color(bitcast<u32>(a.p.w))
                 + hit.uv.x * unpack_color(bitcast<u32>(b.p.w))
                 + hit.uv.y * unpack_color(bitcast<u32>(c.p.w));
            mat = unpack_mat(bitcast<u32>(a.n.w));
        }

        let view_dir = select(-rd, normalize(U.eye.xyz - p), persp);
        if (dot(nrm, view_dir) < 0.0) { nrm = -nrm; } // two-sided

        // Directional key light with a SOFT shadow: jitter the shadow ray within a small
        // cone per sample, so the penumbra softens as samples accumulate (vs a razor-hard
        // single-ray shadow).
        let ndotl = max(dot(nrm, light), 0.0);
        var shadow_vis = 1.0;
        if (U.shadow.z > 0.5 && ndotl > 0.0) {
            let ldir = jitter_cone(light, U.shadow.w * MAX_SHADOW_CONE, rand(&seed), rand(&seed));
            if (any_hit(p + nrm * U.shadow.y, ldir, T_MAX)) { shadow_vis = 1.0 - U.shadow.x; }
        }
        let hv = normalize(light + view_dir);
        let spec = mat.z * pow(max(dot(nrm, hv), 0.0), 2.0 + mat.w * 128.0);

        // Ambient occlusion: AO_RAYS cosine-weighted hemisphere rays per sample → a local
        // occlusion fraction, contrast-boosted (pow) then scaled by strength, so it reads
        // as strongly as SSAO instead of a faint physically-correct estimate.
        var ao_vis = 1.0;
        if (U.ao.w > 0.5) {
            var occ = 0.0;
            for (var i = 0u; i < AO_RAYS; i = i + 1u) {
                let dir = cosine_hemisphere(nrm, rand(&seed), rand(&seed));
                if (any_hit(p + nrm * U.ao.y, dir, U.ao.x)) { occ = occ + 1.0; }
            }
            let frac = pow(occ / f32(AO_RAYS), AO_CONTRAST);
            ao_vis = 1.0 - U.ao.z * frac;
        }

        // Shade normally (ambient fill + floor, key light, specular), then let AO multiply
        // the WHOLE shaded color — exactly what the screen-space-AO pass does (it multiplies
        // the rasterized color toward black). `ao_vis` is binary per sample (hit →
        // 1-strength, miss → 1) and averages over samples to (1 - strength·occluded_frac),
        // so a deep contact reaches ~(1-strength) ≈ 5% brightness, matching SSAO's strong
        // edge/contact darkening (occluding only the ambient term read far too light).
        let ambient = AMBIENT_FLOOR + AMBIENT_FILL;
        let key = KEY_INTENSITY * ndotl * shadow_vis;
        let shaded = base * (ambient + key) + vec3<f32>(spec) * shadow_vis;
        color = color + shaded * ao_vis;
    }

    // `color` holds this step's raw radiance sum over `samples` paths. Blend it into the
    // running average: avg' = (avg·prior + sum) / (prior + samples). `reset` starts fresh.
    let prior = U.accum.x;
    let new_total = prior + samples;
    var prev = vec3<f32>(0.0);
    if (U.accum.y == 0u) {
        prev = textureLoad(accum_prev, vec2<i32>(gid.xy), 0).xyz;
    }
    let avg = (prev * f32(prior) + color) / f32(max(new_total, 1u));
    textureStore(accum, vec2<i32>(gid.xy), vec4<f32>(avg, 1.0));
}

// ---- Resolve: fullscreen triangle, tonemap the accumulator into the sRGB target ----
@group(0) @binding(8) var src: texture_2d<f32>;

@vertex
fn vs_resolve(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_resolve(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(src, vec2<i32>(frag.xy), 0).xyz;
    // Tier-1 near-identity tonemap (clamp). Target is sRGB → GPU encodes on store; no gamma here.
    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
