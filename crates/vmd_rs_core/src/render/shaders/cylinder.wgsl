// Cylinder impostor. Each instance is one half-bond (p0..p1). A camera-facing
// billboard is generated in view space; the fragment shader ray-casts a finite,
// capless cylinder and writes analytic depth. Joints are covered by sphere caps.

struct Camera {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    params: vec4<f32>, // params.x: 1.0 = perspective, 0.0 = orthographic
    cue: vec4<f32>,    // depth cue: near, far, strength, _
    fog_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

// Linear depth cueing: fade toward the background as eye-space distance grows.
fn apply_fog(color: vec3<f32>, eye_z: f32) -> vec3<f32> {
    let d = -eye_z; // eye-space distance (camera looks down -Z)
    let f = clamp((d - camera.cue.x) / max(camera.cue.y - camera.cue.x, 1e-6), 0.0, 1.0) * camera.cue.z;
    return mix(color, camera.fog_color.rgb, f);
}

struct Instance {
    @location(0) p0: vec3<f32>,
    @location(1) radius: f32,
    @location(2) p1: vec3<f32>,
    @location(3) color: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) view_pos: vec3<f32>,
    @location(1) @interpolate(flat) base: vec3<f32>,   // p0 in view space
    @location(2) @interpolate(flat) axis: vec3<f32>,   // unit axis, view space
    @location(3) @interpolate(flat) radius: f32,
    @location(4) @interpolate(flat) seg_len: f32,
    @location(5) @interpolate(flat) color: vec3<f32>,
};

fn unpack_color(c: u32) -> vec3<f32> {
    let r = f32((c >> 0u) & 0xffu) / 255.0;
    let g = f32((c >> 8u) & 0xffu) / 255.0;
    let b = f32((c >> 16u) & 0xffu) / 255.0;
    return vec3<f32>(r, g, b);
}

@vertex
fn vs_main(@builtin(vertex_index) vidx: u32, inst: Instance) -> VsOut {
    let a0 = (camera.view * vec4<f32>(inst.p0, 1.0)).xyz;
    let a1 = (camera.view * vec4<f32>(inst.p1, 1.0)).xyz;
    let axis_v = a1 - a0;
    let seg_len = length(axis_v);
    let d = select(vec3<f32>(0.0, 0.0, 1.0), axis_v / seg_len, seg_len > 1e-8);

    // Screen-perpendicular extent: perpendicular to both the axis and the view
    // direction, so the billboard covers the silhouette width. The view
    // direction is the eye ray (perspective) or constant -Z (orthographic).
    let mid = (a0 + a1) * 0.5;
    let view_dir = select(vec3<f32>(0.0, 0.0, 1.0), normalize(mid), camera.params.x > 0.5);
    var perp = cross(d, view_dir);
    let pl = length(perp);
    perp = select(vec3<f32>(1.0, 0.0, 0.0), perp / pl, pl > 1e-6);

    // vidx -> (which end, which side)
    var ends = array<f32, 4>(0.0, 0.0, 1.0, 1.0);
    var sides = array<f32, 4>(-1.0, 1.0, -1.0, 1.0);
    let end_pt = select(a0 - d * inst.radius, a1 + d * inst.radius, ends[vidx] > 0.5);
    let pos = end_pt + perp * (sides[vidx] * inst.radius);

    var out: VsOut;
    out.clip = camera.proj * vec4<f32>(pos, 1.0);
    out.view_pos = pos;
    out.base = a0;
    out.axis = d;
    out.radius = inst.radius;
    out.seg_len = seg_len;
    out.color = unpack_color(inst.color);
    return out;
}

struct FsOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_main(in: VsOut) -> FsOut {
    var ro: vec3<f32>;
    var rd: vec3<f32>;
    if (camera.params.x > 0.5) {
        ro = vec3<f32>(0.0, 0.0, 0.0);
        rd = normalize(in.view_pos);
    } else {
        // Parallel ray; origin on the camera plane (z=0) so the whole scene
        // (which lies at z<0) is in front and the near intersection has t>0.
        ro = vec3<f32>(in.view_pos.x, in.view_pos.y, 0.0);
        rd = vec3<f32>(0.0, 0.0, -1.0);
    }
    let ua = in.axis;

    // Ray-cylinder in the plane perpendicular to the axis.
    let oc = ro - in.base;
    let rd_p = rd - ua * dot(rd, ua);
    let oc_p = oc - ua * dot(oc, ua);
    let a = dot(rd_p, rd_p);
    let b = 2.0 * dot(rd_p, oc_p);
    let c = dot(oc_p, oc_p) - in.radius * in.radius;
    let disc = b * b - 4.0 * a * c;
    if (disc < 0.0 || a < 1e-12) {
        discard;
    }
    let t = (-b - sqrt(disc)) / (2.0 * a);
    if (t < 0.0) {
        discard;
    }
    let hit = ro + t * rd;
    let h = dot(hit - in.base, ua);
    if (h < 0.0 || h > in.seg_len) {
        discard; // capless; sphere caps cover the joints
    }

    let axis_point = in.base + ua * h;
    let normal = normalize(hit - axis_point);
    let clip = camera.proj * vec4<f32>(hit, 1.0);

    let light_dir = normalize(vec3<f32>(0.3, 0.4, 1.0));
    let diffuse = max(dot(normal, light_dir), 0.0);
    let shade = 0.25 + 0.75 * diffuse;

    var out: FsOut;
    out.depth = clip.z / clip.w;
    out.color = vec4<f32>(apply_fog(in.color * shade, hit.z), 1.0);
    return out;
}
