// Fragment-only point containment, scoped to this representation and periodic image.
struct EnvelopePrimitive {
    a: vec4<f32>,
    b: vec4<f32>,
    lane: vec4<f32>,
};
struct EnvelopeNode {
    lo: vec3<f32>,
    primitive: u32,
    hi: vec3<f32>,
    escape: u32,
};
@group(1) @binding(0) var<storage, read> envelope_nodes: array<EnvelopeNode>;
@group(1) @binding(1) var<storage, read> envelope_primitives: array<EnvelopePrimitive>;
@group(1) @binding(2) var<uniform> envelope_config: vec4<u32>;

fn envelope_hidden(p: vec3<f32>, instance: u32, capsule: bool) -> bool {
    if (envelope_config.x == 0u) { return false; }
    let own = instance + select(0u, envelope_config.y, capsule);
    // Inverse rigid camera transform; this includes a periodic image's translation.
    let rotation = mat3x3<f32>(camera.view[0].xyz, camera.view[1].xyz, camera.view[2].xyz);
    let world = transpose(rotation) * (p - camera.view[3].xyz);
    var node = 0u;
    loop {
        if (node >= envelope_config.x) { break; }
        let n = envelope_nodes[node];
        if (any(world < n.lo) || any(world > n.hi)) {
            node = n.escape;
            continue;
        }
        node = node + 1u;
        if (n.primitive == 0xffffffffu || n.primitive == own) { continue; }
        let primitive = envelope_primitives[n.primitive];
        var a = (camera.view * vec4<f32>(primitive.a.xyz, 1.0)).xyz;
        var b = (camera.view * vec4<f32>(primitive.b.xyz, 1.0)).xyz;
        if (primitive.lane.x != 0.0) {
            let axis = b - a;
            let length_axis = length(axis);
            let direction = select(vec3<f32>(1.0, 0.0, 0.0), axis / max(length_axis, 1e-8), length_axis > 1e-8);
            let side = cross(direction, vec3<f32>(0.0, 0.0, 1.0));
            let length_side = length(side);
            let perpendicular = select(vec3<f32>(1.0, 0.0, 0.0), side / max(length_side, 1e-8), length_side > 1e-4);
            let shift = perpendicular * primitive.lane.x * primitive.lane.y;
            a = a + shift; b = b + shift;
        }
        let ab = b - a;
        let along = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-16), 0.0, 1.0);
        let nearest = a + ab * along;
        let delta = p - nearest;
        let r2 = primitive.a.w * primitive.a.w;
        let d2 = dot(delta, delta);
        let tolerance = max(1e-10, r2 * 2e-4);
        // Strictly internal faces vanish. Coincident caps get a deterministic owner.
        if (d2 < r2 - tolerance || (abs(d2 - r2) <= tolerance && n.primitive < own)) {
            return true;
        }
    }
    return false;
}
