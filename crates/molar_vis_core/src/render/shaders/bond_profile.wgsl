// Shared cubic spline profile for raster, envelope containment and ray tracing.
fn bond_profile_enabled(profile: vec4<f32>) -> bool {
    return profile.y > 0.0 && profile.w > 0.0 && profile.z > profile.x;
}
// Returns value and derivative with respect to the axial position.
fn bond_hermite(y0: f32, y1: f32, m0: f32, m1: f32, span: f32, u: f32) -> vec2<f32> {
    let a = 2.0 * y0 - 2.0 * y1 + span * (m0 + m1);
    let b = -3.0 * y0 + 3.0 * y1 - span * (2.0 * m0 + m1);
    return vec2<f32>(((a * u + b) * u + span * m0) * u + y0,
        (3.0 * a * u * u + 2.0 * b * u) / span + m0);
}
// Left and right positions where the flare reaches the straight neck.
fn bond_profile_waists(length_axis: f32, neck: f32, profile: vec4<f32>, smoothing: f32) -> vec2<f32> {
    let middle = 0.5 * length_axis;
    let tip0 = min(sqrt(max(0.0, profile.x * profile.x + profile.y * profile.y - neck * neck)), middle);
    let x1 = length_axis - profile.z;
    let tip1 = min(sqrt(max(0.0, x1 * x1 + profile.w * profile.w - neck * neck)), middle);
    return vec2<f32>(mix(tip0, middle, smoothing), length_axis - mix(tip1, middle, smoothing));
}
fn bond_profile_radius(s: f32, length_axis: f32, neck: f32, profile: vec4<f32>, smoothing: f32) -> vec2<f32> {
    let middle = 0.5 * length_axis;
    let waists = bond_profile_waists(length_axis, neck, profile, smoothing);
    if (s > waists.x && s < waists.y) { return vec2<f32>(neck, 0.0); }
    if (s <= middle) {
        let span = max(waists.x - profile.x, 1e-8);
        return bond_hermite(profile.y, neck, -profile.x / profile.y, 0.0, span, clamp((s - profile.x) / span, 0.0, 1.0));
    }
    let span = max(profile.z - waists.y, 1e-8);
    return bond_hermite(neck, profile.w, 0.0, (length_axis - profile.z) / profile.w, span, clamp((s - waists.y) / span, 0.0, 1.0));
}
// Multiple-bond tubes stay on their parallel, offset axes. Blend each tube
// with the actual atom spheres, rather than steering its axis into their centers.
// The cubic smooth minimum has continuous normals and a compact blend region.
fn bond_smooth_union(a: vec4<f32>, b: vec4<f32>, k: f32) -> vec4<f32> {
    let h = max(k - abs(a.x - b.x), 0.0) / k;
    let weight = 0.5 * h * h;
    let gradient = select(mix(a.yzw, b.yzw, weight), mix(b.yzw, a.yzw, weight), a.x > b.x);
    return vec4<f32>(min(a.x, b.x) - k * h * h * h / 6.0, gradient);
}
fn bond_profile_atom_radii(length_axis: f32, profile: vec4<f32>) -> vec2<f32> {
    return vec2<f32>(length(profile.xy), length(vec2<f32>(length_axis - profile.z, profile.w)));
}
fn bond_offset_surface(p: vec3<f32>, base: vec3<f32>, axis: vec3<f32>, length_axis: f32,
    neck: f32, profile: vec4<f32>, smoothing: f32, shift: vec3<f32>) -> vec4<f32> {
    let radii = bond_profile_atom_radii(length_axis, profile);
    let s = dot(p - base - shift, axis);
    let tube_delta = p - base - shift - axis * clamp(s, 0.0, length_axis);
    let d0 = p - base;
    let d1 = p - base - axis * length_axis;
    let tube = vec4<f32>(length(tube_delta) - neck, tube_delta / max(length(tube_delta), 1e-8));
    let sphere0 = vec4<f32>(length(d0) - radii.x, d0 / max(length(d0), 1e-8));
    let sphere1 = vec4<f32>(length(d1) - radii.y, d1 / max(length(d1), 1e-8));
    let k = max(1e-7, min(radii.x, radii.y) * smoothing);
    return bond_smooth_union(bond_smooth_union(tube, sphere0, k), sphere1, k);
}
fn bond_profile_normal(p: vec3<f32>, base: vec3<f32>, axis: vec3<f32>, length_axis: f32,
    neck: f32, profile: vec4<f32>, smoothing: f32, shift: vec3<f32>) -> vec3<f32> {
    if (dot(shift, shift) > 1e-16) {
        return normalize(bond_offset_surface(p, base, axis, length_axis, neck, profile, smoothing, shift).yzw);
    }
    let s = dot(p - base, axis);
    let radius = bond_profile_radius(s, length_axis, neck, profile, smoothing);
    let delta = p - base - axis * s;
    let radial = delta / max(length(delta), 1e-8);
    return normalize(radial - axis * radius.y);
}
fn bond_profile_containment(p: vec3<f32>, base: vec3<f32>, axis: vec3<f32>, length_axis: f32,
    neck: f32, profile: vec4<f32>, smoothing: f32, shift: vec3<f32>) -> f32 {
    if (dot(shift, shift) > 1e-16) {
        return 2.0 * bond_offset_surface(p, base, axis, length_axis, neck, profile, smoothing, shift).x / neck;
    }
    let s = dot(p - base, axis);
    if (s < profile.x - 1e-6 || s > profile.z + 1e-6) { return 1.0; }
    let radius = bond_profile_radius(s, length_axis, neck, profile, smoothing).x;
    let delta = p - base - axis * s;
    // Relative squared distance makes the same tolerance work for different radii.
    return dot(delta, delta) / (radius * radius) - 1.0;
}
fn bond_profile_lipschitz(length_axis: f32, neck: f32, profile: vec4<f32>, smoothing: f32, shift: vec3<f32>) -> f32 {
    if (dot(shift, shift) > 1e-16) { return 1.0; }
    let waists = bond_profile_waists(length_axis, neck, profile, smoothing);
    let span0 = max(waists.x - profile.x, 1e-8);
    let span1 = max(profile.z - waists.y, 1e-8);
    let slope0 = profile.x / profile.y;
    let slope1 = (length_axis - profile.z) / profile.w;
    // Hermite derivative is a quadratic Bezier curve: its three control values
    // bound the slope everywhere, including highly unequal atom radii.
    let slope = max(max(slope0, abs(3.0 * (profile.y - neck) / span0 - slope0)),
        max(slope1, abs(3.0 * (profile.w - neck) / span1 - slope1)));
    let bound = slope;
    return sqrt(1.0 + bound * bound);
}
fn bond_profile_field(p: vec3<f32>, base: vec3<f32>, axis: vec3<f32>, length_axis: f32,
    neck: f32, profile: vec4<f32>, smoothing: f32, shift: vec3<f32>, lipschitz: f32) -> f32 {
    if (dot(shift, shift) > 1e-16) {
        return bond_offset_surface(p, base, axis, length_axis, neck, profile, smoothing, shift).x;
    }
    let s = dot(p - base, axis);
    let radius = bond_profile_radius(s, length_axis, neck, profile, smoothing).x;
    let delta = p - base - axis * s;
    return max((length(delta) - radius) / lipschitz, max(profile.x - s, s - profile.z));
}
// Conservative sphere tracing of the spline solid. Work is confined to its
// bounding box and runs only for the optional flared profile. Return t + normal.
// Constants reused by every march step; original helpers remain the reference
// for normals and standalone containment. Cubic arithmetic retains its order.
struct BondCubicCache {
    waists: vec2<f32>, spans: vec2<f32>, left: vec4<f32>, right: vec4<f32>,
}
fn bond_cubic_coefficients(y0: f32, y1: f32, m0: f32, m1: f32, span: f32) -> vec4<f32> {
    return vec4<f32>(2.0 * y0 - 2.0 * y1 + span * (m0 + m1),
        -3.0 * y0 + 3.0 * y1 - span * (2.0 * m0 + m1), span * m0, y0);
}
fn bond_cubic_cache(length_axis: f32, neck: f32, profile: vec4<f32>, smoothing: f32) -> BondCubicCache {
    let waists = bond_profile_waists(length_axis, neck, profile, smoothing);
    let spans = max(vec2<f32>(waists.x - profile.x, profile.z - waists.y), vec2<f32>(1e-8));
    return BondCubicCache(waists, spans,
        bond_cubic_coefficients(profile.y, neck, -profile.x / profile.y, 0.0, spans.x),
        bond_cubic_coefficients(neck, profile.w, 0.0, (length_axis - profile.z) / profile.w, spans.y));
}
fn bond_cached_radius(s: f32, length_axis: f32, neck: f32, profile: vec4<f32>, cache: BondCubicCache) -> f32 {
    if (s > cache.waists.x && s < cache.waists.y) { return neck; }
    var u: f32;
    var c: vec4<f32>;
    if (s <= 0.5 * length_axis) {
        u = clamp((s - profile.x) / cache.spans.x, 0.0, 1.0); c = cache.left;
    } else {
        u = clamp((s - cache.waists.y) / cache.spans.y, 0.0, 1.0); c = cache.right;
    }
    return ((c.x * u + c.y) * u + c.z) * u + c.w;
}
fn bond_cached_field(p: vec3<f32>, base: vec3<f32>, axis: vec3<f32>, length_axis: f32,
    neck: f32, profile: vec4<f32>, smoothing: f32, shift: vec3<f32>, lipschitz: f32, cache: BondCubicCache) -> f32 {
    if (dot(shift, shift) > 1e-16) {
        return bond_offset_surface(p, base, axis, length_axis, neck, profile, smoothing, shift).x;
    }
    let s = dot(p - base, axis);
    let radius = bond_cached_radius(s, length_axis, neck, profile, cache);
    let delta = p - base - axis * s;
    return max((length(delta) - radius) / lipschitz, max(profile.x - s, s - profile.z));
}
struct BondRayDiagnostic { hit: vec4<f32>, iterations: u32 }

