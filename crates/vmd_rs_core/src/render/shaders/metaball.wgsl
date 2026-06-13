// Metaball isosurface, ray-marched against the baked density volume. Full-screen
// triangle; each fragment reconstructs its world ray, intersects the volume AABB,
// marches the trilinearly-sampled density to the isovalue, shades with the field
// gradient + blended color, and writes analytic frag_depth so it composites with
// the impostors/cartoon mesh in the shared depth buffer.

struct Camera {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    params: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
};

struct Uniform {
    origin: vec4<f32>, // xyz = volume min corner, w = voxel size
    dims: vec4<u32>,   // xyz = grid dims, w = atom count
    params: vec4<f32>, // x = isovalue, y = K, z = cutoff, w = unused
};

@group(0) @binding(0) var<uniform> cam: Camera;
@group(1) @binding(0) var<uniform> u: Uniform;
@group(1) @binding(1) var<storage, read> volume: array<vec4<f32>>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var corner = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: VsOut;
    out.clip = vec4<f32>(corner[vi], 0.0, 1.0);
    out.ndc = corner[vi];
    return out;
}

struct FsOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

// Trilinear sample of the volume (rgb = weighted color, a = density).
fn sample_vol(p: vec3<f32>) -> vec4<f32> {
    let f = (p - u.origin.xyz) / u.origin.w;
    let dims = vec3<i32>(u.dims.xyz);
    let base = vec3<i32>(floor(f));
    let fr = f - floor(f);
    var acc = vec4<f32>(0.0);
    for (var c = 0; c < 8; c = c + 1) {
        let off = vec3<i32>(c & 1, (c >> 1) & 1, (c >> 2) & 1);
        let g = base + off;
        var s = vec4<f32>(0.0);
        if (all(g >= vec3<i32>(0)) && all(g < dims)) {
            let idx = (u32(g.z) * u.dims.y + u32(g.y)) * u.dims.x + u32(g.x);
            s = volume[idx];
        }
        let wx = select(1.0 - fr.x, fr.x, off.x == 1);
        let wy = select(1.0 - fr.y, fr.y, off.y == 1);
        let wz = select(1.0 - fr.z, fr.z, off.z == 1);
        acc = acc + s * (wx * wy * wz);
    }
    return acc;
}

fn density(p: vec3<f32>) -> f32 {
    return sample_vol(p).a;
}

@fragment
fn fs_main(in: VsOut) -> FsOut {
    // Reconstruct the world-space ray (works for both perspective and ortho).
    let near4 = cam.inv_view_proj * vec4<f32>(in.ndc, 0.0, 1.0);
    let far4 = cam.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let ro = near4.xyz / near4.w;
    let dir = normalize(far4.xyz / far4.w - ro);

    // Slab intersection against the volume AABB.
    let bmin = u.origin.xyz;
    let bmax = u.origin.xyz + vec3<f32>(u.dims.xyz) * u.origin.w;
    let inv = 1.0 / dir;
    let ta = (bmin - ro) * inv;
    let tb = (bmax - ro) * inv;
    let tlo = max(max(min(ta.x, tb.x), min(ta.y, tb.y)), min(ta.z, tb.z));
    let thi = min(min(max(ta.x, tb.x), max(ta.y, tb.y)), max(ta.z, tb.z));
    if (thi < max(tlo, 0.0)) {
        discard;
    }

    let iso = u.params.x;
    let step = u.origin.w * 0.5;
    var t = max(tlo, 0.0);
    var prev = density(ro + dir * t) - iso;
    var hit = -1.0;
    loop {
        t = t + step;
        if (t > thi) {
            break;
        }
        let cur = density(ro + dir * t) - iso;
        if (prev < 0.0 && cur >= 0.0) {
            var a = t - step;
            var b = t;
            for (var k = 0; k < 5; k = k + 1) {
                let m = 0.5 * (a + b);
                if (density(ro + dir * m) - iso < 0.0) {
                    a = m;
                } else {
                    b = m;
                }
            }
            hit = b;
            break;
        }
        prev = cur;
    }
    if (hit < 0.0) {
        discard;
    }

    let pos = ro + dir * hit;
    let s = sample_vol(pos);
    let col = s.rgb / max(s.a, 1e-4);

    // Surface normal = direction of decreasing density (outward).
    let h = u.origin.w;
    let grad = vec3<f32>(
        density(pos + vec3<f32>(h, 0.0, 0.0)) - density(pos - vec3<f32>(h, 0.0, 0.0)),
        density(pos + vec3<f32>(0.0, h, 0.0)) - density(pos - vec3<f32>(0.0, h, 0.0)),
        density(pos + vec3<f32>(0.0, 0.0, h)) - density(pos - vec3<f32>(0.0, 0.0, h)),
    );
    let n = normalize(-grad);

    let diff = abs(dot(n, -dir)); // headlight along the view ray
    let ambient = 0.35;
    let shade = ambient + (1.0 - ambient) * diff;

    let clip = cam.proj * (cam.view * vec4<f32>(pos, 1.0));
    var out: FsOut;
    out.color = vec4<f32>(col * shade, 1.0);
    out.depth = clip.z / clip.w;
    return out;
}
