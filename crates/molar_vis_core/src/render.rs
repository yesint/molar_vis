//! GPU rendering of the molecular scene.
//!
//! Strategy A: the 3D scene is rendered into our *own* offscreen color +
//! `Depth32Float` targets, then composited into the egui frame as a registered
//! texture (`egui::Image`). egui's own render pass has no depth attachment, so
//! this is what gives us full control of depth — required for the ray-cast
//! impostors, whose fragment shaders write analytic depth and must occlude
//! correctly against each other (and later the cartoon mesh).

mod camera_uniform;
mod cylinder;
mod line;
mod mesh;
mod sphere;

pub use cylinder::CylinderInstance;
pub use line::LineVertex;
pub use mesh::MeshVertex;
pub use sphere::SphereInstance;

use camera_uniform::CameraUniform;
use glam::Mat4;
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

/// Scene background. Used both as the render-pass clear color and as the depth-cue
/// fog color, so distant geometry dissolves seamlessly into the background.
const BG: [f32; 4] = [0.02, 0.02, 0.05, 1.0];

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
        let depth_tex = make("scene-depth", DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
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

    camera_buf: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,

    // Each geometry has an opaque pipeline `[0]` (single alpha-blended target,
    // depth-write on) and a weighted-blended OIT pipeline `[1]` (accum+reveal
    // targets, depth-write off, depth-test on). The OIT pipelines are resolved
    // into the color target by `composite_pipeline` after the transparent pass.
    sphere_pipeline: [wgpu::RenderPipeline; 2],
    cylinder_pipeline: [wgpu::RenderPipeline; 2],
    line_pipeline: [wgpu::RenderPipeline; 2],
    mesh_pipeline: [wgpu::RenderPipeline; 2],
    oit_bgl: wgpu::BindGroupLayout,
    composite_pipeline: wgpu::RenderPipeline,
}

impl SceneRenderer {
    pub fn new(rs: &RenderState) -> Self {
        let device = &rs.device;
        let color_format = rs.target_format;

        // Camera uniform = bind group 0, shared by every pipeline.
        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera-uniform"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera-bg"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buf.as_entire_binding(),
            }],
        });

        // Opaque pass: single alpha-blended target, depth-write on, `fs_main`.
        // OIT pass: accum+reveal targets, depth-write off (test on), `fs_oit`.
        let f = color_format;
        let opaque_t = opaque_targets(f);
        let oit_t = oit_targets();
        let pair = |opaque: wgpu::RenderPipeline, oit: wgpu::RenderPipeline| [opaque, oit];
        let sphere_pipeline = pair(
            sphere::build_pipeline(device, DEPTH_FORMAT, &camera_bgl, &opaque_t, true, "fs_main"),
            sphere::build_pipeline(device, DEPTH_FORMAT, &camera_bgl, &oit_t, false, "fs_oit"),
        );
        let cylinder_pipeline = pair(
            cylinder::build_pipeline(device, DEPTH_FORMAT, &camera_bgl, &opaque_t, true, "fs_main"),
            cylinder::build_pipeline(device, DEPTH_FORMAT, &camera_bgl, &oit_t, false, "fs_oit"),
        );
        let line_pipeline = pair(
            line::build_pipeline(device, DEPTH_FORMAT, &camera_bgl, &opaque_t, true, "fs_main"),
            line::build_pipeline(device, DEPTH_FORMAT, &camera_bgl, &oit_t, false, "fs_oit"),
        );
        let mesh_pipeline = pair(
            mesh::build_pipeline(device, DEPTH_FORMAT, &camera_bgl, &opaque_t, true, "fs_main"),
            mesh::build_pipeline(device, DEPTH_FORMAT, &camera_bgl, &oit_t, false, "fs_oit"),
        );

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

        let targets = Targets::new(device, color_format, &oit_bgl, [1, 1]);
        let egui_texture = rs.renderer.write().register_native_texture(
            device,
            &targets.color_view,
            wgpu::FilterMode::Linear,
        );

        Self {
            color_format,
            targets,
            egui_texture,
            camera_buf,
            camera_bind_group,
            sphere_pipeline,
            cylinder_pipeline,
            line_pipeline,
            mesh_pipeline,
            oit_bgl,
            composite_pipeline,
        }
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
        depth_range: [f32; 2],
        scene: &Scene,
    ) -> TextureId {
        if size_px != self.targets.size {
            self.targets = Targets::new(&rs.device, self.color_format, &self.oit_bgl, size_px);
            rs.renderer.write().update_egui_texture_from_wgpu_texture(
                &rs.device,
                &self.targets.color_view,
                wgpu::FilterMode::Linear,
                self.egui_texture,
            );
        }

        let cam = CameraUniform::new(
            view,
            proj,
            perspective,
            [size_px[0] as f32, size_px[1] as f32],
            cue,
            BG,
            depth_range,
        );
        rs.queue
            .write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&cam));

        // Skip the OIT + composite passes entirely when nothing transparent is
        // visible (idle scenes pay nothing for the transparency machinery).
        let has_transparent = scene.molecules.iter().any(|m| {
            m.visible
                && m.reps
                    .iter()
                    .any(|r| r.visible && r.material.is_transparent())
        });

        let mut encoder = rs
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("scene-encoder"),
            });

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
                            r: BG[0] as f64,
                            g: BG[1] as f64,
                            b: BG[2] as f64,
                            a: BG[3] as f64,
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
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            self.draw_reps(&mut pass, scene, false);
        }

        // Pass 2 — transparent geometry into the weighted-blended OIT targets
        // (accum cleared to 0, reveal to 1). Depth-tests against the opaque depth,
        // but writes no depth, so transparent fragments don't cull each other.
        if has_transparent {
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
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                self.draw_reps(&mut pass, scene, true);
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

        rs.queue.submit(std::iter::once(encoder.finish()));

        self.egui_texture
    }

    /// Draw every visible representation matching the requested transparency into
    /// the active pass, using each geometry's opaque (`transparent == false`) or
    /// OIT (`true`) pipeline. The periodic-box wireframe is opaque-only.
    fn draw_reps(&self, pass: &mut wgpu::RenderPass, scene: &Scene, transparent: bool) {
        let i = transparent as usize;
        for mol in &scene.molecules {
            if !mol.visible {
                continue;
            }
            for rep in &mol.reps {
                if !rep.visible || rep.material.is_transparent() != transparent {
                    continue;
                }
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
            // Periodic-box wireframe (opaque grey).
            if !transparent && mol.show_box {
                if let Some(l) = &mol.box_gpu.lines {
                    pass.set_pipeline(&self.line_pipeline[0]);
                    pass.set_vertex_buffer(0, l.buffer.slice(..));
                    pass.draw(0..4, 0..l.count / 2);
                }
            }
        }
    }
}
