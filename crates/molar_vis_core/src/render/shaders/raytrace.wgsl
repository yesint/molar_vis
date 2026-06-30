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
    light_dir: vec4<f32>,       // xyz world dir toward the key light (shadow ray)
    head_dir: vec4<f32>,        // xyz world dir toward the shading headlight
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

// Lighting model (tier 1, Tachyon/PyMOL-`ray`-like): Blinn-Phong from a view-space
// headlight (using each material's own ambient/diffuse coefficients, so it matches the
// rasterizer's `shade_material` when AO/shadows are off) × a soft cast shadow from a
// separate key light × ambient occlusion — both applied as deferred whole-color multiplies,
// exactly as the realtime SSAO/shadow pass does.
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

// Uniform sky-dome radiance gathered by GI bounce rays that escape the scene (the ambient/
// indirect fill). Decoupled from the visible background (`U.bg`) so a dark backdrop still
// lights the molecule; cavities self-shadow because their bounces hit geometry instead.
const GI_SKY: vec3<f32> = vec3<f32>(0.38, 0.38, 0.38);

// A decoded ray hit: world position, eye-facing normal, base colour, unpacked material
// (ambient, diffuse, specular, shininess) + the raw material word (for the outline bit).
struct Surf {
    p: vec3<f32>,
    nrm: vec3<f32>,
    base: vec3<f32>,
    mat: vec4<f32>,
    mat_raw: u32,
};

fn surface_at(hit: Hit, ro: vec3<f32>, rd: vec3<f32>, persp: bool) -> Surf {
    let p = ro + rd * hit.t;
    let typ = hit.prim >> 30u;
    let idx = hit.prim & IDX_MASK;
    var nrm: vec3<f32>;
    var base: vec3<f32>;
    var mat_raw: u32;
    if (typ == 0u) {
        let sp = spheres[idx];
        nrm = normalize(p - sp.c.xyz);
        base = unpack_color(sp.m.x);
        mat_raw = sp.m.y;
    } else if (typ == 1u) {
        let cy = cylinders[idx];
        let axis = cy.c1.xyz - cy.c0.xyz;
        let seg = length(axis);
        let ua = axis / max(seg, 1e-9);
        let ap = cy.c0.xyz + ua * clamp(dot(p - cy.c0.xyz, ua), 0.0, seg);
        nrm = normalize(p - ap);
        base = unpack_color(cy.m.x);
        mat_raw = cy.m.y;
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
        mat_raw = bitcast<u32>(a.n.w);
    }
    let view_dir = select(-rd, normalize(U.eye.xyz - p), persp);
    if (dot(nrm, view_dir) < 0.0) { nrm = -nrm; } // two-sided
    return Surf(p, nrm, base, unpack_mat(mat_raw), mat_raw);
}

// Soft cast shadow toward the KEY light (cone-jittered → penumbra over samples). Returns
// visibility in [1-strength, 1]; back faces shadow without a ray.
fn shadow_at(s: Surf, light: vec3<f32>, seed: ptr<function, u32>) -> f32 {
    if (U.shadow.z <= 0.5) { return 1.0; }
    if (dot(s.nrm, light) <= 0.0) { return 1.0 - U.shadow.x; }
    let ldir = jitter_cone(light, U.shadow.w * MAX_SHADOW_CONE, rand(seed), rand(seed));
    if (any_hit(s.p + s.nrm * U.shadow.y, ldir, T_MAX)) { return 1.0 - U.shadow.x; }
    return 1.0;
}

