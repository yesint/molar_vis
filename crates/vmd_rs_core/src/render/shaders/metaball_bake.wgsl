// Metaball density bake (compute). One invocation per voxel; gathers the
// Gaussian contribution of every nearby atom into the volume:
//   density a = Σ wᵢ,   color rgb = Σ wᵢ·colorᵢ,   wᵢ = exp(-K·d²/Rᵢ²)
// (so the blended surface color is rgb/a). Pure gather — no atomics.

struct Atom {
    center_radius: vec4<f32>, // xyz = center (nm), w = kernel radius R
    color: vec4<f32>,         // rgb in 0..1
};

struct Uniform {
    origin: vec4<f32>, // xyz = volume min corner, w = voxel size (nm)
    dims: vec4<u32>,   // xyz = grid dims, w = atom count
    params: vec4<f32>, // x = isovalue, y = K, z = cutoff (×R), w = unused
};

@group(0) @binding(0) var<uniform> u: Uniform;
@group(0) @binding(1) var<storage, read> atoms: array<Atom>;
@group(0) @binding(2) var<storage, read_write> volume: array<vec4<f32>>;

@compute @workgroup_size(4, 4, 4)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= u.dims.x || gid.y >= u.dims.y || gid.z >= u.dims.z) {
        return;
    }
    let voxel = u.origin.w;
    let p = u.origin.xyz + (vec3<f32>(gid) + 0.5) * voxel;
    let k = u.params.y;
    let cutoff = u.params.z;

    var accum = vec4<f32>(0.0);
    let n = u.dims.w;
    for (var i = 0u; i < n; i = i + 1u) {
        let a = atoms[i];
        let r = a.center_radius.w;
        let dv = p - a.center_radius.xyz;
        let d2 = dot(dv, dv);
        let sup = cutoff * r;
        if (d2 < sup * sup) {
            let w = exp(-k * d2 / (r * r));
            accum = accum + vec4<f32>(a.color.rgb * w, w);
        }
    }

    let idx = (gid.z * u.dims.y + gid.y) * u.dims.x + gid.x;
    volume[idx] = accum;
}
