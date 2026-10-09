// Test-only baseline traversal, before near-first child ordering.
fn closest_hit_filtered(ro: vec3<f32>, rd: vec3<f32>, opaque_only: bool) -> Hit {
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
                    let t = ray_sphere(spheres[idx], ro, rd, false);
                    if (t > 0.0 && t < hit.t && (!opaque_only || primitive_opacity(tagged, ro + rd * t, vec2<f32>(0.0)) >= 0.999) && !inside_envelope(ro + rd * t, tagged)) { hit.t = t; hit.prim = tagged; }
                } else if (typ == 1u) {
                    let t = ray_cylinder(cylinders[idx], ro, rd, false);
                    if (t > 0.0 && t < hit.t && (!opaque_only || primitive_opacity(tagged, ro + rd * t, vec2<f32>(0.0)) >= 0.999) && !inside_envelope(ro + rd * t, tagged)) { hit.t = t; hit.prim = tagged; }
                } else {
                    let tri = triangles[idx];
                    let r = ray_triangle(mesh_verts[tri.x].p.xyz, mesh_verts[tri.y].p.xyz, mesh_verts[tri.z].p.xyz, ro, rd);
                    if (r.x > 0.0 && r.x < hit.t && (!opaque_only || primitive_opacity(tagged, ro + rd * r.x, r.yz) >= 0.999)) { hit.t = r.x; hit.prim = tagged; hit.uv = r.yz; }
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