// Tier-1 shading: Blinn-Phong from the view-space headlight (matches the rasterizer) ×
// soft shadow × contrast-boosted hemisphere AO, both as deferred whole-color multiplies.
fn shade_tier1(s: Surf, rd: vec3<f32>, persp: bool, light: vec3<f32>, seed: ptr<function, u32>) -> vec3<f32> {
    let view_dir = select(-rd, normalize(U.eye.xyz - s.p), persp);
    let head = U.head_dir.xyz;
    let ndotl = max(dot(s.nrm, head), 0.0);
    let hv = normalize(head + view_dir);
    let spec = s.mat.z * pow(max(dot(s.nrm, hv), 0.0), 2.0 + s.mat.w * 128.0);
    var shaded = s.base * (s.mat.x + s.mat.y * ndotl) + vec3<f32>(spec);
    if (((s.mat_raw >> 31u) & 1u) == 1u) {
        let edge = pow(1.0 - abs(dot(s.nrm, view_dir)), 2.0);
        shaded = shaded * (1.0 - 0.9 * edge);
    }
    let shadow_vis = shadow_at(s, light, seed);
    var ao_vis = 1.0;
    if (U.ao.w > 0.5) {
        var occ = 0.0;
        for (var i = 0u; i < AO_RAYS; i = i + 1u) {
            let dir = cosine_hemisphere(s.nrm, rand(seed), rand(seed));
            if (any_hit(s.p + s.nrm * U.ao.y, dir, U.ao.x)) { occ = occ + 1.0; }
        }
        let frac = pow(occ / f32(AO_RAYS), AO_CONTRAST);
        ao_vis = 1.0 - U.ao.z * frac;
    }
    return shaded * shadow_vis * ao_vis;
}

// Tier-2 global illumination: a diffuse path tracer. At each hit: direct key light
// (soft-shadowed) + a cosine-weighted diffuse bounce that gathers the sky dome on a miss —
// so cavities self-shadow (true AO) and colour bleeds between surfaces. Russian-roulette
// terminated. Converges over the same progressive accumulation as tier-1 (just more samples).
fn shade_gi(first: Surf, persp: bool, light: vec3<f32>, max_bounces: u32, seed: ptr<function, u32>) -> vec3<f32> {
    var radiance = vec3<f32>(0.0);
    var throughput = vec3<f32>(1.0);
    var s = first;
    var bounce = 0u;
    loop {
        let ndotl = max(dot(s.nrm, light), 0.0);
        let shadow_vis = shadow_at(s, light, seed);
        radiance = radiance + throughput * s.base * (s.mat.y * ndotl * shadow_vis);

        if (bounce >= max_bounces) { break; }
        if (bounce >= 2u) {
            let q = clamp(max(throughput.r, max(throughput.g, throughput.b)), 0.05, 1.0);
            if (rand(seed) > q) { break; }
            throughput = throughput / q;
        }
        // Cosine-weighted diffuse bounce (cos/pdf cancel → multiply by albedo).
        throughput = throughput * s.base;
        let ro = s.p + s.nrm * max(U.ao.y, 1e-4);
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
    var color = vec3<f32>(0.0);

    for (var s = 0u; s < samples; s = s + 1u) {
        var seed = pcg(gx + gy * w + (s + U.dims.w) * 9781u + 0x9e3779b9u);
        let px = (f32(gx) + rand(&seed)) / f32(w);
        let py = (f32(gy) + rand(&seed)) / f32(h);
        let ndc = vec2<f32>(px * 2.0 - 1.0, 1.0 - py * 2.0);
        var ro: vec3<f32>;
        var rd: vec3<f32>;
        camera_ray(ndc, &ro, &rd);

        let hit = closest_hit(ro, rd);
        if (hit.prim == 0xffffffffu) {
            color = color + U.bg.xyz;
            continue;
        }
        let s = surface_at(hit, ro, rd, persp);
        // GI bounce count rides U.bg.w (0 = tier-1 direct shading, matching the realtime
        // view; >0 = tier-2 path-traced global illumination — Save-image only).
        let gi_bounces = u32(U.bg.w + 0.5);
        if (gi_bounces > 0u) {
            color = color + shade_gi(s, persp, light, gi_bounces, &seed);
        } else {
            color = color + shade_tier1(s, rd, persp, light, &seed);
        }
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
    // Target is sRGB → GPU encodes on store; no manual gamma here. GI (U.bg.w > 0) tonemaps
    // its HDR radiance (ACES); tier-1 is a near-identity clamp so it matches the raster view.
    if (U.bg.w > 0.5) {
        return vec4<f32>(aces(c), 1.0);
    }
    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
