// Sphere impostor. Each instance is one atom. A camera-facing billboard quad is
// generated in view space from the vertex index; the fragment shader ray-casts
// the analytic sphere and writes true depth so impostors occlude each other (and
// later the cartoon mesh) correctly. All work is done in view (eye) space.

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
    @location(0) center: vec3<f32>,
    @location(1) radius: f32,
    @location(2) color: u32,
    @location(3) mat: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) view_pos: vec3<f32>,    // this fragment's point on the billboard, view space
    @location(1) view_center: vec3<f32>, // sphere center, view space
    @location(2) radius: f32,
    @location(3) color: vec4<f32>, // rgb + opacity (alpha)
    @location(4) @interpolate(flat) mat: u32, // packed material lighting
};

fn unpack_color(c: u32) -> vec4<f32> {
    let r = f32((c >> 0u) & 0xffu) / 255.0;
    let g = f32((c >> 8u) & 0xffu) / 255.0;
    let b = f32((c >> 16u) & 0xffu) / 255.0;
    let a = f32((c >> 24u) & 0xffu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

// Unpack the per-element material lighting coefficients (ambient, diffuse,
// specular, shininess), each a u8 packed as ambient|diffuse<<8|specular<<16|
// shininess<<24.
fn unpack_mat(m: u32) -> vec4<f32> {
    let amb = f32((m >> 0u) & 0xffu) / 255.0;
    let dif = f32((m >> 8u) & 0xffu) / 255.0;
    let spc = f32((m >> 16u) & 0xffu) / 255.0;
    let shn = f32((m >> 24u) & 0xffu) / 255.0;
    return vec4<f32>(amb, dif, spc, shn);
}

// Blinn-Phong shade in view space: matte term + white specular highlight. The
// light is a fixed headlight from the camera side; `view_dir` points to the eye.
fn shade_material(base: vec3<f32>, normal: vec3<f32>, view_dir: vec3<f32>, mat: vec4<f32>) -> vec3<f32> {
    let light_dir = normalize(vec3<f32>(0.3, 0.4, 1.0));
    let ndotl = max(dot(normal, light_dir), 0.0);
    let half = normalize(light_dir + view_dir);
    let ndoth = max(dot(normal, half), 0.0);
    let exponent = 2.0 + mat.w * 128.0;
    let spec = mat.z * pow(ndoth, exponent);
    return base * (mat.x + mat.y * ndotl) + vec3<f32>(spec);
}

@vertex
fn vs_main(@builtin(vertex_index) vidx: u32, inst: Instance) -> VsOut {
    // Triangle-strip quad corners in [-1,1]^2.
    var corners = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0,  1.0),
    );
    let corner = corners[vidx];

    let view_center = (camera.view * vec4<f32>(inst.center, 1.0)).xyz;
    // Oversize slightly so the perspective silhouette is fully covered; extra
    // fragments are discarded by the ray test.
    let r = inst.radius * 1.25;
    let view_pos = view_center + vec3<f32>(corner * r, 0.0);

    var out: VsOut;
    out.clip = camera.proj * vec4<f32>(view_pos, 1.0);
    out.view_pos = view_pos;
    out.view_center = view_center;
    out.radius = inst.radius;
    out.color = unpack_color(inst.color);
    out.mat = inst.mat;
    return out;
}

struct FsOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_main(in: VsOut) -> FsOut {
    // Perspective: ray from the eye (origin) through this fragment.
    // Orthographic: parallel rays along view -Z through this fragment's xy.
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

    // Ray-sphere intersection: |ro + t*rd - center|^2 = radius^2.
    let oc = ro - in.view_center;
    let b = dot(oc, rd);
    let c = dot(oc, oc) - in.radius * in.radius;
    let disc = b * b - c;
    if (disc < 0.0) {
        discard;
    }
    let t = -b - sqrt(disc);
    if (t < 0.0) {
        discard;
    }

    let hit = ro + t * rd;
    let normal = normalize(hit - in.view_center);

    // Analytic depth: project the view-space hit and take NDC z (wgpu: [0,1]).
    let clip = camera.proj * vec4<f32>(hit, 1.0);

    // Per-material Blinn-Phong (view dir to eye: origin for perspective, +z ortho).
    let view_dir = select(vec3<f32>(0.0, 0.0, 1.0), normalize(-hit), camera.params.x > 0.5);
    let lit = shade_material(in.color.rgb, normal, view_dir, unpack_mat(in.mat));

    var out: FsOut;
    out.depth = clip.z / clip.w;
    out.color = vec4<f32>(apply_fog(lit, hit.z), in.color.a);
    return out;
}
