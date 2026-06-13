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
mod sphere;

pub use cylinder::CylinderInstance;
pub use line::LineVertex;
pub use sphere::SphereInstance;

use camera_uniform::CameraUniform;
use glam::Mat4;
use wgpu::util::DeviceExt;

use eframe::egui_wgpu::RenderState;
use egui::TextureId;

use crate::geometry::GeometryData;

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

fn upload<T: bytemuck::Pod>(
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

pub struct SceneRenderer {
    color_format: wgpu::TextureFormat,
    targets: Targets,
    egui_texture: TextureId,

    camera_buf: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,

    sphere_pipeline: wgpu::RenderPipeline,
    cylinder_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,

    spheres: Option<DrawBuffer>,   // 4 verts/instance
    cylinders: Option<DrawBuffer>, // 4 verts/instance
    lines: Option<DrawBuffer>,     // vertex count (LineList)
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
            spheres: None,
            cylinders: None,
            lines: None,
        }
    }

    /// Upload all geometry for the current representation (replacing previous).
    pub fn set_geometry(&mut self, rs: &RenderState, geom: &GeometryData) {
        let device = &rs.device;
        self.spheres = upload(device, &geom.spheres, "spheres");
        self.cylinders = upload(device, &geom.cylinders, "cylinders");
        self.lines = upload(device, &geom.lines, "lines");
    }

    /// Resize offscreen targets if needed, render with the given camera matrices,
    /// and return the egui texture id to display in the central panel.
    pub fn render(
        &mut self,
        rs: &RenderState,
        size_px: [u32; 2],
        view: Mat4,
        proj: Mat4,
        perspective: bool,
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

            if let Some(s) = &self.spheres {
                pass.set_pipeline(&self.sphere_pipeline);
                pass.set_vertex_buffer(0, s.buffer.slice(..));
                pass.draw(0..4, 0..s.count);
            }
            if let Some(c) = &self.cylinders {
                pass.set_pipeline(&self.cylinder_pipeline);
                pass.set_vertex_buffer(0, c.buffer.slice(..));
                pass.draw(0..4, 0..c.count);
            }
            if let Some(l) = &self.lines {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_vertex_buffer(0, l.buffer.slice(..));
                pass.draw(0..l.count, 0..1);
            }
        }
        rs.queue.submit(std::iter::once(encoder.finish()));

        self.egui_texture
    }
}
