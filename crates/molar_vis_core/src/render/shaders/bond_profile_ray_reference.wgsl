// Test-only solver before per-ray coefficient caching.
fn bond_profile_ray_reference(base: vec3<f32>, axis: vec3<f32>, length_axis: f32, neck: f32,
    profile: vec4<f32>, smoothing: f32, shift: vec3<f32>, ro: vec3<f32>, rd: vec3<f32>, exit_inside: bool) -> vec4<f32> {
    let offset = dot(shift, shift) > 1e-16;
    let radii = bond_profile_atom_radii(length_axis, profile);
    let extent = vec3<f32>(max(max(neck, radii.x), radii.y) + length(shift) + smoothing * min(radii.x, radii.y) / 3.0);
    let a = base + axis * select(profile.x, 0.0, offset);
    let b = base + axis * select(profile.z, length_axis, offset);
    let lo = min(a, b) - extent;
    let hi = max(a, b) + extent;
    let safe_rd = select(vec3<f32>(1e-12), rd, abs(rd) > vec3<f32>(1e-12));
    let ta = (lo - ro) / safe_rd;
    let tb = (hi - ro) / safe_rd;
    let near = min(ta, tb);
    let far = max(ta, tb);
    var t = max(max(near.x, near.y), max(near.z, 1e-5));
    let end = min(min(far.x, far.y), far.z);
    if (t > end) { return vec4<f32>(-1.0, 0.0, 0.0, 0.0); }
    let lipschitz = bond_profile_lipschitz(length_axis, neck, profile, smoothing, shift);
    let epsilon = max(1e-6, neck * 1e-4);
    let initial = bond_profile_field(ro + rd * t, base, axis, length_axis, neck, profile, smoothing, shift, lipschitz);
    if (initial < -epsilon && !exit_inside) { return vec4<f32>(-1.0, 0.0, 0.0, 0.0); }
    for (var i = 0u; i < 192u; i = i + 1u) {
        if (t > end) { break; }
        let p = ro + rd * t;
        let distance = bond_profile_field(p, base, axis, length_axis, neck, profile, smoothing, shift, lipschitz);
        if (abs(distance) < epsilon) {
            let s = dot(p - base, axis);
            var normal = bond_profile_normal(p, base, axis, length_axis, neck, profile, smoothing, shift);
            if (!offset && s <= profile.x + epsilon && length(p - a) < profile.y - epsilon) { normal = -axis; }
            if (!offset && s >= profile.z - epsilon && length(p - b) < profile.w - epsilon) { normal = axis; }
            return vec4<f32>(t, normal);
        }
        t = t + max(abs(distance) * 0.9, epsilon * 0.5);
    }
    return vec4<f32>(-1.0, 0.0, 0.0, 0.0);
}
