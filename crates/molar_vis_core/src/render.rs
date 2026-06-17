//! GPU rendering of the molecular scene.
//!
//! Strategy A: the 3D scene is rendered into our *own* offscreen color +
//! `Depth32Float` targets, then composited into the egui frame as a registered
//! texture (`egui::Image`). egui's own render pass has no depth attachment, so
//! this is what gives us full control of depth — required for the ray-cast
//! impostors, whose fragment shaders write analytic depth and must occlude
//! correctly against each other (and later the cartoon mesh).

mod background;
mod camera_uniform;
mod cylinder;
mod line;
mod mesh;
mod sphere;
mod ssao;

pub use cylinder::CylinderInstance;
pub use line::LineVertex;
pub use mesh::MeshVertex;
pub use sphere::SphereInstance;

use background::BgUniform;
use camera_uniform::CameraUniform;
use ssao::SsaoUniform;
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use eframe::egui_wgpu::RenderState;
use egui::TextureId;

use crate::geometry::GeometryData;
use crate::scene::Scene;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Weighted-blended OIT targets: `accum` holds the running sum of weighted
/// premultiplied color (RGB) + weight (A); `reveal` holds the running product of
/// `1 - alpha`. Both are float so the accumulation doesn't clamp/quantize.
const ACCUM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const REVEAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
/// GPU pick id-buffer format: `[x, y]` = `[mol+1, rep<<21 | atom]` per pixel
/// (x = 0 means no hit). Two 32-bit uints, rendered without blending. Native only.
#[cfg(not(target_arch = "wasm32"))]
const PICK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Uint;

/// Scene background. Used both as the render-pass clear color and as the depth-cue
/// fog color, so distant geometry dissolves seamlessly into the background.
const BG: [f32; 4] = [0.02, 0.02, 0.05, 1.0];

/// Stride of one camera entry in the (dynamic-offset) camera uniform buffer. The
/// buffer holds the base camera plus one entry per periodic image (base view ×
/// lattice translation); each draw selects its entry with a dynamic offset. 256
/// satisfies `min_uniform_buffer_offset_alignment` on every target.
const CAMERA_STRIDE: u64 = 256;

// Supersampling factor (`SceneRenderer::ssaa`, from the program settings): the
// offscreen targets are rendered at `ssaa×` the viewport resolution, then egui's
// linear filter downsamples them into the (1×) image rect — a 2×2 box average that
// anti-aliases **everything**, including the ray-cast impostor silhouettes (decided
// per-pixel by `discard`, so MSAA can't touch them). The camera's viewport param
// stays at the *logical* size so fat-line pixel widths come out correct.
//
// Cast-shadow depth-map resolution (`SceneRenderer::shadow_res`, from the settings):
// square, light-space (doesn't track the viewport); 2048² is ample for the molecular
// scale + the 3×3 PCF. The PCF texel size is fed to `ssao.wgsl` via the SSAO uniform's
// `misc.z`, so it stays correct at any resolution.

/// View-space direction **toward** the key light used for cast shadows. Off the
/// view axis (upper-right, moderately toward the camera) so shadows fall on
/// surfaces the camera can see — a near-camera headlight would hide them behind
/// the geometry that casts them. (The diffuse headlight in the lit shaders is a
/// separate, flatter fill; this is the shadow-casting key.)
const SHADOW_LIGHT_DIR_VIEW: glam::Vec3 = glam::Vec3::new(0.45, 0.78, 0.45);

/// Bind-group binding size for one camera entry (the actual `CameraUniform`).
fn camera_binding_size() -> Option<std::num::NonZeroU64> {
    std::num::NonZeroU64::new(std::mem::size_of::<CameraUniform>() as u64)
}

/// (Re)create the camera bind group over `buf` with a dynamic-offset binding.
fn make_camera_bind_group(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    buf: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("camera-bg"),
        layout: bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: buf,
                offset: 0,
                size: camera_binding_size(),
            }),
        }],
    })
}

/// (Re)create the SSAO bind group over the scene `depth_view` + the SSAO uniform.
/// Recreated whenever the targets (hence the depth view) change.
fn make_ssao_bind_group(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    depth_view: &wgpu::TextureView,
    buf: &wgpu::Buffer,
    shadow_view: &wgpu::TextureView,
    shadow_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ssao-bg"),
        layout: bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(shadow_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(shadow_sampler),
            },
        ],
    })
}

/// Color-target descriptors for the opaque pass: a single alpha-blended target.
fn opaque_targets(color_format: wgpu::TextureFormat) -> Vec<Option<wgpu::ColorTargetState>> {
    vec![Some(wgpu::ColorTargetState {
        format: color_format,
        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
        write_mask: wgpu::ColorWrites::ALL,
    })]
}

/// Color-target descriptors for the weighted-blended OIT pass: `accum` is purely
/// additive, `reveal` multiplies the destination by `1 - src` (revealage).
fn oit_targets() -> Vec<Option<wgpu::ColorTargetState>> {
    let add = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    let mul = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::OneMinusSrc,
        operation: wgpu::BlendOperation::Add,
    };
    vec![
        Some(wgpu::ColorTargetState {
            format: ACCUM_FORMAT,
            blend: Some(wgpu::BlendState { color: add, alpha: add }),
            write_mask: wgpu::ColorWrites::ALL,
        }),
        Some(wgpu::ColorTargetState {
            format: REVEAL_FORMAT,
            blend: Some(wgpu::BlendState { color: mul, alpha: mul }),
            write_mask: wgpu::ColorWrites::RED,
        }),
    ]
}

/// Color-target descriptor for the selection-glow pass: a single target that adds
/// the (cyan, Fresnel-weighted) glow onto the already-composited scene color —
/// `dst + glow.rgb * glow.a`.
fn glow_targets(color_format: wgpu::TextureFormat) -> Vec<Option<wgpu::ColorTargetState>> {
    let add = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    let add_a = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    vec![Some(wgpu::ColorTargetState {
        format: color_format,
        blend: Some(wgpu::BlendState { color: add, alpha: add_a }),
        write_mask: wgpu::ColorWrites::ALL,
    })]
}

/// Offscreen render targets, recreated when the viewport size changes. Besides
/// the composited color + depth, this holds the weighted-blended OIT `accum` and
/// `reveal` targets and a bind group exposing them to the composite pass.
struct Targets {
    size: [u32; 2],
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    accum_view: wgpu::TextureView,
    reveal_view: wgpu::TextureView,
    oit_bind_group: wgpu::BindGroup,
    _color_tex: wgpu::Texture,
    _depth_tex: wgpu::Texture,
}

