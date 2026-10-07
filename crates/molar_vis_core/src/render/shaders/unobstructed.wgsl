// Independent of the ray tracer: these are centre visibility queries, not
// surface intersections. Keep the leaf predicate aligned with unobstructed.rs.
struct Node {
    lo: vec4<f32>,
    hi: vec4<f32>,
    links: vec4<u32>,
};
struct Direction {
    u: vec4<f32>,
    v: vec4<f32>,
    d: vec4<f32>,
};
struct Corrections {
    count: atomic<u32>,
    entries: array<vec2<u32>>,
};
@group(0) @binding(0) var<storage, read> atoms: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read> nodes: array<Node>;
@group(0) @binding(2) var<storage, read> order: array<u32>;
@group(0) @binding(3) var<storage, read> directions: array<Direction>;
@group(0) @binding(4) var<storage, read_write> scores: array<atomic<u32>>;
@group(0) @binding(5) var<uniform> params: vec4<u32>;
@group(0) @binding(6) var<storage, read_write> corrections: Corrections;
var<workgroup> visible: array<u32, 64>;

// Broad phase only: a sphere whose projected centre covers the query and whose
// centre is in front must intersect the forward ray. A padded sphere AABB may
// admit extra candidates, which the exact leaf predicate rejects.
fn hits_bounds(node: Node, p: vec3<f32>, inverse: vec3<f32>, parallel: vec3<bool>) -> bool {
    let magnitude = max(max(abs(p.x), abs(p.y)), abs(p.z));
    let bound = max(abs(node.lo.xyz), abs(node.hi.xyz));
    let pad = 1e-5 * (1.0 + magnitude + max(max(bound.x, bound.y), bound.z));
    let lo = node.lo.xyz - vec3<f32>(pad);
    let hi = node.hi.xyz + vec3<f32>(pad);
    // Handle parallel axes explicitly: no reciprocal of zero or 0*infinity.
    if (parallel.x && (p.x < lo.x || p.x > hi.x))
        || (parallel.y && (p.y < lo.y || p.y > hi.y))
        || (parallel.z && (p.z < lo.z || p.z > hi.z)) {
        return false;
    }
    let a = (lo - p) * inverse;
    let b = (hi - p) * inverse;
    let near = select(min(a, b), vec3<f32>(-3.402823e38), parallel);
    let far = select(max(a, b), vec3<f32>(3.402823e38), parallel);
    let enter = max(max(near.x, near.y), max(near.z, 0.0));
    let exit = min(min(far.x, far.y), far.z);
    return enter <= exit;
}

// 0 = definitely hidden, 1 = definitely visible, 2 = resolve on the CPU.
fn visibility(id: u32, dir: Direction) -> u32 {
    let p = atoms[id].xyz;
    let u = dot(p, dir.u.xyz);
    let v = dot(p, dir.v.xyz);
    let depth = dot(p, dir.d.xyz);
    let parallel = abs(dir.d.xyz) < vec3<f32>(1e-20);
    let inverse = 1.0 / select(dir.d.xyz, vec3<f32>(1.0), parallel);
    let magnitude = max(max(abs(p.x), abs(p.y)), abs(p.z));
    var ambiguous = false;
    var node_id = 0u;
    while node_id < params.y {
        let node = nodes[node_id];
        if !hits_bounds(node, p, inverse, parallel) {
            node_id = node.links.z;
            continue;
        }
        if node.links.y == 0u {
            node_id = node_id + 1u;
            continue;
        }
        for (var j = node.links.x; j < node.links.x + node.links.y; j = j + 1u) {
            let q = atoms[order[j]];
            // Identical positions cannot block one another under the centre rule.
            if all(q.xyz == p) || q.w <= 0.0 { continue; }
            let qm = max(max(abs(q.x), abs(q.y)), abs(q.z));
            // Conservative error bounds cover CPU/GPU dot-product ordering and
            // fused multiply-add differences near depth and radius boundaries.
            let error = 2e-6 * (1.0 + magnitude + qm);
            let delta = dot(q.xyz, dir.d.xyz) - (depth + 1e-4);
            if delta <= -error { continue; }
            let du = u - dot(q.xyz, dir.u.xyz);
            let dv = v - dot(q.xyz, dir.v.xyz);
            let distance = du * du + dv * dv;
            let radius = q.w * q.w;
            let radial_error = (2.0 * abs(du) + error) * error
                + (2.0 * abs(dv) + error) * error + 5e-7 * (radius + distance);
            if distance >= radius + radial_error { continue; }
            if delta > error && distance < radius - radial_error {
                return 0u;
            }
            ambiguous = true;
        }
        node_id = node.links.z;
    }
    return select(1u, 2u, ambiguous);
}

@compute @workgroup_size(64)
fn score(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    var count = 0u;
    if gid.x < params.x {
        let result = visibility(gid.x, directions[gid.y]);
        count = select(0u, 1u, result != 0u);
        if result == 2u {
            let slot = atomicAdd(&corrections.count, 1u);
            if slot < arrayLength(&corrections.entries) {
                corrections.entries[slot] = vec2<u32>(gid.y, gid.x);
            }
        }
    }
    visible[lane] = count;
    workgroupBarrier();
    for (var stride = 32u; stride > 0u; stride = stride / 2u) {
        if lane < stride {
            visible[lane] = visible[lane] + visible[lane + stride];
        }
        workgroupBarrier();
    }
    if lane == 0u {
        atomicAdd(&scores[gid.y], visible[0]);
    }
}