fn bond_profile_ray(base: vec3<f32>, axis: vec3<f32>, length_axis: f32, neck: f32,
    profile: vec4<f32>, smoothing: f32, shift: vec3<f32>, ro: vec3<f32>, rd: vec3<f32>, exit_inside: bool) -> vec4<f32> {
    return bond_profile_ray_diagnostic(base, axis, length_axis, neck, profile, smoothing, shift, ro, rd, exit_inside).hit;
}
fn bond_profile_ray_diagnostic(base: vec3<f32>, axis: vec3<f32>, length_axis: f32, neck: f32,
    profile: vec4<f32>, smoothing: f32, shift: vec3<f32>, ro: vec3<f32>, rd: vec3<f32>, exit_inside: bool) -> BondRayDiagnostic {
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
    if (t > end) { return BondRayDiagnostic(vec4<f32>(-1.0, 0.0, 0.0, 0.0), 0u); }
    let lipschitz = bond_profile_lipschitz(length_axis, neck, profile, smoothing, shift);
    let epsilon = max(1e-6, neck * 1e-4);
    let cache = bond_cubic_cache(length_axis, neck, profile, smoothing);
    let initial = bond_cached_field(ro + rd * t, base, axis, length_axis, neck, profile, smoothing, shift, lipschitz, cache);
    if (initial < -epsilon && !exit_inside) { return BondRayDiagnostic(vec4<f32>(-1.0, 0.0, 0.0, 0.0), 0u); }
    var iterations = 0u;
    for (var i = 0u; i < 192u; i = i + 1u) {
        if (t > end) { break; }
        let p = ro + rd * t;
        iterations = i + 1u;
        let distance = bond_cached_field(p, base, axis, length_axis, neck, profile, smoothing, shift, lipschitz, cache);
        if (abs(distance) < epsilon) {
            let s = dot(p - base, axis);
            var normal = bond_profile_normal(p, base, axis, length_axis, neck, profile, smoothing, shift);
            if (!offset && s <= profile.x + epsilon && length(p - a) < profile.y - epsilon) { normal = -axis; }
            if (!offset && s >= profile.z - epsilon && length(p - b) < profile.w - epsilon) { normal = axis; }
            return BondRayDiagnostic(vec4<f32>(t, normal), iterations);
        }
        t = t + max(abs(distance) * 0.9, epsilon * 0.5);
    }
    return BondRayDiagnostic(vec4<f32>(-1.0, 0.0, 0.0, 0.0), iterations);
}