impl Targets {
    fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        oit_bgl: &wgpu::BindGroupLayout,
        size: [u32; 2],
    ) -> Self {
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let make = |label: &str, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let attach_sample =
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let color_tex = make("scene-color", color_format, attach_sample);
        // The depth target is sampled by the SSAO pass, so it also needs TEXTURE_BINDING.
        let depth_tex = make("scene-depth", DEPTH_FORMAT, attach_sample);
        let accum_tex = make("oit-accum", ACCUM_FORMAT, attach_sample);
        let reveal_tex = make("oit-reveal", REVEAL_FORMAT, attach_sample);

        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let accum_view = view(&accum_tex);
        let reveal_view = view(&reveal_tex);

        let oit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("oit-composite-bg"),
            layout: oit_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&accum_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&reveal_view),
                },
            ],
        });

        Self {
            size,
            color_view: view(&color_tex),
            depth_view: view(&depth_tex),
            accum_view,
            reveal_view,
            oit_bind_group,
            _color_tex: color_tex,
            _depth_tex: depth_tex,
        }
    }
}

/// A vertex/instance buffer plus its element count.
struct DrawBuffer {
    buffer: wgpu::Buffer,
    count: u32,
}

/// An indexed triangle mesh (vertex + u32 index buffer).
struct MeshBuffers {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    vertex_count: u32,
    index_count: u32,
}

/// Per-representation GPU geometry (any subset may be present).
#[derive(Default)]
pub struct RepGpu {
    spheres: Option<DrawBuffer>,   // 4 verts/instance
    cylinders: Option<DrawBuffer>, // 4 verts/instance
    lines: Option<DrawBuffer>,     // vertex count (LineList)
    mesh: Option<MeshBuffers>,     // indexed triangles (cartoon)
}

impl RepGpu {
    /// Whether any buffer holds drawable geometry (used to skip empty glow passes).
    pub fn has_geometry(&self) -> bool {
        self.spheres.is_some()
            || self.cylinders.is_some()
            || self.lines.is_some()
            || self.mesh.is_some()
    }
}

fn upload_buf<T: bytemuck::Pod>(
    device: &wgpu::Device,
    data: &[T],
    label: &str,
) -> Option<DrawBuffer> {
    if data.is_empty() {
        return None;
    }
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(data),
        // COPY_DST so an unchanged-size frame update can write in place (see `update`).
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
    });
    Some(DrawBuffer {
        buffer,
        count: data.len() as u32,
    })
}

/// Incrementally update an instance/vertex buffer for a coordinates-only change.
/// If the element count is unchanged, write into the existing buffer (no
/// reallocation); otherwise recreate it.
fn update_buf<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    slot: &mut Option<DrawBuffer>,
    data: &[T],
    label: &str,
) {
    match slot {
        Some(b) if b.count as usize == data.len() && !data.is_empty() => {
            queue.write_buffer(&b.buffer, 0, bytemuck::cast_slice(data));
        }
        _ => *slot = upload_buf(device, data, label),
    }
}

fn upload_mesh(device: &wgpu::Device, mesh: &crate::geometry::MeshData) -> Option<MeshBuffers> {
    if mesh.indices.is_empty() {
        return None;
    }
    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("mesh-verts"),
        contents: bytemuck::cast_slice(&mesh.vertices),
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
    });
    let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("mesh-indices"),
        contents: bytemuck::cast_slice(&mesh.indices),
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
    });
    Some(MeshBuffers {
        vertices,
        indices,
        vertex_count: mesh.vertices.len() as u32,
        index_count: mesh.indices.len() as u32,
    })
}

/// Incrementally update a cartoon mesh for a coordinates-only change. When the
/// vertex and index counts are unchanged (same SS → same topology, only the
/// spline positions moved), write the vertices in place; otherwise recreate.
fn update_mesh(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    slot: &mut Option<MeshBuffers>,
    mesh: &crate::geometry::MeshData,
) {
    match slot {
        Some(m)
            if m.vertex_count as usize == mesh.vertices.len()
                && m.index_count as usize == mesh.indices.len()
                && !mesh.indices.is_empty() =>
        {
            queue.write_buffer(&m.vertices, 0, bytemuck::cast_slice(&mesh.vertices));
        }
        _ => *slot = upload_mesh(device, mesh),
    }
}

