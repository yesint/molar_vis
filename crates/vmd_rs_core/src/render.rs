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

/// Offscreen render targets, recreated when the viewport size changes.
struct Targets {
    size: [u32; 2],
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    _color_tex: wgpu::Texture,
    _depth_tex: wgpu::Texture,
}

impl Targets {
    fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat, size: [u32; 2]) -> Self {
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let color_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene-color"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: color_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene-depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        Self {
            size,
            color_view: color_tex.create_view(&wgpu::TextureViewDescriptor::default()),
            depth_view: depth_tex.create_view(&wgpu::TextureViewDescriptor::default()),
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
        usage: wgpu::BufferUsages::VERTEX,
    });
    Some(DrawBuffer {
        buffer,
        count: data.len() as u32,
    })
}

fn upload_mesh(device: &wgpu::Device, mesh: &crate::geometry::MeshData) -> Option<MeshBuffers> {
    if mesh.indices.is_empty() {
        return None;
    }
    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("mesh-verts"),
        contents: bytemuck::cast_slice(&mesh.vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("mesh-indices"),
        contents: bytemuck::cast_slice(&mesh.indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    Some(MeshBuffers {
        vertices,
        indices,
        index_count: mesh.indices.len() as u32,
    })
}

pub struct SceneRenderer {
    color_format: wgpu::TextureFormat,
    targets: Targets,
    egui_texture: TextureId,

    camera_buf: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,

    sphere_pipeline: wgpu::RenderPipeline,
    cylinder_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    mesh_pipeline: wgpu::RenderPipeline,
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

        let sphere_pipeline = sphere::build_pipeline(device, color_format, DEPTH_FORMAT, &camera_bgl);
        let cylinder_pipeline =
            cylinder::build_pipeline(device, color_format, DEPTH_FORMAT, &camera_bgl);
        let line_pipeline = line::build_pipeline(device, color_format, DEPTH_FORMAT, &camera_bgl);
        let mesh_pipeline = mesh::build_pipeline(device, color_format, DEPTH_FORMAT, &camera_bgl);

        let targets = Targets::new(device, color_format, [1, 1]);
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

    /// Render every visible representation of every visible molecule into the
    /// offscreen target with the given camera. Returns the egui texture id.
    pub fn render_scene(
        &mut self,
        rs: &RenderState,
        size_px: [u32; 2],
        view: Mat4,
        proj: Mat4,
        perspective: bool,
        scene: &Scene,
    ) -> TextureId {
        if size_px != self.targets.size {
            self.targets = Targets::new(&rs.device, self.color_format, size_px);
            rs.renderer.write().update_egui_texture_from_wgpu_texture(
                &rs.device,
                &self.targets.color_view,
                wgpu::FilterMode::Linear,
                self.egui_texture,
            );
        }

        let cam = CameraUniform::new(view, proj, perspective);
        rs.queue
            .write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&cam));

        let mut encoder = rs
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("scene-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.color_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.02,
                            g: 0.02,
                            b: 0.05,
                            a: 1.0,
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

            for mol in &scene.molecules {
                if !mol.visible {
                    continue;
                }
                for rep in &mol.reps {
                    if !rep.visible {
                        continue;
                    }
                    if let Some(s) = &rep.gpu.spheres {
                        pass.set_pipeline(&self.sphere_pipeline);
                        pass.set_vertex_buffer(0, s.buffer.slice(..));
                        pass.draw(0..4, 0..s.count);
                    }
                    if let Some(c) = &rep.gpu.cylinders {
                        pass.set_pipeline(&self.cylinder_pipeline);
                        pass.set_vertex_buffer(0, c.buffer.slice(..));
                        pass.draw(0..4, 0..c.count);
                    }
                    if let Some(l) = &rep.gpu.lines {
                        pass.set_pipeline(&self.line_pipeline);
                        pass.set_vertex_buffer(0, l.buffer.slice(..));
                        pass.draw(0..l.count, 0..1);
                    }
                    if let Some(m) = &rep.gpu.mesh {
                        pass.set_pipeline(&self.mesh_pipeline);
                        pass.set_vertex_buffer(0, m.vertices.slice(..));
                        pass.set_index_buffer(m.indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..m.index_count, 0, 0..1);
                    }
                }
            }
        }
        rs.queue.submit(std::iter::once(encoder.finish()));

        self.egui_texture
    }
}