/// Build the OIT resolve pipeline: a vertex-buffer-less fullscreen triangle that
/// samples the accum + reveal targets (bind group 0) and blends the resolved
/// transparent color over the opaque scene color with `(SrcAlpha, 1-SrcAlpha)`.
fn build_composite_pipeline(
    device: &wgpu::Device,
    color_format: wgpu::TextureFormat,
    oit_bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("oit-composite-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("render/shaders/oit_composite.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("oit-composite-layout"),
        bind_group_layouts: &[Some(oit_bgl)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("oit-composite-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

pub struct SceneRenderer {
    color_format: wgpu::TextureFormat,
    targets: Targets,
    egui_texture: TextureId,
    /// Supersampling factor (1 = off). From the program settings; `reconfigure`
    /// changes it live and the next render recreates the targets at the new size.
    ssaa: u32,
    /// Cast-shadow depth-map resolution (square). From the program settings.
    shadow_res: u32,

    camera_bgl: wgpu::BindGroupLayout,
    camera_buf: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    /// Number of camera entries `camera_buf` can hold (grows with periodic images).
    camera_capacity: u32,

    // Each geometry has an opaque pipeline `[0]` (single alpha-blended target,
    // depth-write on), a weighted-blended OIT pipeline `[1]` (accum+reveal targets,
    // depth-write off, depth-test on), and a selection-glow pipeline `[2]` (additive
    // cyan over the color target, depth-test `≤` against the scene, no depth-write).
    // The OIT pipelines are resolved into the color target by `composite_pipeline`
    // after the transparent pass; the glow pipelines run in a final pass.
    sphere_pipeline: [wgpu::RenderPipeline; 3],
    cylinder_pipeline: [wgpu::RenderPipeline; 3],
    line_pipeline: [wgpu::RenderPipeline; 3],
    mesh_pipeline: [wgpu::RenderPipeline; 3],
    oit_bgl: wgpu::BindGroupLayout,
    composite_pipeline: wgpu::RenderPipeline,
    /// SSAO: a fullscreen pass (after opaque) that reads the scene depth and
    /// multiply-blends an occlusion factor onto the color. `ssao_bind_group`
    /// references the depth view, so it is recreated whenever `targets` change.
    ssao_bgl: wgpu::BindGroupLayout,
    ssao_buf: wgpu::Buffer,
    /// `None` when the device can't support the SSAO pass (WebGL2 — sampling the
    /// depth texture isn't reliable there); the pass is then skipped.
    ssao_pipeline: Option<wgpu::RenderPipeline>,
    ssao_bind_group: wgpu::BindGroup,
    /// Cast-shadow mapping (deferred): the scene is rendered from a key light into
    /// `shadow_depth_view` (a fixed-resolution depth map; `shadow_color_view` is a
    /// throwaway color target so the existing opaque pipelines can be reused for the
    /// depth-only render), then the AO pass samples it with `shadow_sampler` (a
    /// comparison sampler) to darken shadowed pixels. Gated to full WebGPU like SSAO.
    shadow_depth_view: wgpu::TextureView,
    shadow_color_view: wgpu::TextureView,
    shadow_sampler: wgpu::Sampler,
    _shadow_depth_tex: wgpu::Texture,
    _shadow_color_tex: wgpu::Texture,
    /// Fullscreen background-gradient pass (drawn first in the opaque pass when the
    /// background is a gradient; a solid background just uses the clear color).
    bg_buf: wgpu::Buffer,
    bg_pipeline: wgpu::RenderPipeline,
    bg_bind_group: wgpu::BindGroup,
    /// Whether the device supports weighted-blended OIT (needs `INDEPENDENT_BLEND`).
    /// False on WebGL2: transparent reps then render with plain alpha blending in the
    /// opaque pass, and the OIT/composite passes are skipped.
    oit_enabled: bool,

    // GPU atom picking (native only — needs a synchronous readback, which WebGPU
    // can't do, and integer render targets WebGL2 may not support): sphere impostors
    // (one per eligible atom, id-stamped — each molecule's `pick_gpu`) rendered into
    // an Rg32Uint id target at 1× resolution; the pixel under the cursor is read back
    // to identify the front-most atom. Lazily sized; rendered only on a pick request.
    // wasm falls back to the CPU ray-cast.
    #[cfg(not(target_arch = "wasm32"))]
    pick_pipeline: wgpu::RenderPipeline,
    #[cfg(not(target_arch = "wasm32"))]
    pick_id_tex: wgpu::Texture,
    #[cfg(not(target_arch = "wasm32"))]
    pick_id_view: wgpu::TextureView,
    #[cfg(not(target_arch = "wasm32"))]
    pick_depth_view: wgpu::TextureView,
    #[cfg(not(target_arch = "wasm32"))]
    pick_size: [u32; 2],
    #[cfg(not(target_arch = "wasm32"))]
    pick_readback: wgpu::Buffer,
    /// In-flight async pick: the readback is mapped; the flag flips when the GPU
    /// finishes (set in the `map_async` callback, read in `poll_pick`).
    #[cfg(not(target_arch = "wasm32"))]
    pick_pending: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// Pipeline-array index for the selection-glow pass.
const GLOW: usize = 2;

/// Create the pick id target (Rg32Uint, COPY_SRC) + its depth target at `size`.
#[cfg(not(target_arch = "wasm32"))]
fn make_pick_targets(
    device: &wgpu::Device,
    size: [u32; 2],
) -> (wgpu::Texture, wgpu::TextureView, wgpu::TextureView) {
    let extent = wgpu::Extent3d {
        width: size[0].max(1),
        height: size[1].max(1),
        depth_or_array_layers: 1,
    };
    let id_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pick-id"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: PICK_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pick-depth"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let id_view = id_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());
    (id_tex, id_view, depth_view)
}

impl SceneRenderer {
    pub fn new(rs: &RenderState, settings: &crate::settings::RenderingSettings) -> Self {
        let device = &rs.device;
        let color_format = rs.target_format;
        let settings = settings.sanitized();
        let ssaa = settings.ssaa;
        let shadow_res = settings.shadow_res;

        // Camera uniform = bind group 0, shared by every pipeline. It's a
        // **dynamic-offset** buffer: a base camera at entry 0 plus one entry per
        // periodic image (base view × lattice translation), selected per draw.
        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: camera_binding_size(),
                },
                count: None,
            }],
        });
        let camera_capacity = 1u32;
        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera-uniform"),
            size: camera_capacity as u64 * CAMERA_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = make_camera_bind_group(device, &camera_bgl, &camera_buf);

        // Weighted-blended OIT needs **independent per-target blend** (the accum
        // target blends additively while reveal blends multiplicatively). WebGL2
        // lacks this downlevel feature, so detect it: when absent, OIT is disabled
        // and transparent reps fall back to plain alpha blending in the opaque pass.
        let oit_enabled = rs
            .adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::INDEPENDENT_BLEND);
        if !oit_enabled {
            log::warn!(
                "device lacks INDEPENDENT_BLEND (e.g. WebGL2): order-independent \
                 transparency disabled; transparent reps use plain alpha blending"
            );
        }

        // Opaque pass: single alpha-blended target, depth-write on, `fs_main`.
        // OIT pass: accum+reveal targets, depth-write off (test on), `fs_oit`.
        let f = color_format;
        let opaque_t = opaque_targets(f);
        let oit_t = oit_targets();
        let glow_t = glow_targets(f);
        let less = wgpu::CompareFunction::Less;
        // Glow depth-tests `≤` (the glow geometry is identical to the scene's, so it
        // sits at exactly the scene depth → it must pass on equality), no depth-write.
        let lequal = wgpu::CompareFunction::LessEqual;
        let triple = |b: &dyn Fn(&[Option<wgpu::ColorTargetState>], bool, wgpu::CompareFunction, &str) -> wgpu::RenderPipeline| {
            [
                b(&opaque_t, true, less, "fs_main"),
                // OIT slot. Without INDEPENDENT_BLEND this pipeline would fail to
                // create, so build a harmless single-target placeholder (never bound:
                // `render_scene` skips the OIT pass when `oit_enabled` is false).
                if oit_enabled {
                    b(&oit_t, false, less, "fs_oit")
                } else {
                    b(&opaque_t, true, less, "fs_main")
                },
                b(&glow_t, false, lequal, "fs_glow"),
            ]
        };
        let sphere_pipeline = triple(&|t, dw, dc, fs| {
            sphere::build_pipeline(device, DEPTH_FORMAT, &camera_bgl, t, dw, dc, fs)
        });
        let cylinder_pipeline = triple(&|t, dw, dc, fs| {
            cylinder::build_pipeline(device, DEPTH_FORMAT, &camera_bgl, t, dw, dc, fs)
        });
        let line_pipeline = triple(&|t, dw, dc, fs| {
            line::build_pipeline(device, DEPTH_FORMAT, &camera_bgl, t, dw, dc, fs)
        });
        let mesh_pipeline = triple(&|t, dw, dc, fs| {
            mesh::build_pipeline(device, DEPTH_FORMAT, &camera_bgl, t, dw, dc, fs)
        });

        // OIT resolve: a fullscreen pass that reads the accum + reveal targets
        // (bind group 0 here, *not* the camera) and blends the order-independent
        // transparent color over the opaque scene color.
        let oit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("oit-composite-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let composite_pipeline = build_composite_pipeline(device, f, &oit_bgl);

        // SSAO: bind-group layout, uniform buffer, fullscreen multiply-blend pipeline.
        let ssao_bgl = ssao::bind_group_layout(device);
        let ssao_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ssao-uniform"),
            size: std::mem::size_of::<SsaoUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Only on a full-WebGPU device (WebGL2 — `!oit_enabled` — can't reliably
        // sample the depth texture the SSAO pass needs).
        let ssao_pipeline =
            oit_enabled.then(|| ssao::build_pipeline(device, color_format, &ssao_bgl));

        #[cfg(not(target_arch = "wasm32"))]
        let pick_pipeline =
            sphere::build_pick_pipeline(device, PICK_FORMAT, DEPTH_FORMAT, &camera_bgl);
        #[cfg(not(target_arch = "wasm32"))]
        let (pick_id_tex, pick_id_view, pick_depth_view) = make_pick_targets(device, [1, 1]);
        #[cfg(not(target_arch = "wasm32"))]
        let pick_readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pick-readback"),
            size: 256, // one Rg32Uint texel (8 B), padded
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Cast-shadow map: a fixed-resolution depth target rendered from the light,
        // plus a throwaway color target (so the opaque pipelines, which output color,
        // can be reused to fill it) and a comparison sampler for the PCF lookup.
        let shadow_extent = wgpu::Extent3d {
            width: shadow_res,
            height: shadow_res,
            depth_or_array_layers: 1,
        };
        let _shadow_depth_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow-depth"),
            size: shadow_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let _shadow_color_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow-color-throwaway"),
            size: shadow_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: color_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let shadow_depth_view =
            _shadow_depth_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let shadow_color_view =
            _shadow_color_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow-cmp-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        // Background-gradient pass: uniform (top/bottom colors), bind group, pipeline.
        let bg_bgl = background::bind_group_layout(device);
        let bg_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bg-uniform"),
            size: std::mem::size_of::<BgUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg_pipeline = background::build_pipeline(device, color_format, DEPTH_FORMAT, &bg_bgl);
        let bg_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg-bg"),
            layout: &bg_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: bg_buf.as_entire_binding(),
            }],
        });

        let targets = Targets::new(device, color_format, &oit_bgl, [1, 1]);
        let ssao_bind_group = make_ssao_bind_group(
            device,
            &ssao_bgl,
            &targets.depth_view,
            &ssao_buf,
            &shadow_depth_view,
            &shadow_sampler,
        );
        let egui_texture = rs.renderer.write().register_native_texture(
            device,
            &targets.color_view,
            wgpu::FilterMode::Linear,
        );

        Self {
            color_format,
            targets,
            egui_texture,
            ssaa,
            shadow_res,
            camera_bgl,
            camera_buf,
            camera_bind_group,
            camera_capacity,
            sphere_pipeline,
            cylinder_pipeline,
            line_pipeline,
            mesh_pipeline,
            oit_bgl,
            composite_pipeline,
            ssao_bgl,
            ssao_buf,
            ssao_pipeline,
            ssao_bind_group,
            shadow_depth_view,
            shadow_color_view,
            shadow_sampler,
            _shadow_depth_tex,
            _shadow_color_tex,
            bg_buf,
            bg_pipeline,
            bg_bind_group,
            oit_enabled,
            #[cfg(not(target_arch = "wasm32"))]
            pick_pipeline,
            #[cfg(not(target_arch = "wasm32"))]
            pick_id_tex,
            #[cfg(not(target_arch = "wasm32"))]
            pick_id_view,
            #[cfg(not(target_arch = "wasm32"))]
            pick_depth_view,
            #[cfg(not(target_arch = "wasm32"))]
            pick_size: [1, 1],
            #[cfg(not(target_arch = "wasm32"))]
            pick_readback,
            #[cfg(not(target_arch = "wasm32"))]
            pick_pending: None,
        }
    }

    /// Apply changed render settings live (from the settings dialog). SSAA just
    /// updates the field — the next `render_scene` recreates the offscreen targets
    /// when the computed size differs. A shadow-map-resolution change recreates the
    /// shadow textures + the SSAO bind group that samples them (the PCF texel is fed
    /// to the shader via the uniform, so it stays correct). The caller should force a
    /// re-render afterward (e.g. clear `last_render_camera`).
    pub fn reconfigure(&mut self, rs: &RenderState, settings: &crate::settings::RenderingSettings) {
        let settings = settings.sanitized();
        self.ssaa = settings.ssaa;
        if settings.shadow_res != self.shadow_res {
            self.shadow_res = settings.shadow_res;
            let device = &rs.device;
            let extent = wgpu::Extent3d {
                width: self.shadow_res,
                height: self.shadow_res,
                depth_or_array_layers: 1,
            };
            let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("shadow-depth"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let color_tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("shadow-color-throwaway"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.color_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.shadow_depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());
            self.shadow_color_view = color_tex.create_view(&wgpu::TextureViewDescriptor::default());
            self._shadow_depth_tex = depth_tex;
            self._shadow_color_tex = color_tex;
            // The SSAO bind group references the (new) shadow depth view → rebuild.
            self.ssao_bind_group = make_ssao_bind_group(
                device,
                &self.ssao_bgl,
                &self.targets.depth_view,
                &self.ssao_buf,
                &self.shadow_depth_view,
                &self.shadow_sampler,
            );
        }
    }

    /// GPU atom pick: render the pick id-buffer (each molecule's `pick_gpu` sphere
    /// impostors, one per eligible atom, id-stamped) at `size` (1× logical px), then
    /// read back the texel under `(px, py)` and decode the front-most atom. Returns
    /// `(mol, rep, atom)` (indices into `scene.molecules` / `mol.reps` / the System),
    /// Whether an async pick is in flight (its readback hasn't been consumed yet).
    /// While true, callers should keep repainting so `poll_pick` runs and the result
    /// is picked up.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn pick_in_flight(&self) -> bool {
        self.pick_pending.is_some()
    }

    /// Start an async GPU pick: render the pick id-buffer (each molecule's `pick_gpu`
    /// sphere impostors, id-stamped, periodic images baked in) at `size` (1× logical
    /// px) and kick off a non-blocking readback of the texel under `(px, py)`. No-op
    /// if one is already in flight. Reuses camera entry 0 (the current view). The
    /// result is collected later by [`poll_pick`]. Native only.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn request_pick(
        &mut self,
        rs: &RenderState,
        scene: &Scene,
        px: u32,
        py: u32,
        size: [u32; 2],
    ) {
        if self.pick_pending.is_some() {
            return; // one at a time; the latest cursor is picked when this completes
        }
        let size = [size[0].max(1), size[1].max(1)];
        if self.pick_size != size {
            let (t, v, d) = make_pick_targets(&rs.device, size);
            self.pick_id_tex = t;
            self.pick_id_view = v;
            self.pick_depth_view = d;
            self.pick_size = size;
        }
        let (px, py) = (px.min(size[0] - 1), py.min(size[1] - 1));

        let mut encoder = rs
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pick") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pick-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.pick_id_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        // Clear to (0, 0) → x = 0 = "no hit".
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.pick_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.camera_bind_group, &[0]);
            pass.set_pipeline(&self.pick_pipeline);
            for mol in &scene.molecules {
                if !mol.visible {
                    continue;
                }
                if let Some(s) = &mol.pick_gpu.spheres {
                    pass.set_vertex_buffer(0, s.buffer.slice(..));
                    pass.draw(0..4, 0..s.count);
                }
            }
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.pick_id_tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x: px, y: py, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.pick_readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: None, // single row
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        rs.queue.submit(std::iter::once(encoder.finish()));

        // Kick off the async map; the callback flips `ready` when the GPU is done.
        // `poll_pick` (driven each frame) advances the device and collects it.
        let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = ready.clone();
        self.pick_readback
            .slice(0..8)
            .map_async(wgpu::MapMode::Read, move |res| {
                if res.is_ok() {
                    flag.store(true, std::sync::atomic::Ordering::Release);
                }
            });
        self.pick_pending = Some(ready);
    }

    /// Drive the device and, if an async pick has completed, read + decode it.
    /// Returns `Some(hit)` when a pick finished this call (`hit` = `Some((mol, rep,
    /// atom))` or `None` for a miss), or `None` if nothing completed. Native only.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn poll_pick(&mut self, rs: &RenderState) -> Option<Option<(usize, usize, usize)>> {
        self.pick_pending.as_ref()?; // nothing in flight → skip the device poll
        // Non-blocking: lets the map callback fire without stalling on the GPU.
        let _ = rs.device.poll(wgpu::PollType::Poll);
        if !self
            .pick_pending
            .as_ref()
            .unwrap()
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return None;
        }
        let (x, y) = {
            let data = self.pick_readback.slice(0..8).get_mapped_range();
            let x = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
            let y = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
            (x, y)
        };
        self.pick_readback.unmap();
        self.pick_pending = None;

        if x == 0 {
            return Some(None); // a pick completed, but it was a miss
        }
        let mol = (x - 1) as usize;
        let rep = (y >> 21) as usize;
        let atom = (y & ((1 << 21) - 1)) as usize;
        Some(Some((mol, rep, atom)))
    }

    /// The egui texture id of the offscreen color target (for `egui::Image`).
    pub fn texture_id(&self) -> TextureId {
        self.egui_texture
    }

    /// Build GPU buffers for one representation's geometry.
    pub fn upload(&self, rs: &RenderState, geom: &GeometryData) -> RepGpu {
        let device = &rs.device;
        RepGpu {
            spheres: upload_buf(device, &geom.spheres, "spheres"),
            cylinders: upload_buf(device, &geom.cylinders, "cylinders"),
            lines: upload_buf(device, &geom.lines, "lines"),
            mesh: upload_mesh(device, &geom.mesh),
        }
    }

    /// Incrementally update a representation's existing GPU buffers from new
    /// geometry (used for coordinate-only trajectory frame changes). Buffers whose
    /// element counts are unchanged are written in place via `queue.write_buffer`
    /// (no reallocation); any whose size changed are recreated.
    pub fn update(&self, rs: &RenderState, gpu: &mut RepGpu, geom: &GeometryData) {
        let (device, queue) = (&rs.device, &rs.queue);
        update_buf(device, queue, &mut gpu.spheres, &geom.spheres, "spheres");
        update_buf(device, queue, &mut gpu.cylinders, &geom.cylinders, "cylinders");
        update_buf(device, queue, &mut gpu.lines, &geom.lines, "lines");
        update_mesh(device, queue, &mut gpu.mesh, &geom.mesh);
    }

    /// Render every visible representation of every visible molecule into the
    /// offscreen target with the given camera. Returns the egui texture id.
    pub fn render_scene(
        &mut self,
        rs: &RenderState,
        size_px: [u32; 2],
        view: Mat4,
        proj: Mat4,
        perspective: bool,
        cue: [f32; 4],
        ao: [f32; 4],
        shadow: [f32; 4],
        background: crate::camera::Background,
        depth_range: [f32; 2],
        glow_pulse: f32,
        scene: &Scene,
    ) -> TextureId {
        let clear = background.clear_color();
        // Render into SSAA× targets (clamped to the device's max texture size),
        // then let egui's linear filter downsample to the (1×) image rect.
        let max_dim = rs.device.limits().max_texture_dimension_2d;
        let render_size = [
            (size_px[0] * self.ssaa).clamp(1, max_dim),
            (size_px[1] * self.ssaa).clamp(1, max_dim),
        ];
        if render_size != self.targets.size {
            self.targets = Targets::new(&rs.device, self.color_format, &self.oit_bgl, render_size);
            // The SSAO bind group references the (new) depth view → recreate it.
            self.ssao_bind_group = make_ssao_bind_group(
                &rs.device,
                &self.ssao_bgl,
                &self.targets.depth_view,
                &self.ssao_buf,
                &self.shadow_depth_view,
                &self.shadow_sampler,
            );
            rs.renderer.write().update_egui_texture_from_wgpu_texture(
                &rs.device,
                &self.targets.color_view,
                wgpu::FilterMode::Linear,
                self.egui_texture,
            );
        }

        // Build the camera entries. Entry 0 = the base camera; periodic images add
        // one entry each, its view post-multiplied by a lattice translation
        // (`i·a + j·b + k·c`) so the *same* uploaded geometry is re-drawn shifted —
        // no data is duplicated. `images[mi][j]` lists the camera indices to draw
        // rep `j` of molecule `mi` at (empty = nothing; `[0]` = just the central copy).
        let viewport = [size_px[0] as f32, size_px[1] as f32];
        // Depth-cue fog fades geometry toward the background color.
        let fog = background.fog_color();
        let make_cam = |v: Mat4| {
            CameraUniform::new(v, proj, perspective, viewport, cue, fog, depth_range, glow_pulse)
        };
        // Entry 0 = base camera with the (animated) glow pulse; entry 1 = the same
        // base camera but with a *steady* pulse (1.0), used to draw the hover
        // highlight without it breathing. Periodic-image cameras follow from index 2.
        let mut cameras: Vec<CameraUniform> = vec![
            make_cam(view),
            CameraUniform::new(view, proj, perspective, viewport, cue, fog, depth_range, 1.0),
        ];
        let mut images: Vec<Vec<Vec<u32>>> = Vec::with_capacity(scene.molecules.len());
        for mol in &scene.molecules {
            let box_vecs = mol.system.state().pbox.as_ref().map(|pb| {
                let m = pb.get_matrix();
                // Columns of the box matrix are the lattice vectors a, b, c (nm).
                [
                    Vec3::new(m[(0, 0)], m[(1, 0)], m[(2, 0)]),
                    Vec3::new(m[(0, 1)], m[(1, 1)], m[(2, 1)]),
                    Vec3::new(m[(0, 2)], m[(1, 2)], m[(2, 2)]),
                ]
            });
            let mut mol_imgs = Vec::with_capacity(mol.reps.len());
            for rep in &mol.reps {
                let mut idxs: Vec<u32> = Vec::new();
                match box_vecs {
                    Some([a, b, c]) => {
                        // One camera per drawn image; the central (zero) offset reuses
                        // the base camera (entry 0). Shares `offsets` with the picker.
                        for off in rep.periodic.offsets(a, b, c) {
                            if off == Vec3::ZERO {
                                idxs.push(0);
                            } else {
                                cameras.push(make_cam(view * Mat4::from_translation(off)));
                                idxs.push(cameras.len() as u32 - 1);
                            }
                        }
                    }
                    // No box → periodic display is meaningless; just the central copy.
                    None => idxs.push(0),
                }
                mol_imgs.push(idxs);
            }
            images.push(mol_imgs);
        }

        // Cast-shadow map: add a light-space camera entry (rendered into the shadow
        // depth target below) and compute `shadow_matrix`, which maps any image's
        // view space → the light's clip space for the deferred shadow test. The
        // light is a directional key; we fit an orthographic frustum to the scene's
        // bounding sphere (center/radius recovered from the view + `depth_range`).
        let shadow_on = shadow[2] > 0.5 && self.ssao_pipeline.is_some();
        let (shadow_light_idx, shadow_matrix) = if shadow_on {
            let inv_view = view.inverse();
            let eye = inv_view.transform_point3(Vec3::ZERO);
            let fwd = inv_view.transform_vector3(Vec3::NEG_Z).normalize();
            let center = eye + fwd * ((depth_range[0] + depth_range[1]) * 0.5);
            let radius = ((depth_range[1] - depth_range[0]) * 0.5).max(0.1);
            let light_world = inv_view.transform_vector3(SHADOW_LIGHT_DIR_VIEW).normalize();
            let light_eye = center + light_world * (radius * 2.0);
            let up = if light_world.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
            let light_view = Mat4::look_at_rh(light_eye, center, up);
            let light_proj =
                Mat4::orthographic_rh(-radius, radius, -radius, radius, radius * 0.5, radius * 3.5);
            // Ortho (perspective = false) so the impostors ray-cast with parallel
            // rays in light space when filling the depth map.
            cameras.push(CameraUniform::new(
                light_view, light_proj, false, viewport, cue, BG, depth_range, 1.0,
            ));
            (cameras.len() as u32 - 1, light_proj * light_view * inv_view)
        } else {
            (0, Mat4::IDENTITY)
        };


        // Grow the dynamic camera buffer if needed, then upload all entries
        // (each padded to CAMERA_STRIDE so dynamic offsets stay aligned).
        if cameras.len() as u32 > self.camera_capacity {
            self.camera_capacity = cameras.len() as u32;
            self.camera_buf = rs.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("camera-uniform"),
                size: self.camera_capacity as u64 * CAMERA_STRIDE,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.camera_bind_group =
                make_camera_bind_group(&rs.device, &self.camera_bgl, &self.camera_buf);
        }
        let mut bytes = vec![0u8; cameras.len() * CAMERA_STRIDE as usize];
        for (idx, cam) in cameras.iter().enumerate() {
            let off = idx * CAMERA_STRIDE as usize;
            bytes[off..off + std::mem::size_of::<CameraUniform>()]
                .copy_from_slice(bytemuck::bytes_of(cam));
        }
        rs.queue.write_buffer(&self.camera_buf, 0, &bytes);

        // Skip the OIT + composite passes entirely when nothing transparent is
        // visible (idle scenes pay nothing for the transparency machinery). When the
        // device can't do OIT (WebGL2), transparent reps are drawn in the opaque pass
        // with plain alpha blending instead, so the dedicated OIT pass never runs.
        let has_transparent = scene.molecules.iter().any(|m| {
            m.visible
                && m.reps
                    .iter()
                    .any(|r| r.visible && r.material.is_transparent())
        });
        let use_oit = has_transparent && self.oit_enabled;

        let mut encoder = rs
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("scene-encoder"),
            });

        // Pass 0 — cast-shadow map: render opaque geometry from the light into the
        // shadow depth target (front-most wins). The throwaway color target lets us
        // reuse the existing opaque pipelines, so no depth-only variants are needed.
        if shadow_on {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.shadow_color_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.draw_shadow_casters(&mut pass, scene, shadow_light_idx);
        }

        // Pass 1 — opaque geometry into the color + depth targets.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("opaque-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.color_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0] as f64,
                            g: clear[1] as f64,
                            b: clear[2] as f64,
                            a: clear[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // Background gradient (color only, behind everything) over the flat clear.
            if background.is_gradient() {
                let bgu = BgUniform { top: background.top, bottom: background.bottom };
                rs.queue.write_buffer(&self.bg_buf, 0, bytemuck::bytes_of(&bgu));
                pass.set_pipeline(&self.bg_pipeline);
                pass.set_bind_group(0, &self.bg_bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            self.draw_reps(&mut pass, scene, false, 0, &images);
            // No-OIT fallback (WebGL2): draw transparent reps here too, with the
            // opaque pipeline (alpha blend, depth-write on) — order-dependent, but it
            // renders. With OIT they go to the dedicated pass below instead.
            if has_transparent && !self.oit_enabled {
                self.draw_reps(&mut pass, scene, true, 0, &images);
            }
        }

        // Pass 1.5 — SSAO + cast shadows: read the opaque depth and multiply-blend a
        // darkening factor (ambient occlusion × cast-shadow term) onto the opaque
        // color, before transparent geometry is composited over it. Skipped when
        // both AO and shadows are off. AO with strength 0 (when disabled) is a no-op
        // so the pass can run for shadows alone.
        if ao[3] > 0.5 || shadow_on {
            if let Some(ssao_pipeline) = &self.ssao_pipeline {
            let ao_eff = if ao[3] > 0.5 { ao } else { [ao[0], ao[1], 0.0, ao[3]] };
            let su = SsaoUniform::new(
                proj,
                perspective,
                ao_eff,
                shadow_matrix,
                shadow,
                render_size,
                1.0 / self.shadow_res as f32,
            );
            rs.queue.write_buffer(&self.ssao_buf, 0, bytemuck::bytes_of(&su));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ssao-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.color_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(ssao_pipeline);
            pass.set_bind_group(0, &self.ssao_bind_group, &[]);
            pass.draw(0..3, 0..1);
            }
        }

        // Pass 2 — transparent geometry into the weighted-blended OIT targets
        // (accum cleared to 0, reveal to 1). Depth-tests against the opaque depth,
        // but writes no depth, so transparent fragments don't cull each other.
        if use_oit {
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("oit-pass"),
                    color_attachments: &[
                        Some(wgpu::RenderPassColorAttachment {
                            view: &self.targets.accum_view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        }),
                        Some(wgpu::RenderPassColorAttachment {
                            view: &self.targets.reveal_view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                                store: wgpu::StoreOp::Store,
                            },
                        }),
                    ],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.targets.depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                self.draw_reps(&mut pass, scene, true, 1, &images);
            }

            // Pass 3 — resolve the OIT targets over the opaque color.
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("oit-composite-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.targets.color_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.composite_pipeline);
                pass.set_bind_group(0, &self.targets.oit_bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        // Pass 4 — active-selection glow: additive cyan over the composited color,
        // depth-tested `≤` against the scene depth (so occluded selection atoms
        // don't glow) with no depth-write. Drawn at the central image only.
        let has_glow = scene
            .molecules
            .iter()
            .any(|m| m.visible && (m.glow_gpu.has_geometry() || m.hover_gpu.has_geometry()));
        if has_glow {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("glow-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.color_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.draw_glow(&mut pass, scene);
        }

        // Pass 5 — hover detail lens: the faded CPK ball-and-stick of atoms near the
        // cursor view-line, over a Cartoon/Surface rep. Drawn last with the opaque
        // (alpha-blending) pipelines over the composited image, with a **freshly
        // cleared** depth so the lens reveals the hidden atoms *over* the ribbon
        // (depth-testing against the scene depth would let the opaque ribbon occlude
        // the very atoms we're trying to expose). The lens still depth-tests against
        // itself, so its own atoms occlude correctly; the distance fade softens it.
        let has_lens = scene
            .molecules
            .iter()
            .any(|m| m.visible && m.hover_detail_gpu.has_geometry());
        if has_lens {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hover-detail-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.color_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.draw_hover_detail(&mut pass, scene);
        }

        rs.queue.submit(std::iter::once(encoder.finish()));

        self.egui_texture
    }

    /// Draw the shadow casters: every visible **opaque** rep's spheres / cylinders /
    /// mesh once, from the light camera (`light_idx`), into the shadow depth map.
    /// Reuses the opaque pipelines (index 0); lines and the box wireframe don't cast
    /// (too thin to read as shadows). Transparent reps are skipped.
    fn draw_shadow_casters(&self, pass: &mut wgpu::RenderPass, scene: &Scene, light_idx: u32) {
        let off = light_idx * CAMERA_STRIDE as u32;
        pass.set_bind_group(0, &self.camera_bind_group, &[off]);
        for mol in &scene.molecules {
            if !mol.visible {
                continue;
            }
            for rep in &mol.reps {
                if !rep.visible || rep.material.is_transparent() {
                    continue;
                }
                if let Some(s) = &rep.gpu.spheres {
                    pass.set_pipeline(&self.sphere_pipeline[0]);
                    pass.set_vertex_buffer(0, s.buffer.slice(..));
                    pass.draw(0..4, 0..s.count);
                }
                if let Some(c) = &rep.gpu.cylinders {
                    pass.set_pipeline(&self.cylinder_pipeline[0]);
                    pass.set_vertex_buffer(0, c.buffer.slice(..));
                    pass.draw(0..4, 0..c.count);
                }
                if let Some(m) = &rep.gpu.mesh {
                    pass.set_pipeline(&self.mesh_pipeline[0]);
                    pass.set_vertex_buffer(0, m.vertices.slice(..));
                    pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..m.index_count, 0, 0..1);
                }
            }
        }
    }

    /// Draw the additive selection glows at the central image: each molecule's
    /// **pending** glow (`glow_gpu`) with the pulsing camera (entry 0) and its
    /// **hover** highlight (`hover_gpu`) with the steady camera (entry 1) — so the
    /// lasso/pending selection breathes while the hover highlight holds still.
    fn draw_glow(&self, pass: &mut wgpu::RenderPass, scene: &Scene) {
        let stride = CAMERA_STRIDE as u32;
        // Pending (pulsing) at entry 0.
        pass.set_bind_group(0, &self.camera_bind_group, &[0]);
        for mol in &scene.molecules {
            if mol.visible && mol.glow_gpu.has_geometry() {
                self.draw_glow_geom(pass, &mol.glow_gpu);
            }
        }
        // Hover (steady) at entry 1.
        pass.set_bind_group(0, &self.camera_bind_group, &[stride]);
        for mol in &scene.molecules {
            if mol.visible && mol.hover_gpu.has_geometry() {
                self.draw_glow_geom(pass, &mol.hover_gpu);
            }
        }
    }

    /// Draw one glow geometry with the additive `GLOW` pipelines (camera bind group
    /// already set by the caller).
    fn draw_glow_geom(&self, pass: &mut wgpu::RenderPass, g: &RepGpu) {
        if let Some(s) = &g.spheres {
            pass.set_pipeline(&self.sphere_pipeline[GLOW]);
            pass.set_vertex_buffer(0, s.buffer.slice(..));
            pass.draw(0..4, 0..s.count);
        }
        if let Some(c) = &g.cylinders {
            pass.set_pipeline(&self.cylinder_pipeline[GLOW]);
            pass.set_vertex_buffer(0, c.buffer.slice(..));
            pass.draw(0..4, 0..c.count);
        }
        if let Some(l) = &g.lines {
            pass.set_pipeline(&self.line_pipeline[GLOW]);
            pass.set_vertex_buffer(0, l.buffer.slice(..));
            pass.draw(0..4, 0..l.count / 2);
        }
        if let Some(m) = &g.mesh {
            pass.set_pipeline(&self.mesh_pipeline[GLOW]);
            pass.set_vertex_buffer(0, m.vertices.slice(..));
            pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..m.index_count, 0, 0..1);
        }
    }

    /// Draw the hover detail lens (each molecule's `hover_detail_gpu` ball-and-stick)
    /// with the opaque pipelines (alpha-blend + depth-test) at the base camera, so
    /// the faded geometry blends over the scene and the ribbon occludes it.
    fn draw_hover_detail(&self, pass: &mut wgpu::RenderPass, scene: &Scene) {
        pass.set_bind_group(0, &self.camera_bind_group, &[0]);
        for mol in &scene.molecules {
            if !mol.visible {
                continue;
            }
            let g = &mol.hover_detail_gpu;
            if let Some(s) = &g.spheres {
                pass.set_pipeline(&self.sphere_pipeline[0]);
                pass.set_vertex_buffer(0, s.buffer.slice(..));
                pass.draw(0..4, 0..s.count);
            }
            if let Some(c) = &g.cylinders {
                pass.set_pipeline(&self.cylinder_pipeline[0]);
                pass.set_vertex_buffer(0, c.buffer.slice(..));
                pass.draw(0..4, 0..c.count);
            }
        }
    }

    /// Draw every visible representation whose transparency matches `transparent`,
    /// using pipeline-array slot `pipeline_idx` (0 = opaque, 1 = OIT). These are
    /// decoupled so the WebGL2 fallback can draw transparent reps with the *opaque*
    /// pipeline (alpha blend, depth-write on). Each rep is drawn once per periodic
    /// image listed in `images[mi][j]`, selecting that image's camera via a dynamic
    /// offset — same buffers, no data duplication. The box wireframe is drawn only
    /// in the opaque (`!transparent`) call.
    fn draw_reps(
        &self,
        pass: &mut wgpu::RenderPass,
        scene: &Scene,
        transparent: bool,
        pipeline_idx: usize,
        images: &[Vec<Vec<u32>>],
    ) {
        let i = pipeline_idx;
        let stride = CAMERA_STRIDE as u32;
        for (mi, mol) in scene.molecules.iter().enumerate() {
            if !mol.visible {
                continue;
            }
            for (j, rep) in mol.reps.iter().enumerate() {
                if !rep.visible || rep.material.is_transparent() != transparent {
                    continue;
                }
                for &cam in &images[mi][j] {
                    pass.set_bind_group(0, &self.camera_bind_group, &[cam * stride]);
                    if let Some(s) = &rep.gpu.spheres {
                        pass.set_pipeline(&self.sphere_pipeline[i]);
                        pass.set_vertex_buffer(0, s.buffer.slice(..));
                        pass.draw(0..4, 0..s.count);
                    }
                    if let Some(c) = &rep.gpu.cylinders {
                        pass.set_pipeline(&self.cylinder_pipeline[i]);
                        pass.set_vertex_buffer(0, c.buffer.slice(..));
                        pass.draw(0..4, 0..c.count);
                    }
                    if let Some(l) = &rep.gpu.lines {
                        // Instanced fat-line quads: one segment (2 verts) per instance.
                        pass.set_pipeline(&self.line_pipeline[i]);
                        pass.set_vertex_buffer(0, l.buffer.slice(..));
                        pass.draw(0..4, 0..l.count / 2);
                    }
                    if let Some(m) = &rep.gpu.mesh {
                        pass.set_pipeline(&self.mesh_pipeline[i]);
                        pass.set_vertex_buffer(0, m.vertices.slice(..));
                        pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..m.index_count, 0, 0..1);
                    }
                }
            }
            // Periodic-box wireframe (opaque grey): the molecule-level box at the
            // base camera, plus a replica at each image cell of any rep whose
            // `Box` toggle is on.
            if !transparent {
                if let Some(l) = &mol.box_gpu.lines {
                    // Collect the camera indices the box should be drawn at.
                    let mut box_cams: Vec<u32> = Vec::new();
                    if mol.show_box {
                        box_cams.push(0);
                    }
                    for (j, rep) in mol.reps.iter().enumerate() {
                        if rep.visible && rep.periodic.show_box {
                            box_cams.extend_from_slice(&images[mi][j]);
                        }
                    }
                    box_cams.sort_unstable();
                    box_cams.dedup();
                    if !box_cams.is_empty() {
                        pass.set_pipeline(&self.line_pipeline[0]);
                        pass.set_vertex_buffer(0, l.buffer.slice(..));
                        for cam in box_cams {
                            pass.set_bind_group(0, &self.camera_bind_group, &[cam * stride]);
                            pass.draw(0..4, 0..l.count / 2);
                        }
                    }
                }
            }
        }
    }
}
